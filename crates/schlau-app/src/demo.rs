//! A deterministic synthetic mailbox for development, screenshots and performance runs without an
//! account: `schlau --demo [--demo-size N]`.

use schlau_store::{Address, Addresses, Body, BodyIndex, Flags, Mailbox, MessageRecord, Store};

const PEOPLE: &[(&str, &str)] = &[
    ("Jane Cooper", "jane@northwind.io"),
    ("Marcus Webb", "marcus@lumen.dev"),
    ("Priya Natarajan", "priya@halcyon.health"),
    ("Tomás Ortega", "tomas@ortega.studio"),
    ("Lena Fischer", "lena.fischer@bergwerk.de"),
    ("GitHub", "notifications@github.com"),
    ("Linear", "notifications@linear.app"),
    ("Stripe", "receipts@stripe.com"),
    ("Fastmail", "support@fastmail.com"),
    ("Aiko Tanaka", "aiko@kumo.jp"),
    ("Sam Okafor", "sam@okafor.co"),
    ("The Browser Company", "hello@thebrowser.company"),
];

const SUBJECTS: &[&str] = &[
    "Quarterly planning: draft agenda",
    "Invoice #4821 for October",
    "Re: the deployment pipeline is flaky again",
    "Lunch on Thursday?",
    "Your receipt from Stripe",
    "[schlau] Fix keyset paging when two threads share a timestamp (#142)",
    "Design review notes",
    "Contract renewal: two questions",
    "Flight confirmation: BER to LIS",
    "Weekly digest",
    "Offsite photos are up",
    "Can you review the hiring rubric?",
    "Security alert: new sign-in",
    "Proposal: move search to SQLite FTS5",
    "Re: budget for the kangaroo project",
];

const BODIES: &[&str] = &[
    "Hi,\n\nAttached is the draft agenda for the planning session. The main topics are roadmap, hiring and the infrastructure budget. Let me know what is missing before Friday.\n\nThanks,",
    "Thanks for your business. The invoice for October is attached; payment is due within 30 days. Reach out if anything looks off.",
    "The deploy job failed three times this morning with a timeout while pushing the image. I suspect the registry mirror. Could you take a look before the release?",
    "Are you free for lunch on Thursday? There is a new ramen place near the office that everyone keeps talking about.",
    "We received your payment. This email is your receipt; you can download a PDF copy from the dashboard.",
    "This change makes keyset paging stable when two threads have the same latest timestamp by breaking ties on the thread id.",
    "Notes from the design review: the reading view needs more line height, the command palette should show shortcuts, and we want fewer borders.",
    "Before we renew, two questions: can we move to annual billing, and is the data processing addendum still current?",
    "Your flight from Berlin to Lisbon is confirmed. Check-in opens 24 hours before departure. Seat 14A.",
    "Here is what happened this week across your projects: 12 issues closed, 4 opened, and 3 pull requests merged.",
    "The photos from the offsite are in the shared album. Thanks everyone for a great two days in the mountains!",
    "Could you review the hiring rubric for the senior engineer role? I want to send it to the panel tomorrow.",
    "We noticed a new sign-in to your account from Safari on macOS in Berlin. If this was you, there is nothing to do.",
    "Proposal: index every message in SQLite FTS5 with a typo-tolerant vocabulary, and fuse the results with embeddings for meaning.",
    "The kangaroo project budget is approved for the next quarter, with a small increase for hardware.",
];

const NEWSLETTER: &str = r#"<html><body><table width="600"><tr><td>
<h1>This week at Northwind</h1>
<p>Hi there, here is what shipped this week. <b>Search is now typo-tolerant</b>, and the
<a href="https://northwind.io/changelog?utm_source=newsletter&utm_medium=email&utm_campaign=w45">changelog</a> has the details.</p>
<ul><li>Faster sync on large mailboxes</li><li>Keyboard shortcuts for everything</li></ul>
<blockquote>Inbox zero is a state of mind.</blockquote>
<p><img src="https://cdn.northwind.io/hero.png" alt="Product screenshot" width="560" height="300"></p>
<p><a href="https://www.google.com/url?q=https://northwind.io/pricing&sa=D">See pricing</a></p>
<img src="https://northwind.list-manage.com/track/open.php?u=abc&id=def" width="1" height="1">
</td></tr></table></body></html>"#;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }
}

fn mailbox(id: &str, name: &str, role: Option<&str>, order: i64) -> Mailbox {
    Mailbox {
        id: id.into(),
        name: name.into(),
        parent_id: None,
        role: role.map(Into::into),
        sort_order: order,
        total_emails: 0,
        unread_emails: 0,
        total_threads: 0,
        unread_threads: 0,
        is_subscribed: true,
    }
}

/// Fills an empty store with `size` messages over the last year. Does nothing if it has messages.
pub fn seed(store: &mut Store, size: usize, now: i64) -> schlau_store::Result<()> {
    if store.reader()?.message_count()? > 0 {
        return Ok(());
    }
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let me = Address { name: "Denis".into(), email: "me@schlau.example".into() };
    store.write(|tx| {
        [
            mailbox("inbox", "Inbox", Some("inbox"), 1),
            mailbox("archive", "Archive", Some("archive"), 2),
            mailbox("sent", "Sent", Some("sent"), 3),
            mailbox("drafts", "Drafts", Some("drafts"), 4),
            mailbox("junk", "Spam", Some("junk"), 5),
            mailbox("trash", "Trash", Some("trash"), 6),
            mailbox("receipts", "Receipts", None, 10),
            mailbox("travel", "Travel", None, 11),
        ]
        .iter()
        .try_for_each(|m| tx.upsert_mailbox(m))?;
        (0..size).try_for_each(|i| {
            let age = (i as i64) * (365 * 86_400) / size.max(1) as i64 + (rng.next() % 600) as i64;
            let thread = i / 3;
            let topic = thread % SUBJECTS.len();
            let (name, email) = *rng.pick(PEOPLE);
            let from_me = i % 3 == 1;
            let sender = if from_me { me.clone() } else { Address { name: name.into(), email: email.into() } };
            let mailbox = match (from_me, i) {
                (true, _) => "sent",
                (_, i) if i < 60 => "inbox",
                (_, _) if topic == 1 || topic == 4 => "receipts",
                (_, _) if topic == 8 => "travel",
                _ => "archive",
            };
            let subject = if i % 3 == 2 { SUBJECTS[topic].to_string() } else { format!("Re: {}", SUBJECTS[topic]) };
            let body = BODIES[topic];
            let unread = mailbox == "inbox" && rng.next().is_multiple_of(3);
            let record = MessageRecord {
                id: format!("demo-{i}"),
                thread_id: format!("thread-{thread}"),
                received_at: now - age,
                size: 2_000 + (rng.next() % 40_000) as i64,
                subject,
                addresses: Addresses { from: vec![sender], to: vec![me.clone()], ..Default::default() },
                preview: body.replace('\n', " ").chars().take(160).collect(),
                has_attachment: topic == 1 || topic == 10,
                flags: Flags::default().with(Flags::SEEN, !unread).with(Flags::FLAGGED, i % 41 == 0),
                mailbox_ids: vec![mailbox.into()],
                ..Default::default()
            };
            tx.upsert_message(&record)?;
            if i < 2_000 {
                let html = (topic == 9).then(|| NEWSLETTER.to_string());
                let text = (topic != 9).then(|| body.to_string());
                tx.set_body(&record.id, &Body { text, html, ..Default::default() }, &BodyIndex { text: body.to_string() })?;
            }
            Ok(())
        })
    })?;
    store.optimize()
}
