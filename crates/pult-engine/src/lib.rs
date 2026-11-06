//! The audio devices of pult: lists them and runs a route, inputs mixed onto one or more
//! buses (each an output device), on the system's default host (WASAPI shared mode on
//! Windows).
//!
//! Every stream uses its device's own shared-mode format. An input routed to several buses
//! pushes each capture into one ring per bus, since every output runs on its own clock. The
//! device callbacks only move samples through the rings and mixers of `pult-core`;
//! `assert_no_alloc` guards them, so a debug build with the checking allocator aborts on
//! any allocation inside a callback.
//!
//! A device that disappears while a route runs (unplugged, disabled) does not end the
//! route: [`Running::supervise`] takes its stream out, the others play on, and the device
//! is opened again on the same rings once it is back (see `slot`).
//!
//! An input can also be the sound of one program (process loopback, see `process`); the
//! Windows calls for that live in `ffi`, the only module with `unsafe` code.

mod convert;
mod desk;
#[cfg(windows)]
#[allow(unsafe_code)]
mod ffi;
mod live;
mod presence;
mod process;
pub mod profile;
#[cfg(not(windows))]
pub mod session;
mod slot;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use assert_no_alloc::assert_no_alloc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SupportedStreamConfig};
use pult_core::EQ_BANDS;
use pult_core::eq::BandChange;
use pult_core::{
    Band, BusHandle, Capture, Counter, DriftControl, InputConfig, InputControls, InputStats, Mixer,
    MixerInput, SelectError, Sine, connect_with, db_to_linear, select,
};

pub use desk::EngineDesk;
pub use presence::{APP_INSTANCE, Presence};
use process::PROCESS_CHANNELS;
pub use process::program_names;
use slot::{Feed, Health, Slot, Source, fault_recorder};

/// Weight of one resampler chunk in the drift control's level average: with 10 ms callbacks
/// the average spans about 1.3 s, long enough to flatten the block jitter of virtual cables.
const DRIFT_SMOOTHING: f64 = 1.0 / 128.0;

/// Seconds after each priming in which the drift control measures the start-up offset of
/// an input and then skips it once, instead of leaving it to the resampler's slow steering.
const SETTLE_SECONDS: usize = 2;

/// Which side of a route a device is on, or that a program is captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A capture device (microphone, line in, the output side of a virtual cable).
    Input,
    /// A playback device (interface outputs, speakers, headphones).
    Output,
    /// A program whose sound is captured (process loopback).
    Process,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Process => "program",
        })
    }
}

