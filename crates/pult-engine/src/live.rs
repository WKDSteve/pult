//! Changing a running route: the commands of `pult_core::control` applied to its controls,
//! and its current state written back as a profile.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::Ordering;

use pult_core::control::{Command, HELP, Target, search_name};
use pult_core::limiter::LimitChange;
use pult_core::view::{band_line, check_amount, eq_summary};
use pult_core::{EQ_BANDS, EqControls, InputControls, MAX_RAMP_MS, db_to_linear};

use crate::profile::{self, GAIN_DB};
use crate::{
    BusSpec, InputSpec, RouteSpec, Running, RunningRoute, SceneBus, SceneInput, SceneSpec, SendSpec,
};

/// What of a channel strip is not as it starts, for `show`.
#[allow(clippy::float_cmp)] // Exactly: as it starts, or set.
fn strip_summary(strip: &pult_core::strip::StripSettings) -> String {
    use pult_core::strip::{HPF_RANGE_HZ, LPF_RANGE_HZ};
    let mut text = String::new();
    if strip.polarity {
        text.push_str("  polarity over");
    }
    if strip.hpf_hz > HPF_RANGE_HZ.0 {
        let _ = write!(text, "  hpf {:.0} Hz", strip.hpf_hz);
    }
    if strip.lpf_hz < LPF_RANGE_HZ.1 {
        let _ = write!(text, "  lpf {:.0} Hz", strip.lpf_hz);
    }
    if strip.delay_ms > 0.0 {
        let _ = write!(text, "  delay {:.1} ms", strip.delay_ms);
    }
    if strip.width != 100.0 {
        let _ = write!(text, "  width {:.0} %", strip.width);
    }
    text
}

/// Milliseconds a dim or the talkback takes to come and go.
const DIM_MS: u32 = 50;

