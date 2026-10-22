// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mq-mq service, plus the record types and
//! the small helpers more than one operation needs.
//!
//! The crate's `lib.rs` is generated and declares exactly four modules
//! (`ops`, `scaffolded`, `service`, `state`), so a helper several operations
//! share — the broker records, the `ListBrokers` page-token codec, and the
//! model's documented value constraints — lives here rather than in a module
//! `lib.rs` could not name.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// Region used for an ARN when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateBroker` a broker stops reporting
/// `CREATION_IN_PROGRESS` and reports `RUNNING`.
///
/// `CreateBroker` is documented as asynchronous ("Note: This API is
/// asynchronous"), so the broker it mints is returned in a non-terminal state
/// and the move to `RUNNING` is owed by the clock, not by any later call. Real
/// provisioning takes minutes; the emulator picks a span short enough to test
/// yet non-zero, so a caller that describes the broker immediately still
/// observes the transient state.
pub const BROKER_PROVISIONING: Duration = Duration::from_millis(500);

/// `BrokerState` while the broker is still being provisioned.
/// From `BrokerState`'s modelled values (`BROKER_STATE_VALUES`).
pub const BROKER_STATE_CREATION_IN_PROGRESS: &str = "CREATION_IN_PROGRESS";

/// `BrokerState` once provisioning is done.
pub const BROKER_STATE_RUNNING: &str = "RUNNING";

/// The `AuthenticationStrategy` applied when the request omits it — the model
/// documents "The default is SIMPLE".
pub const DEFAULT_AUTHENTICATION_STRATEGY: &str = "SIMPLE";

/// Applied to `AutoMinorVersionUpgrade` when the request omits it — the model
/// documents "Set to true by default, if no value is specified".
pub const DEFAULT_AUTO_MINOR_VERSION_UPGRADE: bool = true;

/// Amazon MQ's default per-Region, per-account broker quota (Amazon MQ
/// Developer Guide, "Quotas in Amazon MQ": 50 brokers per Region). It is also
/// the bound that keeps the store from growing with the number of requests a
/// caller makes: a create past it is refused rather than persisted.
///
/// The store holds one broker map rather than one per Region, so this quota and
/// the broker-name namespace are account-wide here; the emulator serves one
/// account and the Region only reaches a broker through its ARN.
pub const MAX_BROKERS: usize = 50;

/// Amazon MQ's documented per-broker user quota (same quota table: 250 users
/// per ActiveMQ broker), and the bound on one broker record's `users` map.
pub const MAX_USERS_PER_BROKER: usize = 250;

/// A broker is created together with its initial user, so `CreateBroker` has to
/// be given at least one.
///
/// `Users`' own documentation: "The list of broker users (persons or
/// applications) who can access queues and topics. For Amazon MQ for RabbitMQ
/// brokers, an administrative user is required if using simple authentication
/// and authorization. For brokers using OAuth2, this user is optional."
/// (`smithy_query(query="doc", shape="CreateBrokerRequest", member="Users")`).
/// The OAuth2 exemption is not reachable through this model — its
/// `AuthenticationStrategy` carries only CONFIG_MANAGED, LDAP and SIMPLE — so
/// every broker this service can be asked for needs its first user named in the
/// create. The model marks the member non-required, which is why the
/// precondition lives in the handler.
pub const MIN_USERS_PER_BROKER: usize = 1;

/// `Groups`' own documented bound: "The list of groups (20 maximum) to which the
/// ActiveMQ user belongs"
/// (`smithy_query(query="doc", shape="CreateUserRequest", member="Groups")`).
pub const MAX_GROUPS_PER_USER: usize = 20;

/// The `EngineType` value whose brokers the model excludes from some members —
/// `Logs$audit` "does not apply to RabbitMQ brokers". From `ENGINE_TYPE_VALUES`.
pub const ENGINE_TYPE_RABBITMQ: &str = "RABBITMQ";

/// `SecurityGroups`' own documented bound: "The list of rules (1 minimum, 125
/// maximum) that authorize connections to brokers".
pub const MAX_SECURITY_GROUPS: usize = 125;

/// The model publishes no maximum for `SubnetIds`, and the per-deployment-mode
/// rules it documents fix the count only for `SINGLE_INSTANCE` (one) and
/// `ACTIVE_STANDBY_MULTI_AZ` (two). So that a `CLUSTER_MULTI_AZ` create cannot
/// grow one record without bound, the emulator applies a RESOURCE bound of its
/// own — not an AWS one, and deliberately far above the number of Availability
/// Zones any Region has, so it bounds the record without refusing a cluster a
/// real deployment could ask for.
pub const MAX_SUBNET_IDS: usize = 1_000;