/// Why listing devices or starting a route failed.
#[derive(Debug)]
pub enum Error {
    /// The audio system refused an operation.
    Audio {
        /// What pult was doing.
        context: String,
        /// The audio system's error.
        source: cpal::Error,
    },
    /// The device name did not pick exactly one device.
    Select {
        /// Input or output.
        role: Role,
        /// Why not.
        source: SelectError,
    },
    /// The route itself is inconsistent (no input, no bus, a route to a missing bus, one
    /// device on two buses).
    InvalidRoute(String),
    /// The device's format is no PCM pult can mix (DSD, say).
    UnsupportedFormat {
        /// The device.
        device: String,
        /// Its format.
        format: String,
    },
    /// A device came back at another sample rate than its buffers and resamplers were made
    /// for.
    RateChanged {
        /// The device.
        device: String,
        /// Its rate now, in Hz.
        rate: u32,
        /// Its rate when the route started, in Hz.
        expected: u32,
    },
    /// An input device came back with another channel count than its rings were made for.
    LayoutChanged {
        /// The device.
        device: String,
        /// Its channels now.
        channels: usize,
        /// Its channels when the route started.
        expected: usize,
    },
    /// A lost stream did not give its rings back, so the device cannot be opened again.
    Stranded {
        /// The device.
        device: String,
    },
    /// Listing programs or capturing one failed.
    Process {
        /// What pult was doing.
        context: String,
        /// The system's message.
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio { context, source } => write!(f, "{context}: {source}"),
            Self::Select {
                role: Role::Process,
                source,
            } => match source {
                SelectError::NotFound { wanted } => {
                    write!(
                        f,
                        "no running program matches \"{wanted}\" (see `pult programs`)"
                    )
                }
                SelectError::Ambiguous { wanted, candidates } => write!(
                    f,
                    "\"{wanted}\" matches several programs: {}",
                    candidates.join(", ")
                ),
            },
            Self::Select { role, source } => write!(f, "{role} device: {source}"),
            Self::InvalidRoute(why) => f.write_str(why),
            Self::UnsupportedFormat { device, format } => write!(
                f,
                "{device} uses the sample format {format}, which pult cannot mix"
            ),
            Self::RateChanged {
                device,
                rate,
                expected,
            } => write!(
                f,
                "{device} came back at {rate} Hz instead of {expected} Hz; set it back (Sound \
                 settings, device properties, Advanced, default format) or restart pult to use it"
            ),
            Self::LayoutChanged {
                device,
                channels,
                expected,
            } => write!(
                f,
                "{device} came back with {channels} channels instead of {expected}; restart \
                 pult to use it"
            ),
            Self::Stranded { device } => write!(
                f,
                "{device}: its stream did not hand back its buffers; restart pult to use it"
            ),
            Self::Process { context, message } => write!(f, "{context}: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Audio { source, .. } => Some(source),
            Self::Select { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn audio(context: impl Into<String>) -> impl FnOnce(cpal::Error) -> Error {
    let context = context.into();
    move |source| Error::Audio { context, source }
}

/// One device as the audio system presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The name to pass to `--from` or `--to`.
    pub name: String,
    /// Channels of the shared-mode format.
    pub channels: u16,
    /// Sample rate of the shared-mode format, in Hz.
    pub sample_rate: u32,
    /// Sample format of the shared-mode format.
    pub sample_format: String,
    /// Whether this is the system's default device for its role.
    pub is_default: bool,
}

/// All input and output devices.
#[derive(Debug, Clone, Default)]
pub struct Devices {
    /// Capture devices.
    pub inputs: Vec<DeviceInfo>,
    /// Playback devices.
    pub outputs: Vec<DeviceInfo>,
}

/// Lists the devices of the default host. A device whose format cannot be read (unplugged
/// while listing) is left out.
///
/// # Errors
///
/// [`Error::Audio`] when the host cannot enumerate devices.
pub fn list_devices() -> Result<Devices, Error> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.id().ok());
    let default_out = host.default_output_device().and_then(|d| d.id().ok());
    let describe = |device: &Device,
                    config: SupportedStreamConfig,
                    default: Option<cpal::DeviceId>| DeviceInfo {
        name: device.to_string(),
        channels: config.channels(),
        sample_rate: config.sample_rate(),
        sample_format: config.sample_format().to_string(),
        is_default: default.is_some() && device.id().ok() == default,
    };
    let inputs = host
        .input_devices()
        .map_err(audio("listing input devices"))?
        .filter_map(|d| {
            let config = d.default_input_config().ok()?;
            Some(describe(&d, config, default_in.clone()))
        })
        .collect();
    let outputs = host
        .output_devices()
        .map_err(audio("listing output devices"))?
        .filter_map(|d| {
            let config = d.default_output_config().ok()?;
            Some(describe(&d, config, default_out.clone()))
        })
        .collect();
    Ok(Devices { inputs, outputs })
}

/// What an input captures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InputKind {
    /// A capture device.
    #[default]
    Device,
    /// The sound of a program and its child processes (process loopback).
    Process,
}

/// One input (channel strip) of a route.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    /// Device name or program (executable) name, or a unique part of it.
    pub device: String,
    /// Whether `device` names a device or a program.
    pub kind: InputKind,
    /// What an interface calls it instead of the device's name.
    pub label: Option<String>,
    /// Gain in dB.
    pub gain_db: f32,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    /// Its gate as one knob, 0 (off) to 10.
    pub gate: f32,
    /// Its compressor as one knob, 0 (off) to 10.
    pub comp: f32,
    /// Starts muted.
    pub muted: bool,
    /// The buses it feeds, as indices into [`RouteSpec::buses`].
    pub buses: Vec<usize>,
}

impl InputSpec {
    /// An input at `gain_db`, centred, unmuted, without gate or compressor, feeding `buses`.
    #[must_use]
    pub fn new(device: impl Into<String>, gain_db: f32, buses: Vec<usize>) -> Self {
        Self {
            device: device.into(),
            kind: InputKind::Device,
            label: None,
            gain_db,
            pan: 0.0,
            gate: 0.0,
            comp: 0.0,
            muted: false,
            buses,
        }
    }
}

/// One bus of a route: a mix played on one output device.
#[derive(Debug, Clone, PartialEq)]
pub struct BusSpec {
    /// Short name shown in the status line and the log (`A1`).
    pub name: String,
    /// Output device name, or a unique part of it.
    pub device: String,
    /// What an interface calls it instead of its short name.
    pub label: Option<String>,
    /// Gain of the mix in dB.
    pub gain_db: f32,
    /// Starts muted.
    pub muted: bool,
    /// Whether its equalizer is switched on; off, the bands keep their settings.
    pub eq_on: bool,
    /// The bands of its equalizer.
    pub eq: [Band; EQ_BANDS],
}

impl BusSpec {
    /// A bus at 0 dB, unmuted, its equalizer on and flat.
    #[must_use]
    pub fn new(name: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            device: device.into(),
            label: None,
            gain_db: 0.0,
            muted: false,
            eq_on: true,
            eq: Band::all_flat(),
        }
    }
}

