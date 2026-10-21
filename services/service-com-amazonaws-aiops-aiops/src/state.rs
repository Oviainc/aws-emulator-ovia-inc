// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-aiops-aiops service.
//!
//! The service's primary resource is the **investigation group**. Every group is
//! stored exactly once, keyed by its 16-character id — the last segment of its ARN
//! — so each operation reads and writes the one record. `name_index` is the
//! secondary index that lets `{identifier}` resolve by name, which the modelled
//! `InvestigationGroupIdentifier` pattern allows alongside the id and the full ARN.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{CrossAccountConfiguration, EncryptionConfiguration};

/// Characters an investigation-group id is built from.
///
/// Taken from the modelled `InvestigationGroupArn` pattern, whose last segment is
/// `[A-Za-z0-9]{16}`:
/// `smithy_query(query="shape",shape="InvestigationGroupArn")`.
const GROUP_ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Length of the id segment of an investigation-group ARN, from the same pattern.
pub const GROUP_ID_LEN: usize = 16;

/// The resource portion an investigation-group ARN carries before its id, from the
/// same modelled pattern.
pub const GROUP_ARN_RESOURCE_PREFIX: &str = "investigation-group/";

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name `aiops`).
pub const ARN_SERVICE: &str = "aiops";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Retention applied when `CreateInvestigationGroup` omits `retentionInDays`.
///
/// From that member's own documentation: "If you omit this parameter, the default
/// of 90 days is used."
pub const DEFAULT_RETENTION_IN_DAYS: i64 = 90;

