//! The audio devices of pult: lists them and runs a route, inputs mixed onto one or more
//! buses (each an output device), on the system's default host (WASAPI shared mode on
//! Windows).
//!
//! Every stream uses its device's own shared-mode format. An input routed to several buses
//! pushes each capture into one ring per bus, since every output runs on its own clock. The
//! device callbacks only move samples through the rings and mixers of `pult-core`;
//! `assert_no_alloc` guards them, so a debug build with the checking allocator aborts on
//! any allocation inside a callback.

pub mod profile;

use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, ErrorKind, SampleFormat, SupportedStreamConfig};
use pult_core::{
    BusHandle, Counter, DriftControl, InputConfig, InputControls, InputStats, Mixer, MixerInput,
    SelectError, Sine, connect_with, db_to_linear, select,
};

/// How long a backend may take to open a stream.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Weight of one callback in the drift control's level average: with 10 ms callbacks the
/// average spans about 1.3 s, long enough to flatten the block jitter of virtual cables.
const DRIFT_SMOOTHING: f64 = 1.0 / 128.0;

/// Seconds after each priming in which the drift control measures the start-up offset of
/// an input and then skips it once, instead of dropping single frames for twenty seconds.
const SETTLE_SECONDS: usize = 2;

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
    /// The route itself is inconsistent (no input, no bus, a route to a missing bus, one
    /// device on two buses).
    InvalidRoute(String),
    /// The device's shared-mode format is not 32-bit float.
    UnsupportedFormat {
        /// The device.
        device: String,
        /// Its format.
        format: String,
    },
    /// A device runs at another sample rate than the first bus; there is no resampler yet.
    RateMismatch {
        /// The device.
        device: String,
        /// Its rate in Hz.
        rate: u32,
        /// The first bus's device, which sets the route's rate.
        reference: String,
        /// Its rate in Hz.
        reference_rate: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio { context, source } => write!(f, "{context}: {source}"),
            Self::Select { role, source } => write!(f, "{role} device: {source}"),
            Self::InvalidRoute(why) => f.write_str(why),
            Self::UnsupportedFormat { device, format } => write!(
                f,
                "{device} uses the sample format {format}; only 32-bit float is supported yet"
            ),
            Self::RateMismatch {
                device,
                rate,
                reference,
                reference_rate,
            } => write!(
                f,
                "{device} runs at {rate} Hz but {reference} at {reference_rate} Hz; set both to \
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

/// One input (channel strip) of a route.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    /// Device name, or a unique part of it.
    pub device: String,
    /// Gain in dB.
    pub gain_db: f32,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    /// Starts muted.
    pub muted: bool,
    /// The buses it feeds, as indices into [`RouteSpec::buses`].
    pub buses: Vec<usize>,
}

impl InputSpec {
    /// An input at `gain_db`, centred, unmuted, feeding `buses`.
    #[must_use]
    pub fn new(device: impl Into<String>, gain_db: f32, buses: Vec<usize>) -> Self {
        Self {
            device: device.into(),
            gain_db,
            pan: 0.0,
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
    /// Gain of the mix in dB.
    pub gain_db: f32,
    /// Starts muted.
    pub muted: bool,
}

impl BusSpec {
    /// A bus at 0 dB, unmuted.
    #[must_use]
    pub fn new(name: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            device: device.into(),
            gain_db: 0.0,
            muted: false,
        }
    }
}

/// What to route: inputs, buses and which input feeds which bus.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSpec {
    /// The inputs.
    pub inputs: Vec<InputSpec>,
    /// The buses; the first sets the route's sample rate.
    pub buses: Vec<BusSpec>,
    /// Buffering per input in milliseconds; more is safer against dropouts, less is faster.
    pub latency_ms: u32,
}

impl RouteSpec {
    /// Checks what does not need a device: at least one input and one bus, every input
    /// routed to existing buses, each at most once.
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
        Ok(())
    }
}

