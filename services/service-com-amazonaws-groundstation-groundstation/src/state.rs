// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-groundstation-groundstation service.
//!
//! Every resource this service hands out lives here: configs keyed by their
//! `(configType, configId)` pair (the two labels `GET /config/{configType}/{configId}`
//! carries), mission profiles and contacts keyed by their uuid, plus one ARN index so a
//! `trackingConfigArn` / `missionProfileArn` reference resolves without scanning a map.
//!
//! The helpers beside the struct are the pieces every handler of this service needs —
//! the request's region, the union-member-to-`ConfigCapabilityType` mapping, the
//! capability's user-facing spelling, the `ConfigArn` reader, uuid minting and the
//! opaque pagination token codec — so they are written once here rather than in each
//! `src/ops/<operation>.rs`.
//!
//! Error TEXT, on the other hand, is per operation: this service answers a
//! `ResourceNotFoundException` with the text of the operation that raised it, so each
//! `src/ops/<operation>.rs` carries its own (see [`MISSION_PROFILE_NOT_FOUND`] for the
//! one literal two operations genuinely share).

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{ConfigTypeData, ContactVersion, KmsKey, TrackingOverrides};

/// Region used in minted ARNs when the request carries no usable SigV4 credential
/// scope (an unsigned request still routes by endpoint host, so this is reachable).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, per `OPERATIONS.md`.
pub const ARN_SERVICE: &str = "groundstation";

/// `ReserveContact` admits a contact in `SCHEDULING`; the scheduler moves it to
/// `SCHEDULED` on its own. CHOSEN span (AWS documents no duration for it), inside the
/// 250–750 ms band this project gives a clock-owed transition, so a caller that reads
/// the contact straight after reserving it still observes the transient status.
pub const SCHEDULING_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// Which `ConfigCapabilityType` a `ConfigTypeData` union member selects.
///
/// The config's type is NOT a request field: it is implied by whichever member of the
/// union the caller set, and it is what both the `configArn`'s `config/<type>/<id>`
/// segment and the `{configType}` label of `GetConfig` carry. The eight pairs are the
/// model's own: `smithy_query(query="shape", shape="ConfigCapabilityType")` lists the
/// enum values and `ConfigTypeData` the members.
pub fn config_type_of(config_data: &ConfigTypeData) -> &'static str {
    match config_data {
        ConfigTypeData::antennaDownlinkConfig(_) => "antenna-downlink",
        ConfigTypeData::antennaDownlinkDemodDecodeConfig(_) => "antenna-downlink-demod-decode",
        ConfigTypeData::antennaUplinkConfig(_) => "antenna-uplink",
        ConfigTypeData::dataflowEndpointConfig(_) => "dataflow-endpoint",
        ConfigTypeData::s3RecordingConfig(_) => "s3-recording",
        ConfigTypeData::telemetrySinkConfig(_) => "telemetry-sink",
        ConfigTypeData::trackingConfig(_) => "tracking",
        ConfigTypeData::uplinkEchoConfig(_) => "uplink-echo",
    }
}

