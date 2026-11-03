pub mod mapping;
pub mod reader;
pub mod resolver;

use crate::BoxFuture;
use anyhow::Result;
pub use reader::{FileMailbox, MailTicket};

pub trait Mailbox: Send + Sync {
    fn prepare<'a>(&'a self, email: &'a str) -> BoxFuture<'a, Result<MailTicket>>;
    fn poll<'a>(&'a self, ticket: &'a MailTicket) -> BoxFuture<'a, Result<String>>;
}