/// What a scene changes of one input; what it does not name stays as it is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneInput {
    /// Index into [`RouteSpec::inputs`].
    pub input: usize,
    /// Gain in dB.
    pub gain_db: Option<f32>,
    /// -1 (left) to 1 (right).
    pub pan: Option<f32>,
    /// Muted.
    pub muted: Option<bool>,
    /// The buses it feeds from now on, as indices into [`RouteSpec::buses`].
    pub buses: Option<Vec<usize>>,
    /// Its gate as one knob. Goes along its own times, not the scene's.
    pub gate: Option<f32>,
    /// Its compressor as one knob. Goes along its own times, not the scene's.
    pub comp: Option<f32>,
}

/// What a scene changes of one bus; what it does not name stays as it is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneBus {
    /// Index into [`RouteSpec::buses`].
    pub bus: usize,
    /// Gain in dB.
    pub gain_db: Option<f32>,
    /// Muted.
    pub muted: Option<bool>,
    /// The switch of its equalizer.
    pub eq_on: Option<bool>,
    /// Settings of the bands of its equalizer, each with its band from 0. They glide at their own
    /// pace, not the scene's.
    pub eq: Vec<(usize, BandChange)>,
}

/// A named state of the mixer. It changes values only and opens no device, so it never
/// interrupts the sound.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneSpec {
    /// What it is called; unique in a route, ignoring case.
    pub name: String,
    /// Milliseconds the change takes.
    pub blend_ms: u32,
    /// What changes on the inputs.
    pub inputs: Vec<SceneInput>,
    /// What changes on the buses.
    pub buses: Vec<SceneBus>,
}

/// What to route: inputs, buses and which input feeds which bus.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSpec {
    /// The inputs.
    pub inputs: Vec<InputSpec>,
    /// The buses, each at its own device's sample rate.
    pub buses: Vec<BusSpec>,
    /// Buffering per input in milliseconds; more is safer against dropouts, less is faster.
    pub latency_ms: u32,
    /// The scenes it can change to.
    pub scenes: Vec<SceneSpec>,
}

impl RouteSpec {
    /// Checks what does not need a device: at least one input and one bus, every input
    /// routed to existing buses, each at most once, and scenes that name only what is there.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRoute`] naming the first problem.
    pub fn validate(&self) -> Result<(), Error> {
        let invalid = |why: String| Err(Error::InvalidRoute(why));
        if self.inputs.is_empty() {
            return invalid("a route needs at least one input".into());
        }
        if self.buses.is_empty() {
            return invalid("a route needs at least one bus (output)".into());
        }
        for input in &self.inputs {
            if input.buses.is_empty() {
                return invalid(format!("input \"{}\" feeds no bus", input.device));
            }
            for (what, amount) in [("gate", input.gate), ("comp", input.comp)] {
                if let Err(why) = pult_core::view::check_amount(what, amount) {
                    return invalid(format!("input \"{}\": {why}", input.device));
                }
            }
            for (i, &bus) in input.buses.iter().enumerate() {
                if bus >= self.buses.len() {
                    return invalid(format!(
                        "input \"{}\" feeds bus {}, but there are only {}",
                        input.device,
                        bus + 1,
                        self.buses.len()
                    ));
                }
                if input.buses[..i].contains(&bus) {
                    return invalid(format!(
                        "input \"{}\" feeds bus {} twice",
                        input.device, self.buses[bus].name
                    ));
                }
            }
        }
        for bus in &self.buses {
            for (index, band) in bus.eq.iter().enumerate() {
                if let Err(why) = band.checked() {
                    return invalid(format!("bus {}, band {}: {why}", bus.name, index + 1));
                }
            }
        }
        for scene in &self.scenes {
            let fits = scene.inputs.iter().all(|change| {
                change.input < self.inputs.len()
                    && change.buses.as_ref().is_none_or(|buses| {
                        !buses.is_empty() && buses.iter().all(|&bus| bus < self.buses.len())
                    })
            }) && scene.buses.iter().all(|change| {
                change.bus < self.buses.len() && change.eq.iter().all(|(band, _)| *band < EQ_BANDS)
            });
            if !fits {
                return invalid(format!("scene \"{}\" does not fit the route", scene.name));
            }
        }
        Ok(())
    }
}

/// One ring of a running input: its way onto one bus. Every input has one for every bus;
/// only the routed ones play.
#[derive(Debug)]
pub struct RunningRoute {
    /// Index into [`Running::buses`].
    pub bus: usize,
    /// Peak and counters of this ring.
    pub stats: Arc<InputStats>,
    /// Whether the input feeds this bus now (the matrix button).
    pub routed: Arc<AtomicBool>,
}

impl RunningRoute {
    /// Whether the input feeds this bus now.
    #[must_use]
    pub fn is_routed(&self) -> bool {
        self.routed.load(Ordering::Acquire)
    }
}