impl Running {
    /// Sets the delay of `bus` in milliseconds, and returns what to show for it.
    ///
    /// # Errors
    ///
    /// A message when there is no such bus or the delay is out of its range.
    pub fn bus_delay(&mut self, bus: usize, ms: f32) -> Result<Vec<String>, String> {
        let ms = pult_core::busdelay::check_delay(ms)?;
        let Some(running) = self.buses.get(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        running.handle.controls.delay.set(ms);
        self.spec.buses[bus].delay_ms = ms;
        Ok(vec![format!("{}: delay {ms:.1} ms", running.name)])
    }

    /// Makes one change to the limiter of `bus`, and returns what to show for it.
    fn change_limit(&mut self, bus: usize, change: LimitChange) -> Result<Vec<String>, String> {
        let change = change.checked()?;
        let spec = &mut self.spec.buses[bus];
        spec.limit.apply(change);
        let line = spec.limit.line();
        self.buses[bus].handle.controls.limiter.change(change);
        Ok(vec![format!("{}: {line}", self.buses[bus].name)])
    }

    /// The input names a command may name, labels included, in route order.
    #[must_use]
    pub fn input_names(&self) -> Vec<String> {
        self.inputs
            .iter()
            .zip(&self.spec.inputs)
            .map(|(input, spec)| search_name(&input.name, spec.label.as_deref()))
            .collect()
    }

    /// The bus names, in route order.
    #[must_use]
    pub fn bus_names(&self) -> Vec<String> {
        self.buses.iter().map(|bus| bus.name.clone()).collect()
    }

    /// The bus labels, in route order; empty for a bus without one.
    #[must_use]
    pub fn bus_labels(&self) -> Vec<String> {
        self.spec
            .buses
            .iter()
            .map(|bus| bus.label.clone().unwrap_or_default())
            .collect()
    }

    /// The route as it is now: the spec it started with, with the gains, mutes, pans, gates
    /// and compressors changed since.
    #[must_use]
    pub fn current(&self) -> RouteSpec {
        self.spec.clone()
    }

    /// Applies `command` and returns what to show for it, one line each. `Stop` changes
    /// nothing; ending the run is the caller's business.
    ///
    /// # Errors
    ///
    /// A message when a value is out of range or the profile cannot be written.
    pub fn apply(&mut self, command: &Command) -> Result<Vec<String>, String> {
        match command {
            Command::Gain(target, gain_db, ramp_ms) => {
                if !GAIN_DB.contains(gain_db) {
                    return Err(format!(
                        "gain must be {} to {} dB, not {gain_db}",
                        GAIN_DB.start(),
                        GAIN_DB.end()
                    ));
                }
                check_ramp(*ramp_ms)?;
                self.set_gain(*target, *gain_db, *ramp_ms);
                Ok(vec![format!(
                    "{}: gain {gain_db:.1} dB{}",
                    self.target_label(*target),
                    over(*ramp_ms)
                )])
            }
            Command::Mute(target, muted) => {
                self.set_mute(*target, *muted, 0);
                let state = if *muted { "muted" } else { "unmuted" };
                Ok(vec![format!("{}: {state}", self.target_label(*target))])
            }
            Command::Pan(index, pan, ramp_ms) => {
                if !(-1.0..=1.0).contains(pan) {
                    return Err(format!("pan must be -1 to 1, not {pan}"));
                }
                check_ramp(*ramp_ms)?;
                self.set_pan(*index, *pan, *ramp_ms);
                Ok(vec![format!(
                    "{}: pan {pan:.2}{}",
                    self.target_label(Target::Input(*index)),
                    over(*ramp_ms)
                )])
            }
            Command::Gate(index, amount) => self.dynamics(*index, true, *amount),
            Command::Comp(index, amount) => self.dynamics(*index, false, *amount),
            Command::Eq(bus, on) => {
                self.buses[*bus].handle.controls.eq.switch(*on);
                self.spec.buses[*bus].eq_on = *on;
                let state = if *on { "on" } else { "off" };
                Ok(vec![format!("{}: eq {state}", self.buses[*bus].name)])
            }
            Command::Limit(bus, change) => self.change_limit(*bus, *change),
            Command::EqBand(bus, band, change) => {
                let Some(set) = self.spec.buses[*bus].eq.get_mut(*band) else {
                    return Err(format!(
                        "there is no band {}; bands are 1 to {EQ_BANDS}",
                        band + 1
                    ));
                };
                *set = set.changed(*change)?;
                let set = *set;
                self.buses[*bus].handle.controls.eq.set(*band, set);
                Ok(vec![format!(
                    "{}: eq {}: {}",
                    self.buses[*bus].name,
                    band + 1,
                    band_line(set)
                )])
            }
            Command::Route(input, bus, on) => self.route(*input, *bus, *on),
            Command::Monitor(bus, on) => self.monitor(*bus, *on),
            Command::Dim(bus, on) => self.dim(*bus, Some(*on), None),
            Command::DimLevel(bus, db) => self.dim(*bus, None, Some(*db)),
            Command::Talkback(input, on) => self.talkback(*input, *on),
            Command::Talk(on) => Ok(self.talk(*on)),
            Command::Pfl(on) => Ok(self.pfl(*on)),
            Command::BusComp(bus, amount) => self.bus_comp(*bus, Some(*amount), None, None),
            Command::BusCompMix(bus, percent) => self.bus_comp(*bus, None, Some(*percent), None),
            Command::BusCompHpf(bus, hz) => self.bus_comp(*bus, None, None, Some(*hz)),
            Command::Multiband(bus, change) => self.multiband(*bus, *change),
            Command::BusDelay(bus, ms) => self.bus_delay(*bus, *ms),
            Command::Strip(input, change) => self.strip(*input, *change),
            Command::Duck(input, change) => self.duck(*input, *change),
            Command::Automix(input, on) => self.automix(*input, Some(*on), None),
            Command::AutomixWeight(input, db) => self.automix(*input, None, Some(*db)),
            Command::Send(input, bus, gain_db, ramp_ms) => {
                self.send(*input, *bus, *gain_db, *ramp_ms)
            }
            Command::SendPre(input, bus, pre) => self.send_pre(*input, *bus, *pre),
            Command::Scene(name) => self.scene(name),
            Command::Scenes => Ok(vec![self.scene_names()]),
            // Who is soloed silences the others, and only the desk knows them all.
            Command::Solo(..) => Err("solo is not for the route but for its desk".to_owned()),
            Command::Learn(_) | Command::Unlearn(_) => {
                Err("MIDI is not for the route but for the one who runs it".to_owned())
            }
            Command::Show => Ok(self.show()),
            Command::Save(path) => self.save_profile(Path::new(path)),
            Command::Help => Ok(HELP.lines().map(str::to_owned).collect()),
            Command::Stop => Ok(Vec::new()),
        }
    }

    /// Sets a gain in dB, reached in `ramp_ms`; the range is the caller's to check.
    fn set_gain(&mut self, target: Target, gain_db: f32, ramp_ms: u32) {
        let linear = db_to_linear(gain_db);
        match target {
            Target::Input(index) => {
                self.inputs[index].controls.gain.ramp_to(linear, ramp_ms);
                self.spec.inputs[index].gain_db = gain_db;
            }
            Target::Bus(index) => {
                self.spec.buses[index].gain_db = gain_db;
                self.apply_bus_gain(index, ramp_ms);
            }
        }
    }

    /// Mutes or unmutes, fading for `fade_ms`.
    fn set_mute(&mut self, target: Target, muted: bool, fade_ms: u32) {
        match target {
            Target::Input(index) => {
                self.spec.inputs[index].muted = muted;
                self.apply_input_mute(index, fade_ms);
            }
            Target::Bus(index) => {
                let controls = &self.buses[index].handle.controls;
                controls.fade_ms.store(fade_ms, Ordering::Relaxed);
                controls.muted.store(muted, Ordering::Release);
                self.spec.buses[index].muted = muted;
            }
        }
    }

    /// The gain of bus `index` as its fader and its dim say, reached in `ramp_ms`: dimmed by its
    /// switch, or, a monitor bus, while someone talks.
    fn apply_bus_gain(&self, index: usize, ramp_ms: u32) {
        let spec = &self.spec.buses[index];
        let dimmed = self.dim[index] || (self.talk && spec.monitor);
        let db = spec.gain_db - if dimmed { spec.dim_db } else { 0.0 };
        self.buses[index]
            .handle
            .controls
            .gain
            .ramp_to(db_to_linear(db), ramp_ms);
    }

    /// The mute of input `index` as its switch and the talkback say, fading for `fade_ms`: a
    /// talkback microphone is silent until someone talks.
    fn apply_input_mute(&self, index: usize, fade_ms: u32) {
        let spec = &self.spec.inputs[index];
        let controls = &self.inputs[index].controls;
        controls.fade_ms.store(fade_ms, Ordering::Relaxed);
        controls.muted.store(
            spec.muted || (spec.talkback && !self.talk),
            Ordering::Release,
        );
    }

    /// Whether the dim switch of `bus` is on.
    #[must_use]
    pub fn is_dimmed(&self, bus: usize) -> bool {
        self.dim.get(bus).copied().unwrap_or(false)
    }

    /// Whether someone talks.
    #[must_use]
    pub fn is_talking(&self) -> bool {
        self.talk
    }

    /// Switches the dim of `bus` and/or sets how far it turns down, in dB.
    ///
    /// # Errors
    ///
    /// A message when there is no such bus or the level is out of its range.
    pub fn dim(
        &mut self,
        bus: usize,
        on: Option<bool>,
        level_db: Option<f32>,
    ) -> Result<Vec<String>, String> {
        if bus >= self.buses.len() {
            return Err(format!("there is no bus {}", bus + 1));
        }
        let level_db = level_db.map(pult_core::view::check_dim).transpose()?;
        let mut said = Vec::new();
        if let Some(db) = level_db {
            self.spec.buses[bus].dim_db = db;
            said.push(format!("{}: dim level {db:.0} dB", self.buses[bus].name));
        }
        if let Some(on) = on {
            self.dim[bus] = on;
            let state = if on { "dimmed" } else { "no longer dimmed" };
            said.push(format!("{}: {state}", self.buses[bus].name));
        }
        self.apply_bus_gain(bus, DIM_MS);
        Ok(said)
    }

    /// Makes `input` a talkback microphone (silent until someone talks) or a plain input again.
    ///
    /// # Errors
    ///
    /// A message when there is no such input.
    pub fn talkback(&mut self, input: usize, on: bool) -> Result<Vec<String>, String> {
        let Some(spec) = self.spec.inputs.get_mut(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        spec.talkback = on;
        self.apply_input_mute(input, DIM_MS);
        let label = self.target_label(Target::Input(input));
        Ok(vec![if on {
            format!("{label}: a talkback microphone, silent until talk on")
        } else {
            format!("{label}: no talkback microphone")
        }])
    }

    /// Opens the talkback microphones and dims the monitor buses, or ends that.
    pub fn talk(&mut self, on: bool) -> Vec<String> {
        self.talk = on;
        for input in 0..self.inputs.len() {
            if self.spec.inputs[input].talkback {
                self.apply_input_mute(input, DIM_MS);
            }
        }
        for bus in 0..self.buses.len() {
            self.apply_bus_gain(bus, DIM_MS);
        }
        let microphones = self.spec.inputs.iter().filter(|i| i.talkback).count();
        vec![match (on, microphones) {
            (true, 0) => "talk on, but no input is a talkback microphone".to_owned(),
            (true, _) => "talk on".to_owned(),
            (false, _) => "talk off".to_owned(),
        }]
    }

    /// Sets a pan, reached in `ramp_ms`; the range is the caller's to check.
    fn set_pan(&mut self, input: usize, pan: f32, ramp_ms: u32) {
        self.inputs[input].controls.pan.ramp_to(pan, ramp_ms);
        self.spec.inputs[input].pan = pan;
    }

    /// Changes to the scene called `name`, ignoring case: all its values together, gains, pans,
    /// mutes and routes over its blend time, gate, compressor and equalizer along their own times.
    /// Only values change, so nothing interrupts the sound. Saving afterwards saves them as the
    /// state of the route, as with any other change.
    ///
    /// # Errors
    ///
    /// A message listing the scenes when there is none of that name.
    pub fn scene(&mut self, name: &str) -> Result<Vec<String>, String> {
        let Some(scene) = self
            .spec
            .scenes
            .iter()
            .find(|scene| scene.name.eq_ignore_ascii_case(name))
            .cloned()
        else {
            return Err(format!("no scene \"{name}\"; {}", self.scene_names()));
        };
        self.apply_scene(&scene);
        Ok(vec![format!(
            "scene {}{}",
            scene.name,
            over(scene.blend_ms)
        )])
    }

    /// Sets all values of `scene`, which the profile checked against this route.
    fn apply_scene(&mut self, scene: &SceneSpec) {
        let blend = scene.blend_ms;
        for change in &scene.inputs {
            let target = Target::Input(change.input);
            if let Some(gain_db) = change.gain_db {
                self.set_gain(target, gain_db, blend);
            }
            if let Some(pan) = change.pan {
                self.set_pan(change.input, pan, blend);
            }
            if let Some(muted) = change.muted {
                self.set_mute(target, muted, blend);
            }
            if let Some(buses) = &change.buses {
                let input = &self.inputs[change.input];
                input.controls.fade_ms.store(blend, Ordering::Relaxed);
                for route in &input.routes {
                    route
                        .routed
                        .store(buses.contains(&route.bus), Ordering::Release);
                }
                self.spec.inputs[change.input].buses.clone_from(buses);
            }
            if let Some(on) = change.automix {
                if let Some((group, index)) = self.inputs[change.input].controls.automix.get() {
                    group.member(*index).set_on(on);
                }
                self.spec.inputs[change.input].automix = on;
            }
            for send in &change.sends {
                if let Some(route) = self.inputs[change.input].routes.get(send.bus) {
                    route.send.ramp_to(db_to_linear(send.gain_db), blend);
                    route.send_pre.store(send.pre, Ordering::Relaxed);
                }
                self.spec.inputs[change.input].set_send(*send);
            }
            scene_dynamics(
                change,
                &self.inputs[change.input].controls,
                &mut self.spec.inputs[change.input],
            );
        }
        for change in &scene.buses {
            let target = Target::Bus(change.bus);
            if let Some(gain_db) = change.gain_db {
                self.set_gain(target, gain_db, blend);
            }
            if let Some(muted) = change.muted {
                self.set_mute(target, muted, blend);
            }
            scene_equalizer(
                change,
                &self.buses[change.bus].handle.controls.eq,
                &mut self.spec.buses[change.bus],
            );
        }
    }

    /// The names of the scenes, in profile order, as a line.
    fn scene_names(&self) -> String {
        if self.spec.scenes.is_empty() {
            return "this route has no scenes".to_owned();
        }
        let names: Vec<&str> = self.spec.scenes.iter().map(|s| s.name.as_str()).collect();
        format!("scenes: {}", names.join(", "))
    }

    /// Routes input `input` to bus `bus` or takes it off (a matrix button); the mixer of the
    /// bus follows in its next callback, a newly routed ring after priming.
    ///
    /// # Errors
    ///
    /// A message when an index is out of range or the input would feed no bus.
    pub fn route(&mut self, input: usize, bus: usize, on: bool) -> Result<Vec<String>, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        let Some(route) = running.routes.get(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        let label = self.target_label(Target::Input(input));
        let bus_name = self.buses[bus].name.clone();
        if on && self.spec.buses[bus].minus == Some(input) {
            return Err(format!(
                "{bus_name} is the mix-minus of {label}: it hears everything but {label}"
            ));
        }
        if !on && route.is_routed() && running.routes.iter().filter(|r| r.is_routed()).count() == 1
        {
            return Err(format!("{label} would feed no bus; mute it instead"));
        }
        running.controls.fade_ms.store(0, Ordering::Relaxed);
        route.routed.store(on, Ordering::Release);
        let buses = &mut self.spec.inputs[input].buses;
        buses.retain(|&b| b != bus);
        if on {
            buses.push(bus);
            buses.sort_unstable();
        }
        let state = if on { "->" } else { "off" };
        Ok(vec![format!("{label} {state} {bus_name}")])
    }

    /// Sets the level of the crossing of `input` and `bus` in dB, reached in `ramp_ms`.
    ///
    /// # Errors
    ///
    /// A message when an index, the level or the ramp is out of range.
    pub fn send(
        &mut self,
        input: usize,
        bus: usize,
        gain_db: f32,
        ramp_ms: u32,
    ) -> Result<Vec<String>, String> {
        let route = self.crossing(input, bus)?;
        if !GAIN_DB.contains(&gain_db) {
            return Err(format!(
                "a send must be {} to {} dB, not {gain_db}",
                GAIN_DB.start(),
                GAIN_DB.end()
            ));
        }
        check_ramp(ramp_ms)?;
        route.send.ramp_to(db_to_linear(gain_db), ramp_ms);
        let spec = &mut self.spec.inputs[input];
        let pre = spec.send(bus).pre;
        spec.set_send(SendSpec { bus, gain_db, pre });
        Ok(vec![format!(
            "{} send to {} {gain_db:.1} dB{}",
            self.target_label(Target::Input(input)),
            self.buses[bus].name,
            over(ramp_ms)
        )])
    }

    /// Takes the send of `input` to `bus` before (`pre`) or after the input's fader.
    ///
    /// # Errors
    ///
    /// A message when an index is out of range.
    pub fn send_pre(&mut self, input: usize, bus: usize, pre: bool) -> Result<Vec<String>, String> {
        let route = self.crossing(input, bus)?;
        route.send_pre.store(pre, Ordering::Relaxed);
        let spec = &mut self.spec.inputs[input];
        let gain_db = spec.send(bus).gain_db;
        spec.set_send(SendSpec { bus, gain_db, pre });
        let point = if pre { "before" } else { "after" };
        Ok(vec![format!(
            "{} send to {} {point} the fader",
            self.target_label(Target::Input(input)),
            self.buses[bus].name
        )])
    }

    /// Sets the knob, the mix (percent) and/or the sidechain filter (Hz) of the compressor of
    /// `bus`.
    ///
    /// # Errors
    ///
    /// A message when there is no such bus or a value is out of its range.
    pub fn bus_comp(
        &mut self,
        bus: usize,
        amount: Option<f32>,
        mix: Option<f32>,
        hpf_hz: Option<f32>,
    ) -> Result<Vec<String>, String> {
        let Some(running) = self.buses.get(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        let amount = amount.map(|a| check_amount("comp", a)).transpose()?;
        if let Some(mix) = mix
            && !(0.0..=100.0).contains(&mix)
        {
            return Err(format!("a mix is 0 to 100 %, not {mix}"));
        }
        let (low, high) = pult_core::buscomp::HPF_HZ;
        if let Some(hz) = hpf_hz
            && !(low..=high).contains(&hz)
        {
            return Err(format!(
                "the sidechain filter is {low} to {high} Hz, not {hz}"
            ));
        }
        let controls = &running.handle.controls.comp;
        let spec = &mut self.spec.buses[bus];
        let mut said = Vec::new();
        if let Some(amount) = amount {
            controls.comp.set_simple(amount);
            spec.comp = amount;
            said.push(format!("{}: comp {}", running.name, knob(amount)));
        }
        if let Some(mix) = mix {
            controls.set_mix(mix / 100.0);
            spec.comp_mix = mix;
            said.push(format!("{}: comp mix {mix:.0} %", running.name));
        }
        if let Some(hz) = hpf_hz {
            controls.set_hpf(hz);
            spec.comp_hpf_hz = hz;
            said.push(format!("{}: comp hpf {hz:.0} Hz", running.name));
        }
        Ok(said)
    }

    /// Takes `input` into the automixer or out of it, and/or sets its priority in dB.
    ///
    /// # Errors
    ///
    /// A message when there is no such input or the weight is not -12 to 12 dB.
    pub fn automix(
        &mut self,
        input: usize,
        on: Option<bool>,
        weight_db: Option<f32>,
    ) -> Result<Vec<String>, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        if let Some(db) = weight_db
            && !(-12.0..=12.0).contains(&db)
        {
            return Err(format!("an automix weight is -12 to 12 dB, not {db}"));
        }
        let label = self.target_label(Target::Input(input));
        let member = running
            .controls
            .automix
            .get()
            .map(|(group, index)| group.member(*index));
        let spec = &mut self.spec.inputs[input];
        let mut said = Vec::new();
        if let Some(on) = on {
            spec.automix = on;
            if let Some(member) = member {
                member.set_on(on);
            }
            let state = if on { "in" } else { "out of" };
            said.push(format!("{label}: {state} the automixer"));
        }
        if let Some(db) = weight_db {
            spec.automix_weight_db = db;
            if let Some(member) = member {
                member.set_weight_db(db);
            }
            said.push(format!("{label}: automix weight {db:+.1} dB"));
        }
        Ok(said)
    }

    /// Sets the gate (`gate`) or the compressor of `index` by its one knob.
    fn dynamics(&mut self, index: usize, gate: bool, amount: f32) -> Result<Vec<String>, String> {
        let what = if gate { "gate" } else { "comp" };
        let amount = check_amount(what, amount)?;
        let controls = &self.inputs[index].controls;
        let spec = &mut self.spec.inputs[index];
        if gate {
            controls.gate.set_simple(amount);
            spec.gate = amount;
        } else {
            controls.compressor.set_simple(amount);
            spec.comp = amount;
        }
        Ok(vec![format!(
            "{}: {what} {}",
            self.target_label(Target::Input(index)),
            knob(amount)
        )])
    }

    /// Changes one setting of the multiband compressor of `bus`.
    ///
    /// # Errors
    ///
    /// A message when there is no such bus or band, or the value is out of its range.
    pub fn multiband(
        &mut self,
        bus: usize,
        change: pult_core::multiband::MultibandChange,
    ) -> Result<Vec<String>, String> {
        let Some(running) = self.buses.get(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        let change = change.checked()?;
        running.handle.controls.multiband.set(change);
        self.spec.buses[bus].multiband.apply(change);
        Ok(vec![format!("{}: {}", running.name, change.said())])
    }

    /// Changes one setting of the channel strip of `input`.
    ///
    /// # Errors
    ///
    /// A message when there is no such input or the value is out of its range.
    pub fn strip(
        &mut self,
        input: usize,
        change: pult_core::strip::StripChange,
    ) -> Result<Vec<String>, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        let change = change.checked()?;
        running.controls.strip.set(change);
        self.spec.inputs[input].strip.apply(change);
        let label = self.target_label(Target::Input(input));
        Ok(vec![format!("{label}: {}", change.said())])
    }

    /// Changes one setting of the ducker of `input`.
    ///
    /// # Errors
    ///
    /// A message when there is no such input, the value is out of its range, or the key is an
    /// input the route does not have or the input itself.
    pub fn duck(
        &mut self,
        input: usize,
        change: pult_core::duck::DuckChange,
    ) -> Result<Vec<String>, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        let change = change.checked()?;
        if let pult_core::duck::DuckChange::Key(key) = change {
            pult_core::duck::check_key(input, key, self.inputs.len())?;
        }
        let spec = &mut self.spec.inputs[input];
        spec.duck.apply(change);
        running.controls.duck.set_all(spec.duck);
        let label = self.target_label(Target::Input(input));
        Ok(vec![format!("{label}: {}", change.said())])
    }

    /// Makes `bus` a monitor bus or takes that away; solo then acts where [`Self::apply_monitors`]
    /// says.
    ///
    /// # Errors
    ///
    /// A message when there is no such bus.
    pub fn monitor(&mut self, bus: usize, on: bool) -> Result<Vec<String>, String> {
        let Some(spec) = self.spec.buses.get_mut(bus) else {
            return Err(format!("there is no bus {}", bus + 1));
        };
        spec.monitor = on;
        self.apply_monitors();
        // A monitor bus is dimmed while someone talks.
        self.apply_bus_gain(bus, DIM_MS);
        let name = &self.buses[bus].name;
        let any = self.spec.buses.iter().any(|b| b.monitor);
        Ok(vec![if on {
            format!("{name} is the monitor: solo acts only on monitor buses")
        } else if any {
            format!("{name} is no monitor bus")
        } else {
            format!("{name} is no monitor bus; without one, solo acts on every bus")
        }])
    }

    /// Where solo acts: on every bus while no bus is a monitor bus, else only on the monitor
    /// buses (a talk bus keeps hearing every source).
    pub fn apply_monitors(&self) {
        let any = self.spec.buses.iter().any(|bus| bus.monitor);
        for (running, spec) in self.buses.iter().zip(&self.spec.buses) {
            let controls = &running.handle.controls;
            controls
                .solo_here
                .store(!any || spec.monitor, Ordering::Release);
            // Before the fader only where someone listens: never on a bus that goes out.
            controls
                .pfl
                .store(self.spec.pfl && spec.monitor, Ordering::Release);
        }
    }

    /// Whether solo on a monitor bus listens before the fader.
    #[must_use]
    pub fn is_pfl(&self) -> bool {
        self.spec.pfl
    }

    /// Lets solo on the monitor buses listen before the fader (PFL) or after it (AFL).
    pub fn pfl(&mut self, on: bool) -> Vec<String> {
        self.spec.pfl = on;
        self.apply_monitors();
        let monitors = self.spec.buses.iter().any(|bus| bus.monitor);
        vec![match (on, monitors) {
            (true, true) => "solo listens before the fader (PFL) on the monitor bus".to_owned(),
            (true, false) => {
                "solo listens before the fader (PFL), once a bus is the monitor (monitor <bus> on)"
                    .to_owned()
            }
            (false, _) => "solo listens after the fader (AFL)".to_owned(),
        }]
    }

    /// The ring of `input` on `bus`.
    fn crossing(&self, input: usize, bus: usize) -> Result<&RunningRoute, String> {
        let Some(running) = self.inputs.get(input) else {
            return Err(format!("there is no input {}", input + 1));
        };
        running
            .routes
            .get(bus)
            .ok_or_else(|| format!("there is no bus {}", bus + 1))
    }

    /// Writes the current state as a profile to `path`.
    ///
    /// # Errors
    ///
    /// A message when the file cannot be written.
    pub fn save_profile(&self, path: &Path) -> Result<Vec<String>, String> {
        save(path, &self.spec)
            .map(|()| vec![format!("saved the current state to {}", path.display())])
    }

    /// `[1] CABLE Output` or `A1`.
    fn target_label(&self, target: Target) -> String {
        match target {
            Target::Input(index) => format!("[{}] {}", index + 1, self.inputs[index].name),
            Target::Bus(index) => self.buses[index].name.clone(),
        }
    }

    /// One line per input and bus with its settings.
    fn show(&self) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.inputs.len() + self.buses.len());
        for (index, (input, spec)) in self.inputs.iter().zip(&self.spec.inputs).enumerate() {
            let buses: Vec<&str> = input
                .routes
                .iter()
                .filter(|route| route.is_routed())
                .map(|route| self.buses[route.bus].name.as_str())
                .collect();
            let mut line = format!(
                "[{}] {}  gain {:.1} dB  pan {:.2}  -> {}",
                index + 1,
                input.name,
                spec.gain_db,
                spec.pan,
                buses.join(", ")
            );
            for (what, amount) in [("gate", spec.gate), ("comp", spec.comp)] {
                if amount > 0.0 {
                    let _ = write!(line, "  {what} {}", knob(amount));
                }
            }
            line.push_str(&strip_summary(&spec.strip));
            if spec.talkback {
                line.push_str(if self.talk {
                    "  talkback (open)"
                } else {
                    "  talkback"
                });
            }
            line.push_str(&spec.duck.summary());
            for send in &spec.sends {
                let point = if send.pre { " pre" } else { "" };
                let _ = write!(
                    line,
                    "  send {} {:.1} dB{point}",
                    self.buses[send.bus].name, send.gain_db
                );
            }
            if spec.muted {
                line.push_str("  muted");
            }
            lines.push(line);
        }
        for (index, (bus, spec)) in self.buses.iter().zip(&self.spec.buses).enumerate() {
            let mut line = format!("{} {}  gain {:.1} dB", bus.name, bus.device, spec.gain_db);
            if spec.comp > 0.0 {
                let _ = write!(
                    line,
                    "  comp {} mix {:.0} % hpf {:.0} Hz",
                    knob(spec.comp),
                    spec.comp_mix,
                    spec.comp_hpf_hz
                );
            }
            if self.dim[index] || (self.talk && spec.monitor) {
                let _ = write!(line, "  dimmed {:.0} dB", spec.dim_db);
            }
            if spec.multiband.is_on() {
                let [low, mid, high] = spec.multiband.bands;
                let _ = write!(
                    line,
                    "  mb {}/{}/{} split {:.0}/{:.0} Hz",
                    knob(low),
                    knob(mid),
                    knob(high),
                    spec.multiband.low_hz,
                    spec.multiband.high_hz
                );
            }
            line.push_str(&eq_summary(spec.eq_on, &spec.eq));
            line.push_str(&spec.limit.summary());
            if spec.delay_ms > 0.0 {
                let _ = write!(line, "  delay {:.1} ms", spec.delay_ms);
            }
            if spec.muted {
                line.push_str("  muted");
            }
            lines.push(line);
        }
        lines
    }
}