/// The capability's name as this service spells it in user-facing text: the
/// UpperCamelCase form of a `ConfigCapabilityType` token.
///
/// That form is not a second naming scheme — it is exactly the name of the shape the
/// matching `ConfigTypeData` member targets, minus its `Config` suffix:
/// `antenna-uplink` -> `AntennaUplinkConfig` -> `AntennaUplink`, `s3-recording` ->
/// `S3RecordingConfig` -> `S3Recording`, `antenna-downlink-demod-decode` ->
/// `AntennaDownlinkDemodDecodeConfig` -> `AntennaDownlinkDemodDecode`. Source:
/// `smithy_query(query="shape", shape="ConfigTypeData,ConfigCapabilityType")` lists the
/// eight member targets beside the eight tokens, and the two agree segment for segment —
/// so the spelling is DERIVED from the token rather than tabulated here, and holds for
/// every capability type including any the model gains later.
pub fn capability_display_name(config_type: &str) -> String {
    config_type
        .split('-')
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// The `(configType, configId)` pair a `ConfigArn` addresses, or `None` when `arn` is
/// not one of this service's config ARNs.
///
/// The modelled `ConfigArn` pattern ends in `config/<type>/<uuid>(/<suffix>)?` (it is on
/// `configArn` in `smithy_query(query="operation", operation="GetConfig")`), so the two
/// segments after `:config/` are the very pair [`ComAmazonawsGroundstationGroundstationState::configs`]
/// is keyed on — which is what lets an ARN-valued reference be reported by the id and
/// type it names rather than echoed back whole.
pub fn config_arn_parts(arn: &str) -> Option<(String, String)> {
    let (_, resource) = arn.split_once(":config/")?;
    let mut segments = resource.split('/');
    let config_type = segments.next()?;
    let config_id = segments.next()?;
    (!config_type.is_empty() && !config_id.is_empty())
        .then(|| (config_type.to_string(), config_id.to_string()))
}

/// What this service answers when a mission profile a request names does not exist: a
/// fixed literal that discloses neither the id nor the ARN that was asked for.
///
/// It lives here rather than in one handler because BOTH operations that resolve a
/// mission profile answer it — `GetMissionProfile`, which resolves one by
/// `{missionProfileId}`, and `ReserveContact`, which resolves one by `missionProfileArn`
/// — so it is one text of the service, not one text per operation.
pub const MISSION_PROFILE_NOT_FOUND: &str = "MissionProfile not found";

/// The region to mint this request's ARNs in: the SigV4 credential scope's region,
/// which is the region the caller addressed, falling back to [`DEFAULT_REGION`].
///
/// A scope region that could not appear in an ARN (the model's `ConfigArn` pattern
/// allows `[-a-z0-9]{1,50}` there) falls back too, so a minted ARN always satisfies the
/// modelled pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is spellable in the region segment of this service's ARNs
/// (`[-a-z0-9]{1,50}`, from the `ConfigArn`/`MissionProfileArn` patterns).
fn is_arn_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 50
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// How many ids this process has minted: the part of an id that makes it UNIQUE.
///
/// A counter cannot repeat within a process, so no two ids this service hands out can
/// collide however fast they are minted — which a clock reading alone cannot promise.
static IDS_MINTED: AtomicU64 = AtomicU64::new(0);

/// A per-process random value, the part of an id that makes it UNGUESSABLE.
///
/// `RandomState` seeds each of its hashers with keys the standard library draws from the
/// OS once per process, so hashing a fixed value through one yields a value that differs
/// between runs and is not derivable from the request.
fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut hasher = ::std::hash::BuildHasher::build_hasher(&RandomState::new());
        ::std::hash::Hash::hash(&"com.amazonaws.groundstation", &mut hasher);
        ::std::hash::Hasher::finish(&hasher)
    })
}

/// A fresh lowercase, hyphenated uuid — the `Uuid` shape every id member of this
/// service targets (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`),
/// in the version-4 layout (version nibble `4`, variant bits `10`).
///
/// The 16 bytes are a SHA-256 digest — from the maintained `sha2` crate, not written
/// here — of this process's random seed, the number of ids minted before this one and
/// the current clock reading, rendered by `hex`. The counter is what makes an id unique
/// and the seed is what makes it unguessable; digesting them is what spreads both across
/// all 16 bytes, so an id reveals neither.
///
/// It does NOT use the `uuid` crate, which would otherwise be the obvious choice: taking
/// a new dependency rewrites the workspace `Cargo.lock`, the one file every concurrently
/// landing change in this repository also rewrites, and this change has to merge beside
/// them. `sha2` and `hex` are already in this crate's dependency closure, so minting the
/// id from them leaves the lockfile untouched.
pub fn new_uuid() -> String {
    use ::sha2::Digest as _;

    let minted = IDS_MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed().to_be_bytes());
    digest.update(minted.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    let mut bytes = <[u8; 16]>::try_from(&digest.finalize()[..16]).expect("16 of 32 digest bytes");

    // The version-4 layout: `4` in the high nibble of byte 6, `10` in the top two bits
    // of byte 8. Everything else stays as digested.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `now` as a Smithy timestamp.
pub fn now_timestamp() -> DateTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    DateTime::from_secs_f64(secs)
}

/// `timestamp`, shifted by `seconds` (negative shifts earlier).
pub fn shift_seconds(timestamp: &DateTime, seconds: i64) -> DateTime {
    DateTime::from_secs_f64(timestamp.as_secs_f64() + seconds as f64)
}

