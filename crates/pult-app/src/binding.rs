//! What the controls of one frame changed, over the parameter addresses of the contract.

use pult_core::view::{Action, MixerView};

pub use pult_core::view::{
    AddressError, BandParam, Binding, BusParam, InputParam, Value, action_gain,
};

/// A view and what the controls changed in this frame, so a readout drawn after its fader
/// shows the new value at once.
#[derive(Debug)]
pub struct Values<'a> {
    view: &'a MixerView,
    edits: Vec<(Binding, Value)>,
}

impl<'a> Values<'a> {
    #[must_use]
    pub fn new(view: &'a MixerView) -> Self {
        Self {
            view,
            edits: Vec::new(),
        }
    }

    #[must_use]
    pub fn view(&self) -> &'a MixerView {
        self.view
    }

    /// The latest value of `binding`: this frame's edit, else the view's.
    #[must_use]
    pub fn read(&self, binding: Binding) -> Option<Value> {
        self.edits
            .iter()
            .rev()
            .find(|(bound, _)| *bound == binding)
            .map(|(_, value)| *value)
            .or_else(|| binding.read(self.view))
    }

    /// Notes that a control set `binding` to `value`; returns the action for the desk.
    pub fn write(&mut self, binding: Binding, value: Value) -> Option<Action> {
        self.edits.push((binding, value));
        binding.write(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pult_core::view::{Desk, FakeDesk};

    fn view() -> MixerView {
        FakeDesk::new().view()
    }

    #[test]
    fn values_see_this_frames_edits_first() {
        let view = view();
        let gain = Binding::Input(0, InputParam::Gain);
        let mut values = Values::new(&view);
        assert_eq!(values.read(gain), Some(Value::Gain(view.inputs[0].gain_db)));
        let action = values.write(gain, Value::Gain(-7.0));
        assert_eq!(
            action,
            Some(Action::InputGain {
                input: 0,
                db: -7.0,
                ramp_ms: 0
            })
        );
        assert_eq!(values.read(gain), Some(Value::Gain(-7.0)));
        assert_eq!(
            values.read(Binding::Input(1, InputParam::Gain)),
            Some(Value::Gain(view.inputs[1].gain_db))
        );
    }
}
