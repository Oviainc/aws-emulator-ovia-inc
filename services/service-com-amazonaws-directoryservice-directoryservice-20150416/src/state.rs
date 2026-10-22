// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-directoryservice-directoryservice-20150416 service.
//!
//! This module owns the records the handlers in `src/ops/` share — the directories
//! `CreateDirectory` persists, the aliases `CreateAlias` assigns, the Amazon SNS topics
//! `RegisterEventTopic` associates and the tags `AddTagsToResource` merges — plus the
//! three pieces of behaviour that cannot live in any single operation file:
//!
//! * [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`], which
//!   materialises the stage transition the clock has already made due (a directory
//!   leaving `Requested`). Every handler calls it before reading or writing, so a
//!   reporting operation reports what the clock already owed and nothing more.
//! * [`Directory::to_description`], the one place a stored directory is rendered as the
//!   modelled `DirectoryDescription`, so every operation that reports a directory
//!   reports the same derived values (`Alias`, `AccessUrl`).
//! * [`encode_cursor`] / [`decode_cursor`], the opaque `NextToken` the two paginated
//!   operations of this slice (`DescribeDirectories`, `ListTagsForResource`) issue and
//!   resolve.
//!
//! The administrator `Password` `CreateDirectory` is called with is deliberately NOT a
//! field here: the model marks it `@sensitive`, no operation reports it back, and state
//! this emulator does not need is state it must not hold.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{DirectoryDescription, DirectoryVpcSettingsDescription, EventTopic, Tag};

/// How long the stage transition a create owes takes to fall due.
///
/// `CreateDirectory` answers while the directory is still being built — the modelled
/// `DirectoryStage` enum's first value is `Requested` — so the move to `Active` is owed
/// by the clock rather than by any later call. The project's emulator contract requires
/// such a transition to fall due 250–750 ms after the call that starts it, so a caller
/// acting immediately after the create still observes the transient stage. 500 ms sits
/// in the middle of that band.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// The stage a directory is in between `CreateDirectory` and [`TRANSITION_SPAN`].
pub const STAGE_REQUESTED: &str = "Requested";

/// The stage a created directory reaches once [`TRANSITION_SPAN`] has elapsed.
pub const STAGE_ACTIVE: &str = "Active";

/// The `DirectoryType` of a directory created by `CreateDirectory`: Simple AD.
///
/// `CreateDirectory`'s own documentation ("Creates a Simple AD directory"), rendered
/// with the `SimpleAD` value of the modelled `DirectoryType` enum.
pub const TYPE_SIMPLE_AD: &str = "SimpleAD";

/// The DNS suffix a directory's access URL is built on, from the `AccessUrl` member's
/// documentation: "The access URL for the directory, such as
/// `http://<alias>.awsapps.com`".
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// The network types a Simple AD directory may be created with.
///
/// From `CreateDirectoryRequest$NetworkType`'s own documentation: "The network type for
/// your directory. Simple AD supports IPv4 and Dual-stack only." The third value of the
/// modelled `NetworkType` enum, `IPv6`, is therefore refused for this directory type.
pub const SIMPLE_AD_NETWORK_TYPES: [&str; 2] = ["IPv4", "Dual-stack"];

/// Directory Service's default quota "Simple AD directories: 10" per account per
/// Region, the bound `DirectoryLimitExceededException` reports.
pub const MAX_DIRECTORIES: usize = 10;

/// AWS's tagging limit of 50 tags per resource, the bound
/// `TagLimitExceededException` reports.
pub const MAX_TAGS: usize = 50;

/// The `TopicStatus` of an association this emulator has just made.
///
/// The first value of the modelled `TopicStatus` enum, and the one the `Status`
/// member ("The topic registration status") reports for a registration that holds:
/// the other values describe a topic the SNS data plane this emulator does not have
/// would have to report on.
pub const TOPIC_STATUS_REGISTERED: &str = "Registered";

/// The ARN service segment of an Amazon SNS topic, so a reported `TopicArn` is built
/// through [`::engine::emulator_arn`] rather than written as a literal.
const SNS_ARN_SERVICE: &str = "sns";

/// The Region an ARN this service reports carries when the request carries no sigv4
/// credential scope to read one from — the default `emulator_arn` documents for
/// generated emulators.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most Amazon SNS topics this emulator will associate with one directory.
///
/// NOT an AWS quota: a resource guard. `RegisterEventTopic` is the one write a caller
/// can repeat with a fresh name forever, so the per-directory index carries its own
/// bound, enforced at the single insert site in `register_event_topic.rs`.
pub const MAX_EVENT_TOPICS_PER_DIRECTORY: usize = 50;