/// AWS's tags-per-resource limit (50), identical for every taggable resource,
/// and the bound on one broker record's `tags` map.
pub const MAX_TAGS: usize = 50;

/// A broker user. The password is held so the record is complete, and is never
/// put in a response: no modelled output member carries it.
#[derive(Debug, Clone)]
pub struct BrokerUser {
    pub username: String,
    pub password: String,
    pub console_access: bool,
    pub groups: Vec<String>,
    pub replication_user: bool,
}

/// The broker logging `CreateBroker` was asked for. `Logs$general` is a plain
/// flag, and `Logs$audit` is kept optional because the member "does not apply to
/// RabbitMQ brokers", so a request that never set it must not report one.
#[derive(Debug, Clone, Default)]
pub struct BrokerLogs {
    pub audit: Option<bool>,
    pub general: bool,
}

/// The broker's encryption options, as the request set them.
#[derive(Debug, Clone)]
pub struct BrokerEncryptionOptions {
    pub kms_key_id: Option<String>,
    pub use_aws_owned_key: bool,
}

/// The broker's maintenance window. `time_zone` is stored as sent; the model
/// documents "UTC by default", which [`DEFAULT_MAINTENANCE_TIME_ZONE`] supplies
/// when the request omitted it.
#[derive(Debug, Clone)]
pub struct BrokerMaintenanceWindow {
    pub day_of_week: String,
    pub time_of_day: String,
    pub time_zone: String,
}

/// The broker's LDAP server metadata. `service_account_password` is held so the
/// record is complete and is never put in a response: `LdapServerMetadataOutput`
/// carries no password member.
#[derive(Debug, Clone)]
pub struct BrokerLdapServerMetadata {
    pub hosts: Vec<String>,
    pub role_base: String,
    pub role_name: Option<String>,
    pub role_search_matching: String,
    pub role_search_subtree: Option<bool>,
    pub service_account_username: String,
    pub service_account_password: String,
    pub user_base: String,
    pub user_role_name: Option<String>,
    pub user_search_matching: String,
    pub user_search_subtree: Option<bool>,
}

/// The time zone a maintenance window reports when the request set none, from
/// `WeeklyStartTime$timeZone`: "The time zone, UTC by default".
pub const DEFAULT_MAINTENANCE_TIME_ZONE: &str = "UTC";

/// A broker, as `CreateBroker` persisted it.
#[derive(Debug, Clone)]
pub struct Broker {
    pub broker_id: String,
    pub broker_arn: String,
    pub broker_name: String,
    pub engine_type: String,
    pub engine_version: Option<String>,
    pub deployment_mode: String,
    pub host_instance_type: String,
    pub publicly_accessible: bool,
    pub auto_minor_version_upgrade: bool,
    pub authentication_strategy: String,
    pub storage_type: Option<String>,
    pub storage_size: Option<i32>,
    pub security_groups: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub data_replication_mode: Option<String>,
    pub data_replication_primary_broker_arn: Option<String>,
    pub created: SystemTime,
    /// The instant the clock owes this broker its move to `RUNNING`.
    pub available_at: SystemTime,
    pub creator_request_id: Option<String>,
    /// The members of the `CreateBroker` request that decide WHICH broker was
    /// asked for, so a replay carrying the same `@idempotencyToken` can be told
    /// apart from a different request reusing that token.
    pub creation_fingerprint: String,
    pub users: BTreeMap<String, BrokerUser>,
    pub tags: BTreeMap<String, String>,
    pub logs: BrokerLogs,
    pub encryption_options: Option<BrokerEncryptionOptions>,
    pub maintenance_window_start_time: Option<BrokerMaintenanceWindow>,
    pub ldap_server_metadata: Option<BrokerLdapServerMetadata>,
}

