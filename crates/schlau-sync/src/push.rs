use std::sync::mpsc::Sender;
use std::time::Duration;

use schlau_jmap::PushEvent;

use crate::engine::{Command, Connect};

/// Listens to the account's event stream and asks the engine to sync on every state change.
/// Reconnects with backoff; ends when the engine is gone.
pub fn spawn(connect: Connect, commands: Sender<Command>) {
    let _ = std::thread::Builder::new().name("schlau-push".into()).spawn(move || {
        let mut backoff = Duration::from_secs(2);
        let mut last_event_id: Option<String> = None;
        loop {
            let stream = connect().and_then(|client| client.event_source("Email,Mailbox", 30, last_event_id.as_deref()));
            match stream {
                Ok(mut stream) => {
                    backoff = Duration::from_secs(2);
                    for event in stream.by_ref() {
                        match event {
                            Ok(PushEvent::StateChange(_)) => {
                                if commands.send(Command::Refresh).is_err() {
                                    return;
                                }
                            }
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                    last_event_id = stream.last_event_id().map(str::to_string);
                }
                Err(_) => backoff = (backoff * 2).min(Duration::from_secs(300)),
            }
            if commands.send(Command::Refresh).is_err() {
                return;
            }
            std::thread::sleep(backoff);
        }
    });
}