/// One input of a running route.
#[derive(Debug)]
pub struct RunningInput {
    /// The device's full name (as last opened).
    pub name: String,
    /// Gain, mute and pan, for all its routes.
    pub controls: Arc<InputControls>,
    /// One per bus it feeds, in the order of the spec.
    pub routes: Vec<RunningRoute>,
    /// The rate its rings and resamplers were made for, in Hz: the device's, or for a
    /// program the rate of the first bus it feeds.
    pub sample_rate: u32,
    /// Whether the device is playing; false while it is gone.
    pub present: bool,
    /// Times the device was lost so far.
    pub outages: u64,
}

/// One bus of a running route.
#[derive(Debug)]
pub struct RunningBus {
    /// The bus's short name.
    pub name: String,
    /// The output device's full name (as last opened).
    pub device: String,
    /// Gain, mute and statistics.
    pub handle: BusHandle,
    /// The output device's rate, in Hz; the bus's inputs are resampled to it.
    pub sample_rate: u32,
    /// Whether the device is playing; false while it is gone.
    pub present: bool,
    /// Times the device was lost so far.
    pub outages: u64,
}

/// What [`Running::supervise`] did to a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The device stopped; the route plays on without it and looks for it every second.
    Lost {
        /// The input (`[1] CABLE Output`) or bus (`A1 Focusrite`).
        label: String,
        /// The audio system's error, or that its callbacks stopped.
        reason: String,
    },
    /// The device is there again but cannot be opened yet; reported once per reason.
    Refused {
        /// The input or bus.
        label: String,
        /// Why not.
        reason: String,
    },
    /// The device plays again.
    Back {
        /// The input or bus, with the device's name now.
        label: String,
    },
    /// The device's callback thread has no realtime priority: Windows refused it, and pult
    /// cannot ask for it elsewhere yet. It plays, but heavy load (a build on every core) can make
    /// it drop out.
    Slow {
        /// The input or bus.
        label: String,
    },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lost { label, reason } => write!(
                f,
                "{label} is gone ({reason}); the rest plays on, it is taken back when it returns"
            ),
            Self::Refused { label, reason } => {
                write!(f, "{label} is there but does not open yet: {reason}")
            }
            Self::Back { label } => write!(f, "{label} is back"),
            Self::Slow { label } => write!(
                f,
                "{label} plays without realtime priority ({NO_REALTIME}); under heavy load it \
                 may drop out"
            ),
        }
    }
}

/// Why a callback thread has no realtime priority.
const NO_REALTIME: &str = if cfg!(windows) {
    "Windows refused MMCSS \"Pro Audio\""
} else {
    "pult cannot ask for it on this system yet"
};

/// A started route; dropping it stops all streams.
pub struct Running {
    host: cpal::Host,
    /// The inputs' slots, then the buses'.
    slots: Vec<Slot>,
    /// The inputs, in the order of the spec.
    pub inputs: Vec<RunningInput>,
    /// The buses, in the order of the spec.
    pub buses: Vec<RunningBus>,
    xruns: Arc<Counter>,
    /// The spec as it stands now: changed by `apply` along with the controls.
    spec: RouteSpec,
}

impl fmt::Debug for Running {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Running")
            .field("inputs", &self.inputs)
            .field("buses", &self.buses)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// Glitches the devices reported (buffer under- or overruns inside the audio system).
    #[must_use]
    pub fn xruns(&self) -> u64 {
        self.xruns.get()
    }

    /// Checks every device: a stream that failed or stopped calling back is taken out, and
    /// a lost device is looked for again (each one at most every second). Returns what
    /// changed; call it a few times a second from the control thread.
    pub fn supervise(&mut self) -> Vec<Change> {
        let now = Instant::now();
        let mut changes = Vec::new();
        for index in 0..self.slots.len() {
            if self.slots[index].newly_slow() {
                changes.push(Change::Slow {
                    label: self.label(index),
                });
            }
            if let Some(reason) = self.slots[index].trouble(now) {
                self.slots[index].close(now);
                self.mark(index, false);
                changes.push(Change::Lost {
                    label: self.label(index),
                    reason: readable(&reason),
                });
            } else if self.slots[index].retry_due(now) {
                match self.reopen(index) {
                    Ok(()) => {
                        self.mark(index, true);
                        changes.push(Change::Back {
                            label: self.label(index),
                        });
                    }
                    // Not plugged in yet: nothing worth a line every second.
                    Err(Error::Select {
                        source: SelectError::NotFound { .. },
                        ..
                    }) => {}
                    Err(err) => {
                        let reason = readable(&err.to_string());
                        if self.slots[index].refused.as_ref() != Some(&reason) {
                            self.slots[index].refused = Some(reason.clone());
                            changes.push(Change::Refused {
                                label: self.label(index),
                                reason,
                            });
                        }
                    }
                }
            }
        }
        changes
    }