/// The scope a listing's pagination tokens are issued under: the listing's own name,
/// plus a fingerprint of the filters the offset counts WITHIN when it has any.
///
/// An offset is only meaningful against the sequence it was taken from, so a token
/// carries its scope: a token from another listing, or from the same listing under
/// different filters, is then refused rather than silently resuming at an offset into a
/// different sequence (skipping or repeating records). The fingerprint is a
/// non-cryptographic hash — it only has to change when the filters do.
///
/// `DefaultHasher`'s output is NOT stable across process runs or std versions, which is
/// fine and deliberate: a token only ever addresses an offset into this process's
/// in-memory store, so it is not meant to survive a restart (after one there is no
/// sequence left to resume), and a token from an earlier run is refused rather than
/// resumed.
pub fn page_scope(listing: &str, filters: &[String]) -> String {
    if filters.is_empty() {
        return listing.to_string();
    }
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    // Hashed as ONE string joined on US (0x1f), a byte none of this service's filter
    // values can contain (every one is an ARN, a ground-station name, an enum value or a
    // timestamp), so no two different filter sets can run together into the same input.
    ::std::hash::Hash::hash(&filters.join("\u{1f}"), &mut hasher);
    format!("{listing}.{:016x}", ::std::hash::Hasher::finish(&hasher))
}

/// A pagination token for "resume `scope` at item `offset`".
///
/// Spelled `<scope>.<n>`: every character is inside the model's `PaginationToken`
/// pattern (`^[A-Za-z0-9-/+_.=]+$`) and needs no escaping in the query string
/// `ListMissionProfiles` binds its `nextToken` to, and the shortest form clears the
/// shape's 3-character minimum.
pub fn encode_page_token(scope: &str, offset: usize) -> String {
    format!("{scope}.{offset}")
}

/// The offset a token issued by [`encode_page_token`] for `scope` names, or `None` when
/// the token was not issued by this service for that scope.
pub fn decode_page_token(scope: &str, token: &str) -> Option<usize> {
    token.strip_prefix(scope)?.strip_prefix('.')?.parse().ok()
}

/// Largest page any listing returns, and the ceiling on a caller's `maxResults`: the
/// modelled maximum of `PaginationMaxResults` (`smithy_query(query="shape",
/// shape="PaginationMaxResults")` — range 1..100). A request that names no page size
/// gets this bounded page and a `nextToken`, rather than the whole store in one answer.
pub const MAX_PAGE_SIZE: usize = 100;

/// Largest number of records this emulator keeps in any one of its stores.
///
/// NOT an AWS quota — AWS Ground Station's own limits are per-account and adjustable.
/// It is a bound on the shared in-process store, so a caller cannot grow the emulator's
/// memory without limit; a create that would pass it is refused with the service's
/// modelled `ResourceLimitExceededException` (HTTP 429, "Account limits for this
/// resource have been exceeded"), which is what the service answers when a create runs
/// past an account limit.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// One page of `items`, plus the token that resumes after it.
///
/// The page is at most [`MAX_PAGE_SIZE`] items whatever the caller asked for, so no
/// single request can draw the whole store out in one answer.
pub fn paginate<T: Clone>(
    scope: &str,
    items: &[T],
    max_results: Option<i32>,
    offset: usize,
) -> (Vec<T>, Option<String>) {
    let start = offset.min(items.len());
    let limit = match max_results {
        Some(max) if max > 0 => (max as usize).min(MAX_PAGE_SIZE),
        _ => MAX_PAGE_SIZE,
    };
    let end = start.saturating_add(limit).min(items.len());
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then(|| encode_page_token(scope, end));
    (page, next)
}

/// What an ARN this service minted points at, so an ARN-valued reference resolves in
/// one lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A config, by the `(configType, configId)` pair its map is keyed on.
    Config {
        config_type: String,
        config_id: String,
    },
    /// A mission profile, by `missionProfileId`.
    MissionProfile { mission_profile_id: String },
    /// A contact, by `contactId`.
    Contact { contact_id: String },
}

/// A config as `CreateConfig` stored it.
#[derive(Debug, Clone)]
pub struct Config {
    pub config_id: String,
    pub config_arn: String,
    pub name: String,
    pub config_type: String,
    pub config_data: ConfigTypeData,
    pub tags: HashMap<String, String>,
}

/// A mission profile as `CreateMissionProfile` stored it.
#[derive(Debug, Clone)]
pub struct MissionProfile {
    pub mission_profile_id: String,
    pub mission_profile_arn: String,
    pub name: String,
    pub region: String,
    pub contact_pre_pass_duration_seconds: Option<i32>,
    pub contact_post_pass_duration_seconds: Option<i32>,
    pub minimum_viable_contact_duration_seconds: i32,
    pub dataflow_edges: Vec<Vec<String>>,
    pub tracking_config_arn: String,
    pub telemetry_sink_config_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub streams_kms_key: Option<KmsKey>,
    pub streams_kms_role: Option<String>,
}

