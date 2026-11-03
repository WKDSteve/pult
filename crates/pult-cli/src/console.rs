//! The terminal of `pult run`: the live status line, lines that stay above it (devices lost
//! or back, answers to commands) and an input line for commands.
//!
//! On a console (stdin and stdout both terminals) the terminal runs in raw mode: the bottom
//! row is the input line, the row above it the status line, and every other line scrolls
//! up above them. Otherwise commands come as lines from stdin (a pipe, a file), and the
//! status line is drawn with `\r` only when stdout is a terminal.

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Instant;

use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};

/// What the user did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A line was entered (empty for a bare Enter).
    Line(String),
    /// Ctrl+C on the console.
    Interrupt,
}

/// The input line being typed, with a history of entered lines.
#[derive(Debug, Default)]
struct Editor {
    text: String,
    history: Vec<String>,
    /// Position in `history` while browsing it; `None` while typing.
    browsing: Option<usize>,
}

impl Editor {
    /// Applies one key; returns what it finished, if anything.
    fn key(&mut self, key: KeyEvent) -> Option<Input> {
        // Windows reports AltGr as Ctrl+Alt; its characters (\ @ { [ on a German keyboard)
        // are text, not shortcuts.
        let shortcut = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('c' | 'd') if shortcut => return Some(Input::Interrupt),
            KeyCode::Char(c) if !shortcut => self.text.push(c),
            KeyCode::Backspace => {
                self.text.pop();
            }
            KeyCode::Esc => self.text.clear(),
            KeyCode::Up if !self.history.is_empty() => {
                let index = self
                    .browsing
                    .map_or(self.history.len() - 1, |i| i.saturating_sub(1));
                self.browsing = Some(index);
                self.text.clone_from(&self.history[index]);
            }
            KeyCode::Down => match self.browsing {
                Some(i) if i + 1 < self.history.len() => {
                    self.browsing = Some(i + 1);
                    self.text.clone_from(&self.history[i + 1]);
                }
                _ => {
                    self.browsing = None;
                    self.text.clear();
                }
            },
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.text);
                self.browsing = None;
                if !line.trim().is_empty() && self.history.last() != Some(&line) {
                    self.history.push(line.clone());
                }
                return Some(Input::Line(line));
            }
            _ => {}
        }
        None
    }
}

/// Where input comes from and how output is drawn.
enum Mode {
    /// Raw mode with an input line; keys arrive from a reader thread.
    Console {
        keys: Receiver<KeyEvent>,
        editor: Editor,
    },
    /// Lines from stdin; `live` draws the status line with `\r`.
    Lines { lines: Receiver<String>, live: bool },
}

/// The terminal of a run.
pub struct Console {
    mode: Mode,
    status: String,
}

/// The width of the terminal, 80 when unknown.
fn width() -> usize {
    terminal::size().map_or(80, |(w, _)| usize::from(w))
}

/// `line` cut or padded to one column less than `width`, so that it never wraps and fully
/// covers the previous refresh.
pub fn fit(line: &str, width: usize) -> String {
    let columns = width.saturating_sub(1);
    let mut fitted: String = line.chars().take(columns).collect();
    let used = fitted.chars().count();
    fitted.extend(std::iter::repeat_n(' ', columns - used));
    fitted
}

/// The input line: a prompt and the end of `text` that fits into `width`.
fn prompt(text: &str, width: usize) -> String {
    let room = width.saturating_sub(3);
    let skip = text.chars().count().saturating_sub(room);
    let shown: String = text.chars().skip(skip).collect();
    format!("> {shown}")
}

impl Console {
    /// Takes over the terminal: raw mode with an input line on a console, lines otherwise.
    pub fn open() -> Self {
        let console = io::stdin().is_terminal() && io::stdout().is_terminal();
        let mode = if console && terminal::enable_raw_mode().is_ok() {
            let (sender, keys) = mpsc::channel();
            thread::spawn(move || {
                while let Ok(event) = event::read() {
                    // Windows reports releases too; only presses type.
                    if let Event::Key(key) = event
                        && key.kind != KeyEventKind::Release
                        && sender.send(key).is_err()
                    {
                        return;
                    }
                }
            });
            // The status row and, below it, the input row.
            print!("\r\n> ");
            let _ = io::stdout().flush();
            Mode::Console {
                keys,
                editor: Editor::default(),
            }
        } else {
            let (sender, lines) = mpsc::channel();
            thread::spawn(move || {
                // A closed stdin (started without a console) ends the commands, not the run.
                for line in io::stdin().lock().lines() {
                    let Ok(line) = line else { return };
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            });
            Mode::Lines {
                lines,
                live: io::stdout().is_terminal(),
            }
        };
        Self {
            mode,
            status: String::new(),
        }
    }

    /// Waits until `deadline` for input; returns early with the first line entered.
    pub fn wait(&mut self, deadline: Instant) -> Option<Input> {
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            match &mut self.mode {
                Mode::Console { keys, editor } => match keys.recv_timeout(timeout) {
                    Ok(key) => {
                        if let Some(input) = editor.key(key) {
                            // The entered line stays visible above, as typed.
                            if let Input::Line(line) = &input {
                                let line = format!("> {line}");
                                self.notes(&[line]);
                            }
                            return Some(input);
                        }
                        self.draw_prompt();
                    }
                    Err(RecvTimeoutError::Timeout) => return None,
                    Err(RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(timeout);
                        return None;
                    }
                },
                Mode::Lines { lines, .. } => match lines.recv_timeout(timeout) {
                    Ok(line) => return Some(Input::Line(line)),
                    Err(RecvTimeoutError::Timeout) => return None,
                    Err(RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(timeout);
                        return None;
                    }
                },
            }
        }
    }

