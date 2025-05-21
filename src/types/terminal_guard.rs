use crossterm::{ExecutableCommand, cursor, terminal};
use std::io::{Write, stdout};

pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new() -> Self {
        let mut stdout = stdout();
        let _ = stdout.execute(terminal::EnterAlternateScreen);
        let _ = stdout.execute(cursor::Hide);
        Self
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = stdout();
        let _ = stdout.execute(cursor::Show);
        let _ = stdout.execute(terminal::LeaveAlternateScreen);
        let _ = stdout.flush();
    }
}