/// A contact as `ReserveContact` stored it.
#[derive(Debug, Clone)]
pub struct Contact {
    pub contact_id: String,
    pub mission_profile_arn: String,
    pub satellite_arn: Option<String>,
    pub start_time: DateTime,
    pub end_time: DateTime,
    pub pre_pass_start_time: DateTime,
    pub post_pass_end_time: DateTime,
    pub ground_station: String,
    pub contact_status: String,
    pub region: String,
    pub version_id: i32,
    pub tracking_overrides: Option<TrackingOverrides>,
    pub tags: HashMap<String, String>,
    /// When the reservation was accepted.
    pub created: DateTime,
    /// When the contact last changed.
    pub last_updated: DateTime,
    /// The instant the scheduler owes this contact its `SCHEDULING` -> `SCHEDULED`
    /// move; `None` once the move has been materialised or the contact left
    /// `SCHEDULING`.
    pub scheduling_settles_at: Option<SystemTime>,
}

/// The status of the one version a reserved contact has: it is the current version of
/// the contact, and the model's `VersionStatus` enum documents `ACTIVE` as exactly that
/// ("The version is the current active version of the contact" —
/// `smithy_query(query="shape", shape="VersionStatus")`, whose values are `UPDATING`,
/// `ACTIVE`, `SUPERSEDED` and `FAILED_TO_UPDATE`). DERIVED, not recalled: this service
/// mints one version per contact, nothing is updating it and nothing supersedes it while
/// `UpdateContact` answers 501, which leaves `ACTIVE` as the only value that holds.
pub const ACTIVE_VERSION_STATUS: &str = "ACTIVE";

impl Contact {
    /// This contact's version record, as `DescribeContact` and `ListContacts` report
    /// it: the one version `ReserveContact` created, still the active one.
    pub fn version(&self) -> ContactVersion {
        ContactVersion {
            version_id: Some(self.version_id),
            created: Some(self.created),
            activated: None,
            superseded: None,
            last_updated: Some(self.last_updated),
            status: Some(ACTIVE_VERSION_STATUS.to_string()),
            failure_codes: None,
            failure_message: None,
        }
    }

    /// `tags` the way a response carries them: always the map, empty when the
    /// resource carries no tag.
    pub fn response_tags(&self) -> Option<HashMap<String, String>> {
        response_tags(&self.tags)
    }
}

/// `tags` the way a response of this service carries them: the resource's tag map,
/// SERIALIZED even when the resource carries no tag — `{}` then, never an omitted
/// member.
///
/// A `TagsMap`-targeting response member of this service reports how a resource is
/// tagged, and "no tags" is a map with no entry, not the absence of an answer: a client
/// that diffs or iterates tags reads a map in both cases. Every read that reports tags
/// goes through here, so the whole service answers the same way.
pub fn response_tags(tags: &HashMap<String, String>) -> Option<HashMap<String, String>> {
    Some(tags.clone())
}

/// The message every operation addressed at a config by its `(configType, configId)`
/// pair carries when that pair names no config — built from the two `@httpLabel`
/// members of the request, id before type, which is the rule the recorded corpus shows
/// for this service's config reads.
///
/// It lives here rather than in one handler because every operation that addresses a
/// config by that pair answers it — `GetConfig`, `DeleteConfig` and `UpdateConfig` —
/// so it is one text of the service, not one text per operation.
pub fn config_not_found_message(config_type: &str, config_id: &str) -> String {
    format!("No config found with id {config_id} and type {config_type}")
}

