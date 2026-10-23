// SPDX-License-Identifier: CC0-1.0

//! The CloudTrail Lake **channel** registry and the rules every channel operation
//! shares.
//!
//! A channel identifier is a channel ARN or the bare id CloudTrail minted for it
//! (`DeleteChannelRequest$Channel` / `GetChannelRequest$Channel`: "The ARN or the UUID
//! value of the channel"), and the ARN carries the account that owns the channel and the
//! Region it lives in. Reading those two forms, and the wording a channel operation
//! refuses a malformed identifier and an absent channel with, live here — beside the
//! registry they resolve against — so `CreateChannel`, `GetChannel`, `UpdateChannel`,
//! `ListChannels` and `DeleteChannel` cannot drift from one another.
//!
//! The registry itself is a field of [`super::ComAmazonawsCloudtrailCloudtrail20131101State`],
//! the one store this service keeps; this module is where its rules are written, the way
//! `src/service/rpc_target.rs` holds the target reading `src/service.rs` applies.
//!
//! The rules a channel is CREATED under live here too, beside the registry they write
//! into: the one `Source` this emulator can ingest, how a `Destinations` entry is
//! resolved to an event data store the channel may log to, the registry's own bound, and
//! the name and source a Region holds at most one channel for.

use super::{ComAmazonawsCloudtrailCloudtrail20131101State, EventDataStoreRecord, SERVICE_NAMESPACE};
use crate::ops::types::{Destination, Tag};

/// The resource prefix of a channel ARN:
/// `arn:aws:cloudtrail:<region>:<account>:channel/<uuid>`. The format is the one the
/// model doc for `CloudTrailARNInvalidException` publishes: "The following is the
/// format of a channel ARN: arn:aws:cloudtrail:us-east-2:123456789012:channel/01234567890".
const CHANNEL_RESOURCE_PREFIX: &str = "channel/";

/// The resource TYPE of that ARN — [`CHANNEL_RESOURCE_PREFIX`] without its separator —
/// which is what `::service_common::arn` reads the resource field by.
const CHANNEL_RESOURCE_TYPE: &str = "channel";

/// The partition every ARN this emulator mints carries: `::engine::emulator_arn` builds
/// `arn:aws:<service>:<region>:<account>:<resource>`, so an ARN built in another
/// partition names a resource no call here can reference.
const AWS_PARTITION: &str = "aws";

/// Whether `value` is written the way this service's model writes a `UUID`
/// (`com.amazonaws.cloudtrail#UUID`: `@length 36`, `@pattern ^[a-f0-9\-]+$`) — five
/// lowercase-hex groups of 8-4-4-4-12 digits, the canonical form AWS mints a channel
/// id and an event data store id in.
///
/// It lives in this module because the channel path is what reads it: a minted channel
/// id, and the `<uuid>` of a `Destination$Location`. (`widget_query::is_event_data_store_id`
/// and `ops::list_imports::is_import_id` read the same form for their own ids; folding the
/// three into one reader belongs with whichever of those operations is worked next.)
fn is_uuid(value: &str) -> bool {
    /// The 8-4-4-4-12 group lengths that make up the 36 characters of a `UUID`.
    const GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

    let mut groups = value.split('-');
    let grouped_as_a_uuid = GROUP_LENGTHS.iter().all(|length| {
        groups.next().is_some_and(|group| {
            group.len() == *length
                && group
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b.is_ascii_lowercase() && b.is_ascii_hexdigit()))
        })
    });
    grouped_as_a_uuid && groups.next().is_none()
}

/// A fresh channel id in the modelled `UUID` form — "the `UUID` value of the channel"
/// (`GetChannelRequest$Channel`), written as a version-4 UUID (the `4` and the `8` in the
/// third and fourth groups).
///
/// Minted off a process-local counter, so it carries NOTHING from the request and no two
/// channels this process creates share an id — the low group alone is the counter, which
/// makes the whole value unique, and the registry is this process's. The service's other
/// minted ids are counted off the store the same way (`refreshes_started`), which is why
/// this needs no randomness and no dependency of its own.
pub fn new_channel_id() -> String {
    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let minted = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (minted >> 32) as u32,
        ((minted >> 16) & 0xffff) as u16,
        (minted >> 4) & 0x0fff,
        minted & 0x0fff,
        minted & 0xffff_ffff_ffff,
    )
}