/// Applied when `CreateInvestigationGroup` omits `isCloudTrailEventHistoryEnabled`.
///
/// From that member's own documentation: "The default is true."
pub const DEFAULT_CLOUD_TRAIL_EVENT_HISTORY_ENABLED: bool = true;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteInvestigationGroup`, so each growth site carries a cap and nothing
// caller-sized reaches the store unchecked. A cap is this service's own published
// quota where it publishes one; where it publishes none, the constant says so and
// the value is this emulator's resource guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_INVESTIGATION_GROUPS groups, each
// carrying at most MAX_TAGS tags, MAX_COLLECTION_ENTRIES tag key boundaries and
// MAX_COLLECTION_ENTRIES notification channels. Every remaining caller-sized member
// is already bounded by the model, so the centralized walk refuses it before a
// handler runs: `crossAccountConfigurations` (0..25), each channel's
// `ChatConfigurationArns` list (1..5) and its SNS topic key (20..2048), each tag key
// (1..128) and value (1..256), each `tagKeyBoundaries` entry (1..128), `roleArn`
// (20..2048), `name` (1..512) and the policy document (1..32768).
// ---------------------------------------------------------------------------

/// The most investigation groups this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. AWS documents a far tighter soft limit
/// ("Currently, you can have one investigation group in each Region in your
/// account"), which this slice deliberately does not enforce because
/// `ListInvestigationGroups`' pagination needs several groups to exist; this cap is
/// what keeps the store finite in its place. A create that would exceed it is
/// refused with the modelled `ServiceQuotaExceededException`.
pub const MAX_INVESTIGATION_GROUPS: usize = 500;

/// The most tags one group may carry.
///
/// From the `tags` member's own documentation: "You can associate as many as 50 tags
/// with an investigation group."
pub const MAX_TAGS: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will
/// store for a caller-sized list or map whose size the model publishes no length
/// for — `tagKeyBoundaries` and `chatbotNotificationChannel`.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// One stored investigation group: every member `GetInvestigationGroup` reports,
/// plus the resource policy `Put/GetInvestigationGroupPolicy` attach to it and the
/// tags the tagging operations will key off its ARN.
#[derive(Debug, Clone, Default)]
pub struct InvestigationGroup {
    /// The 16-character id: the ARN's last segment and this record's store key.
    pub id: String,
    pub arn: String,
    pub name: String,
    pub role_arn: String,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub retention_in_days: Option<i64>,
    pub tag_key_boundaries: Vec<String>,
    pub chatbot_notification_channel: BTreeMap<String, Vec<String>>,
    pub is_cloud_trail_event_history_enabled: Option<bool>,
    pub cross_account_configurations: Vec<CrossAccountConfiguration>,
    /// The IAM resource policy document, present only once a Put stored one.
    pub policy: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_at: i64,
    pub last_modified_by: String,
    pub last_modified_at: i64,
}

impl InvestigationGroup {
    /// `chatbotNotificationChannel` as the wire shape wants it: omitted when the
    /// group carries no channel at all.
    pub fn chatbot_notification_channel_output(&self) -> Option<HashMap<String, Vec<String>>> {
        (!self.chatbot_notification_channel.is_empty()).then(|| {
            self.chatbot_notification_channel
                .iter()
                .map(|(topic, configs)| (topic.clone(), configs.clone()))
                .collect()
        })
    }

    /// `tagKeyBoundaries`, omitted when none were ever set.
    pub fn tag_key_boundaries_output(&self) -> Option<Vec<String>> {
        (!self.tag_key_boundaries.is_empty()).then(|| self.tag_key_boundaries.clone())
    }

    /// `crossAccountConfigurations`, omitted when none were ever set.
    pub fn cross_account_configurations_output(&self) -> Option<Vec<CrossAccountConfiguration>> {
        (!self.cross_account_configurations.is_empty())
            .then(|| self.cross_account_configurations.clone())
    }

    /// Record that `principal` just changed this group.
    pub fn touch(&mut self, principal: &str, at: i64) {
        self.last_modified_by = principal.to_string();
        self.last_modified_at = at;
    }
}

/// Per-service state for com-amazonaws-aiops-aiops. A handler reaches it through
/// [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAiopsAiopsState {
    /// Every investigation group, keyed by its 16-character id. A `BTreeMap` so
    /// `ListInvestigationGroups` walks a stable, total order and a page token can
    /// name the next id.
    pub investigation_groups: BTreeMap<String, InvestigationGroup>,
    /// Group name -> group id, so `{identifier}` resolves by name and a create can
    /// detect a name already in use.
    pub name_index: HashMap<String, String>,
    /// Mixed into each generated id so two creates in the same nanosecond differ.
    id_counter: u64,
}

impl ComAmazonawsAiopsAiopsState {
    /// The id of the group `identifier` names, accepting either form the modelled
    /// `InvestigationGroupIdentifier` allows: the full ARN, or the bare name (the
    /// form `GetInvestigationGroup`'s own docs name) — and, since the bare pattern
    /// admits it, the bare id.
    ///
    /// An ARN must match the stored ARN in EVERY field, so an ARN naming another
    /// partition, region or account resolves to nothing rather than to the group
    /// that happens to share its id segment. The name is tried before the id, so a
    /// group named after another group's id still resolves to itself.
    pub fn resolve_group_id(&self, identifier: &str) -> Option<String> {
        if let Some(id) = group_id_from_arn(identifier) {
            return self
                .investigation_groups
                .get(id)
                .filter(|group| group.arn == identifier)
                .map(|group| group.id.clone());
        }
        if let Some(id) = self.name_index.get(identifier) {
            return Some(id.clone());
        }
        self.investigation_groups
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }

    /// The group `identifier` names, for a reporting operation.
    pub fn group(&self, identifier: &str) -> Option<&InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        self.investigation_groups.get(&id)
    }

    /// The group `identifier` names, for an operation the spec says mutates it.
    ///
    /// `name` and `id` are part of this record's identity: `name_index` is keyed on
    /// the name and the store on the id, so neither may be reassigned through this
    /// handle without rebuilding both. No operation needs to — the model binds no
    /// `name` member on `UpdateInvestigationGroup` — and a future rename must go
    /// through [`Self::remove_group`] + [`Self::insert_group`].
    pub fn group_mut(&mut self, identifier: &str) -> Option<&mut InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        self.investigation_groups.get_mut(&id)
    }

    /// Store a newly created group and index it by name.
    pub fn insert_group(&mut self, group: InvestigationGroup) {
        self.name_index.insert(group.name.clone(), group.id.clone());
        self.investigation_groups.insert(group.id.clone(), group);
    }

    /// Remove the group `identifier` names, with its policy, tags and name index
    /// entry. `None` when nothing matched.
    pub fn remove_group(&mut self, identifier: &str) -> Option<InvestigationGroup> {
        let id = self.resolve_group_id(identifier)?;
        let group = self.investigation_groups.remove(&id)?;
        self.name_index.remove(&group.name);
        Some(group)
    }

    /// A 16-character alphanumeric id no stored group is using.
    pub fn fresh_group_id(&mut self) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let seed = now_nanos() ^ self.id_counter.rotate_left(17);
            let candidate = group_id_from_seed(seed);
            if !self.investigation_groups.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[A-Za-z0-9]{16}`, and there are always
        // more of them than there are stored groups.
        (0u64..=self.investigation_groups.len() as u64)
            .map(|n| format!("{n:016}"))
            .find(|id| !self.investigation_groups.contains_key(id))
            .unwrap_or_else(|| format!("{:016}", self.investigation_groups.len()))
    }
}

