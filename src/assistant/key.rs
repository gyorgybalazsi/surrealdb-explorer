//! Getting the Claude API key: from the `ANTHROPIC_API_KEY` environment
//! variable if set, otherwise from 1Password with the `op` command-line tool.
//!
//! The key is never written to disk or to the log.

use std::io::ErrorKind;
use std::process::Command;

use crate::Error;

/// Where `op` may be installed. An app started from Finder doesn't get the
/// shell's PATH, so the usual Homebrew folders are tried too.
const OP_CANDIDATES: [&str; 3] = ["op", "/opt/homebrew/bin/op", "/usr/local/bin/op"];

/// Blocks while 1Password asks for approval, so call it off the UI thread.
pub fn fetch(reference: &str) -> Result<String, Error> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY")
        && !key.trim().is_empty()
    {
        tracing::info!("using the API key from ANTHROPIC_API_KEY");
        return Ok(key.trim().to_string());
    }

    let reference = reference.trim();
    if reference.is_empty() {
        return Err("No API key is set up. In Settings → Assistant, enter the 1Password \
                    reference to your Claude API key (op://…)."
            .into());
    }

    for op in OP_CANDIDATES {
        let output = match Command::new(op).args(["read", reference]).output() {
            Ok(output) => output,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("could not run the 1Password tool: {e}").into()),
        };
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if message.contains("authorization timeout") {
                return Err("1Password's approval prompt wasn't answered in time. Ask again and \
                            approve the prompt (it may be behind other windows)."
                    .into());
            }
            if message.contains("authorization prompt dismissed") {
                return Err("Reading the API key from 1Password was declined.".into());
            }
            return Err(format!("1Password could not read {reference}: {message}").into());
        }
        let key = String::from_utf8(output.stdout)?.trim().to_string();
        if key.is_empty() {
            return Err(format!("{reference} is empty in 1Password").into());
        }
        tracing::info!(%reference, "read the API key from 1Password");
        return Ok(key);
    }
    Err("The 1Password command-line tool (op) isn't installed. Install it with \
         `brew install --cask 1password-cli`."
        .into())
}
