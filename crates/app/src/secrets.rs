//! Secrets (the Vcity session) live in the OS keychain (macOS Keychain, Windows Credential
//! Manager, Linux kernel keyring), never in settings files or the data root. Never logged.

pub fn get(service: &str, account: &str) -> Option<String> {
    keyring::Entry::new(service, account).ok()?.get_password().ok().filter(|p| !p.is_empty())
}

/// Stores `secret`; an empty one deletes the entry.
pub fn set(service: &str, account: &str, secret: &str) -> Result<(), String> {
    let e = keyring::Entry::new(service, account).map_err(|e| e.to_string())?;
    if secret.is_empty() {
        return match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    e.set_password(secret).map_err(|e| e.to_string())
}
