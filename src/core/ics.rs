//! Calendar invitations (iCalendar, RFC 5545) — parse events and build RSVP replies.
//! STUB: to be implemented.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    /// unix seconds
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub all_day: bool,
    pub method: String,
    pub sequence: i64,
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rsvp {
    Accept,
    Tentative,
    Decline,
}

pub fn parse(_ics: &str) -> Option<CalendarEvent> {
    None
}

/// Build a METHOD:REPLY calendar object answering `ev` as `attendee_email`.
pub fn build_reply(_ev: &CalendarEvent, _attendee_email: &str, _rsvp: Rsvp) -> String {
    String::new()
}
