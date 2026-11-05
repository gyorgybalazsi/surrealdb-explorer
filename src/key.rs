//! The database encryption key, kept in the macOS Keychain.
//!
//! On first use a random 256-bit key is created and stored as a generic
//! password (service from the config, by default `surrealdb-explore`;
//! account `data-encryption-key`).
//! Afterwards it is read from there. Keychain Access shows the item, and
//! deleting it makes the encrypted database unreadable for good.

use std::io::Read;
use std::process::Command;
use std::sync::Arc;

use surrealkv::SoftwareKeyManager;

use crate::Error;

const ACCOUNT: &str = "data-encryption-key";
/// Exit code of `security find-generic-password` when there is no such item.
const NOT_FOUND: i32 = 44;

/// Installs the encryption key for every database this process opens.
/// Call before opening the database; later calls do nothing.
pub fn install(service: &str) -> Result<(), Error> {
    let hex = load_or_create(service)?;
    let manager = SoftwareKeyManager::from_hex(&hex)
        .map_err(|e| format!("the key in the Keychain is not a valid encryption key: {e}"))?;
    if surrealdb_kvs_surrealkv::set_key_manager(Arc::new(manager)) {
        tracing::info!(service, "encryption key installed");
    }
    Ok(())
}

/// The key as 64 hex digits, from the Keychain, or a new one stored there.
fn load_or_create(service: &str) -> Result<String, Error> {
    let found = Command::new("security")
        .args(["find-generic-password", "-s", service, "-a", ACCOUNT, "-w"])
        .output()?;
    if found.status.success() {
        return Ok(String::from_utf8(found.stdout)?.trim().to_string());
    }
    if found.status.code() != Some(NOT_FOUND) {
        return Err(format!(
            "could not read the encryption key from the Keychain: {}",
            String::from_utf8_lossy(&found.stderr).trim()
        )
        .into());
    }

    let hex = new_key()?;
    // `security` takes the secret as an argument; it is visible to other
    // processes of this user only for the moment the command runs, once.
    let stored = Command::new("security")
        .args(["add-generic-password", "-s", service, "-a", ACCOUNT, "-w", &hex])
        .args(["-D", "database encryption key", "-j", "Encrypts the surrealdb-explore database at rest"])
        .output()?;
    if !stored.status.success() {
        return Err(format!(
            "could not store a new encryption key in the Keychain: {}",
            String::from_utf8_lossy(&stored.stderr).trim()
        )
        .into());
    }
    tracing::warn!(
        service,
        account = ACCOUNT,
        "created a new database encryption key in the Keychain; back it up, the data can't be read without it"
    );
    Ok(hex)
}

/// 32 random bytes from the operating system, as hex.
fn new_key() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