/// The message an operation that resolves a config BY ARN — a mission profile's
/// `trackingConfigArn`, `telemetrySinkConfigArn` or a `dataflowEdges` endpoint — carries
/// when that reference resolves to nothing.
///
/// It identifies the config the lookup looked FOR: its id, the config type it searched
/// under, and the account the search was scoped to, rather than echoing the ARN back.
/// The id and the fallback type come from the ARN the request gave
/// ([`config_arn_parts`]); `required_type` is the type the member itself requires, which
/// is `None` for a dataflow edge, whose endpoints may be of any type. An ARN that is not
/// one of this service's config ARNs names no id to report, so the ARN is all there is
/// to say.
///
/// Shared by every operation that takes such a reference, so a create and an update of
/// the same profile answer one text.
pub fn config_reference_not_found_message(arn: &str, required_type: Option<&str>) -> String {
    let from_arn = config_arn_parts(arn);
    let searched_type = required_type
        .map(str::to_string)
        .or_else(|| from_arn.as_ref().map(|(config_type, _)| config_type.clone()));
    match (&from_arn, &searched_type) {
        (Some((_, config_id)), Some(config_type)) => format!(
            "No config found with ID {config_id} and config type: {config_type} for customer {}",
            ::engine::emulator_account_id()
        ),
        _ => format!("Unable to find Config with ARN: {arn}"),
    }
}

/// Per-service state for com-amazonaws-groundstation-groundstation. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGroundstationGroundstationState {
    /// Configs, keyed by the `(configType, configId)` pair `GetConfig` addresses.
    pub configs: HashMap<(String, String), Config>,
    /// Mission profiles, keyed by `missionProfileId`.
    pub mission_profiles: HashMap<String, MissionProfile>,
    /// Contacts, keyed by `contactId`.
    pub contacts: HashMap<String, Contact>,
    /// Every ARN this service minted, and what it points at.
    pub arn_index: HashMap<String, ResourceRef>,
}

impl ComAmazonawsGroundstationGroundstationState {
    /// The config `arn` names, if it names one.
    pub fn config_by_arn(&self, arn: &str) -> Option<&Config> {
        match self.arn_index.get(arn)? {
            ResourceRef::Config {
                config_type,
                config_id,
            } => self.configs.get(&(config_type.clone(), config_id.clone())),
            _ => None,
        }
    }

    /// Whether `arn` resolves to a config this service still holds, of `required_type`
    /// when the member that carries the reference is for one config type only.
    ///
    /// Shared by every operation that takes a config reference, so a reference to a
    /// config that was never created and one to a config that has since been DELETED
    /// are refused the same way.
    pub fn resolves_config(&self, arn: &str, required_type: Option<&str>) -> bool {
        self.config_by_arn(arn)
            .is_some_and(|config| required_type.is_none_or(|wanted| config.config_type == wanted))
    }

    /// Remove the config the `(configType, configId)` pair addresses, along with the ARN
    /// index entry that resolved to it, and answer it; `None` when the pair addresses no
    /// config.
    ///
    /// Dropping the index entry is what makes the deletion complete: an ARN-valued
    /// reference to a deleted config then resolves to nothing, exactly as a reference to
    /// one that never existed does.
    pub fn remove_config(&mut self, config_type: &str, config_id: &str) -> Option<Config> {
        let removed = self
            .configs
            .remove(&(config_type.to_string(), config_id.to_string()))?;
        self.arn_index.remove(&removed.config_arn);
        Some(removed)
    }

    /// Remove the mission profile `mission_profile_id` addresses, along with its ARN
    /// index entry, and answer it; `None` when no profile has that id.
    pub fn remove_mission_profile(&mut self, mission_profile_id: &str) -> Option<MissionProfile> {
        let removed = self.mission_profiles.remove(mission_profile_id)?;
        self.arn_index.remove(&removed.mission_profile_arn);
        Some(removed)
    }

    /// The mission profile `arn` names, if it names one.
    pub fn mission_profile_by_arn(&self, arn: &str) -> Option<&MissionProfile> {
        match self.arn_index.get(arn)? {
            ResourceRef::MissionProfile { mission_profile_id } => {
                self.mission_profiles.get(mission_profile_id)
            }
            _ => None,
        }
    }

    /// Materialise every `SCHEDULING` -> `SCHEDULED` move the clock has already made
    /// due. A reporting operation calls this before reading: the move was owed at this
    /// instant whether or not anybody called, so applying it changes nothing a later
    /// caller could otherwise have seen.
    pub fn settle_due_contacts(&mut self) {
        let now = SystemTime::now();
        for contact in self.contacts.values_mut() {
            let Some(due) = contact.scheduling_settles_at else {
                continue;
            };
            if due <= now {
                if contact.contact_status == "SCHEDULING" {
                    contact.contact_status = "SCHEDULED".to_string();
                    contact.last_updated = now_timestamp();
                }
                contact.scheduling_settles_at = None;
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGroundstationGroundstationState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGroundstationGroundstationState>)
}