/// Why an `encryptionConfiguration` is unacceptable, or `None` when it is fine.
///
/// `EncryptionConfiguration.type` targets the modelled `EncryptionConfigurationType`
/// enum, so only that shape's own value set is accepted. The catalog is the
/// generated `ENCRYPTION_CONFIGURATION_TYPE_VALUES`, which carries exactly the
/// values `smithy_query(query="shape",shape="EncryptionConfigurationType")` reports.
/// Every operation that accepts the structure applies this one rule, each rendering
/// the refusal as its own modelled `ValidationException`.
///
/// `type` is NOT `@required` on the structure, so a configuration that omits it is a
/// request the model admits and this rule has nothing to say about — hence the early
/// `?` on the member rather than a refusal.
pub fn encryption_configuration_rejection(
    configuration: Option<&EncryptionConfiguration>,
) -> Option<String> {
    let supplied = configuration?.r#type.as_deref()?;
    if crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&supplied) {
        return None;
    }
    Some(format!(
        "encryptionConfiguration.type must be one of [{}], got '{supplied}'",
        crate::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES.join(", ")
    ))
}

/// Why the caller-sized collections on a create or update request are unacceptable,
/// or `None` when every one of them is within the store's bounds.
///
/// The model publishes no length for the `tags` and `chatbotNotificationChannel`
/// maps or the `tagKeyBoundaries` list, so the centralized walk cannot refuse an
/// oversized one and this is where the bound lives. Every operation that stores any
/// of them applies this one rule, each rendering the refusal as its own modelled
/// `ValidationException`. Pass `None` for a member the operation does not accept.
pub fn collection_bounds_rejection(
    tags: Option<&std::collections::HashMap<String, String>>,
    tag_key_boundaries: Option<&[String]>,
    chatbot_notification_channel: Option<&std::collections::HashMap<String, Vec<String>>>,
) -> Option<String> {
    let too_many = |member: &str, actual: usize, cap: usize| {
        (actual > cap).then(|| format!("{member} carries {actual} entries, more than the {cap} this service accepts"))
    };
    too_many("tags", tags.map_or(0, std::collections::HashMap::len), MAX_TAGS)
        .or_else(|| {
            too_many(
                "tagKeyBoundaries",
                tag_key_boundaries.map_or(0, <[String]>::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
        .or_else(|| {
            too_many(
                "chatbotNotificationChannel",
                chatbot_notification_channel.map_or(0, std::collections::HashMap::len),
                MAX_COLLECTION_ENTRIES,
            )
        })
}

/// The id segment of an investigation-group ARN, or `None` when `identifier` is not
/// an ARN of this service's investigation-group resource.
pub fn group_id_from_arn(identifier: &str) -> Option<&str> {
    if !identifier.starts_with("arn:") {
        return None;
    }
    let mut fields = identifier.splitn(6, ':');
    let (_arn, _partition, service, _region, _account, resource) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    if service != ARN_SERVICE {
        return None;
    }
    let id = resource.strip_prefix(GROUP_ARN_RESOURCE_PREFIX)?;
    (id.len() == GROUP_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(id)
}

/// The ARN of the investigation group `id` in `region`.
pub fn group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{GROUP_ARN_RESOURCE_PREFIX}{id}"))
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[a-zA-Z0-9-]*` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The principal that made this request, for `createdBy`/`lastModifiedBy`.
///
/// Derived from the request's own SigV4 access key id, so the two members report
/// who called rather than a fabricated identity. That id is caller-controlled text,
/// so it is first reduced to the characters an access key id can hold; the rendering
/// then satisfies `IdentifierStringWithPatternAndLengthLimits`
/// (`^[\-_\/A-Za-z0-9:\.]+$`, 1..512) by construction, whatever the header said.
pub fn caller_principal(parts: &::http::request::Parts) -> String {
    let raw = ::engine::extract_akid(&parts.headers);
    let filtered: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let akid = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// Now, in whole seconds since the Unix epoch — the unit the modelled `createdAt` /
/// `lastModifiedAt` longs carry.
pub fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `[A-Za-z0-9]{16}` id from `seed`.
fn group_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..GROUP_ID_LEN)
        .map(|_| {
            let draw = splitmix64(&mut state);
            GROUP_ID_ALPHABET[(draw % GROUP_ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAiopsAiopsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAiopsAiopsState>)
}
