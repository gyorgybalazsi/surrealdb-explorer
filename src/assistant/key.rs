//! Getting the Claude API key: from the `ANTHROPIC_API_KEY` environment
//! variable if set, otherwise from 1Password with the `op` command-line tool.
//!
//! The key is never written to disk or to the log.

use crate::Error;

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

    let key = crate::secret::read_1password(reference)?;
    tracing::info!(%reference, "read the API key from 1Password");
    Ok(key)
}