/// The most `TopicNames` entries a single `DescribeEventTopics` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a directory holds at most [`MAX_EVENT_TOPICS_PER_DIRECTORY`]
/// associations, so a longer list can only repeat names — and it is what keeps the
/// caller-chosen filter from being allocated and scanned unbounded.
pub const MAX_TOPIC_NAMES_PER_REQUEST: usize = 100;

/// The most entries this emulator keeps in the alias reservation index.
///
/// NOT an AWS quota: a resource guard. An alias "cannot be deleted or reused", so an
/// entry outlives the directory it was created for and the index is the one store here
/// that a caller can grow without deleting anything (create, alias, delete, repeat).
/// Ten times [`MAX_DIRECTORIES`] is far more reservations than a Region's worth of
/// directories can hold at once; past it `CreateAlias` is refused.
pub const MAX_RESERVED_ALIASES: usize = 10 * MAX_DIRECTORIES;

/// The most `DirectoryIds` entries a single `DescribeDirectories` will resolve.
///
/// NOT an AWS quota: a resource guard, deliberately well above any list a caller has
/// reason to send — a Region holds at most [`MAX_DIRECTORIES`] directories, so a longer
/// list can only repeat ids — and it is what keeps the caller-chosen list from being
/// allocated and scanned unbounded.
pub const MAX_DIRECTORY_IDS_PER_REQUEST: usize = 100;

/// The page size a paginated operation uses when the request declares none.
///
/// NOT an AWS quota: `Limit`'s own documentation says "If this value is zero, the
/// maximum number of items is specified by the limitations of the operation", and this
/// is that limitation for this emulator. It is above [`MAX_DIRECTORIES`] and
/// [`MAX_TAGS`], so an unpaginated caller always sees everything stored.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The longest `NextToken` this service will decode.
///
/// NOT an AWS quota: a resource guard. Every token this service issues is far shorter
/// (the kind tag, a creation rank or a tag key, and the listing's filter, hex-encoded),
/// so a longer one cannot be one of ours and is refused BEFORE anything is allocated
/// from it.
pub const MAX_NEXT_TOKEN_CHARS: usize = 1024;

/// The version tag every [`encode_cursor`] token carries, so a token minted by an older
/// build is refused rather than misread.
const CURSOR_VERSION: &str = "v1";

/// One stored association between a directory and an Amazon SNS topic: what
/// `RegisterEventTopic` persisted and `DescribeEventTopics` reports.
#[derive(Debug, Clone)]
pub struct EventTopicRegistration {
    pub topic_name: String,
    /// The topic's ARN, built once at registration through [`sns_topic_arn`] so every
    /// later read reports the same one.
    pub topic_arn: String,
    /// When the association was made — the `CreatedDateTime` member's "the date and
    /// time of when you associated your directory with the Amazon SNS topic".
    pub created_at: SystemTime,
    /// One of the modelled `TopicStatus` values; [`TOPIC_STATUS_REGISTERED`] here.
    pub status: String,
}

impl EventTopicRegistration {
    /// This association as the modelled `EventTopic`, reported for `directory_id`.
    ///
    /// The one place a stored registration is rendered, so `DescribeEventTopics`
    /// reports exactly what was persisted.
    pub fn to_event_topic(&self, directory_id: &str) -> EventTopic {
        EventTopic {
            directory_id: Some(directory_id.to_string()),
            topic_name: Some(self.topic_name.clone()),
            topic_arn: Some(self.topic_arn.clone()),
            created_date_time: Some(as_timestamp(self.created_at)),
            status: Some(self.status.clone()),
        }
    }
}

