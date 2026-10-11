//! Import and export of mailboxes in mbox format (and folders of .eml files).
//! STUB: to be implemented.

use std::path::Path;

/// Read all messages from an mbox file (mboxrd/mboxo) or a directory of `.eml` files / Maildir.
pub fn import(_path: &Path) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(Vec::new())
}

/// Write messages to an mbox file.
pub fn export(_path: &Path, _messages: &[Vec<u8>]) -> anyhow::Result<()> {
    Ok(())
}
