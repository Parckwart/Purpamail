//! Automatic categorization of incoming mail (Primary / Transactions / Updates / Promotions).
//! STUB: to be implemented.

use super::mime::ParsedMessage;
use super::model::Category;

/// Decide the category of a message from its headers and content.
pub fn categorize(_p: &ParsedMessage) -> Category {
    Category::Primary
}