/// What a scene changes of the gate and the compressor of an input, in its controls and its spec.
fn scene_dynamics(change: &SceneInput, controls: &InputControls, spec: &mut InputSpec) {
    if let Some(amount) = change.gate {
        controls.gate.set_simple(amount);
        spec.gate = amount;
    }
    if let Some(amount) = change.comp {
        controls.compressor.set_simple(amount);
        spec.comp = amount;
    }
}

/// What a scene changes of the equalizer of a bus, in its controls and its spec. The route's
/// validation checked its bands and values.
fn scene_equalizer(change: &SceneBus, eq: &EqControls, spec: &mut BusSpec) {
    if let Some(on) = change.eq_on {
        eq.switch(on);
        spec.eq_on = on;
    }
    for (band, setting) in &change.eq {
        if let Some(set) = spec.eq.get_mut(*band)
            && let Ok(changed) = set.changed(*setting)
        {
            *set = changed;
            eq.set(*band, changed);
        }
    }
}

/// Refuses a ramp longer than the mixer takes.
fn check_ramp(ramp_ms: u32) -> Result<(), String> {
    if ramp_ms > MAX_RAMP_MS {
        return Err(format!(
            "a ramp takes at most {MAX_RAMP_MS} ms, not {ramp_ms}"
        ));
    }
    Ok(())
}

