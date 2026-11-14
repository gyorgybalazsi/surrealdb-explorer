//! Reading secrets from 1Password with its `op` command-line tool.
//! Secrets are never written to disk or to the log.

use std::io::ErrorKind;
use std::process::Command;

use crate::Error;

/// Where `op` may be installed. An app started from Finder doesn't get the
/// shell's PATH, so the usual Homebrew folders are tried too.
const OP_CANDIDATES: [&str; 3] = ["op", "/opt/homebrew/bin/op", "/usr/local/bin/op"];

/// Whether `value` is a 1Password reference rather than the secret itself.
pub fn is_1password_reference(value: &str) -> bool {
    value.trim().starts_with("op://")
}

/// Reads the secret at `reference` (`op://vault/item/field`). Blocks while
/// 1Password asks for approval, so call it off the UI thread.
pub fn read_1password(reference: &str) -> Result<String, Error> {
    let reference = reference.trim();
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
                return Err("Reading the secret from 1Password was declined.".into());
            }
            return Err(format!("1Password could not read {reference}: {message}").into());
        }
        let secret = String::from_utf8(output.stdout)?.trim().to_string();
        if secret.is_empty() {
            return Err(format!("{reference} is empty in 1Password").into());
        }
        return Ok(secret);
    }
    Err("The 1Password command-line tool (op) isn't installed. Install it with \
         `brew install --cask 1password-cli`."
        .into())
}