impl Broker {
    /// This broker's `BrokerState` as of `now`: derived from the stored instants,
    /// so a reporting operation computes it without writing anything.
    pub fn broker_state(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            BROKER_STATE_CREATION_IN_PROGRESS
        } else {
            BROKER_STATE_RUNNING
        }
    }

    /// `Created`, in the modelled `__timestampIso8601` type.
    pub fn created_timestamp(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from(self.created)
    }

    /// The CloudWatch Logs log group a broker's logs of `kind` are sent to.
    /// Amazon MQ names it after the broker, so it is DERIVED from the stored id
    /// rather than held: the same broker always reports the same group, and a
    /// broker that was never created has none.
    pub fn log_group(&self, kind: &str) -> String {
        format!("/aws/amazonmq/broker/{}/{kind}", self.broker_id)
    }

    /// `Tags`, as the modelled map, or `None` when the broker carries none.
    pub fn optional_tags(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        }
    }
}

/// Per-service state for com-amazonaws-mq-mq. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMqMqState {
    /// Every broker that exists, keyed by its minted `BrokerId`.
    pub brokers: BTreeMap<String, Broker>,
    /// The strictly increasing sequence the next minted `BrokerId` is derived
    /// from, so two creates in the same instant cannot mint one id.
    pub next_broker_sequence: u64,
}

impl ComAmazonawsMqMqState {
    /// The broker carrying `name`. Broker names are unique per account, so this
    /// scan over the one map is the name index — a second map could desync from it.
    pub fn broker_by_name(&self, name: &str) -> Option<&Broker> {
        self.brokers.values().find(|b| b.broker_name == name)
    }

    /// The broker an earlier `CreateBroker` with this `@idempotencyToken` created.
    pub fn broker_by_creator_request_id(&self, token: &str) -> Option<&Broker> {
        self.brokers
            .values()
            .find(|b| b.creator_request_id.as_deref() == Some(token))
    }