/// The HTTP status a channel operation answers `ChannelNotFoundException` with.
///
/// The model annotates the shape `@httpError(404)`, and CloudTrail answers **400** for it
/// on the wire: every recorded channel refusal in the corpus carries HTTP 400 with
/// `__type: ChannelNotFoundException` (`DeleteChannel`, `GetChannel`, `UpdateChannel`,
/// `ListTags`, `RemoveTags` on an unknown channel). It is stated here, beside
/// [`channel_not_found_message`], so every channel operation answers the same status — the
/// error CODE is still the model's, and the status is applied through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`].
pub const CHANNEL_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// The HTTP status a channel operation answers `EventDataStoreNotFoundException` with
/// for a destination that names no store.
///
/// The model annotates the shape `@httpError(404)`, and the recorded CloudTrail response
/// carries HTTP **400** with `__type: EventDataStoreNotFoundException` (recorded:
/// `CreateChannel` against a well-formed event data store ARN that names no store). As
/// with [`super::IMPORT_NOT_FOUND_HTTP_STATUS`], the status a client's retry classifier
/// sees is the service's, not the model's, and a handler answers it through
/// [`crate::scaffolded::Scaffolded::modeled_with_status`], which keeps the modelled
/// shape's body and changes only the status line.
pub const EVENT_DATA_STORE_NOT_FOUND_HTTP_STATUS: u16 = 400;

/// Read a `Destination$Location`: `arn:aws:cloudtrail:<region>:<account>:eventdatastore/<uuid>`,
/// answering the Region and the store id it carries — or `None` when `value` is not one.
///
/// The six fields come from the tree's ONE ARN parse (`service_common::arn`), and this
/// reader adds what a `Destination$Location` needs on top of it: the Region and account
/// fields that parse allows to be EMPTY (a partition-global ARN) must be a Region label
/// and a 12-digit account, because an event data store is a Regional, account-owned
/// resource, and the resource must be an `eventdatastore` carrying an id in the modelled
/// `UUID` form. Stricter than [`super::parse_event_data_store_identifier`], which also
/// accepts the bare id the query operations' members document — a `Destination$Location`
/// is documented as an ARN, so only the ARN form is read here; expressing this as a
/// narrowing of that one grammar belongs with the store operations that own it.
fn parse_destination_event_data_store_arn(value: &str) -> Option<(&str, &str)> {
    let arn = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
    let region = ::service_common::arn::region_of(value, SERVICE_NAMESPACE)?;
    if arn.account.len() != 12 || !arn.account.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = ::service_common::arn::resource_id_of_type(
        value,
        super::EVENT_DATA_STORE_RESOURCE_PREFIX.trim_end_matches('/'),
    )?;
    if !is_uuid(id) {
        return None;
    }
    Some((region, id))
}

/// The registry key: a channel id and the Region the channel lives in.
///
/// A channel ARN carries a Region, so a channel is a regional resource and its id
/// alone is not a key. The id comes FIRST so the `BTreeMap` iterates id-ordered,
/// which is the order `ListChannels` pages through.
pub type ChannelKey = (String, String);

/// One persisted CloudTrail Lake channel: what the create request was given, plus
/// the values the service mints (the channel id and the ARN built from it).
#[derive(Debug, Clone, Default)]
pub struct ChannelRecord {
    /// The UUID CloudTrail minted for this channel — the registry's key, and the
    /// `<uuid>` of its ARN.
    pub channel_id: String,
    pub name: String,
    pub channel_arn: String,
    pub source: String,
    pub destinations: Vec<Destination>,
    pub tags: Vec<Tag>,
    pub home_region: String,
}

impl ChannelRecord {
    /// A channel `CreateChannel` just accepted: a freshly minted id, the ARN that id
    /// makes in `home_region`, and the values the request carried.
    pub fn created(
        home_region: &str,
        name: &str,
        source: &str,
        destinations: Vec<Destination>,
        tags: Vec<Tag>,
    ) -> Self {
        let channel_id = new_channel_id();
        Self {
            channel_arn: channel_arn(home_region, &channel_id),
            channel_id,
            name: name.to_string(),
            source: source.to_string(),
            destinations,
            tags,
            home_region: home_region.to_string(),
        }
    }

    /// This channel's registry key: its id and the Region it lives in.
    pub fn key(&self) -> ChannelKey {
        (self.channel_id.clone(), self.home_region.clone())
    }
}

/// The `Source` value `CreateChannel` documents for events that are not an Amazon
/// Web Services service's: "A source can be either `Custom` for all valid
/// non-Amazon Web Services events, or the name of a partner event source."
pub const CUSTOM_CHANNEL_SOURCE: &str = "Custom";

