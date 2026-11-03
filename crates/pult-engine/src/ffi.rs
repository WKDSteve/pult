//! The Windows calls behind process loopback: listing processes, and capturing the sound of
//! one process tree through a WASAPI loopback client (Windows 10 2004 or later).
//!
//! This is the only module of pult with `unsafe` code. Every block names the condition it
//! relies on; everything outside works with the safe types below.

use std::mem::size_of;
use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, E_OUTOFMEMORY, ERROR_TIMEOUT, HANDLE, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
    AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceCompletionHandler, IAudioCaptureClient, IAudioClient,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::{
    BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemAlloc, CoUninitialize,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, CreateEventW, OpenProcess, PROCESS_SYNCHRONIZE,
    WaitForMultipleObjects,
};
use windows::Win32::System::Variant::VT_BLOB;
use windows::core::{Interface, PCWSTR, Result, w};

/// Puts the calling thread into the multimedia class "Pro Audio" (MMCSS), which runs it
/// ahead of ordinary work such as a build on every core. Returns whether Windows agreed.
/// The registration ends with the thread, so it is never undone by hand.
pub(crate) fn join_pro_audio() -> bool {
    let mut task_index = 0_u32;
    // SAFETY: the task name is a static NUL-terminated wide string and `task_index` a valid
    // place for the index Windows writes.
    unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) }.is_ok()
}

/// One running process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessEntry {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    /// The executable's file name (`Spotify.exe`).
    pub(crate) name: String,
}

/// A kernel handle, closed on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create/Open call and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// All processes running now.
pub(crate) fn processes() -> Result<Vec<ProcessEntry>> {
    // SAFETY: a plain system call; the handle is owned and closed by `Handle`.
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..PROCESSENTRY32W::default()
    };
    let mut list = Vec::new();
    // SAFETY: `entry` is a valid PROCESSENTRY32W with `dwSize` set, as both calls require.
    let mut next = unsafe { Process32FirstW(snapshot.0, &raw mut entry) };
    while next.is_ok() {
        let length = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        list.push(ProcessEntry {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..length]),
        });
        // SAFETY: as above.
        next = unsafe { Process32NextW(snapshot.0, &raw mut entry) };
    }
    Ok(list)
}

/// COM for the calling thread (multithreaded apartment), released on drop.
pub(crate) struct Com(());

impl Com {
    pub(crate) fn init() -> Result<Self> {
        // SAFETY: initialises COM for this thread; balanced by `CoUninitialize` in drop.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self(()))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: this thread initialised COM in `init`.
        unsafe { CoUninitialize() };
    }
}

/// Why [`LoopbackCapture::wait`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wake {
    /// Packets may be waiting.
    Ready,
    /// Nothing happened within the timeout.
    Timeout,
    /// The target process ended.
    Exited,
}

/// One packet of captured sound.
pub(crate) enum Packet<'a> {
    /// Interleaved samples.
    Samples(&'a [f32]),
    /// This many frames of silence.
    Silence(usize),
}

/// A started loopback capture of one process tree. Must stay on the thread that opened it.
pub(crate) struct LoopbackCapture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Handle,
    process: Handle,
    channels: usize,
}

/// The handler `ActivateAudioInterfaceAsync` calls when the client is ready.
mod completion {
    // The code `implement` generates trips these pedantic lints.
    #![allow(clippy::ref_as_ptr, clippy::inline_always)]

    use std::sync::mpsc;

    use windows::Win32::Media::Audio::{
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl,
    };
    use windows::core::{IUnknown, Ref, Result, implement};

