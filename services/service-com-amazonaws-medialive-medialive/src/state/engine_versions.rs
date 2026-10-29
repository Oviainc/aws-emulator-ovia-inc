// SPDX-License-Identifier: CC0-1.0

//! The channel ENGINE VERSION slice of this service's state: the encoder engine
//! versions this account can run a channel on, which `ListVersions` reports.
//!
//! One file for the whole slice — the record, the catalogue that holds it, the
//! size and the spans the catalogue is seeded with, and the projection a read
//! reports through — so the slice can be read, and merged, on its own. Its store
//! is the `channel_engine_versions` member of
//! [`super::ComAmazonawsMedialiveMedialiveState`] in the parent module.
//!
//! An engine version is NOT a resource a caller creates: `ListVersions`
//! "retrieves an array of all the encoder engine versions that are available in
//! this AWS account", and no operation of this service creates, deletes or
//! changes one. So the catalogue is SEEDED when the account's store comes up and
//! no request path writes it, which is what makes the versions a caller reads
//! back the ones that were persisted rather than ones a handler assembled.
//!
//! What a read does take from the clock is which of those versions are still
//! AVAILABLE. `ChannelEngineVersionResponse$ExpirationDate` is "The UTC time when
//! the version expires", so an entry whose expiry has passed is no longer one of
//! the versions available to the account and is not reported — while its record
//! stays in the catalogue, because a read never writes.
//!
//! The three facts AWS publishes nowhere machine-readable — how many versions an
//! account is offered, how far apart they are released and how long one lasts —
//! are this EMULATOR's own ([`CHANNEL_ENGINE_VERSION_COUNT`],
//! [`CHANNEL_ENGINE_VERSION_INTERVAL`], [`CHANNEL_ENGINE_VERSION_LIFETIME`]), and
//! so is the FORM of a version's build identifier
//! ([`channel_engine_version_label`]), since the model documents `Version` only as
//! "The build identifier for this version of the channel version". Nothing should
//! read them as AWS's own release calendar.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::ChannelEngineVersionResponse;

/// How many encoder engine versions the account's catalogue carries.
///
/// A CHOSEN count: `ListVersions` documents that an account has a set of
/// available engine versions and the model carries no list of its entries, so the
/// SIZE of the seeded catalogue is the emulator's own, as the [`super::offerings`]
/// catalog's shape is. Several of them, so a caller can tell the newest from the
/// rest.
pub const CHANNEL_ENGINE_VERSION_COUNT: usize = 4;

/// How far apart two consecutive engine versions of that catalogue are released.
///
/// A CHOSEN span, not a documented one: nothing in the model says how often the
/// encoder engine is released. It is non-zero, which is what gives the catalogue a
/// strict newest-to-oldest order and so one newest version.
pub const CHANNEL_ENGINE_VERSION_INTERVAL: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How long after its release an engine version expires.
///
/// A CHOSEN span, not a documented one: `ExpirationDate` ("The UTC time when the
/// version expires") documents THAT a version expires, never how long it lasts. A
/// year, which is longer than the whole seeded catalogue spans
/// (`CHANNEL_ENGINE_VERSION_COUNT - 1` intervals), so every entry of a freshly
/// constructed account's catalogue is still available.
pub const CHANNEL_ENGINE_VERSION_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The release series every seeded version belongs to — the leading component of
/// its build identifier.
const CHANNEL_ENGINE_VERSION_SERIES: u32 = 1;

/// The build identifier of the `release`'th engine version of the seeded series,
/// counting from its oldest (`0`).
///
/// `ChannelEngineVersionResponse$Version` is "The build identifier for this
/// version of the channel version" — the model states that a version is named by a
/// build identifier and nothing at all about that identifier's form, so the form
/// is this emulator's own: a dotted release label whose middle component counts up
/// with each release of the series, which is also the form this crate's channel
/// suites pin on a channel.
pub fn channel_engine_version_label(release: usize) -> String {
    format!("{CHANNEL_ENGINE_VERSION_SERIES}.{release}.0")
}

/// One engine version of the account's catalogue: the build identifier it is named
/// by, when it was released and when it expires.
///
/// Both instants are stored, so the `ExpirationDate` a caller reads is the one the
/// catalogue was built with rather than a value recomputed per request — two reads
/// of one version report the same instant. Nothing on a request path writes a
/// record of this kind.
#[derive(Debug, Clone)]
pub struct ChannelEngineVersionRecord {
    /// The build identifier this version is named by, and this catalogue's key.
    pub version: String,
    /// When the version was released — the instant that orders the catalogue, so
    /// which version is the newest is a fact of the store.
    pub released_at: SystemTime,
    /// When it expires, as `ExpirationDate` reports it.
    pub expires_at: SystemTime,
}

impl ChannelEngineVersionRecord {
    /// The catalogue entry for the version `version` released at `released_at`: it
    /// expires a [`CHANNEL_ENGINE_VERSION_LIFETIME`] later.
    ///
    /// `checked_add` rather than `+`, so an instant near the end of the
    /// representable range cannot panic; it falls back to the release instant,
    /// which simply makes the entry already expired.
    pub fn released(version: String, released_at: SystemTime) -> Self {
        Self {
            version,
            released_at,
            expires_at: released_at
                .checked_add(CHANNEL_ENGINE_VERSION_LIFETIME)
                .unwrap_or(released_at),
        }
    }