    /// Prints `notes` as lines that stay, then the status line again.
    pub fn notes(&mut self, notes: &[String]) {
        if notes.is_empty() {
            return;
        }
        let mut out = io::stdout().lock();
        match &self.mode {
            Mode::Console { editor, .. } => {
                let width = width();
                // Clear the input row and the status row, write the notes where the status
                // was, then both rows again below them.
                let _ = queue!(
                    out,
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine),
                    MoveUp(1),
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine)
                );
                for note in notes {
                    let _ = queue!(out, Print(note), Print("\r\n"));
                }
                let _ = queue!(
                    out,
                    Print(fit(&self.status, width)),
                    Print("\r\n"),
                    Print(prompt(&editor.text, width))
                );
            }
            Mode::Lines { live, .. } => {
                let width = width();
                for note in notes {
                    if *live {
                        // Padded to cover the status line it replaces; a long note may wrap.
                        let _ =
                            writeln!(out, "\r{note:<columns$}", columns = width.saturating_sub(1));
                    } else {
                        let _ = writeln!(out, "{note}");
                    }
                }
                if *live {
                    let _ = write!(out, "\r{}", fit(&self.status, width));
                }
            }
        }
        let _ = out.flush();
    }

    /// Replaces the status line.
    pub fn status(&mut self, line: String) {
        self.status = line;
        let mut out = io::stdout().lock();
        match &self.mode {
            Mode::Console { editor, .. } => {
                let width = width();
                let _ = queue!(
                    out,
                    MoveUp(1),
                    MoveToColumn(0),
                    Print(fit(&self.status, width)),
                    Print("\r\n"),
                    Clear(ClearType::CurrentLine),
                    Print(prompt(&editor.text, width))
                );
            }
            Mode::Lines { live: true, .. } => {
                let _ = write!(out, "\r{}", fit(&self.status, width()));
            }
            Mode::Lines { live: false, .. } => {}
        }
        let _ = out.flush();
    }

    fn draw_prompt(&self) {
        if let Mode::Console { editor, .. } = &self.mode {
            let mut out = io::stdout().lock();
            let _ = queue!(
                out,
                MoveToColumn(0),
                Clear(ClearType::CurrentLine),
                Print(prompt(&editor.text, width()))
            );
            let _ = out.flush();
        }
    }

    /// Gives the terminal back, with the last status line printed for good.
    pub fn close(self) {
        let mut out = io::stdout().lock();
        match &self.mode {
            Mode::Console { .. } => {
                let _ = queue!(
                    out,
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine),
                    MoveUp(1),
                    MoveToColumn(0),
                    Clear(ClearType::CurrentLine),
                    Print(&self.status),
                    Print("\r\n")
                );
                let _ = out.flush();
                let _ = terminal::disable_raw_mode();
            }
            Mode::Lines { .. } => {
                let _ = writeln!(out, "\n{}", self.status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(editor: &mut Editor, text: &str) -> Option<Input> {
        for c in text.chars() {
            assert_eq!(editor.key(press(KeyCode::Char(c))), None);
        }
        editor.key(press(KeyCode::Enter))
    }

    #[test]
    fn fit_cuts_long_lines_and_pads_short_ones() {
        assert_eq!(fit("abcdef", 4), "abc");
        assert_eq!(fit("ab", 5), "ab  ");
        assert_eq!(fit("äöü-x", 4), "äöü");
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn prompt_shows_the_end_of_a_long_line() {
        assert_eq!(prompt("gain", 20), "> gain");
        assert_eq!(prompt("gain A1 -6", 8), "> A1 -6");
    }

    #[test]
    fn editor_types_edits_and_enters_lines() {
        let mut editor = Editor::default();
        assert_eq!(
            typed(&mut editor, "mute 1"),
            Some(Input::Line("mute 1".into()))
        );
        for c in "gainx".chars() {
            editor.key(press(KeyCode::Char(c)));
        }
        editor.key(press(KeyCode::Backspace));
        assert_eq!(
            typed(&mut editor, " A1 -6"),
            Some(Input::Line("gain A1 -6".into()))
        );
        editor.key(press(KeyCode::Char('x')));
        editor.key(press(KeyCode::Esc));
        assert_eq!(
            editor.key(press(KeyCode::Enter)),
            Some(Input::Line(String::new()))
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(editor.key(ctrl_c), Some(Input::Interrupt));
    }

    #[test]
    fn editor_takes_altgr_characters_as_text() {
        let mut editor = Editor::default();
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        editor.key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT));
        editor.key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT));
        for c in ['\\', '@', '{'] {
            assert_eq!(editor.key(KeyEvent::new(KeyCode::Char(c), altgr)), None);
        }
        assert_eq!(editor.text, "C:\\@{");
    }

    #[test]
    fn editor_browses_the_history() {
        let mut editor = Editor::default();
        typed(&mut editor, "mute 1");
        typed(&mut editor, "gain A1 -6");
        typed(&mut editor, "gain A1 -6");
        editor.key(press(KeyCode::Up));
        assert_eq!(editor.text, "gain A1 -6");
        editor.key(press(KeyCode::Up));
        assert_eq!(editor.text, "mute 1");
        editor.key(press(KeyCode::Up));
        assert_eq!(editor.text, "mute 1");
        editor.key(press(KeyCode::Down));
        assert_eq!(editor.text, "gain A1 -6");
        editor.key(press(KeyCode::Down));
        assert_eq!(editor.text, "");
    }
}
