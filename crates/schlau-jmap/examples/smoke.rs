//! Prints the session account, mailboxes with counts, and the 10 newest inbox subjects.
//!
//! Run with: FASTMAIL_API_TOKEN=... cargo run -p schlau-jmap --example smoke

use schlau_jmap::{
    Client, Comparator, Email, EmailFilterCondition, EmailGetArgs, Query, find_by_role, role,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("FASTMAIL_API_TOKEN")
        .or_else(|_| std::env::var("FASTMAIL_TOKEN"))
        .map_err(|_| "set FASTMAIL_API_TOKEN to a Fastmail API token")?;
    let client = Client::fastmail(&token)?;
    let session = client.session();
    let account_id = client.mail_account_id()?;
    println!("user: {}  mail account: {account_id}", session.username);

    let mut mailboxes = client.mailboxes()?.list;
    mailboxes.sort_by_key(|m| (m.sort_order.unwrap_or(u32::MAX), m.name.clone()));
    mailboxes.iter().for_each(|m| {
        println!(
            "{:>7} {:>7}  {:<30} {}",
            m.unread_emails.unwrap_or(0),
            m.total_emails.unwrap_or(0),
            m.name.as_deref().unwrap_or("?"),
            m.role.as_deref().unwrap_or(""),
        )
    });

    let inbox = find_by_role(&mailboxes, role::INBOX).ok_or("no inbox mailbox")?;
    let query = Query::<Email>::new(account_id)
        .filter(EmailFilterCondition::in_mailbox(inbox.id.clone()))
        .sort([Comparator::received_at_desc()])
        .limit(10);
    let (_, emails) = client.query_and_get_emails(
        query,
        Some(&["id", "receivedAt", "from", "subject", "keywords"]),
        EmailGetArgs::default(),
    )?;
    println!("\nnewest in {}:", inbox.name.as_deref().unwrap_or("Inbox"));
    emails.list.iter().for_each(|e| {
        let from = e
            .from
            .as_ref()
            .and_then(|f| f.first())
            .map(|a| a.name.clone().unwrap_or_else(|| a.email.clone()))
            .unwrap_or_default();
        println!(
            "{} {} {:<24} {}",
            if e.is_unread() { "*" } else { " " },
            e.received_at.as_deref().unwrap_or(""),
            from,
            e.subject.as_deref().unwrap_or("(no subject)")
        )
    });
    Ok(())
}