/// How a line says the one knob of a gate or a compressor: `off` at 0, else its setting.
pub(crate) fn knob(amount: f32) -> String {
    if amount > 0.0 {
        format!("{amount:.1}")
    } else {
        "off".to_owned()
    }
}

/// How a confirmation says the time a change takes; nothing for the shortest.
fn over(ramp_ms: u32) -> String {
    if ramp_ms == 0 {
        String::new()
    } else {
        format!(" over {ramp_ms} ms")
    }
}

/// Writes `spec` as a profile to `path`, through a temporary file so that a failed write
/// leaves an existing profile intact.
fn save(path: &Path, spec: &RouteSpec) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_or_else(
        |_| profile::render(spec),
        |existing| profile::merge(&existing, spec),
    );
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = Path::new(&temporary);
    let failed = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    std::fs::write(temporary, text).map_err(failed)?;
    std::fs::rename(temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(temporary);
        failed(e)
    })
}

/// A TOML basic string.
pub(crate) fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_reaches_gate_and_compressor_and_leaves_what_it_does_not_name() {
        use pult_core::{CompressorParams, GateParams};
        let controls = InputControls::default();
        controls.compressor.set_simple(5.0);
        let mut spec = InputSpec {
            comp: 5.0,
            ..InputSpec::new("m", 0.0, vec![0])
        };
        let gate = SceneInput {
            gate: Some(3.0),
            ..SceneInput::default()
        };
        scene_dynamics(&gate, &controls, &mut spec);
        assert!(controls.gate.is_on());
        assert_eq!(controls.gate.params(), GateParams::simple(3.0));
        assert_eq!(spec.gate, 3.0);
        assert!(controls.compressor.is_on(), "it did not name this one");
        assert_eq!(spec.comp, 5.0);

        let off = SceneInput {
            comp: Some(0.0),
            ..SceneInput::default()
        };
        scene_dynamics(&off, &controls, &mut spec);
        assert!(!controls.compressor.is_on());
        assert_eq!(controls.compressor.params(), CompressorParams::simple(0.0));
        assert_eq!(spec.comp, 0.0);
        assert!(controls.gate.is_on());
    }

    #[test]
    fn a_scene_reaches_the_equalizer_and_leaves_what_it_does_not_name() {
        use pult_core::eq::BandChange;
        use pult_core::{Band, BandKind};
        let eq = EqControls::default();
        let mut spec = BusSpec::new("A1", "x");
        let change = SceneBus {
            eq_on: Some(false),
            eq: vec![
                (1, BandChange::Kind(BandKind::HighShelf)),
                (1, BandChange::Gain(6.0)),
            ],
            ..SceneBus::default()
        };
        scene_equalizer(&change, &eq, &mut spec);
        assert!(!eq.is_on());
        assert!(!spec.eq_on);
        let band = Band {
            kind: BandKind::HighShelf,
            gain_db: 6.0,
            ..Band::at(1)
        };
        assert_eq!(eq.band(1), Some(band), "in the controls");
        assert_eq!(spec.eq[1], band, "in the spec");
        assert_eq!(eq.band(0), Some(Band::at(0)));
        assert_eq!(spec.eq[0], Band::at(0));

        // Only the switch: the bands stay, and the other way round.
        let on = SceneBus {
            eq_on: Some(true),
            ..SceneBus::default()
        };
        scene_equalizer(&on, &eq, &mut spec);
        assert!(eq.is_on());
        assert_eq!(eq.band(1), Some(band));
    }

    #[test]
    fn a_knob_at_zero_reads_as_off() {
        assert_eq!(knob(0.0), "off");
        assert_eq!(knob(3.0), "3.0");
        assert_eq!(knob(7.25), "7.2");
    }

    #[test]
    fn quoted_escapes_what_toml_needs() {
        assert_eq!(quoted("CABLE Output"), "\"CABLE Output\"");
        assert_eq!(quoted("a\"b\\c\td\u{1}"), "\"a\\\"b\\\\c\\td\\u0001\"");
        assert_eq!(quoted("Kopfhörer"), "\"Kopfhörer\"");
    }

    #[cfg(windows)]
    #[test]
    fn save_writes_to_a_path_that_is_not_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let dir = std::env::temp_dir().join(format!("pult-save-wide-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // A lone surrogate: a valid Windows file name, but no valid Unicode.
        let name = std::ffi::OsString::from_wide(&[0xD800, u16::from(b'a'), u16::from(b'.')]);
        let path = dir.join(name);
        assert!(path.to_str().is_none());
        let spec = profile::parse(include_str!("../../../examples/pult.toml")).expect("example");
        save(&path, &spec).expect("save");
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn save_replaces_a_file_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("pult-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("profil.toml");
        std::fs::write(&path, "old").expect("old file");
        let spec = profile::parse(include_str!("../../../examples/pult.toml")).expect("example");
        save(&path, &spec).expect("save");
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(profile::parse(&text).expect("valid"), spec);
        assert!(!dir.join("profil.toml.tmp").exists());
        std::fs::remove_dir_all(&dir).expect("clean up");
    }
}