/// `Destination$Type` for a CloudTrail Lake integration channel, from the model's
/// `DestinationType` enum: "For channels used for a CloudTrail Lake integration, the
/// value is `EVENT_DATA_STORE`."
pub const DESTINATION_TYPE_EVENT_DATA_STORE: &str = "EVENT_DATA_STORE";

/// `Destination$Type` for a service-linked channel, from the same enum: "For
/// service-linked channels, the value is `AWS_SERVICE`."
pub const DESTINATION_TYPE_AWS_SERVICE: &str = "AWS_SERVICE";

/// How many channels the registry holds before `CreateChannel` refuses with the
/// modelled `ChannelMaxLimitExceededException`.
///
/// Like [`super::TRAIL_QUOTA`], this is the emulator's OWN bound on a store that
/// lives for the life of the process rather than a claim about any account's
/// published channel quota, and it bounds the WHOLE registry rather than one
/// Region's share of it: the Region a channel is created in comes from the request,
/// so a per-Region bound would still let a caller grow the store without end by
/// varying the Region it signs for. It sits far above any workflow an emulator
/// serves, and a record is released by `DeleteChannel`.
pub const CHANNEL_QUOTA: usize = 1_000;

/// Check `CreateChannel$Source` / the source of a channel.
///
/// `Source` is documented as "either `Custom` for all valid non-Amazon Web Services
/// events, or the name of a partner event source … For information about the source
/// names for available partners, see Additional information about integration
/// partners". A partner source name therefore names a partner integration that must
/// be AVAILABLE, and this emulator carries no partner integrations — so
/// [`CUSTOM_CHANNEL_SOURCE`] is the one source a channel can be created for, and
/// every other value is refused the way the service refuses a source it does not
/// have: `InvalidSourceException`. A partner-integration registry, if one ever
/// lands, is what this rule would check a partner name against; until then,
/// accepting an arbitrary name would promise a channel that can ingest nothing.
pub fn validate_channel_source(source: &str) -> Result<(), String> {
    if source == CUSTOM_CHANNEL_SOURCE {
        Ok(())
    } else {
        // Request-derived: it names the value it refused, so the caller can fix the
        // request from it alone.
        Err(format!("The source {source} is not valid."))
    }
}

/// Why a channel destination could not be accepted.
///
/// Each variant names the modelled error the handler answers with, so every
/// operation that takes a `Destinations` list refuses the same destination the same
/// way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationRejection {
    /// Not an event data store ARN at all ⇒ `EventDataStoreARNInvalidException`
    /// (HTTP 400).
    ArnInvalid(String),
    /// A well-formed event data store ARN naming no store the caller's Region can
    /// log to ⇒ `EventDataStoreNotFoundException`, answered with
    /// [`EVENT_DATA_STORE_NOT_FOUND_HTTP_STATUS`].
    NotFound(String),
    /// The store exists but is not usable ⇒ `InactiveEventDataStoreException`
    /// (HTTP 400).
    Inactive(String),
    /// A destination this operation cannot create ⇒ `InvalidParameterException`
    /// (HTTP 400).
    InvalidParameter(String),
}

/// A caller-supplied channel identifier, in either of the two forms the model
/// documents for `DeleteChannelRequest$Channel` / `GetChannelRequest$Channel` ("The
/// ARN or the UUID value of the channel").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelIdentifier {
    /// A bare channel id, which names a channel of the caller's own account in the
    /// caller's own Region.
    Id(String),
    /// A channel ARN, which also names the account that owns the channel and the
    /// Region it lives in.
    Arn {
        account: String,
        region: String,
        id: String,
    },
}

impl ChannelIdentifier {
    /// The channel id this identifier names — the registry's key.
    pub fn id(&self) -> &str {
        match self {
            Self::Id(id) => id,
            Self::Arn { id, .. } => id,
        }
    }

    /// The Region this identifier pins the channel to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Id(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }

    /// The account this identifier pins the channel to, if it pins one.
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::Id(_) => None,
            Self::Arn { account, .. } => Some(account),
        }
    }
}

/// What a well-formed channel identifier addresses.
///
/// The registry holds the channels of the emulator's own account
/// ([`::engine::emulator_account_id`]), so an identifier naming another account is
/// well formed and still names no channel — which is a different refusal from one
/// that is not a channel identifier at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelAddress {
    /// The registry key the identifier addresses.
    Key(ChannelKey),
    /// A channel of another account, which this registry never holds.
    AnotherAccount,
}