/// One stored directory: what `CreateDirectory` persisted and `DescribeDirectories`
/// reports.
#[derive(Debug, Clone)]
pub struct Directory {
    pub directory_id: String,
    pub name: String,
    pub short_name: Option<String>,
    pub description: Option<String>,
    pub size: Option<String>,
    /// Only AWS Managed Microsoft AD has an edition, so this stays `None` for a
    /// Simple AD directory; `CreateMicrosoftAD` (still scaffolded) will set it.
    pub edition: Option<String>,
    /// One of the modelled `DirectoryType` values; [`TYPE_SIMPLE_AD`] here.
    pub directory_type: String,
    /// One of the modelled `DirectoryStage` values.
    pub stage: String,
    /// When the stage transition the create owes falls due; `None` once there is no
    /// transition left to make. See [`TRANSITION_SPAN`].
    pub stage_due_at: Option<SystemTime>,
    pub launch_time: SystemTime,
    pub stage_last_updated: SystemTime,
    /// The alias `CreateAlias` assigned, if any. `None` means the directory id stands
    /// in for it, as the `Alias` member's documentation describes.
    pub alias: Option<String>,
    pub vpc_settings: Option<DirectoryVpcSettingsDescription>,
    pub network_type: Option<String>,
    pub sso_enabled: bool,
    /// Set by `UpdateNumberOfDomainControllers` (still scaffolded) and reported only
    /// for AWS Managed Microsoft AD, so it stays `None` for Simple AD.
    pub desired_number_of_domain_controllers: Option<i32>,
    /// This directory's tags, keyed by tag key: a second `AddTagsToResource` with a key
    /// already present overwrites that key's value.
    pub tags: BTreeMap<String, String>,
    /// The Amazon SNS topics `RegisterEventTopic` associated with this directory, keyed
    /// by topic name — a directory publishes to a given topic at most once, and the
    /// associations live on the directory record, so deleting the directory takes its
    /// associations with it.
    pub event_topics: BTreeMap<String, EventTopicRegistration>,
    /// Creation rank, from
    /// [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::next_seq`]. A
    /// listing pages over directories sorted by this, and its opaque `NextToken`
    /// carries the rank the next page resumes after, so a page boundary survives a
    /// directory being deleted between pages.
    pub seq: u64,
}

impl Directory {
    /// The `Alias` this directory reports: the alias `CreateAlias` assigned, or the
    /// directory id when none was.
    ///
    /// From the `Alias` member's documentation: "If no alias exists, the alias is the
    /// directory identifier, such as `d-XXXXXXXXXX`."
    pub fn effective_alias(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.directory_id)
    }

    /// The `AccessUrl` this directory reports, derived from [`Self::effective_alias`].
    ///
    /// From the `AccessUrl` member's documentation: "The access URL for the directory,
    /// such as `http://<alias>.awsapps.com`. If no alias exists, `<alias>` is the
    /// directory identifier".
    pub fn access_url(&self) -> String {
        format!(
            "http://{alias}{ACCESS_URL_SUFFIX}",
            alias = self.effective_alias()
        )
    }

    /// This directory's associations as the modelled `EventTopics` list, in topic-name
    /// order, narrowed to `wanted` when the request asked for particular topic names.
    pub fn event_topic_list(&self, wanted: Option<&BTreeSet<&String>>) -> Vec<EventTopic> {
        self.event_topics
            .values()
            .filter(|registration| {
                wanted.is_none_or(|names| names.contains(&registration.topic_name))
            })
            .map(|registration| registration.to_event_topic(&self.directory_id))
            .collect()
    }

    /// This directory's tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// This directory as the modelled `DirectoryDescription`.
    ///
    /// The members this slice has no record behind — the DNS addresses of directory
    /// servers, RADIUS, sharing, regions, the owner description — are left absent
    /// rather than invented: the operations that would populate them are still
    /// scaffolded.
    pub fn to_description(&self) -> DirectoryDescription {
        DirectoryDescription {
            directory_id: Some(self.directory_id.clone()),
            name: Some(self.name.clone()),
            short_name: self.short_name.clone(),
            size: self.size.clone(),
            edition: self.edition.clone(),
            alias: Some(self.effective_alias().to_string()),
            access_url: Some(self.access_url()),
            description: self.description.clone(),
            stage: Some(self.stage.clone()),
            launch_time: Some(as_timestamp(self.launch_time)),
            stage_last_updated_date_time: Some(as_timestamp(self.stage_last_updated)),
            r#type: Some(self.directory_type.clone()),
            vpc_settings: self.vpc_settings.clone(),
            sso_enabled: self.sso_enabled,
            desired_number_of_domain_controllers: self.desired_number_of_domain_controllers,
            network_type: self.network_type.clone(),
            ..DirectoryDescription::default()
        }
    }
}

/// Per-service state for com-amazonaws-directoryservice-directoryservice-20150416. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Every stored directory, keyed by its `DirectoryId`.
    pub directories: BTreeMap<String, Directory>,
    /// Which directory owns which alias. An alias is unique across the account, so
    /// this is what `CreateAlias` checks before assigning one. An entry OUTLIVES the
    /// directory it was created for: `CreateAlias`' documentation says an alias
    /// "cannot be deleted or reused", so `DeleteDirectory` leaves it reserved — which
    /// is why this index carries its own cap, [`MAX_RESERVED_ALIASES`], enforced at
    /// the single insert site in `create_alias.rs`.
    pub aliases: BTreeMap<String, String>,
    /// Drives [`Self::mint_directory_id`]; see there for how an id is derived from it.
    id_seed: u64,
    /// Drives [`Self::next_seq`]: the creation rank stamped on each directory.
    seq_counter: u64,
}

/// The number of distinct ids [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::mint_directory_id`]
/// can render: `16^10`, the width the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// allows.
const ID_SPACE: u64 = 1 << 40;