    /// Mint the `BrokerId` for a broker created at `created`: `b-` followed by
    /// 128 bits in the 8-4-4-4-12 grouping Amazon MQ's broker ids carry.
    ///
    /// The bits are a digest of this store's own mint sequence, the creating
    /// instant and the broker's name, so the id is unique within the store
    /// (the sequence alone guarantees that, and a collision with a stored id
    /// takes the next sequence value), is not guessable from the request alone,
    /// and is MINTED here rather than taken from any caller-supplied value.
    pub fn mint_broker_id(&mut self, created: SystemTime, broker_name: &str) -> String {
        loop {
            let sequence = self.next_broker_sequence;
            self.next_broker_sequence = sequence.wrapping_add(1);
            let nanos = created
                .duration_since(::std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
            ::sha2::Digest::update(&mut digest, sequence.to_be_bytes());
            ::sha2::Digest::update(&mut digest, nanos.to_be_bytes());
            ::sha2::Digest::update(&mut digest, broker_name.as_bytes());
            let bytes = ::sha2::Digest::finalize(digest);
            let hex: String = bytes[..16].iter().map(|b| format!("{b:02x}")).collect();
            let candidate = format!(
                "b-{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            );
            if !self.brokers.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Every broker in a stable order: by creation instant, then by id, so two
    /// listings of an unchanged set page through it identically. A broker
    /// created or deleted between two pages shifts the offsets a `NextToken`
    /// names, which is why the ordering is a stability guarantee over an
    /// unchanged set and not a snapshot across a paginating client's calls.
    pub fn brokers_in_order(&self) -> Vec<&Broker> {
        let mut all: Vec<&Broker> = self.brokers.values().collect();
        all.sort_by(|a, b| {
            a.created
                .cmp(&b.created)
                .then_with(|| a.broker_id.cmp(&b.broker_id))
        });
        all
    }
}

/// A list member, or `None` when nothing was stored — restJson1 omits a `None`
/// member rather than emitting an empty array for a value the request never set.
///
/// A free function because both a broker's own lists and a user's `Groups` go
/// through it; it has nothing to do with a particular record.
pub fn optional_list(values: &[String]) -> Option<Vec<String>> {
    if values.is_empty() {
        None
    } else {
        Some(values.to_vec())
    }
}

/// A request this service refuses: the member at fault, and why.
///
/// Each operation turns it into its OWN modelled `BadRequestException` (every
/// operation declares one, with its own generated shape), which is why this
/// carries the parts rather than a built error.
#[derive(Debug, Clone)]
pub struct Invalid {
    /// The member at fault, in its wire (`jsonName`) spelling, for
    /// `BadRequestException$errorAttribute`; `None` when no member of the
    /// request is at fault (a quota, say).
    pub attribute: Option<String>,
    /// What was wrong, naming the member and the constraint it broke.
    pub message: String,
}

impl Invalid {
    pub fn new(attribute: &str, message: impl Into<String>) -> Self {
        Self {
            attribute: Some(attribute.to_string()),
            message: message.into(),
        }
    }
}

/// The idempotency fingerprint of a `CreateBroker` request: every member that
/// defines the broker being asked for, in a canonical form. It is compared,
/// never parsed, so its spelling only has to be stable and collision-free.
///
/// `parts` are joined with the unit separator, which no member's documented
/// character set contains, so two different requests cannot fold to one string.
/// A member the request omitted contributes an empty part, so omitting a member
/// on a replay is itself a mismatch.
pub fn fingerprint_of(parts: &[String]) -> String {
    parts.join("\u{1f}")
}

/// The subject Amazon MQ names when it refuses a value outside an enumerated
/// member's value set.
///
/// MQ states such a refusal as one sentence built from the request and the
/// model: the member's documented subject, the rejected value in brackets, then
/// the values it does accept — for `engineType`, "Broker engine type
/// [<value>] is invalid. Valid values are: [ACTIVEMQ, RABBITMQ]". The subject is
/// the member's own documented description, so it is held per member here rather
/// than derived from the wire name; a member with no row falls back to its wire
/// name, which keeps the sentence well-formed for any member added later.
fn enum_subject(attribute: &str) -> &str {
    match attribute {
        // "Required. The type of broker engine." (`CreateBrokerRequest$EngineType`)
        "engineType" => "Broker engine type",
        // "Required. The broker's deployment mode." (`CreateBrokerRequest$DeploymentMode`)
        "deploymentMode" => "Broker deployment mode",
        // "The broker's storage type." (`CreateBrokerRequest$StorageType`)
        "storageType" => "Broker storage type",
        // "Optional. The authentication strategy used to secure the broker."
        // (`CreateBrokerRequest$AuthenticationStrategy`)
        "authenticationStrategy" => "Broker authentication strategy",
        // "Defines whether this broker is a part of a data replication pair."
        // (`CreateBrokerRequest$DataReplicationMode`)
        "dataReplicationMode" => "Broker data replication mode",
        other => other,
    }
}

/// Refuse a value the model declares as an enum but the request spelled
/// otherwise. `allowed` is the generated `*_VALUES` constant for that enum.
///
/// This is the ONLY check an enum-typed member of this service goes through: the
/// input schemas skip the framework's boundary check for them
/// (`skip_validation`, see `ops::types`), because a boundary refusal is worded
/// through `Refused { code, message }` and so cannot carry
/// `BadRequestException$errorAttribute`.
pub fn validate_enum(attribute: &str, value: &str, allowed: &[&str]) -> Result<(), Invalid> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!(
            "{} [{value}] is invalid. Valid values are: [{}]",
            enum_subject(attribute),
            allowed.join(", ")
        ),
    ))
}

/// Refuse an integer outside the `@range` its member declares. As with
/// [`validate_enum`], the handler owns this refusal so that it can name the
/// member in `BadRequestException$errorAttribute`.
pub fn validate_range(attribute: &str, value: i32, min: i32, max: i32) -> Result<(), Invalid> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!("{attribute} must be between {min} and {max}, got {value}"),
    ))
}

/// Refuse a `CreateBroker` that names no broker user, and one that names more
/// than a broker record may hold. `count` is the number of users the request
/// carried — an absent list counts as none, exactly like an empty one.
/// See [`MIN_USERS_PER_BROKER`] for the rule and where it comes from.
///
/// The missing-user sentence is Amazon MQ's own, and names the input rather than
/// any value from the request, so it reads the same for every request in the
/// class.
pub fn validate_users_requested(count: usize) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "users";
    if count < MIN_USERS_PER_BROKER {
        return Err(Invalid::new(ATTRIBUTE, "Specify at least one user."));
    }
    validate_list_size(
        ATTRIBUTE,
        count,
        MIN_USERS_PER_BROKER,
        MAX_USERS_PER_BROKER,
    )
}

/// Refuse an absent or blank required string member.
pub fn validate_present(attribute: &str, value: &str) -> Result<(), Invalid> {
    if value.trim().is_empty() {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} is required and must not be blank"),
        ));
    }
    Ok(())
}

/// The sentence Amazon MQ answers a `BrokerId` that resolves to no broker
/// with, on every operation that resolves one. Only the requested id varies, so
/// the four handlers that look a broker up share this one sentence rather than
/// each wording the same failure differently.
pub fn broker_not_found_message(broker_id: &str) -> String {
    format!(
        "Cannot find broker ID [{broker_id}]. Verify that your broker exists or check \
         broker permissions."
    )
}

