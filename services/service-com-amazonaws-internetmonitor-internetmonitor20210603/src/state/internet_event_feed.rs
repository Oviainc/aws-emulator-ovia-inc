// SPDX-License-Identifier: CC0-1.0

//! The internet-event feed this service observes for itself.
//!
//! Internet events are a property of the SERVICE, not of anything a caller created:
//! Internet Monitor "displays information about recent global health events, called
//! internet events, on a global outages map that is available to all Amazon Web Services
//! customers"
//! (`smithy_query(query="doc",shape="ListInternetEvents")`), and the model declares no
//! operation that creates, updates or deletes one. So an account that has configured
//! nothing still reads a populated feed, and this emulator holds the observations it
//! reports from the moment its store exists — written through
//! [`super::ComAmazonawsInternetmonitorInternetmonitor20210603State::record_internet_event`],
//! the one writer of that store, so `ListInternetEvents` and `GetInternetEvent` report the
//! same records.
//!
//! The feed below is this emulator's own fixed observation set, not a capture of any one
//! run: a bounded number of events spread evenly over the window that ends at the instant
//! the store is created, alternating over the modelled `InternetEventType` members and
//! ending each event after a span that leaves the most recent ones still unresolved.

use std::time::{Duration, SystemTime};

use super::{InternetEventRecord, MAX_INTERNET_EVENTS};
use crate::ops::types::{ClientLocation, INTERNET_EVENT_TYPE_VALUES};

/// How far back the feed reaches: every seeded event started inside the window of this
/// length that ends when the store was created.
///
/// Chosen (AWS publishes no retention for the outages map), and wide enough that a caller
/// listing the last day of internet events reads the whole feed.
pub const INTERNET_EVENT_FEED_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How many internet events the feed holds.
///
/// A count this emulator chooses, kept well inside [`MAX_INTERNET_EVENTS`] so the feed
/// leaves room for anything else recorded into the same store.
pub const SEEDED_INTERNET_EVENTS: usize = 96;

/// One client location of the feed: the members `ClientLocation` declares
/// (`smithy_query(query="shape",shape="ClientLocation")`), as the catalog below fills
/// them.
struct SeedLocation {
    /// `AsName` — the network the impacted clients are on.
    as_name: &'static str,
    /// `AsNumber`.
    as_number: i64,
    /// `Country`.
    country: &'static str,
    /// `Subdivision`.
    subdivision: &'static str,
    /// `Metro`.
    metro: &'static str,
    /// `City`.
    city: &'static str,
    /// `Latitude`, degrees north.
    latitude: f64,
    /// `Longitude`, degrees east.
    longitude: f64,
}

/// The client locations the feed's events impact: one fixed catalog this emulator
/// observes, spread over several countries and networks so a caller filtering or
/// rendering the outages map sees more than one place. It is this emulator's own
/// catalog — no recorded run's values — and every entry fills exactly the
/// `ClientLocation` members the model requires.
const SEED_LOCATIONS: &[SeedLocation] = &[
    SeedLocation {
        as_name: "AMAZON-02",
        as_number: 16509,
        country: "United States",
        subdivision: "Washington",
        metro: "Seattle-Tacoma-Bellevue WA",
        city: "Seattle",
        latitude: 47.6062,
        longitude: -122.3321,
    },
    SeedLocation {
        as_name: "COMCAST-7922",
        as_number: 7922,
        country: "United States",
        subdivision: "Pennsylvania",
        metro: "Philadelphia-Camden-Wilmington PA-NJ-DE-MD",
        city: "Philadelphia",
        latitude: 39.9526,
        longitude: -75.1652,
    },
    SeedLocation {
        as_name: "DTAG",
        as_number: 3320,
        country: "Germany",
        subdivision: "Hesse",
        metro: "Frankfurt am Main",
        city: "Frankfurt am Main",
        latitude: 50.1109,
        longitude: 8.6821,
    },
    SeedLocation {
        as_name: "NTT-COMMUNICATIONS",
        as_number: 2914,
        country: "Japan",
        subdivision: "Tokyo",
        metro: "Tokyo",
        city: "Tokyo",
        latitude: 35.6762,
        longitude: 139.6503,
    },
    SeedLocation {
        as_name: "BHARTI-AIRTEL",
        as_number: 9498,
        country: "India",
        subdivision: "Maharashtra",
        metro: "Mumbai",
        city: "Mumbai",
        latitude: 19.076,
        longitude: 72.8777,
    },
    SeedLocation {
        as_name: "TELSTRA-AS",
        as_number: 1221,
        country: "Australia",
        subdivision: "New South Wales",
        metro: "Sydney",
        city: "Sydney",
        latitude: -33.8688,
        longitude: 151.2093,
    },
    SeedLocation {
        as_name: "EIRCOM",
        as_number: 5466,
        country: "Ireland",
        subdivision: "Leinster",
        metro: "Dublin",
        city: "Dublin",
        latitude: 53.3498,
        longitude: -6.2603,
    },
    SeedLocation {
        as_name: "UNINET",
        as_number: 8151,
        country: "Mexico",
        subdivision: "Mexico City",
        metro: "Mexico City",
        city: "Mexico City",
        latitude: 19.4326,
        longitude: -99.1332,
    },
    SeedLocation {
        as_name: "ALGAR-TELECOM",
        as_number: 16735,
        country: "Brazil",
        subdivision: "Sao Paulo",
        metro: "Sao Paulo",
        city: "Sao Paulo",
        latitude: -23.5505,
        longitude: -46.6333,
    },
    SeedLocation {
        as_name: "KIXS-AS-KR",
        as_number: 4766,
        country: "South Korea",
        subdivision: "Seoul",
        metro: "Seoul",
        city: "Seoul",
        latitude: 37.5665,
        longitude: 126.978,
    },
];

