//! Adaptive junk mail filter (naive Bayes over tokens stored in the `junk_tokens` table).
//! STUB: to be implemented.

use super::mime::ParsedMessage;
use super::store::Store;

/// Probability (0..1) that a message is junk.
pub fn score(_store: &Store, _p: &ParsedMessage) -> f32 {
    0.0
}

/// Train the filter with a message the user marked as junk (`spam = true`) or not junk.
pub fn train(_store: &Store, _p: &ParsedMessage, _spam: bool) {}

/// Whether server-side spam headers (X-Spam-Flag, X-Spam-Status, ...) mark the message as junk.
pub fn server_says_junk(_p: &ParsedMessage) -> bool {
    false
}
