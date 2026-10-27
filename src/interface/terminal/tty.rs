//! src/interface/terminal/tty.rs owns terminal modes and bounded keyboard decoding.
//! Terminal restores the original screen and input mode on normal and error exits.

use anyhow::{Context, Result, ensure};
use rustix::termios::{self, OptionalActions, SpecialCodeIndex, Termios};
use std::io::{self, IsTerminal, Read, Write};

#[derive(Debug, PartialEq)]
pub(super) enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Backspace,
    Details,
    Character(char),
    Ignore,
}

#[derive(Debug, thiserror::Error)]
#[error("terminal exit requested")]
pub(super) struct ExitRequested;

pub(super) struct Terminal {
    original: Termios,
    active: bool,
    pasting: bool,
}

impl Terminal {
    pub(super) fn open() -> Result<Self> {
        ensure!(
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            "interactive mode requires a terminal on stdin and stdout; use pace --help for commands"
        );
        ensure!(
            std::env::var("TERM").as_deref() != Ok("dumb"),
            "interactive mode requires an ANSI terminal; use pace --help for commands"
        );
        let mut terminal = Self {
            original: termios::tcgetattr(io::stdin()).context("reading terminal mode")?,
            active: false,
            pasting: false,
        };
        terminal.resume()?;
        Ok(terminal)
    }

    pub(super) fn resume(&mut self) -> Result<()> {
        let mut raw = self.original.clone();
        raw.make_raw();
        raw.special_codes[SpecialCodeIndex::VMIN] = 0;
        raw.special_codes[SpecialCodeIndex::VTIME] = 1;
        termios::tcsetattr(io::stdin(), OptionalActions::Flush, &raw)
            .context("entering terminal input mode")?;
        self.active = true;
        self.pasting = false;
        self.write("\x1b[?1049h\x1b[?25l\x1b[?2004h")
    }

    pub(super) fn suspend(&mut self) -> Result<()> {
        if self.active {
            // Restore input even if the output stream can no longer be written.
            let screen = self.write("\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l");
            termios::tcsetattr(io::stdin(), OptionalActions::Flush, &self.original)
                .context("restoring terminal input mode")?;
            self.active = false;
            screen?;
        }
        Ok(())
    }

    pub(super) fn size(&self) -> Result<(usize, usize)> {
        let size = termios::tcgetwinsize(io::stdout()).context("reading terminal dimensions")?;
        Ok((usize::from(size.ws_col), usize::from(size.ws_row)))
    }

    pub(super) fn write(&self, text: &str) -> Result<()> {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(text.as_bytes())
            .context("drawing terminal")?;
        stdout.flush().context("flushing terminal")
    }

    fn byte(&self) -> Result<Option<u8>> {
        let mut byte = [0];
        match io::stdin().read(&mut byte) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(byte[0])),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(error) => Err(error).context("reading terminal input"),
        }
    }

    pub(super) fn key(&mut self) -> Result<Key> {
        self.read_key(false)
    }

    /// Read a key only if one is already queued, so long operations can poll for Esc.
    pub(super) fn pending_key(&mut self) -> Result<Key> {
        let waiting = termios::tcgetattr(io::stdin()).context("reading terminal mode")?;
        let mut immediate = waiting.clone();
        immediate.special_codes[SpecialCodeIndex::VTIME] = 0;
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &immediate)
            .context("polling terminal input")?;
        let key = self.read_key(false);
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &waiting)
            .context("restoring terminal input timeout")?;
        key
    }

    // Pasted text is accepted only by explicit input fields; pasted controls cannot submit.
    pub(super) fn text_key(&mut self) -> Result<Key> {
        self.read_key(true)
    }

    fn read_key(&mut self, allow_paste: bool) -> Result<Key> {
        let Some(byte) = self.byte()? else {
            return Ok(Key::Ignore);
        };
        if byte == 27 {
            return self.escape();
        }
        if self.pasting && (!allow_paste || byte.is_ascii_control()) {
            return Ok(Key::Ignore);
        }
        match byte {
            3 | 4 => Err(ExitRequested.into()),
            b'\r' | b'\n' => Ok(Key::Enter),
            8 | 127 => Ok(Key::Backspace),
            9 => Ok(Key::Details),
            32..=126 => Ok(Key::Character(char::from(byte))),
            0xc2..=0xf4 => self.unicode(byte),
            _ => Ok(Key::Ignore),
        }
    }

    fn unicode(&self, first: u8) -> Result<Key> {
        let length = if first < 0xe0 {
            2
        } else if first < 0xf0 {
            3
        } else {
            4
        };
        let mut bytes = vec![first];
        for _ in 1..length {
            let Some(byte) = self.byte()? else {
                return Ok(Key::Ignore);
            };
            bytes.push(byte);
        }
        Ok(std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| s.chars().next())
            .filter(|c| !c.is_control())
            .map_or(Key::Ignore, Key::Character))
    }

    fn escape(&mut self) -> Result<Key> {
        let Some(prefix) = self.byte()? else {
            return Ok(Key::Escape);
        };
        if !matches!(prefix, b'[' | b'O') {
            return Ok(Key::Ignore);
        }
        let mut sequence = Vec::new();
        for _ in 0..16 {
            let Some(byte) = self.byte()? else {
                return Ok(Key::Ignore);
            };
            sequence.push(byte);
            if (0x40..=0x7e).contains(&byte) {
                break;
            }
        }
        match sequence.as_slice() {
            b"200~" => {
                self.pasting = true;
                Ok(Key::Ignore)
            }
            b"201~" => {
                self.pasting = false;
                Ok(Key::Ignore)
            }
            _ if self.pasting => Ok(Key::Ignore),
            b"A" => Ok(Key::Up),
            b"B" => Ok(Key::Down),
            b"5~" => Ok(Key::PageUp),
            b"6~" => Ok(Key::PageDown),
            _ => Ok(Key::Ignore),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Err(error) = self.suspend() {
            eprintln!("error: {error:#}");
        }
    }
}
