use rusqlite::OptionalExtension;

use crate::Result;
use crate::model::{Action, Flags, MessageUpdate};
use crate::write::{WriteTx, flag_for_keyword};

impl WriteTx<'_> {
    /// Applies a user action to the local replica and queues it for the server, atomically, so the
    /// UI shows the result on its next read and the outbox replays it later.
    pub fn perform(&self, action: &Action) -> Result<i64> {
        self.apply_locally(action)?;
        self.enqueue(action)
    }

    /// Applies an action to the local replica only. Unknown messages are skipped.
    pub fn apply_locally(&self, action: &Action) -> Result<()> {
        match action {
            Action::SetFlag { message_ids, flag, on } => {
                let Some(flag) = flag_for_keyword(flag) else { return Ok(()) };
                message_ids.iter().try_for_each(|id| {
                    let Some((flags, _)) = self.message_state(id)? else { return Ok(()) };
                    self.update_message(&MessageUpdate {
                        id: id.clone(),
                        flags: Some(flags.with(flag, *on)),
                        ..Default::default()
                    })
                    .map(|_| ())
                })
            }
            Action::Move { message_ids, from, to } => self.edit_mailboxes(message_ids, |boxes| {
                match from {
                    Some(from) => boxes.retain(|b| b != from),
                    None => boxes.clear(),
                }
                if !boxes.contains(to) {
                    boxes.push(to.clone());
                }
            }),
            Action::AddMailbox { message_ids, mailbox_id } => self.edit_mailboxes(message_ids, |boxes| {
                if !boxes.contains(mailbox_id) {
                    boxes.push(mailbox_id.clone());
                }
            }),
            Action::RemoveMailbox { message_ids, mailbox_id } => {
                self.edit_mailboxes(message_ids, |boxes| boxes.retain(|b| b != mailbox_id))
            }
            Action::Destroy { message_ids } => {
                message_ids.iter().try_for_each(|id| self.delete_message(id).map(|_| ()))
            }
        }
    }

    fn edit_mailboxes(&self, message_ids: &[String], edit: impl Fn(&mut Vec<String>)) -> Result<()> {
        message_ids.iter().try_for_each(|id| {
            let Some((_, mut boxes)) = self.message_state(id)? else { return Ok(()) };
            edit(&mut boxes);
            self.update_message(&MessageUpdate { id: id.clone(), mailbox_ids: Some(boxes), ..Default::default() })
                .map(|_| ())
        })
    }

    fn message_state(&self, id: &str) -> Result<Option<(Flags, Vec<String>)>> {
        let row: Option<(u32, Option<String>)> = self
            .tx
            .prepare_cached(
                "SELECT m.flags, (SELECT group_concat(mailbox_id, char(31)) FROM message_mailboxes
                                  WHERE message = m.rowid)
                 FROM messages m WHERE m.id = ?1",
            )?
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        Ok(row.map(|(flags, boxes)| {
            (Flags(flags), boxes.map(|b| b.split('\u{1f}').map(str::to_string).collect()).unwrap_or_default())
        }))
    }
}
