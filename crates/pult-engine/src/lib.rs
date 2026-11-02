//! The audio devices of pult: lists them and runs one route, several inputs mixed into one
//! output, on the system's default host (WASAPI shared mode on Windows).
//!
//! Every stream uses its device's own shared-mode format. The device callbacks only move
//! samples through the rings and the mixer of `pult-core`; `assert_no_alloc` guards them, so
//! a debug build with the checking allocator aborts on any allocation inside a callback.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, SampleFormat, SupportedStreamConfig};
use pult_core::{
    Counter, DriftControl, InputConfig, InputHandle, MasterStats, Mixer, SelectError, Sine,
    connect, db_to_linear, select,
};

/// How long a backend may take to open a stream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Weight of one callback in the drift control's level average: with 10 ms callbacks the
/// average spans about 1.3 s, long enough to flatten the block jitter of virtual cables.
const DRIFT_SMOOTHING: f64 = 1.0 / 128.0;

/// Which side of a route a device is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A capture device (microphone, line in, the output side of a virtual cable).
    Input,
    /// A playback device (interface outputs, speakers, headphones).
    Output,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Input => "input",
            Self::Output => "output",
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
    /// A route needs at least one input.
    NoSources,
    /// The device's shared-mode format is not 32-bit float.
    UnsupportedFormat {
        /// The device.
        device: String,
        /// Its format.
        format: String,
    },
    /// An input runs at another sample rate than the output; there is no resampler yet.
    RateMismatch {
        /// The input device.
        input: String,
        /// Its rate in Hz.
        input_rate: u32,
        /// The output device.
        output: String,
        /// Its rate in Hz.
        output_rate: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio { context, source } => write!(f, "{context}: {source}"),
            Self::Select { role, source } => write!(f, "{role} device: {source}"),
            Self::NoSources => f.write_str("a route needs at least one input (--from)"),
            Self::UnsupportedFormat { device, format } => write!(
                f,
                "{device} uses the sample format {format}; only 32-bit float is supported yet"
            ),
            Self::RateMismatch {
                input,
                input_rate,
                output,
                output_rate,
            } => write!(
                f,
                "{input} runs at {input_rate} Hz but {output} at {output_rate} Hz; set both to \
                 the same rate (Sound settings, device properties, Advanced, default format)"
            ),
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

/// One input of a route.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpec {
    /// Device name, or a unique part of it.
    pub device: String,
    /// Gain in dB.
    pub gain_db: f32,
}

/// What to route: inputs mixed into one output.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSpec {
    /// The inputs.
    pub sources: Vec<SourceSpec>,
    /// The output device name, or a unique part of it.
    pub sink: String,
    /// Buffering per input in milliseconds; more is safer against dropouts, less is faster.
    pub latency_ms: u32,
}

/// One input of a running route.
#[derive(Debug)]
pub struct RunningInput {
    /// The device's full name.
    pub name: String,
    /// Gain, mute and statistics.
    pub handle: InputHandle,
}

/// A started route; dropping it stops all streams.
pub struct Running {
    _streams: Vec<cpal::Stream>,
    /// The inputs, in the order of the spec.
    pub inputs: Vec<RunningInput>,
    /// Statistics of the mix.
    pub master: Arc<MasterStats>,
    /// The output device's full name.
    pub sink: String,
    /// The route's sample rate in Hz.
    pub sample_rate: u32,
    faults: Arc<Faults>,
}

impl fmt::Debug for Running {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Running")
            .field("inputs", &self.inputs)
            .field("sink", &self.sink)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// The first fatal stream error so far (a device unplugged, the format changed), if any.
    #[must_use]
    pub fn fault(&self) -> Option<String> {
        self.faults.first()
    }

    /// Glitches the devices reported (buffer under- or overruns inside the audio system).
    #[must_use]
    pub fn xruns(&self) -> u64 {
        self.faults.xruns.get()
    }
}

fn pick(devices: &[Device], wanted: &str, role: Role) -> Result<usize, Error> {
    let names: Vec<String> = devices.iter().map(ToString::to_string).collect();
    select(wanted, &names).map_err(|source| Error::Select { role, source })
}

fn require_f32(device: &str, config: &SupportedStreamConfig) -> Result<(), Error> {
    if config.sample_format() == SampleFormat::F32 {
        Ok(())
    } else {
        Err(Error::UnsupportedFormat {
            device: device.to_owned(),
            format: config.sample_format().to_string(),
        })
    }
}

/// Stream errors of a route, written by the error callbacks (not the realtime ones).
#[derive(Debug, Default)]
struct Faults {
    fatal: Mutex<Option<String>>,
    xruns: Counter,
}