/// Scrambles consecutive seeds across [`ID_SPACE`]. Odd, so it is coprime with
/// `16^10 = 2^40`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)` a
/// BIJECTION: distinct seeds can never mint the same id.
const ID_MULTIPLIER: u64 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading zeros.
const ID_OFFSET: u64 = 734_291;

impl ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Mint a fresh `DirectoryId` of the shape the model declares,
    /// `^d-[0-9a-f]{10}$`.
    ///
    /// The id is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so it is unpredictable-looking yet never repeats within a
    /// process.
    pub fn mint_directory_id(&mut self) -> String {
        self.id_seed = self.id_seed.wrapping_add(1);
        let scrambled = self
            .id_seed
            .wrapping_mul(ID_MULTIPLIER)
            .wrapping_add(ID_OFFSET)
            % ID_SPACE;
        format!("d-{scrambled:010x}")
    }

    /// The next creation rank. Stamped on a directory when it is created, so a listing
    /// reports creation order even though the records are keyed by id.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }

    /// Materialise every stage transition the clock has already made due at `now`.
    ///
    /// This is the ONLY write a reporting operation performs: the change here was owed
    /// at `now` whether or not anybody called, because its due instant was stored when
    /// the directory was created.
    pub fn advance_clock(&mut self, now: SystemTime) {
        for directory in self.directories.values_mut() {
            let Some(due) = directory.stage_due_at else {
                continue;
            };
            if now >= due {
                directory.stage = STAGE_ACTIVE.to_string();
                directory.stage_last_updated = due;
                directory.stage_due_at = None;
            }
        }
    }

    /// Directories in creation order — the order `DescribeDirectories` pages over.
    pub fn directories_in_creation_order(&self) -> Vec<&Directory> {
        let mut ordered: Vec<&Directory> = self.directories.values().collect();
        ordered.sort_by_key(|directory| directory.seq);
        ordered
    }
}

/// The opaque `NextToken` for a page of `kind` that ended at `value`.
///
/// Hex-encoded so the token carries no meaning a client could construct, and tagged
/// with both a version and the kind of listing that issued it, so a token replayed
/// against another operation is refused rather than misread.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{kind}:{value}"))
}

/// The cursor value inside a `kind` token, or `None` when `token` is not one this
/// service issued for that listing — which every caller answers with
/// `InvalidNextTokenException`.
///
/// The length check comes FIRST, so a caller-sized token is never decoded into a
/// caller-sized buffer: see [`MAX_NEXT_TOKEN_CHARS`].
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    if token.len() > MAX_NEXT_TOKEN_CHARS {
        return None;
    }
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let rest = decoded.strip_prefix(&format!("{CURSOR_VERSION}:{kind}:"))?;
    Some(rest.to_string())
}

/// The page size a `Limit` member resolves to.
///
/// From `Limit`'s own documentation: "The maximum number of items to return. If this
/// value is zero, the maximum number of items is specified by the limitations of the
/// operation" — so zero, and an absent member, both mean [`DEFAULT_PAGE_SIZE`].
pub fn page_size(limit: Option<i32>) -> usize {
    limit
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

/// A `SystemTime` as the modelled timestamp type.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The `EntityDoesNotExistException` message for an id that names no stored directory.
/// Shared so every operation that resolves a directory says the same thing about the
/// same failure.
///
/// The wording follows Directory Service's own: the quoted id the request asked for,
/// then "does not exist." — so the message is built from the request and names what the
/// caller has to fix. The request id AWS appends to it is per-run and this emulator
/// mints none, so none is invented here.
pub fn no_such_directory(directory_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist.")
}

/// The ARN of the Amazon SNS topic named `topic_name` in `region`.
///
/// Built through [`::engine::emulator_arn`], so the account segment is the one
/// canonical account id every generated emulator presents. An SNS topic ARN's resource
/// portion is the bare topic name.
pub fn sns_topic_arn(region: &str, topic_name: &str) -> String {
    ::engine::emulator_arn(SNS_ARN_SERVICE, region, topic_name)
}

/// The Region a reported ARN carries: the Region of the caller's sigv4 credential
/// scope, or [`DEFAULT_REGION`] when the request carries none.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single source
/// of SigV4-scope parsing, so a header that is not a well-formed sigv4 `Authorization`
/// yields the default rather than a bogus Region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `InvalidNextTokenException` message for a token this service did not issue.
pub const INVALID_NEXT_TOKEN: &str = "The NextToken value is not valid.";

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the Directory Service emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDirectoryserviceDirectoryservice20150416State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDirectoryserviceDirectoryservice20150416State>)
}
