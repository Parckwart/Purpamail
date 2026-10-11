//! vCard (RFC 6350 / 2426) import and export for contacts.
//! STUB: to be implemented.

use super::store::Contact;

pub fn parse(_s: &str) -> Vec<Contact> {
    Vec::new()
}

pub fn to_vcard(_c: &Contact) -> String {
    String::new()
}

pub fn export_all(contacts: &[Contact]) -> String {
    contacts.iter().map(to_vcard).collect()
}