    /// Looks for the device of a closed slot and opens it, if it still runs at the rate its
    /// buffers were made for.
    fn reopen(&mut self, index: usize) -> Result<(), Error> {
        let slot = &self.slots[index];
        let expected = match index.checked_sub(self.inputs.len()) {
            None => self.inputs[index].sample_rate,
            Some(bus) => self.buses[bus].sample_rate,
        };
        if slot.role == Role::Process {
            let found = process::find(&slot.wanted)?;
            let source = Source::Process {
                pid: found.pid,
                name: &found.name,
                sample_rate: expected,
            };
            return self.slots[index].open(&source, &self.xruns);
        }
        let (device, name, config) = find_device(&self.host, slot.role, &slot.wanted)?;
        if config.sample_rate() != expected {
            return Err(Error::RateChanged {
                device: name,
                rate: config.sample_rate(),
                expected,
            });
        }
        let source = Source::Device {
            device: &device,
            config: &config,
        };
        self.slots[index].open(&source, &self.xruns)
    }

    /// Updates the input or bus behind a slot after it closed or opened.
    fn mark(&mut self, index: usize, present: bool) {
        let name = self.slots[index].name.clone();
        let (state, outages) = match index.checked_sub(self.inputs.len()) {
            None => {
                let input = &mut self.inputs[index];
                input.name = name;
                (&mut input.present, &mut input.outages)
            }
            Some(bus) => {
                let bus = &mut self.buses[bus];
                bus.device = name;
                (&mut bus.present, &mut bus.outages)
            }
        };
        if *state && !present {
            *outages += 1;
        }
        *state = present;
    }

    /// `[1] CABLE Output` or `A1 Focusrite`.
    fn label(&self, index: usize) -> String {
        match index.checked_sub(self.inputs.len()) {
            None => format!("[{}] {}", index + 1, self.inputs[index].name),
            Some(bus) => format!("{} {}", self.buses[bus].name, self.buses[bus].device),
        }
    }
}

/// Windows cannot put the audio client's own error codes into words (`FormatMessageW()`
/// returned error 317); the ones a lost or busy device brings, in words with their name.
const AUDIO_CLIENT_ERRORS: &[(i32, &str)] = &[
    (
        -2_004_287_484,
        "the device was removed or disabled (AUDCLNT_E_DEVICE_INVALIDATED)",
    ),
    (
        -2_004_287_478,
        "another program uses the device exclusively (AUDCLNT_E_DEVICE_IN_USE)",
    ),
    (
        -2_004_287_472,
        "the Windows audio service is not running (AUDCLNT_E_SERVICE_NOT_RUNNING)",
    ),
    (
        -2_004_287_450,
        "Windows reset the audio device (AUDCLNT_E_RESOURCES_INVALIDATED)",
    ),
];

/// `reason` with a known audio client error code put into words; anything else unchanged.
fn readable(reason: &str) -> String {
    for (code, words) in AUDIO_CLIENT_ERRORS {
        if reason.contains(&format!("os error {code})")) {
            return (*words).to_owned();
        }
    }
    reason.to_owned()
}

fn pick(devices: &[Device], wanted: &str, role: Role) -> Result<usize, Error> {
    let names: Vec<String> = devices.iter().map(ToString::to_string).collect();
    select(wanted, &names).map_err(|source| Error::Select { role, source })
}

fn require_mixable(device: &str, config: &SupportedStreamConfig) -> Result<(), Error> {
    if convert::supported(config.sample_format()) {
        Ok(())
    } else {
        Err(Error::UnsupportedFormat {
            device: device.to_owned(),
            format: config.sample_format().to_string(),
        })
    }
}

/// The device of `role` that `wanted` names, its full name and its shared-mode format.
fn find_device(
    host: &cpal::Host,
    role: Role,
    wanted: &str,
) -> Result<(Device, String, SupportedStreamConfig), Error> {
    let mut devices: Vec<Device> = if role == Role::Output {
        host.output_devices()
            .map_err(audio("listing output devices"))?
            .collect()
    } else {
        host.input_devices()
            .map_err(audio("listing input devices"))?
            .collect()
    };
    let device = devices.swap_remove(pick(&devices, wanted, role)?);
    let name = device.to_string();
    let config = if role == Role::Output {
        device.default_output_config()
    } else {
        device.default_input_config()
    }
    .map_err(audio(format!("reading the format of {name}")))?;
    require_mixable(&name, &config)?;
    Ok((device, name, config))
}

/// A playing test tone; dropping it stops the tone.
pub struct Tone {
    _stream: cpal::Stream,
    /// The output device's full name.
    pub sink: String,
    /// The tone's sample rate in Hz.
    pub sample_rate: u32,
    health: Arc<Health>,
}

impl fmt::Debug for Tone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tone")
            .field("sink", &self.sink)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Tone {
    /// The first fatal stream error so far, if any.
    #[must_use]
    pub fn fault(&self) -> Option<String> {
        self.health
            .fatal()
            .map(|err| format!("{}: {err}", self.sink))
    }
}