/// One ring of a running input: its way onto one bus.
#[derive(Debug)]
pub struct RunningRoute {
    /// Index into [`Running::buses`].
    pub bus: usize,
    /// Peak and counters of this ring.
    pub stats: Arc<InputStats>,
}

/// One input of a running route.
#[derive(Debug)]
pub struct RunningInput {
    /// The device's full name.
    pub name: String,
    /// Gain, mute and pan, for all its routes.
    pub controls: Arc<InputControls>,
    /// One per bus it feeds, in the order of the spec.
    pub routes: Vec<RunningRoute>,
}

/// One bus of a running route.
#[derive(Debug)]
pub struct RunningBus {
    /// The bus's short name.
    pub name: String,
    /// The output device's full name.
    pub device: String,
    /// Gain, mute and statistics.
    pub handle: BusHandle,
}

/// A started route; dropping it stops all streams.
pub struct Running {
    _streams: Vec<cpal::Stream>,
    /// The inputs, in the order of the spec.
    pub inputs: Vec<RunningInput>,
    /// The buses, in the order of the spec.
    pub buses: Vec<RunningBus>,
    /// The route's sample rate in Hz.
    pub sample_rate: u32,
    faults: Arc<Faults>,
}

impl fmt::Debug for Running {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Running")
            .field("inputs", &self.inputs)
            .field("buses", &self.buses)
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

/// An opened output device of a bus, before its mixer exists.
struct OpenBus {
    device: Device,
    name: String,
    config: SupportedStreamConfig,
}

/// Finds the output device of every bus: each device at most once, all at the rate of the
/// first.
fn open_buses(host: &cpal::Host, spec: &RouteSpec) -> Result<Vec<OpenBus>, Error> {
    let mut open_buses: Vec<OpenBus> = Vec::with_capacity(spec.buses.len());
    for bus in &spec.buses {
        let (device, name, config) = find_output(host, &bus.device)?;
        if let Some(other) = open_buses.iter().position(|b| b.name == name) {
            return Err(Error::InvalidRoute(format!(
                "buses {} and {} both play on {name}",
                spec.buses[other].name, bus.name
            )));
        }
        if let Some(first) = open_buses.first()
            && config.sample_rate() != first.config.sample_rate()
        {
            return Err(Error::RateMismatch {
                device: name,
                rate: config.sample_rate(),
                reference: first.name.clone(),
                reference_rate: first.config.sample_rate(),
            });
        }
        open_buses.push(OpenBus {
            device,
            name,
            config,
        });
    }
    Ok(open_buses)
}

/// Where an input's rings go and how they buffer.
struct Rings<'a> {
    /// The mixer inputs of every bus, filled as inputs open.
    bus_inputs: &'a mut [Vec<MixerInput>],
    target: usize,
    band: usize,
    capacity: usize,
    sample_rate: u32,
    /// The first bus's device, named when an input's rate differs.
    reference: &'a str,
}