    #[implement(IActivateAudioInterfaceCompletionHandler)]
    pub(super) struct Completion(pub(super) mpsc::Sender<Result<IUnknown>>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
        fn ActivateCompleted(
            &self,
            operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> Result<()> {
            let result = operation.ok().and_then(|operation| {
                let mut status = windows::core::HRESULT::default();
                let mut interface: Option<IUnknown> = None;
                // SAFETY: both out pointers are valid locals.
                unsafe { operation.GetActivateResult(&raw mut status, &raw mut interface)? };
                status.ok()?;
                interface.ok_or_else(|| {
                    windows::core::Error::from(windows::Win32::Foundation::E_NOINTERFACE)
                })
            });
            let _ = self.0.send(result);
            Ok(())
        }
    }
}

use completion::Completion;

impl LoopbackCapture {
    /// Opens and starts a capture of process `pid` and its children as 32-bit float with
    /// `channels` channels at `sample_rate`; Windows converts from whatever they play.
    /// Needs [`Com`] on the calling thread.
    pub(crate) fn open(
        pid: u32,
        sample_rate: u32,
        channels: u16,
        timeout: Duration,
    ) -> Result<Self> {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // Windows takes the blob over and frees it with `CoTaskMemFree`: a blob on the stack
        // or in a Rust allocation corrupted the heap. So it is allocated that way and never
        // freed here.
        let size = size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>();
        // SAFETY: a plain allocation, checked for null below.
        let blob = unsafe { CoTaskMemAlloc(size) }.cast::<AUDIOCLIENT_ACTIVATION_PARAMS>();
        if blob.is_null() {
            return Err(E_OUTOFMEMORY.into());
        }
        // SAFETY: `blob` is a fresh allocation of the parameters' size, aligned for any
        // type by `CoTaskMemAlloc`.
        unsafe { blob.write(params) };
        let variant = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: u32::try_from(size).unwrap_or(u32::MAX),
                            pBlobData: blob.cast(),
                        },
                    },
                }),
            },
        };
        let (sender, receiver) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = Completion(sender).into();
        // SAFETY: `variant` is a valid VT_BLOB variant for the duration of the call.
        unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&raw const variant),
                &handler,
            )?;
        }
        let client: IAudioClient = receiver
            .recv_timeout(timeout)
            .map_err(|_| windows::core::Error::from(ERROR_TIMEOUT.to_hresult()))??
            .cast()?;

        let block = channels * 4;
        let format = WAVEFORMATEX {
            wFormatTag: u16::try_from(WAVE_FORMAT_IEEE_FLOAT).unwrap_or_default(),
            nChannels: channels,
            nSamplesPerSec: sample_rate,
            nAvgBytesPerSec: sample_rate * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // 100 ms of buffer in 100 ns units: room for a late wake-up of the capture thread.
        let buffer = 1_000_000;
        // SAFETY: `format` is a complete WAVEFORMATEX and lives across the call; the event
        // and process handles are owned by `Handle`s that outlive the client.
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                buffer,
                0,
                &raw const format,
                None,
            )?;
            let event = Handle(CreateEventW(None, false, false, PCWSTR::null())?);
            client.SetEventHandle(event.0)?;
            let process = Handle(OpenProcess(PROCESS_SYNCHRONIZE, false, pid)?);
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                event,
                process,
                channels: usize::from(channels),
            })
        }
    }

    /// Waits up to `timeout` for packets or for the process to end.
    pub(crate) fn wait(&self, timeout: Duration) -> Result<Wake> {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: both handles are open for the lifetime of `self`.
        let woke: WAIT_EVENT =
            unsafe { WaitForMultipleObjects(&[self.event.0, self.process.0], false, millis) };
        match woke {
            WAIT_TIMEOUT => Ok(Wake::Timeout),
            w if w == WAIT_OBJECT_0 => Ok(Wake::Ready),
            w if w.0 == WAIT_OBJECT_0.0 + 1 => Ok(Wake::Exited),
            _ => Err(windows::core::Error::from_thread()),
        }
    }

    /// Hands every waiting packet to `packet`.
    pub(crate) fn drain(&self, mut packet: impl FnMut(Packet<'_>)) -> Result<()> {
        loop {
            // SAFETY: a plain call on a started capture client.
            if unsafe { self.capture.GetNextPacketSize()? } == 0 {
                return Ok(());
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            // SAFETY: the out pointers are valid locals; the buffer stays valid until
            // `ReleaseBuffer`.
            unsafe {
                self.capture.GetBuffer(
                    &raw mut data,
                    &raw mut frames,
                    &raw mut flags,
                    None,
                    None,
                )?;
            }
            let count = frames as usize;
            // The flag is a small positive constant; the cast cannot change it.
            #[allow(clippy::cast_sign_loss)]
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            // The audio engine hands out 16-byte aligned buffers; a misaligned one (never seen)
            // would count as silence rather than be read wrongly.
            #[allow(clippy::cast_ptr_alignment)]
            let floats = data.cast::<f32>();
            if silent || data.is_null() || !floats.is_aligned() {
                packet(Packet::Silence(count));
            } else {
                // SAFETY: in the 32-bit float format of `open`, the buffer holds `frames`
                // frames of `channels` aligned f32 samples until `ReleaseBuffer`.
                let samples = unsafe { std::slice::from_raw_parts(floats, count * self.channels) };
                packet(Packet::Samples(samples));
            }
            // SAFETY: releases exactly the frames `GetBuffer` gave.
            unsafe { self.capture.ReleaseBuffer(frames)? };
        }
    }
}

impl Drop for LoopbackCapture {
    fn drop(&mut self) {
        // SAFETY: stopping a started client; failure leaves nothing to clean up.
        let _ = unsafe { self.client.Stop() };
    }
}