impl Faults {
    fn first(&self) -> Option<String> {
        self.fatal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The error callback of one device's stream. A glitch, a followed device change or a
/// refused realtime priority keeps the stream running and is not fatal; anything else is.
fn fault_recorder(faults: &Arc<Faults>, device: &str) -> impl FnMut(cpal::Error) + Send + 'static {
    let faults = Arc::clone(faults);
    let device = device.to_owned();
    move |err| {
        if err.kind() == ErrorKind::Xrun {
            faults.xruns.add(1);
        } else if is_fatal(err.kind()) {
            let mut slot = faults.fatal.lock().unwrap_or_else(PoisonError::into_inner);
            slot.get_or_insert_with(|| format!("{device}: {err}"));
        }
    }
}

/// Whether a stream error ends the route.
fn is_fatal(kind: ErrorKind) -> bool {
    !matches!(
        kind,
        ErrorKind::Xrun | ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied
    )
}

/// The output device `wanted` names, its full name and its shared-mode format.
fn find_output(
    host: &cpal::Host,
    wanted: &str,
) -> Result<(Device, String, SupportedStreamConfig), Error> {
    let mut outputs: Vec<Device> = host
        .output_devices()
        .map_err(audio("listing output devices"))?
        .collect();
    let sink = outputs.swap_remove(pick(&outputs, wanted, Role::Output)?);
    let name = sink.to_string();
    let config = sink
        .default_output_config()
        .map_err(audio(format!("reading the format of {name}")))?;
    require_f32(&name, &config)?;
    Ok((sink, name, config))
}

/// A playing test tone; dropping it stops the tone.
pub struct Tone {
    _stream: cpal::Stream,
    /// The output device's full name.
    pub sink: String,
    /// The tone's sample rate in Hz.
    pub sample_rate: u32,
    faults: Arc<Faults>,
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
        self.faults.first()
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
    let faults = Arc::new(Faults::default());
    let (device, name, config) = find_output(&host, sink)?;
    let sample_rate = config.sample_rate();
    let channels = usize::from(config.channels());
    let mut oscillator = Sine::new(frequency, sample_rate, db_to_linear(gain_db));
    let stream = device
        .build_output_stream::<f32, _, _>(
            config.config(),
            move |data, _| assert_no_alloc(|| oscillator.fill(data, channels)),
            fault_recorder(&faults, &name),
            Some(OPEN_TIMEOUT),
        )
        .map_err(audio(format!("opening {name}")))?;
    stream.play().map_err(audio(format!("starting {name}")))?;
    Ok(Tone {
        _stream: stream,
        sink: name,
        sample_rate,
        faults,
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

/// Opens every device of `spec` and starts mixing.
///
/// # Errors
///
/// [`Error`] when a device is missing or ambiguous, has an unsupported format or rate, or a
/// stream cannot be opened or started.
pub fn start(spec: &RouteSpec) -> Result<Running, Error> {
    if spec.sources.is_empty() {
        return Err(Error::NoSources);
    }
    let host = cpal::default_host();
    let faults = Arc::new(Faults::default());

    let (sink, sink_name, sink_config) = find_output(&host, &spec.sink)?;
    let sample_rate = sink_config.sample_rate();
    let out_channels = usize::from(sink_config.channels());
    let (target, band, capacity) = buffering(sample_rate, spec.latency_ms);

    let devices: Vec<Device> = host
        .input_devices()
        .map_err(audio("listing input devices"))?
        .collect();
    let mut streams = Vec::with_capacity(spec.sources.len() + 1);
    let mut mixer_inputs = Vec::with_capacity(spec.sources.len());
    let mut inputs = Vec::with_capacity(spec.sources.len());
    for source in &spec.sources {
        let device = &devices[pick(&devices, &source.device, Role::Input)?];
        let name = device.to_string();
        let config = device
            .default_input_config()
            .map_err(audio(format!("reading the format of {name}")))?;
        require_f32(&name, &config)?;
        if config.sample_rate() != sample_rate {
            return Err(Error::RateMismatch {
                input: name,
                input_rate: config.sample_rate(),
                output: sink_name,
                output_rate: sample_rate,
            });
        }
        let (mut capture, mixer_input, handle) = connect(InputConfig {
            channels: usize::from(config.channels()),
            capacity,
            target,
            band,
            smoothing: DRIFT_SMOOTHING,
        });
        handle.controls.gain.store(db_to_linear(source.gain_db));
        let stream = device
            .build_input_stream::<f32, _, _>(
                config.config(),
                move |data, _| assert_no_alloc(|| capture.push(data)),
                fault_recorder(&faults, &name),
                Some(OPEN_TIMEOUT),
            )
            .map_err(audio(format!("opening {name}")))?;
        streams.push(stream);
        mixer_inputs.push(mixer_input);
        inputs.push(RunningInput { name, handle });
    }

    let (mut mixer, master) = Mixer::new(mixer_inputs, out_channels);
    let output = sink
        .build_output_stream::<f32, _, _>(
            sink_config.config(),
            move |data, _| assert_no_alloc(|| mixer.process(data)),
            fault_recorder(&faults, &sink_name),
            Some(OPEN_TIMEOUT),
        )
        .map_err(audio(format!("opening {sink_name}")))?;
    streams.push(output);
    for (stream, name) in streams
        .iter()
        .zip(inputs.iter().map(|i| &i.name).chain([&sink_name]))
    {
        stream.play().map_err(audio(format!("starting {name}")))?;
    }

    Ok(Running {
        _streams: streams,
        inputs,
        master,
        sink: sink_name,
        sample_rate,
        faults,
    })
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
    fn only_lost_devices_and_real_failures_are_fatal() {
        for kind in [
            ErrorKind::Xrun,
            ErrorKind::DeviceChanged,
            ErrorKind::RealtimeDenied,
        ] {
            assert!(!is_fatal(kind), "{kind:?}");
        }
        for kind in [
            ErrorKind::DeviceNotAvailable,
            ErrorKind::StreamInvalidated,
            ErrorKind::BackendError,
        ] {
            assert!(is_fatal(kind), "{kind:?}");
        }
    }

    #[test]
    fn rate_mismatch_names_both_devices_and_the_fix() {
        let message = Error::RateMismatch {
            input: "CABLE Output".into(),
            input_rate: 44_100,
            output: "Focusrite".into(),
            output_rate: 48_000,
        }
        .to_string();
        assert!(message.contains("CABLE Output runs at 44100 Hz"));
        assert!(message.contains("Focusrite at 48000 Hz"));
        assert!(message.contains("default format"));
    }
}