/// Plays a sine of `frequency` Hz at `gain_db` on the output `sink` names.
///
/// # Errors
///
/// [`Error`] when the device is missing or ambiguous, has an unsupported format, or the
/// stream cannot be opened or started.
pub fn play_tone(sink: &str, frequency: f32, gain_db: f32) -> Result<Tone, Error> {
    let host = cpal::default_host();
    let health = Arc::new(Health::default());
    let (device, name, config) = find_device(&host, Role::Output, sink)?;
    let sample_rate = config.sample_rate();
    let channels = usize::from(config.channels());
    let mut oscillator = Sine::new(frequency, sample_rate, db_to_linear(gain_db));
    let stream = convert::build_output(
        &device,
        &config,
        move |data| assert_no_alloc(|| oscillator.fill(data, channels)),
        fault_recorder(&health, &Arc::new(Counter::default())),
    )?;
    stream.play().map_err(audio(format!("starting {name}")))?;
    Ok(Tone {
        _stream: stream,
        sink: name,
        sample_rate,
        health,
    })
}

/// Buffer sizes in frames for a rate and latency: (target, band, ring capacity).
fn buffering(sample_rate: u32, latency_ms: u32) -> (usize, usize, usize) {
    let target = (u64::from(sample_rate) * u64::from(latency_ms.max(1)) / 1000) as usize;
    let band = (target / 4).max(1);
    // The drift control resyncs above its hard limit; the ring holds that and a second.
    let capacity = DriftControl::new(target, band).hard_limit() + sample_rate as usize;
    (target, band, capacity)
}

/// The output device of a bus, found before its mixer exists.
struct FoundBus {
    device: Device,
    name: String,
    config: SupportedStreamConfig,
}

/// Finds the output device of every bus, each device at most once.
fn find_buses(host: &cpal::Host, spec: &RouteSpec) -> Result<Vec<FoundBus>, Error> {
    let mut found: Vec<FoundBus> = Vec::with_capacity(spec.buses.len());
    for bus in &spec.buses {
        let (device, name, config) = find_device(host, Role::Output, &bus.device)?;
        if let Some(other) = found.iter().position(|b| b.name == name) {
            return Err(Error::InvalidRoute(format!(
                "buses {} and {} both play on {name}",
                spec.buses[other].name, bus.name
            )));
        }
        found.push(FoundBus {
            device,
            name,
            config,
        });
    }
    Ok(found)
}

/// Where an input's rings go and how they buffer.
struct Rings<'a> {
    /// The mixer inputs of every bus, filled as inputs are made.
    bus_inputs: &'a mut [Vec<MixerInput>],
    /// The sample rate of every bus's device, in Hz.
    bus_rates: &'a [u32],
    /// Buffering per ring in milliseconds.
    latency_ms: u32,
}

/// What a slot opens at the start of a route.
enum Opening {
    Device(Device, SupportedStreamConfig),
    Process {
        pid: u32,
        name: String,
        sample_rate: u32,
    },
}

/// An input whose device or program is found and whose rings exist, before its stream
/// opens.
struct FoundInput {
    opening: Opening,
    captures: Vec<Capture>,
    running: RunningInput,
}

/// Finds the device or program of one input and makes its rings: one per bus it feeds.
fn find_input(
    host: &cpal::Host,
    input: &InputSpec,
    rings: &mut Rings<'_>,
) -> Result<FoundInput, Error> {
    let (opening, name, channels, sample_rate) = match input.kind {
        InputKind::Device => {
            let (device, name, config) = find_device(host, Role::Input, &input.device)?;
            let (channels, rate) = (config.channels(), config.sample_rate());
            (Opening::Device(device, config), name, channels, rate)
        }
        InputKind::Process => {
            let found = process::find(&input.device)?;
            // Windows converts a program's sound to any rate; the first bus's spares the
            // resampler that bus's conversion.
            let rate = rings.bus_rates[input.buses[0]];
            let opening = Opening::Process {
                pid: found.pid,
                name: found.name.clone(),
                sample_rate: rate,
            };
            (opening, found.name, PROCESS_CHANNELS, rate)
        }
    };
    let controls = Arc::new(InputControls::default());
    controls.gain.store(db_to_linear(input.gain_db));
    controls.pan.store(input.pan);
    controls.gate.set_simple(input.gate);
    controls.compressor.set_simple(input.comp);
    controls.muted.store(input.muted, Ordering::Relaxed);
    let (target, band, capacity) = buffering(sample_rate, rings.latency_ms);
    let ring = InputConfig {
        channels: usize::from(channels),
        input_rate: sample_rate,
        output_rate: sample_rate,
        capacity,
        target,
        band,
        smoothing: DRIFT_SMOOTHING,
        settle: SETTLE_SECONDS * sample_rate as usize,
    };
    // A ring for every bus, so the matrix can change while the route runs; the ones the
    // spec does not route stay closed and cost no processing.
    let bus_count = rings.bus_rates.len();
    let mut captures = Vec::with_capacity(bus_count);
    let mut routes = Vec::with_capacity(bus_count);
    for bus in 0..bus_count {
        let ring = InputConfig {
            output_rate: rings.bus_rates[bus],
            ..ring
        };
        let (capture, mixer_input, handle) = connect_with(ring, Arc::clone(&controls));
        let mixer_input = if input.buses.contains(&bus) {
            mixer_input
        } else {
            mixer_input.unrouted()
        };
        captures.push(capture);
        rings.bus_inputs[bus].push(mixer_input);
        routes.push(RunningRoute {
            bus,
            stats: handle.stats,
            routed: handle.routed,
        });
    }
    Ok(FoundInput {
        opening,
        captures,
        running: RunningInput {
            name,
            controls,
            routes,
            sample_rate,
            present: false,
            outages: 0,
        },
    })
}

