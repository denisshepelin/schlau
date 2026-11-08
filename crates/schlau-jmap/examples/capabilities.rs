//! Prints the capability names the account's session advertises, and whether the screener can use
//! contact groups. Read-only: it fetches the session and, with contacts, the address books.
//!
//! Run with: FASTMAIL_API_TOKEN=... cargo run -p schlau-jmap --example capabilities

use schlau_jmap::{Client, capability};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("FASTMAIL_API_TOKEN")
        .or_else(|_| std::env::var("FASTMAIL_TOKEN"))
        .map_err(|_| "set FASTMAIL_API_TOKEN to a Fastmail API token")?;
    let client = Client::fastmail(&token)?;
    let mut names: Vec<&String> = client.session().capabilities.keys().collect();
    names.sort();
    println!("session capabilities:");
    names.iter().for_each(|n| println!("  {n}"));
    let sieve = client.session().has_capability("urn:ietf:params:jmap:sieve");
    println!("\nsieve over JMAP: {}", if sieve { "yes" } else { "no" });
    match client.has_contacts() {
        false => println!(
            "contacts: no. Create a token with the {} scope for the screener.",
            capability::CONTACTS
        ),
        true => {
            let books = client.address_books()?;
            println!("contacts: yes, {} address book(s)", books.len());
        }
    }
    Ok(())
}
