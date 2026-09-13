use super::error;
use crate::output::AppError;
use rustix::{
    fs::{fcntl_getfl, fcntl_setfl, OFlags},
    termios::{tcgetattr, tcsetattr, LocalModes, OptionalActions, Termios},
};
use std::{
    fs::File,
    io::{IsTerminal, Write},
};
use zeroize::Zeroizing;

pub struct Terminal {
    file: File,
    original: Termios,
}

fn failed() -> AppError {
    error(
        "terminal_input",
        "Could not read the terminal securely; account setup stopped.",
        2,
    )
}

impl Terminal {
    pub fn check(json: bool) -> Result<(), AppError> {
        if json || !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(error("interactive_terminal_required", "Run account add in an interactive terminal. For scripts, use account add-google or account add-imap.", 2));
        }
        Ok(())
    }
    pub fn new() -> Result<Self, AppError> {
        Self::check(false)?;
        let file = File::open("/dev/tty").map_err(|_| failed())?;
        let original = tcgetattr(&file).map_err(|_| failed())?;
        let flags = fcntl_getfl(&file).map_err(|_| failed())?;
        fcntl_setfl(&file, flags | OFlags::NONBLOCK).map_err(|_| failed())?;
        Ok(Self { file, original })
    }
    pub fn say(&self, text: &str) -> Result<(), AppError> {
        writeln!(std::io::stderr().lock(), "{text}").map_err(|_| failed())
    }
    pub async fn ask(&self, label: &str, default: Option<&str>) -> Result<String, AppError> {
        let value = self.line(label, default, false).await?;
        let value = value.trim();
        Ok(if value.is_empty() {
            default.unwrap_or("").to_owned()
        } else {
            value.to_owned()
        })
    }
    pub async fn password(&self, label: &str) -> Result<Zeroizing<String>, AppError> {
        self.line(label, None, true).await
    }
    async fn line(
        &self,
        label: &str,
        default: Option<&str>,
        secret: bool,
    ) -> Result<Zeroizing<String>, AppError> {
        let mut attributes = self.original.clone();
        if secret {
            attributes
                .local_modes
                .remove(LocalModes::ECHO | LocalModes::ECHONL);
        }
        tcsetattr(&self.file, OptionalActions::Now, &attributes).map_err(|_| failed())?;
        {
            let mut output = std::io::stderr().lock();
            if let Some(value) = default {
                write!(output, "{label} [{value}]: ")
            } else {
                write!(output, "{label}: ")
            }
            .map_err(|_| failed())?;
            output.flush().map_err(|_| failed())?;
        }
        let mut bytes = Zeroizing::new(Vec::new());
        loop {
            let mut chunk = Zeroizing::new([0u8; 1024]);
            match rustix::io::read(&self.file, &mut *chunk) {
                Ok(0) => return Err(error("setup_cancelled", "Account setup cancelled.", 130)),
                Ok(n) => {
                    bytes.extend_from_slice(&chunk[..n]);
                    if bytes.len() > 4096 {
                        return Err(error(
                            "input_too_long",
                            "That value is too long; account setup stopped.",
                            2,
                        ));
                    }
                    if bytes.last() == Some(&b'\n') {
                        break;
                    }
                }
                Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await
                }
                Err(_) => return Err(failed()),
            }
        }
        tcsetattr(&self.file, OptionalActions::Now, &self.original).map_err(|_| failed())?;
        if secret {
            self.say("")?;
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let value = std::str::from_utf8(&bytes).map_err(|_| failed())?;
        if value.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        }) {
            return Err(error(
                "invalid_input",
                "Control characters are not allowed in setup values.",
                2,
            ));
        }
        Ok(Zeroizing::new(value.to_owned()))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = tcsetattr(&self.file, OptionalActions::Now, &self.original);
    }
}