    /// The catalogue entry for a version released at `released_at` that expires at
    /// `expires_at` — the one constructor that takes the expiry rather than
    /// deriving it, for a catalogue whose entries do not all last the seeded
    /// lifetime.
    pub fn expiring(version: String, released_at: SystemTime, expires_at: SystemTime) -> Self {
        Self {
            version,
            released_at,
            expires_at,
        }
    }

    /// Whether this version is still one of the engine versions "available in this
    /// AWS account" at `now` — an expired one is past the UTC time it expires.
    pub fn is_available_at(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }

    /// The modelled `ChannelEngineVersionResponse` for this entry: its stored build
    /// identifier and its stored expiry.
    pub fn view(&self) -> ChannelEngineVersionResponse {
        ChannelEngineVersionResponse {
            expiration_date: Some(::aws_smithy_types::DateTime::from(self.expires_at)),
            version: Some(self.version.clone()),
        }
    }
}

/// The account's engine version catalogue, keyed by build identifier.
///
/// A type of its own rather than a bare map, so the SEEDING is part of
/// constructing the store: the state struct derives `Default`, and this
/// [`Default`] is what puts the account's versions there without any request path
/// — reporting ones included — having to write them.
#[derive(Debug)]
pub struct ChannelEngineVersionCatalog {
    /// Every version the catalogue holds, expired ones included, keyed by its
    /// build identifier, which is unique because consecutive releases are a
    /// [`CHANNEL_ENGINE_VERSION_INTERVAL`] apart and labelled in release order.
    pub versions: BTreeMap<String, ChannelEngineVersionRecord>,
}

impl Default for ChannelEngineVersionCatalog {
    /// The catalogue of an account whose store has just come up, seeded as of now.
    fn default() -> Self {
        Self::seeded_at(SystemTime::now())
    }
}

impl ChannelEngineVersionCatalog {
    /// The seeded catalogue as of `now`: [`CHANNEL_ENGINE_VERSION_COUNT`] versions,
    /// the newest released at `now` and each earlier one a
    /// [`CHANNEL_ENGINE_VERSION_INTERVAL`] further back, so every entry is inside
    /// its lifetime and none is dated in the future.
    ///
    /// `checked_sub` rather than `-`, so an instant near the start of the
    /// representable range cannot panic — it simply yields a shorter catalogue.
    pub fn seeded_at(now: SystemTime) -> Self {
        let versions = (0..CHANNEL_ENGINE_VERSION_COUNT)
            .filter_map(|release| {
                let generations = CHANNEL_ENGINE_VERSION_COUNT - 1 - release;
                let age =
                    CHANNEL_ENGINE_VERSION_INTERVAL.checked_mul(u32::try_from(generations).ok()?)?;
                let record = ChannelEngineVersionRecord::released(
                    channel_engine_version_label(release),
                    now.checked_sub(age)?,
                );
                Some((record.version.clone(), record))
            })
            .collect();
        Self { versions }
    }

    /// A catalogue holding exactly `records` — the constructor a caller that knows
    /// the versions an account is offered uses instead of the seeded set.
    pub fn of(records: impl IntoIterator<Item = ChannelEngineVersionRecord>) -> Self {
        Self {
            versions: records
                .into_iter()
                .map(|record| (record.version.clone(), record))
                .collect(),
        }
    }

    /// The `ChannelEngineVersionResponse` for the build identifier `version`,
    /// as a reader that holds only the identifier (a channel's pin) reports it.
    ///
    /// `ExpirationDate` is "The UTC time when the version expires", so the
    /// instant to report for an identifier is the one THIS catalogue stores for
    /// it — the same stored instant [`Self::available_at`] reports for that
    /// identifier, so two readers of one record cannot disagree. An identifier
    /// the catalogue does not hold (one no release of this account's series is
    /// named by) has no stored expiry, and then only the identifier is reported.
    /// An entry past its expiry is still reported with that expiry: when it
    /// expires is a fact of the record, which availability filtering does not
    /// unmake.
    pub fn described(&self, version: &str) -> ChannelEngineVersionResponse {
        self.versions.get(version).map_or_else(
            || ChannelEngineVersionResponse {
                expiration_date: None,
                version: Some(version.to_string()),
            },
            ChannelEngineVersionRecord::view,
        )
    }

    /// The engine versions available to the account at `now`, newest release first,
    /// as the modelled shape reports them.
    ///
    /// An entry past its expiry is left out — it is no longer available — and
    /// nothing here writes, so two reads of one catalogue report the same versions.
    /// The order is by release instant, newest first (the build identifier breaking
    /// a tie, so the order is total), which puts the newest engine a caller can pin
    /// at the head of the list.
    pub fn available_at(&self, now: SystemTime) -> Vec<ChannelEngineVersionResponse> {
        let mut available: Vec<&ChannelEngineVersionRecord> = self
            .versions
            .values()
            .filter(|record| record.is_available_at(now))
            .collect();
        available.sort_by(|left, right| {
            right
                .released_at
                .cmp(&left.released_at)
                .then_with(|| right.version.cmp(&left.version))
        });
        available
            .into_iter()
            .map(ChannelEngineVersionRecord::view)
            .collect()
    }
}
