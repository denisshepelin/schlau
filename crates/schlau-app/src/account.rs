//! Which JMAP account to sync, and its credentials. The token comes from the environment
//! (`FASTMAIL_API_TOKEN` or `FASTMAIL_TOKEN`, or `JMAP_TOKEN` with `JMAP_SESSION_URL` for another server), else from the
//! macOS Keychain item `schlau` (`security add-generic-password -s schlau -a <account> -w <token>`).

use std::sync::Arc;

use schlau_jmap::{Client, FASTMAIL_SESSION_URL, Transport, UreqTransport};
use schlau_sync::Connect;

pub struct Account {
    pub session_url: String,
    token: String,
}

impl Account {
    pub fn from_environment() -> Option<Account> {
        let session_url = std::env::var("JMAP_SESSION_URL").unwrap_or_else(|_| FASTMAIL_SESSION_URL.to_string());
        let token = ["FASTMAIL_API_TOKEN", "FASTMAIL_TOKEN", "JMAP_TOKEN"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|t| !t.trim().is_empty()))
            .or_else(keychain_token)?;
        Some(Account { session_url, token })
    }

    pub fn connect(self) -> Connect {
        Arc::new(move || {
            let transport: Arc<dyn Transport> = Arc::new(UreqTransport::bearer(&self.token));
            Client::connect(transport, &self.session_url)
        })
    }
}

#[cfg(target_os = "macos")]
fn keychain_token() -> Option<String> {
    let out = std::process::Command::new("security").args(["find-generic-password", "-s", "schlau", "-w"]).output().ok()?;
    let token = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

#[cfg(not(target_os = "macos"))]
fn keychain_token() -> Option<String> {
    None
}