/// `BrokerName`'s documented constraint: "1-50 characters long, must contain
/// only letters, numbers, dashes, and underscores"
/// (`smithy_query(query="doc", shape="CreateBrokerRequest", member="BrokerName")`).
pub fn validate_broker_name(name: &str) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "brokerName";
    if name.is_empty() || name.chars().count() > 50 {
        return Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "{ATTRIBUTE} must be 1-50 characters long, got {} characters",
                name.chars().count()
            ),
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "{ATTRIBUTE} must contain only letters, numbers, dashes and underscores, \
                 got the character {bad:?}"
            ),
        ));
    }
    Ok(())
}

/// A user's `Username` constraint, as the model documents it: "can contain only
/// alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~).
/// This value must be 2-100 characters long."
pub fn validate_username(attribute: &str, username: &str) -> Result<(), Invalid> {
    let length = username.chars().count();
    if !(2..=100).contains(&length) {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} must be 2-100 characters long, got {length} characters"),
        ));
    }
    if let Some(bad) = username
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')))
    {
        return Err(Invalid::new(
            attribute,
            format!(
                "{attribute} must contain only alphanumeric characters, dashes, periods, \
                 underscores and tildes (- . _ ~), got the character {bad:?}"
            ),
        ));
    }
    Ok(())
}

/// A user's `Password` constraint, as the model documents it: "must be at least
/// 12 characters long, must contain at least 4 unique characters, and must not
/// contain commas, colons, or equal signs (,:=)."
pub fn validate_password(attribute: &str, password: &str) -> Result<(), Invalid> {
    let length = password.chars().count();
    if length < 12 {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} must be at least 12 characters long, got {length} characters"),
        ));
    }
    let unique: std::collections::BTreeSet<char> = password.chars().collect();
    if unique.len() < 4 {
        return Err(Invalid::new(
            attribute,
            format!(
                "{attribute} must contain at least 4 unique characters, got {}",
                unique.len()
            ),
        ));
    }
    if let Some(bad) = password.chars().find(|c| matches!(c, ',' | ':' | '=')) {
        return Err(Invalid::new(
            attribute,
            format!("{attribute} must not contain commas, colons or equal signs (,:=), got {bad:?}"),
        ));
    }
    Ok(())
}

/// A user's `Groups` constraint, as the model documents it: "The list of groups
/// (20 maximum) to which the ActiveMQ user belongs. This value can contain only
/// alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~).
/// This value must be 2-100 characters long." — the same value rule as a
/// username, which is why it reuses [`validate_username`].
pub fn validate_groups(attribute: &str, groups: &[String]) -> Result<(), Invalid> {
    validate_list_size(attribute, groups.len(), 0, MAX_GROUPS_PER_USER)?;
    for (position, group) in groups.iter().enumerate() {
        validate_username(attribute, group).map_err(|invalid| Invalid {
            attribute: invalid.attribute,
            message: format!("{attribute}[{position}]: {}", invalid.message),
        })?;
    }
    Ok(())
}

/// Refuse a caller-sized list outside `min..=max` members. Bounding what one
/// record can hold is what keeps a create from growing the store by as much as
/// the request asks for.
pub fn validate_list_size(
    attribute: &str,
    len: usize,
    min: usize,
    max: usize,
) -> Result<(), Invalid> {
    if (min..=max).contains(&len) {
        return Ok(());
    }
    Err(Invalid::new(
        attribute,
        format!("{attribute} must hold between {min} and {max} members, got {len}"),
    ))
}

