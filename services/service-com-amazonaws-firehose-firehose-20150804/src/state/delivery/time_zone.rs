// SPDX-License-Identifier: CC0-1.0

//! The zone a delivery renders the time parts of its object names in.
//!
//! A destination configuration states it: "The time zone you prefer. UTC is the default."
//! (`ExtendedS3DestinationConfiguration$CustomTimeZone`, an IANA zone name). Everything a
//! delivery writes the clock into — the `!{timestamp:<pattern>}` expansions of `Prefix` and
//! `ErrorOutputPrefix`, the automatic `YYYY/MM/DD/HH` segments a prefix carrying no
//! expression takes, and the timestamp a delivered object's own name carries — is the
//! delivery's instant as THAT zone's wall clock reads it, so the digits a key carries are
//! the local ones a consumer partitions on.
//!
//! The zone's offset at the delivery's instant — and with it every daylight-saving change
//! the zone has ever made — comes from the IANA database the maintained zone library
//! carries, never from arithmetic here.

use std::time::SystemTime;

/// The zone one destination's deliveries are named in: the zone its `CustomTimeZone` names,
/// and UTC — the member's documented default — when it names none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeliveryTimeZone(Option<::chrono_tz::Tz>);

/// The calendar fields of one instant in a delivery's zone, each zero-padded as a key
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarFields {
    /// The proleptic calendar year, at least four digits.
    pub year: String,
    /// The month of the year, `01`..`12`.
    pub month: String,
    /// The day of the month, `01`..`31`.
    pub day: String,
    /// The hour of the day, `00`..`23`.
    pub hour: String,
    /// The minute of the hour, `00`..`59`.
    pub minute: String,
    /// The second of the minute, `00`..`59`.
    pub second: String,
}

impl DeliveryTimeZone {
    /// The zone a destination's `CustomTimeZone` names.
    ///
    /// UTC when the member is absent or empty — "UTC is the default", and the member's own
    /// pattern (`^$|[a-zA-Z/_]+$`) admits the empty value — and UTC for a value naming no
    /// zone the IANA database holds, which leaves such a destination delivering under the
    /// default rather than under digits invented for a zone nobody published.
    #[must_use]
    pub fn of(configured: Option<&str>) -> Self {
        let Some(name) = configured.map(str::trim).filter(|name| !name.is_empty()) else {
            return Self(None);
        };
        match name.parse::<::chrono_tz::Tz>() {
            Ok(zone) => Self(Some(zone)),
            Err(_) => {
                ::tracing::warn!(
                    %name,
                    "the destination's custom time zone names no zone of the IANA database, \
                     so its deliveries are named in UTC"
                );
                Self(None)
            }
        }
    }

    /// The calendar fields of `at` as this zone's wall clock reads them.
    ///
    /// `None` for an instant the calendar cannot place: `SystemTime` reaches further than
    /// the date library does, so such an instant is answered with no fields instead of
    /// panicking inside the conversion.
    #[must_use]
    pub fn fields(self, at: SystemTime) -> Option<CalendarFields> {
        let since_epoch = at.duration_since(SystemTime::UNIX_EPOCH).ok()?;
        let seconds = i64::try_from(since_epoch.as_secs()).ok()?;
        let utc = ::chrono::DateTime::from_timestamp(seconds, 0)?;
        match self.0 {
            // The same instant, read off the zone's own clock: the library applies the
            // offset the zone was keeping AT that instant, so a zone that changes its
            // offset over the year is read correctly on either side of the change.
            Some(zone) => Some(fields_of(&utc.with_timezone(&zone))),
            None => Some(fields_of(&utc)),
        }
    }
}

/// The calendar fields of an instant already placed in a zone.
fn fields_of<Zone: ::chrono::TimeZone>(at: &::chrono::DateTime<Zone>) -> CalendarFields {
    use ::chrono::{Datelike, Timelike};
    CalendarFields {
        year: format!("{:04}", at.year()),
        month: format!("{:02}", at.month()),
        day: format!("{:02}", at.day()),
        hour: format!("{:02}", at.hour()),
        minute: format!("{:02}", at.minute()),
        second: format!("{:02}", at.second()),
    }
}
