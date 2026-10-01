use std::fs::OpenOptions;
use std::io::{self, Write};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(crate) fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut tty = OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .context("open controlling terminal for OSC 52 clipboard (requires a TTY)")?;
    write_osc52(&mut tty, text).context("write OSC 52 clipboard request")
}

fn write_osc52(mut writer: impl Write, text: &str) -> io::Result<()> {
    write!(writer, "\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))?;
    writer.flush()
}