/// Refuse a `SubnetIds` list whose size the selected deployment mode does not
/// allow. From `SubnetIds`' own documentation: "A SINGLE_INSTANCE deployment
/// requires one subnet (for example, the default subnet). An
/// ACTIVE_STANDBY_MULTI_AZ Amazon MQ for ActiveMQ deployment requires two
/// subnets. A CLUSTER_MULTI_AZ Amazon MQ for RabbitMQ deployment has no subnet
/// requirements when deployed with public accessibility."
///
/// An ABSENT list is not refused: Amazon MQ then places the broker in the
/// account's default subnet, so absence is not a constraint violation. An
/// explicitly EMPTY list is read the same way — "no subnets supplied" — unlike
/// `SecurityGroups`, whose own documentation states a minimum of 1. A list that
/// was sent has to match the mode it was sent with.
pub fn validate_subnet_ids(deployment_mode: &str, subnet_ids: &[String]) -> Result<(), Invalid> {
    const ATTRIBUTE: &str = "subnetIds";
    if subnet_ids.is_empty() {
        return Ok(());
    }
    validate_list_size(ATTRIBUTE, subnet_ids.len(), 1, MAX_SUBNET_IDS)?;
    let required = match deployment_mode {
        "SINGLE_INSTANCE" => Some(1),
        "ACTIVE_STANDBY_MULTI_AZ" => Some(2),
        // CLUSTER_MULTI_AZ documents no exact count.
        _ => None,
    };
    match required {
        Some(count) if subnet_ids.len() != count => Err(Invalid::new(
            ATTRIBUTE,
            format!(
                "a {deployment_mode} deployment requires {count} subnet(s), got {}",
                subnet_ids.len()
            ),
        )),
        _ => Ok(()),
    }
}

/// The page size `ListBrokers` uses when the request omits `MaxResults`, from
/// the member's own documentation: "The maximum number of brokers that Amazon
/// MQ can return per page (20 by default)". The modelled `@range` (1..100)
/// bounds an explicit `MaxResults`; it is not the default.
pub const DEFAULT_MAX_RESULTS: usize = 20;

/// `MaxResults`' modelled `@range`, from
/// `smithy_query(query="operation", operation="ListBrokers")`:
/// `MaxResults { range: { min: 1, max: 100 } }`. The handler enforces it (see
/// [`validate_range`]) so that the refusal names the member.
pub const MIN_MAX_RESULTS: i32 = 1;

/// The upper end of `MaxResults`' modelled `@range` — see [`MIN_MAX_RESULTS`].
pub const MAX_MAX_RESULTS: i32 = 100;

/// Tags a page token as this listing's own, so a token from somewhere else
/// decodes to `None` rather than to an offset it was never meant to name.
const PAGE_TOKEN_PREFIX: &str = "mqb1";

/// One page of a listing: the slice to serialize and the token that resumes
/// after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub start: usize,
    pub end: usize,
    pub next_token: Option<String>,
}

/// Encode the offset a `NextToken` resumes at: the tag plus the offset in
/// zero-padded hexadecimal, so the token carries no readable number and a client
/// has to round-trip it rather than compute one.
pub fn encode_page_token(offset: usize) -> String {
    format!(
        "{PAGE_TOKEN_PREFIX}{offset:0width$x}",
        width = PAGE_TOKEN_DIGITS
    )
}

/// The number of hexadecimal digits [`encode_page_token`] emits, so a token is
/// recognised by its exact shape rather than by what a radix parser tolerates.
const PAGE_TOKEN_DIGITS: usize = 12;

/// Decode a `NextToken` this service issued. `None` for anything else — the
/// caller answers that with the modelled client-input error.
///
/// The digits are checked explicitly: `from_str_radix` alone would also accept a
/// sign prefix (`+1`) or a shorter run, which this service never emits, so such
/// a token would decode to an offset it was never issued for.
pub fn decode_page_token(token: &str) -> Option<usize> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    if digits.len() != PAGE_TOKEN_DIGITS
        || !digits.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return None;
    }
    usize::from_str_radix(digits, 16).ok()
}

/// The sentence Amazon MQ answers a `NextToken` it did not issue with. Only the
/// token the request sent varies; it lives beside the codec that decides which
/// tokens are this service's.
pub fn invalid_page_token_message(token: &str) -> String {
    format!("Invalid next token [{token}]. Make sure you copied the token correctly.")
}

/// The page of `total` items `max_results` and `next_token` select.
///
/// A token past the end yields an empty page rather than an error, and the last
/// page carries no `NextToken`, so a paginating client terminates.
///
/// `max_results` is already inside the member's modelled `range` (1..100) by the
/// time a handler calls this — the handler refuses 0, anything negative and
/// anything past 100 itself (`validate_range`), so the `filter`/`unwrap_or`
/// below is a total-function fallback, not a silent clamp of a value a caller
/// could actually send.
pub fn page_of(total: usize, max_results: Option<i32>, offset: usize) -> Page {
    let size = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let start = offset.min(total);
    let end = start.saturating_add(size).min(total);
    Page {
        start,
        end,
        next_token: (end < total).then(|| encode_page_token(end)),
    }
}

/// The region to build an ARN in: the one the caller signed for, else
/// [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMqMqState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMqMqState>)
}
