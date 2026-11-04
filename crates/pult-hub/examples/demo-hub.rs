//! The hub on the demo desk, with a name in it that tries to be markup: for working on the page and
//! for `tools/page-check.mjs`. No audio device is touched. Prints a pairing link and ends on Enter.
//!
//! ```text
//! PULT_CONFIG_DIR=<an empty folder> PULT_PORT=0 cargo run -p pult-hub --example demo-hub
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pult_core::view::{Action, Desk, FakeDesk, MixerView, Reply};
use pult_hub::{Client, Control};

/// A name the page has to show as text.
const HOSTILE: &str = "<img src=x onerror=\"document.title='owned'\">";

struct Demo(FakeDesk);

impl Desk for Demo {
    fn view(&mut self) -> MixerView {
        let mut view = self.0.view();
        view.inputs[2].label = Some(HOSTILE.to_owned());
        view
    }

    fn act(&mut self, action: Action) -> Reply {
        self.0.act(action)
    }
}

fn main() -> Result<(), String> {
    let (control, mut pump) = Control::start()?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let owner = std::thread::spawn(move || {
        let mut desk = Demo(FakeDesk::new());
        while !stopping.load(Ordering::Relaxed) && !pump.stop_asked() {
            let view = desk.view();
            pump.pump(&mut desk, &view);
            std::thread::sleep(Duration::from_millis(30));
        }
    });
    let mut client = Client::find().map_err(|e| e.to_string())?;
    client.state().map_err(|e| e.to_string())?;
    println!("{}", client.page_link().map_err(|e| e.to_string())?);
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    stop.store(true, Ordering::Relaxed);
    let _ = owner.join();
    drop(control);
    Ok(())
}