/// Opens every device of `spec` and starts mixing.
///
/// # Errors
///
/// [`Error`] when the route is invalid, a device is missing or ambiguous, has an
/// unsupported format or rate, or a stream cannot be opened or started.
pub fn start(spec: &RouteSpec) -> Result<Running, Error> {
    spec.validate()?;
    let host = cpal::default_host();
    let now = Instant::now();

    let found_buses = find_buses(&host, spec)?;
    let bus_rates: Vec<u32> = found_buses.iter().map(|b| b.config.sample_rate()).collect();

    let mut bus_inputs: Vec<Vec<MixerInput>> = spec.buses.iter().map(|_| Vec::new()).collect();
    let mut rings = Rings {
        bus_inputs: &mut bus_inputs,
        bus_rates: &bus_rates,
        latency_ms: spec.latency_ms,
    };
    let mut found_inputs = Vec::with_capacity(spec.inputs.len());
    for input in &spec.inputs {
        found_inputs.push(find_input(&host, input, &mut rings)?);
    }

    let mut slots = Vec::with_capacity(spec.inputs.len() + spec.buses.len());
    let mut opening = Vec::with_capacity(slots.capacity());
    let mut inputs = Vec::with_capacity(spec.inputs.len());
    for (input, found) in spec.inputs.iter().zip(found_inputs) {
        let feed = Feed::Input(found.captures);
        let role = match input.kind {
            InputKind::Device => Role::Input,
            InputKind::Process => Role::Process,
        };
        slots.push(Slot::new(role, &input.device, feed, now));
        opening.push(found.opening);
        inputs.push(found.running);
    }
    let mut buses = Vec::with_capacity(spec.buses.len());
    for ((bus, found), mixer_inputs) in spec.buses.iter().zip(found_buses).zip(bus_inputs) {
        let (mut mixer, handle) = Mixer::new(mixer_inputs, usize::from(found.config.channels()));
        // The input streams open first; nothing may pile up until this bus's stream calls back.
        mixer.suspend();
        let sample_rate = found.config.sample_rate();
        handle.controls.gain.store(db_to_linear(bus.gain_db));
        handle.controls.muted.store(bus.muted, Ordering::Relaxed);
        handle.controls.eq.set_all(bus.eq_on, &bus.eq);
        slots.push(Slot::new(Role::Output, &bus.device, Feed::Bus(mixer), now));
        opening.push(Opening::Device(found.device, found.config));
        buses.push(RunningBus {
            name: bus.name.clone(),
            device: found.name,
            handle,
            sample_rate,
            present: false,
            outages: 0,
        });
    }

    let mut running = Running {
        host,
        slots,
        inputs,
        buses,
        xruns: Arc::new(Counter::default()),
        spec: spec.clone(),
    };
    // Inputs first: a ring opens with the first callback of its output, and primes faster
    // when its input already delivers.
    for (index, opening) in opening.iter().enumerate() {
        let source = match opening {
            Opening::Device(device, config) => Source::Device { device, config },
            Opening::Process {
                pid,
                name,
                sample_rate,
            } => Source::Process {
                pid: *pid,
                name,
                sample_rate: *sample_rate,
            },
        };
        running.slots[index].open(&source, &running.xruns)?;
        running.mark(index, true);
    }
    Ok(running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffering_follows_rate_and_latency() {
        assert_eq!(buffering(48_000, 40), (1920, 480, 3840 + 1920 + 48_000));
        let (target, band, capacity) = buffering(44_100, 0);
        assert_eq!((target, band), (44, 11));
        assert!(capacity > DriftControl::new(target, band).hard_limit());
    }

    #[test]
    fn audio_client_errors_read_as_words() {
        // What cpal reports when the Scarlett is pulled during a run.
        let pulled = "Failed to get current padding: OS Error -2004287484 (FormatMessageW() \
                      returned error 317) (os error -2004287484)";
        assert_eq!(
            readable(pulled),
            "the device was removed or disabled (AUDCLNT_E_DEVICE_INVALIDATED)"
        );
        assert_eq!(readable("device not available"), "device not available");
        // The codes are the HRESULTs 0x8889_0004, 000A, 0010 and 0026 read as i32.
        for (code, hresult) in [
            (-2_004_287_484, 0x8889_0004_u32),
            (-2_004_287_478, 0x8889_000A),
            (-2_004_287_472, 0x8889_0010),
            (-2_004_287_450, 0x8889_0026),
        ] {
            assert_eq!(code, hresult.cast_signed());
            assert!(AUDIO_CLIENT_ERRORS.iter().any(|(c, _)| *c == code));
        }
    }

    #[test]
    fn changes_read_as_sentences() {
        let lost = Change::Lost {
            label: "A1 Focusrite".into(),
            reason: "device not available".into(),
        };
        assert_eq!(
            lost.to_string(),
            "A1 Focusrite is gone (device not available); the rest plays on, it is taken back \
             when it returns"
        );
        let back = Change::Back {
            label: "[1] CABLE Output".into(),
        };
        assert_eq!(back.to_string(), "[1] CABLE Output is back");
        let slow = Change::Slow {
            label: "A1 Focusrite".into(),
        };
        assert!(
            slow.to_string()
                .starts_with("A1 Focusrite plays without realtime priority"),
            "{slow}"
        );
        assert_eq!(
            slow.to_string().contains("Windows"),
            cfg!(windows),
            "{slow}"
        );
        let layout = Error::LayoutChanged {
            device: "Mic".into(),
            channels: 1,
            expected: 2,
        };
        assert!(layout.to_string().contains("1 channels instead of 2"));
    }

    #[test]
    fn rate_changed_names_both_rates_and_the_fix() {
        let message = Error::RateChanged {
            device: "Focusrite".into(),
            rate: 44_100,
            expected: 48_000,
        }
        .to_string();
        assert!(message.contains("Focusrite came back at 44100 Hz instead of 48000 Hz"));
        assert!(message.contains("default format"));
    }

    fn route(inputs: Vec<InputSpec>, buses: usize) -> RouteSpec {
        RouteSpec {
            inputs,
            buses: (1..=buses)
                .map(|i| BusSpec::new(format!("A{i}"), format!("out {i}")))
                .collect(),
            latency_ms: 40,
            scenes: Vec::new(),
        }
    }

    #[test]
    fn a_scene_has_to_fit_the_route() {
        let mut spec = route(vec![InputSpec::new("in", 0.0, vec![0])], 1);
        spec.validate().expect("no scenes");
        let scene = |inputs, buses| SceneSpec {
            name: "abend".into(),
            blend_ms: 0,
            inputs,
            buses,
        };
        let input = |input, buses| SceneInput {
            input,
            buses,
            ..SceneInput::default()
        };
        spec.scenes = vec![scene(vec![input(0, Some(vec![0]))], Vec::new())];
        spec.validate().expect("fits");
        for bad in [
            scene(vec![input(1, None)], Vec::new()),
            scene(vec![input(0, Some(vec![1]))], Vec::new()),
            scene(vec![input(0, Some(Vec::new()))], Vec::new()),
            scene(
                Vec::new(),
                vec![SceneBus {
                    bus: 1,
                    ..SceneBus::default()
                }],
            ),
        ] {
            spec.scenes = vec![bad];
            assert!(invalid(&spec).contains("scene \"abend\""));
        }
    }

    fn invalid(spec: &RouteSpec) -> String {
        match spec.validate() {
            Err(Error::InvalidRoute(why)) => why,
            other => panic!("expected an invalid route, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_a_matrix_and_names_each_problem() {
        let ok = route(
            vec![
                InputSpec::new("cable", 0.0, vec![0, 1]),
                InputSpec::new("mic", -6.0, vec![1]),
            ],
            2,
        );
        assert!(ok.validate().is_ok());

        assert!(invalid(&route(Vec::new(), 1)).contains("at least one input"));
        assert!(invalid(&route(vec![InputSpec::new("a", 0.0, vec![0])], 0)).contains("bus"));
        assert!(invalid(&route(vec![InputSpec::new("a", 0.0, vec![])], 1)).contains("no bus"));
        assert!(invalid(&route(vec![InputSpec::new("a", 0.0, vec![2])], 2)).contains("only 2"));
        assert!(
            invalid(&route(vec![InputSpec::new("a", 0.0, vec![1, 1])], 2)).contains("A2 twice")
        );
        let mut sharp = route(vec![InputSpec::new("a", 0.0, vec![0])], 1);
        sharp.buses[0].eq[2].q = 50.0;
        assert!(invalid(&sharp).contains("bus A1, band 3: the q of a band must be 0.1 to 10"));
        let loud = InputSpec {
            comp: 11.0,
            ..InputSpec::new("a", 0.0, vec![0])
        };
        assert!(invalid(&route(vec![loud], 1)).contains("comp must be 0 (off) to 10"));
    }
}