/// Opens the stream of one input: one ring per bus it feeds, all filled by one capture
/// callback.
fn open_input(
    devices: &[Device],
    input: &InputSpec,
    rings: &mut Rings<'_>,
    faults: &Arc<Faults>,
) -> Result<(cpal::Stream, RunningInput), Error> {
    let device = &devices[pick(devices, &input.device, Role::Input)?];
    let name = device.to_string();
    let config = device
        .default_input_config()
        .map_err(audio(format!("reading the format of {name}")))?;
    require_f32(&name, &config)?;
    if config.sample_rate() != rings.sample_rate {
        return Err(Error::RateMismatch {
            device: name,
            rate: config.sample_rate(),
            reference: rings.reference.to_owned(),
            reference_rate: rings.sample_rate,
        });
    }
    let controls = Arc::new(InputControls::default());
    controls.gain.store(db_to_linear(input.gain_db));
    controls.pan.store(input.pan);
    controls.muted.store(input.muted, Ordering::Relaxed);
    let ring = InputConfig {
        channels: usize::from(config.channels()),
        capacity: rings.capacity,
        target: rings.target,
        band: rings.band,
        smoothing: DRIFT_SMOOTHING,
        settle: SETTLE_SECONDS * rings.sample_rate as usize,
    };
    let mut captures = Vec::with_capacity(input.buses.len());
    let mut routes = Vec::with_capacity(input.buses.len());
    for &bus in &input.buses {
        let (capture, mixer_input, handle) = connect_with(ring, Arc::clone(&controls));
        captures.push(capture);
        rings.bus_inputs[bus].push(mixer_input);
        routes.push(RunningRoute {
            bus,
            stats: handle.stats,
        });
    }
    let stream = device
        .build_input_stream::<f32, _, _>(
            config.config(),
            move |data, _| {
                assert_no_alloc(|| {
                    for capture in &mut captures {
                        capture.push(data);
                    }
                });
            },
            fault_recorder(faults, &name),
            Some(OPEN_TIMEOUT),
        )
        .map_err(audio(format!("opening {name}")))?;
    let running = RunningInput {
        name,
        controls,
        routes,
    };
    Ok((stream, running))
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
    let faults = Arc::new(Faults::default());

    let open_buses = open_buses(&host, spec)?;
    let sample_rate = open_buses[0].config.sample_rate();
    let (target, band, capacity) = buffering(sample_rate, spec.latency_ms);

    let devices: Vec<Device> = host
        .input_devices()
        .map_err(audio("listing input devices"))?
        .collect();
    let mut streams = Vec::with_capacity(spec.inputs.len() + spec.buses.len());
    let mut stream_names = Vec::with_capacity(streams.capacity());
    let mut bus_inputs: Vec<Vec<MixerInput>> = spec.buses.iter().map(|_| Vec::new()).collect();
    let mut rings = Rings {
        bus_inputs: &mut bus_inputs,
        target,
        band,
        capacity,
        sample_rate,
        reference: &open_buses[0].name,
    };
    let mut inputs = Vec::with_capacity(spec.inputs.len());
    for input in &spec.inputs {
        let (stream, running) = open_input(&devices, input, &mut rings, &faults)?;
        streams.push(stream);
        stream_names.push(running.name.clone());
        inputs.push(running);
    }

    let mut buses = Vec::with_capacity(spec.buses.len());
    for ((bus, open), mixer_inputs) in spec.buses.iter().zip(open_buses).zip(bus_inputs) {
        let (mut mixer, handle) = Mixer::new(mixer_inputs, usize::from(open.config.channels()));
        handle.controls.gain.store(db_to_linear(bus.gain_db));
        handle.controls.muted.store(bus.muted, Ordering::Relaxed);
        let stream = open
            .device
            .build_output_stream::<f32, _, _>(
                open.config.config(),
                move |data, _| assert_no_alloc(|| mixer.process(data)),
                fault_recorder(&faults, &open.name),
                Some(OPEN_TIMEOUT),
            )
            .map_err(audio(format!("opening {}", open.name)))?;
        streams.push(stream);
        stream_names.push(open.name.clone());
        buses.push(RunningBus {
            name: bus.name.clone(),
            device: open.name,
            handle,
        });
    }
    // Inputs first, so the outputs find their rings filling.
    for (stream, name) in streams.iter().zip(&stream_names) {
        stream.play().map_err(audio(format!("starting {name}")))?;
    }

    Ok(Running {
        _streams: streams,
        inputs,
        buses,
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
            device: "CABLE Output".into(),
            rate: 44_100,
            reference: "Focusrite".into(),
            reference_rate: 48_000,
        }
        .to_string();
        assert!(message.contains("CABLE Output runs at 44100 Hz"));
        assert!(message.contains("Focusrite at 48000 Hz"));
        assert!(message.contains("default format"));
    }

    fn route(inputs: Vec<InputSpec>, buses: usize) -> RouteSpec {
        RouteSpec {
            inputs,
            buses: (1..=buses)
                .map(|i| BusSpec::new(format!("A{i}"), format!("out {i}")))
                .collect(),
            latency_ms: 40,
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
    }
}