/// The ARN of the channel whose id is `channel_id`, in `region`.
pub fn channel_arn(region: &str, channel_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{CHANNEL_RESOURCE_PREFIX}{channel_id}"),
    )
}

/// Whether `value` has the shape of an id CloudTrail minted: the modelled `UUID`
/// (`com.amazonaws.cloudtrail#UUID`, `@length 36` of `^[a-f0-9\-]+$` in `8-4-4-4-12`
/// groups), which is the `<uuid>` of `DeleteChannelRequest$Channel` ("The ARN or the
/// UUID value of the channel"). It is the ONE reading of that form in this crate
/// ([`is_uuid`]), shared with the service's other `UUID`-shaped ids.
///
/// It is what a MINTED id looks like, not a rule a caller-supplied id is checked
/// against: an id CloudTrail never minted names no channel, which is an absent channel
/// and not a malformed identifier. Only the ARN FORM is checked, because only an ARN
/// has a format to get wrong.
///
/// Only a MINTER needs this (`CreateChannel`, to mint one, and a caller that wants to
/// know whether an id could be one): resolving an identifier does not, because an id of
/// any other shape simply names no channel.
pub fn is_minted_channel_id(value: &str) -> bool {
    is_uuid(value)
}

/// Read a caller-supplied channel identifier as either a channel ARN or a bare channel
/// id. `None` is a value that is neither, so the operation refuses it with
/// `ChannelARNInvalidException`.
///
/// A value is read as an ARN as soon as it starts with `arn:`, and then it must BE a
/// channel ARN, because an ARN is the only form that has a format to get wrong: the ARN
/// itself is read through `::service_common::arn`, the one ARN parse the services share
/// (the same one `super::parse_taggable_arn` reads a tagging ARN with), and this reader
/// adds what the shared parse leaves to the service — the partition this emulator mints
/// ([`AWS_PARTITION`]), a Region and an account to address a regional resource with, and
/// the `channel` resource type. Anything else is read as a bare channel id whatever its
/// syntax: an id CloudTrail never minted names no channel, which is an absent channel,
/// not a malformed identifier.
pub fn parse_channel_identifier(value: &str) -> Option<ChannelIdentifier> {
    if value.starts_with("arn:") {
        let arn = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
        if arn.partition != AWS_PARTITION || arn.region.is_empty() || arn.account.is_empty() {
            return None;
        }
        let id = ::service_common::arn::resource_id_of_type(value, CHANNEL_RESOURCE_TYPE)?;
        if id.is_empty() {
            return None;
        }
        return Some(ChannelIdentifier::Arn {
            account: arn.account.to_string(),
            region: arn.region.to_string(),
            id: id.to_string(),
        });
    }
    if value.is_empty() {
        return None;
    }
    Some(ChannelIdentifier::Id(value.to_string()))
}

/// The `ChannelARNInvalidException` text for an `arn:` value that is not a channel ARN.
///
/// Built from the request, so a caller can see which value was refused; every channel
/// operation words it the same way. The tagging path words its own channel-ARN refusal
/// differently (`super::TaggableArnRejection::ChannelArn`, "The channel ARN `<id>` is
/// invalid.") because that is what the `RemoveTags` recording shows, while this is what
/// the `GetChannel` recording shows; the two converge when a recording pins one wording
/// for both paths.
pub fn channel_arn_invalid_message(identifier: &str) -> String {
    format!("The format of the channel ARN {identifier} is not valid.")
}

/// The `ChannelNotFoundException` text for a well-formed identifier that names no
/// stored channel. Built from the request, like [`channel_arn_invalid_message`], and the
/// same sentence `super::TaggableArn::not_found_message` answers a tagging call with.
///
/// `subject` is the channel ARN the identifier ADDRESSED, which is what the recorded
/// refusals name: a call that passes a bare channel id is answered
/// "The channel `arn:aws:cloudtrail:<region>:<account>:channel/<id>` was not found.",
/// not with the id as typed — so a caller-supplied identifier is completed through
/// [`addressed_channel_arn`] first. (`super::event_data_store_not_found_message` names
/// the addressed ARN for the same reason.)
pub fn channel_not_found_message(subject: &str) -> String {
    format!("The channel {subject} was not found.")
}