/// The `EventId` the feed's `index`th event carries.
///
/// `InternetEventId` is constrained to `^[a-zA-Z0-9-]+$`, length 1..255
/// (`smithy_query(query="operation",operation="GetInternetEvent")`), which this form
/// satisfies; it is opaque to the caller, and ordering ids by index is what gives
/// `ListInternetEvents` one stable total order to page over.
pub fn seed_event_id(index: usize) -> String {
    format!("internet-event-{index:04}")
}

/// The feed as it stands at `now`: [`SEEDED_INTERNET_EVENTS`] events whose starts are
/// spread evenly over the [`INTERNET_EVENT_FEED_WINDOW`] that ends at `now`.
///
/// Each event alternates over the modelled `InternetEventType` members and impacts the
/// next location of [`SEED_LOCATIONS`], so both types and every location are represented.
/// An event lasts a span that grows with its index modulo the catalog's cycle: an event
/// whose span has already elapsed at `now` carries its `EndedAt` and so reports
/// `RESOLVED`, and one whose span has not carries none and so reports `ACTIVE` — the
/// derivation [`InternetEventRecord::status`] makes, so the two members cannot disagree.
pub fn seed_internet_events(now: SystemTime) -> Vec<InternetEventRecord> {
    let count = SEEDED_INTERNET_EVENTS.min(MAX_INTERNET_EVENTS);
    let step = INTERNET_EVENT_FEED_WINDOW / u32::try_from(count).unwrap_or(1).max(1);
    (0..count)
        .map(|index| {
            let age = step * u32::try_from(count - index).unwrap_or(1);
            let started_at = now
                .checked_sub(age)
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .max(SystemTime::UNIX_EPOCH);
            // A span of one to five steps, so the feed holds both long and short events
            // and the most recent ones are still running at `now`.
            let span = step * (1 + u32::try_from(index % 5).unwrap_or(0));
            let ended_at = started_at
                .checked_add(span)
                .filter(|ends_at| *ends_at <= now);
            let location = &SEED_LOCATIONS[index % SEED_LOCATIONS.len()];
            InternetEventRecord::new(
                seed_event_id(index),
                started_at,
                ended_at,
                ClientLocation {
                    as_name: location.as_name.to_string(),
                    as_number: location.as_number,
                    country: location.country.to_string(),
                    subdivision: Some(location.subdivision.to_string()),
                    metro: Some(location.metro.to_string()),
                    city: location.city.to_string(),
                    latitude: location.latitude,
                    longitude: location.longitude,
                },
                INTERNET_EVENT_TYPE_VALUES[index % INTERNET_EVENT_TYPE_VALUES.len()],
            )
        })
        .collect()
}