/// The channel ARN a caller-supplied identifier addresses: the ARN as given, or — for
/// a bare channel id — the ARN that id names in `request_region`, in this account.
///
/// An identifier that is no channel identifier at all is named as given: it addresses
/// nothing, and it is refused by its form ([`channel_arn_invalid_message`]) rather than
/// as an absent channel.
pub fn addressed_channel_arn(identifier: &str, request_region: &str) -> String {
    match parse_channel_identifier(identifier) {
        Some(ChannelIdentifier::Id(id)) => channel_arn(request_region, &id),
        Some(ChannelIdentifier::Arn { .. }) | None => identifier.to_string(),
    }
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// What a caller-supplied channel identifier addresses: the channel of that id in
    /// the Region the identifier pins — the ARN's own Region, or, for a bare id, the
    /// Region the call was made in — of the account the ARN names.
    ///
    /// `None` is an identifier that is not a channel identifier at all.
    pub fn addressed_channel(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Option<ChannelAddress> {
        let parsed = parse_channel_identifier(identifier)?;
        if parsed
            .account()
            .is_some_and(|account| account != ::engine::emulator_account_id())
        {
            return Some(ChannelAddress::AnotherAccount);
        }
        Some(ChannelAddress::Key((
            parsed.id().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        )))
    }

    /// The channel called `name` in `region`, if the account holds one.
    ///
    /// A channel name is taken within the Region it was created in, which is what
    /// makes `ChannelAlreadyExistsException` reachable: the registry is keyed by
    /// `(channel id, Region)`, so the name is not a key and is checked here.
    pub fn channel_named(&self, region: &str, name: &str) -> Option<&ChannelRecord> {
        self.channels
            .values()
            .find(|channel| channel.home_region == region && channel.name == name)
    }

    /// The channel in `region` that already logs `source`, if the account holds one.
    ///
    /// `CreateChannelRequest$Source` documents the rule: "A maximum of one channel
    /// is allowed per source." A channel is a Regional resource, so the bound is per
    /// Region, like the name.
    pub fn channel_sourced_from(&self, region: &str, source: &str) -> Option<&ChannelRecord> {
        self.channels
            .values()
            .find(|channel| channel.home_region == region && channel.source == source)
    }

    /// How many channels `region` holds — the registry spans every Region, so one
    /// Region's share of it is a filtered count.
    pub fn channels_in(&self, region: &str) -> usize {
        self.channels
            .values()
            .filter(|channel| channel.home_region == region)
            .count()
    }

    /// Check one `Destination` of a channel, for a call made in `request_region`.
    ///
    /// A channel created through `CreateChannel` logs to event data stores, so a
    /// destination must be typed `EVENT_DATA_STORE` and its `Location` must be the
    /// ARN of a store that exists in the SAME Region as the channel — a store lives
    /// in one Region and a channel can only log to one it shares a Region with — and
    /// be usable. The refusals are distinct: not an event data store ARN at all, a
    /// well-formed ARN naming no store this Region can log to, and a store that is
    /// inactive.
    pub fn resolve_channel_destination(
        &self,
        destination: &Destination,
        request_region: &str,
    ) -> Result<&EventDataStoreRecord, DestinationRejection> {
        let location = destination.location.as_str();
        if destination.r#type != DESTINATION_TYPE_EVENT_DATA_STORE {
            return Err(DestinationRejection::InvalidParameter(format!(
                "Destination type '{}' is not valid for a channel that logs to an event \
                 data store: the type must be {DESTINATION_TYPE_EVENT_DATA_STORE} \
                 ({DESTINATION_TYPE_AWS_SERVICE} is reserved for the service-linked \
                 channels Amazon Web Services services create)",
                destination.r#type
            )));
        }
        // A `Location` is documented as "the ARN of an event data store", so only
        // the ARN form is read here; the refusal wording is the store module's
        // shared one, which every operation that reads a store identifier answers
        // with.
        let Some((arn_region, id)) = parse_destination_event_data_store_arn(location) else {
            return Err(DestinationRejection::ArnInvalid(
                super::event_data_store_arn_invalid_message(location, request_region),
            ));
        };
        // The store has to be the one this ARN names AND be in the Region the call
        // was made in; a store of another Region names no store this channel can
        // log to.
        let store = self
            .event_data_stores
            .get(id)
            .filter(|store| store.region == arn_region && store.region == request_region);
        let Some(store) = store else {
            return Err(DestinationRejection::NotFound(
                super::event_data_store_not_found_message(location),
            ));
        };
        // A store awaiting deletion is the one status the modelled
        // `InactiveEventDataStoreException` reports, for ingestion as for queries.
        if !store.status.is_queryable() {
            return Err(DestinationRejection::Inactive(format!(
                "The event data store {location} is inactive."
            )));
        }
        Ok(store)
    }
}

