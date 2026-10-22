// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkfirewall-networkfirewall-20201112 service.
//!
//! This module owns the service's persisted records and every projection from a
//! record onto a modelled shape, so each `src/ops/` handler only resolves,
//! mutates and projects — it never re-derives an ARN, an id or a status token.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Attachment, EncryptionConfiguration, Firewall, FirewallMetadata, FirewallPolicy,
    FirewallPolicyMetadata, FirewallPolicyResponse, FirewallStatus, PerObjectStatus, SubnetMapping,
    SyncState, Tag,
};

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, from the
/// model's `aws.api#service` trait: `arnNamespace: network-firewall`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "network-firewall";

/// The region an ARN is built in when the request carries no SigV4 credential
/// scope to read one from, matching the region the emulator's generated client
/// configuration defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateFirewall` the firewall's endpoints finish provisioning,
/// i.e. when `FirewallStatus.Status` becomes `READY`.
///
/// CHOSEN, not documented: AWS publishes no provisioning duration for Network
/// Firewall, so this is the emulator's own span. It is deliberately non-zero so a
/// caller that describes the firewall immediately still observes `PROVISIONING`,
/// and short enough that a test can wait it out.
pub const FIREWALL_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// `FirewallStatusValue` members (`smithy_query(query="shape",shape="FirewallStatusValue")`).
pub const FIREWALL_PROVISIONING: &str = "PROVISIONING";
/// See [`FIREWALL_PROVISIONING`].
pub const FIREWALL_READY: &str = "READY";
/// See [`FIREWALL_PROVISIONING`].
pub const FIREWALL_DELETING: &str = "DELETING";

/// `ConfigurationSyncState` / `PerObjectSyncStatus` member `IN_SYNC`.
pub const SYNC_IN_SYNC: &str = "IN_SYNC";

/// `AttachmentStatus` members (`smithy_query(query="shape",shape="AttachmentStatus")`).
pub const ATTACHMENT_CREATING: &str = "CREATING";
/// See [`ATTACHMENT_CREATING`].
pub const ATTACHMENT_READY: &str = "READY";
/// See [`ATTACHMENT_CREATING`].
pub const ATTACHMENT_DELETING: &str = "DELETING";

/// `ResourceStatus` members (`smithy_query(query="shape",shape="ResourceStatus")`).
pub const RESOURCE_ACTIVE: &str = "ACTIVE";
/// See [`RESOURCE_ACTIVE`].
pub const RESOURCE_DELETING: &str = "DELETING";

/// The upper bound of `PaginationMaxResults` (`range 1..100`), and therefore the
/// page size a request that omits `MaxResults` gets.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The highest number of distinct Availability Zones one firewall can span here:
/// every firewall subnet must sit in a different Availability Zone, and
/// [`availability_zone_of`] names them `<region>a`..`<region>z`.
pub const MAX_AVAILABILITY_ZONES: usize = 26;

/// How many firewalls this emulator keeps.
///
/// CHOSEN, not an AWS quota: every `CreateFirewall` retains a record for as long
/// as the firewall is not deleted, so without a ceiling a caller that loops with
/// fresh names grows the emulator's memory without bound. The ceiling is far
/// above any workflow an emulator serves, and a request past it is refused with
/// the modelled `LimitExceededException` ("Unable to perform the operation
/// because doing so would violate a limit setting").
pub const MAX_STORED_FIREWALLS: usize = 1_000;

/// How many firewall policies this emulator keeps. See [`MAX_STORED_FIREWALLS`].
pub const MAX_STORED_FIREWALL_POLICIES: usize = 1_000;

/// How many entries any one list inside a retained `FirewallPolicy` document may
/// carry.
///
/// CHOSEN, not an AWS quota: the model puts NO length constraint on the policy's
/// rule-group reference, default-action or custom-action lists, so one request
/// could otherwise make a single retained record arbitrarily large. AWS bounds
/// the same thing by rule CAPACITY (the `Consumed*RuleCapacity` members), which
/// this emulator cannot compute until rule groups are implemented; this entry
/// count stands in for it and is far above any real policy. A request past it is
/// refused with the modelled `LimitExceededException`.
pub const MAX_POLICY_LIST_ENTRIES: usize = 64;

/// The first list in `policy` that exceeds [`MAX_POLICY_LIST_ENTRIES`], named
/// with its length, or `None` when the document is within bounds.
pub fn policy_document_breach(policy: &FirewallPolicy) -> Option<String> {
    let lists = [
        (
            "StatelessRuleGroupReferences",
            policy
                .stateless_rule_group_references
                .as_ref()
                .map_or(0, Vec::len),
        ),
        (
            "StatefulRuleGroupReferences",
            policy
                .stateful_rule_group_references
                .as_ref()
                .map_or(0, Vec::len),
        ),
        (
            "StatelessCustomActions",
            policy.stateless_custom_actions.as_ref().map_or(0, Vec::len),
        ),
        (
            "StatelessDefaultActions",
            policy.stateless_default_actions.len(),
        ),
        (
            "StatelessFragmentDefaultActions",
            policy.stateless_fragment_default_actions.len(),
        ),
        (
            "StatefulDefaultActions",
            policy.stateful_default_actions.as_ref().map_or(0, Vec::len),
        ),
    ];
    lists
        .into_iter()
        .find(|(_, length)| *length > MAX_POLICY_LIST_ENTRIES)
        .map(|(member, length)| {
            format!(
                "FirewallPolicy.{member} has {length} entries; at most \
                 {MAX_POLICY_LIST_ENTRIES} are allowed."
            )
        })
}

/// Sixteen bytes nothing else in this process will produce again: the first half
/// of a SHA-256 (the `sha2` crate, not a hand-written hash) over a seed that is
/// unique per call.
///
/// The seed is a process-wide counter that strictly increases (so two mints in
/// one clock tick differ), the current instant (so a restarted process does not
/// repeat the sequence) and one operating-system seeded value per process
/// (`RandomState`'s keys are OS-seeded the first time one is built, which is the
/// entropy `std` exposes), so ids minted by different processes differ too.
/// NOTHING about the request enters the seed: an id is never derived from client
/// input.
///
/// This is the same scheme the sibling service crates mint their modelled ids
/// with (for example `service-com-amazonaws-synthetics-synthetics`'s
/// `state::new_uuid`).
fn fresh_bytes() -> [u8; 16] {
    use ::sha2::Digest as _;
    use ::std::hash::BuildHasher as _;
    use ::std::sync::atomic::{AtomicU64, Ordering};
    use ::std::sync::OnceLock;

    /// Distinguishes two values minted in the same instant by the same process.
    static MINTED: AtomicU64 = AtomicU64::new(0);
    /// One value per process, drawn from the operating system.
    static PROCESS_SEED: OnceLock<u64> = OnceLock::new();

    let process_seed = *PROCESS_SEED.get_or_init(|| {
        ::std::collections::hash_map::RandomState::new().hash_one(::std::process::id())
    });
    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed.to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    bytes
}

/// A fresh server-generated id in the `ResourceId` / `UpdateToken` form
/// (`^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`, length 36), which is
/// exactly a lowercase-hyphenated version-4 UUID: [`fresh_bytes`] laid out with
/// the version-4 and RFC 4122 variant nibbles and hex-encoded (the `hex` crate,
/// not a hand-written codec) into the five modelled groups.
pub fn new_uuid() -> String {
    let mut bytes = fresh_bytes();
    // Version 4 in the high nibble of byte 6 and the RFC 4122 variant in the top
    // bits of byte 8, the layout a random UUID carries on the wire.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The region this request was signed for, read from its SigV4 credential scope
/// so every ARN an operation mints matches the caller's own endpoint region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:network-firewall:<region>:<account>:firewall/<name>`.
pub fn firewall_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("firewall/{name}"))
}

/// `arn:aws:network-firewall:<region>:<account>:firewall-policy/<name>`.
pub fn firewall_policy_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("firewall-policy/{name}"))
}

/// The Availability Zone the firewall endpoint for the `index`-th subnet mapping
/// lives in.
///
/// The emulator holds no VPC/subnet inventory, so it cannot look a subnet's zone
/// up. Instead each firewall subnet is given its own zone in the request's
/// region, in request order, which is exactly the invariant the model documents
/// for `Firewall.SubnetMappings`: "each must belong to a different Availability
/// Zone".
pub fn availability_zone_of(region: &str, index: usize) -> String {
    // Unreachable today: `CreateFirewall` refuses more than
    // `MAX_AVAILABILITY_ZONES` subnet mappings before any zone is named, so the
    // modulo never wraps two subnets onto one zone key.
    debug_assert!(
        index < MAX_AVAILABILITY_ZONES,
        "subnet index {index} should have been refused before reaching here"
    );
    let letter = char::from(b'a' + u8::try_from(index % MAX_AVAILABILITY_ZONES).unwrap_or(0));
    format!("{region}{letter}")
}

/// A firewall endpoint id (`vpce-` plus 17 hex digits, the shape AWS VPC endpoint
/// ids take).
pub fn new_endpoint_id() -> String {
    // `hex::encode` renders the 16 bytes as exactly 32 digits, so the 17-digit
    // prefix is always there.
    let digits = ::hex::encode(fresh_bytes());
    format!("vpce-{}", &digits[..17])
}

/// Render a `SystemTime` as the modelled timestamp type.
///
/// Built from whole seconds plus the sub-second nanoseconds rather than a float,
/// so the stored instant round-trips exactly.
pub fn as_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The listing kind [`ListFirewalls`](crate::ops::list_firewalls) stamps on the
/// cursors it mints, so a cursor is only ever resumed by the listing that issued
/// it.
pub const PAGE_KIND_FIREWALLS: &str = "firewalls";

/// The listing kind [`ListFirewallPolicies`](crate::ops::list_firewall_policies)
/// stamps on its cursors. See [`PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_FIREWALL_POLICIES: &str = "firewall-policies";

/// Base64 of `<kind>:<key>` standing for "resume after this key" in that
/// listing's order.
///
/// Base64 (`aws_smithy_types::base64`, the codec AWS's own opaque tokens use)
/// keeps the token inside the modelled `PaginationToken` character class
/// (`^[0-9A-Za-z:\/+=]+$`), which a raw resource name does not: `ResourceName`
/// admits `-`, which the token class forbids. The `kind` prefix is what lets
/// [`decode_page_token`] refuse a cursor another listing minted; no
/// `ResourceName` can contain `:`, so the prefix is unambiguous.
pub fn encode_page_token(kind: &str, after_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{after_key}"))
}

/// The key a cursor of `kind` names, or `None` when the token is not one THAT
/// listing issued.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let (decoded_kind, after_key) = decoded.split_once(':')?;
    (decoded_kind == kind).then(|| after_key.to_string())
}

/// One page of a listing: the items, plus the token that resumes after them when
/// more remain.
#[derive(Debug)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Present only when at least one further item remains.
    pub next_token: Option<String>,
}

/// A `NextToken` the listing it was sent to did not mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// Cut `entries` — key-ordered `(key, item)` pairs — into the page `next_token`
/// and `max_results` ask for.
///
/// Resuming by KEY rather than by offset keeps a resumed page correct across a
/// delete between the two calls: the second page starts at the first key after
/// the one the token names, whether or not that key still exists.
pub fn paginate<T>(
    mut entries: Vec<(String, T)>,
    kind: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, InvalidPageToken> {
    if let Some(token) = next_token {
        let after = decode_page_token(kind, token).ok_or(InvalidPageToken)?;
        entries.retain(|(key, _)| key.as_str() > after.as_str());
    }
    // `MaxResults` carries the modelled `PaginationMaxResults` range (1..100),
    // which the boundary validator already refused outside, so this clamp is
    // unreachable defence and never the rule: an out-of-range value is answered
    // with a 400 before this runs, not silently narrowed.
    debug_assert!(
        max_results.is_none_or(|requested| (1..=MAX_PAGE_SIZE).contains(&requested)),
        "MaxResults should have been refused at the boundary: {max_results:?}"
    );
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    let limit = usize::try_from(limit).unwrap_or(1);
    let next_token = if entries.len() > limit {
        entries
            .get(limit - 1)
            .map(|(key, _)| encode_page_token(kind, key))
    } else {
        None
    };
    entries.truncate(limit);
    Ok(Page {
        items: entries.into_iter().map(|(_, item)| item).collect(),
        next_token,
    })
}

/// Why a name/ARN pair did not name a stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// Neither the name nor the ARN was supplied.
    NoIdentifier,
    /// Both were supplied and they name different resources.
    Mismatch,
    /// No stored resource carries that name or ARN.
    NotFound,
}

/// The status of one firewall endpoint, as persisted.
#[derive(Debug, Clone)]
pub struct SyncStateRecord {
    /// The subnet the endpoint was created in.
    pub subnet_id: String,
    /// The endpoint id reported in route tables.
    pub endpoint_id: String,
    /// An `AttachmentStatus` member.
    pub attachment_status: String,
}

/// A firewall, as persisted. Every member `Firewall` / `FirewallStatus` reports
/// is read back from here.
#[derive(Debug, Clone)]
pub struct FirewallRecord {
    /// `Firewall.FirewallName`.
    pub firewall_name: String,
    /// `Firewall.FirewallArn`.
    pub firewall_arn: String,
    /// `Firewall.FirewallId`.
    pub firewall_id: String,
    /// `Firewall.FirewallPolicyArn`.
    pub firewall_policy_arn: String,
    /// `Firewall.VpcId`.
    pub vpc_id: String,
    /// `Firewall.SubnetMappings`, in the order the create request gave them.
    pub subnet_mappings: Vec<SubnetMapping>,
    /// `Firewall.DeleteProtection`.
    pub delete_protection: bool,
    /// `Firewall.SubnetChangeProtection`.
    pub subnet_change_protection: bool,
    /// `Firewall.FirewallPolicyChangeProtection`.
    pub firewall_policy_change_protection: bool,
    /// `Firewall.Description`.
    pub description: Option<String>,
    /// `Firewall.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `Firewall.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `Firewall.EnabledAnalysisTypes`.
    pub enabled_analysis_types: Option<Vec<String>>,
    /// A `FirewallStatusValue` member.
    pub status: String,
    /// A `ConfigurationSyncState` member.
    pub config_sync_state: String,
    /// `FirewallStatus.SyncStates`, keyed by Availability Zone.
    pub sync_states: HashMap<String, SyncStateRecord>,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// When `CreateFirewall` stored this record, and so the instant the clock
    /// measures this firewall's provisioning from (see [`FirewallRecord::ready_at`]).
    pub created_at: SystemTime,
}

impl FirewallRecord {
    /// When the clock owes this firewall its `PROVISIONING` -> `READY` move:
    /// [`FIREWALL_PROVISIONING_SPAN`] after the create that started it.
    pub fn ready_at(&self) -> SystemTime {
        self.created_at + FIREWALL_PROVISIONING_SPAN
    }

    /// Apply the status move the clock already made due, and say whether
    /// anything changed.
    ///
    /// A reporting operation calls this so a caller that waits out
    /// [`FIREWALL_PROVISIONING_SPAN`] sees `READY` — the transition was owed at
    /// [`FirewallRecord::ready_at`] whether or not anybody asked.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status != FIREWALL_PROVISIONING || now < self.ready_at() {
            return false;
        }
        self.status = FIREWALL_READY.to_string();
        for sync_state in self.sync_states.values_mut() {
            sync_state.attachment_status = ATTACHMENT_READY.to_string();
        }
        true
    }

    /// The `Firewall` shape for this record.
    pub fn to_firewall(&self) -> Firewall {
        Firewall {
            firewall_name: Some(self.firewall_name.clone()),
            firewall_arn: Some(self.firewall_arn.clone()),
            firewall_policy_arn: self.firewall_policy_arn.clone(),
            vpc_id: self.vpc_id.clone(),
            subnet_mappings: self.subnet_mappings.clone(),
            delete_protection: self.delete_protection,
            subnet_change_protection: self.subnet_change_protection,
            firewall_policy_change_protection: self.firewall_policy_change_protection,
            description: self.description.clone(),
            firewall_id: self.firewall_id.clone(),
            tags: self.tags.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            enabled_analysis_types: self.enabled_analysis_types.clone(),
            // `NumberOfAssociations` counts the `VpcEndpointAssociation`
            // resources that use this firewall. That resource is not implemented
            // (its operations still answer 501), so there is no count to report
            // and the member is left absent rather than filled with a guess.
            ..Firewall::default()
        }
    }

    /// The `FirewallStatus` shape for this record.
    pub fn to_firewall_status(&self) -> FirewallStatus {
        let sync_states = self
            .sync_states
            .iter()
            .map(|(zone, endpoint)| {
                let config = HashMap::from([(
                    self.firewall_policy_arn.clone(),
                    PerObjectStatus {
                        sync_status: Some(self.config_sync_state.clone()),
                        update_token: Some(self.update_token.clone()),
                    },
                )]);
                let sync_state = SyncState {
                    attachment: Some(Attachment {
                        subnet_id: Some(endpoint.subnet_id.clone()),
                        endpoint_id: Some(endpoint.endpoint_id.clone()),
                        status: Some(endpoint.attachment_status.clone()),
                        ..Attachment::default()
                    }),
                    config: Some(config),
                    ..SyncState::default()
                };
                (zone.clone(), sync_state)
            })
            .collect();
        FirewallStatus {
            status: self.status.clone(),
            configuration_sync_state_summary: self.config_sync_state.clone(),
            sync_states: Some(sync_states),
            ..FirewallStatus::default()
        }
    }

    /// The `FirewallMetadata` entry `ListFirewalls` reports for this record.
    pub fn to_metadata(&self) -> FirewallMetadata {
        FirewallMetadata {
            firewall_name: Some(self.firewall_name.clone()),
            firewall_arn: Some(self.firewall_arn.clone()),
            ..FirewallMetadata::default()
        }
    }
}

/// A firewall policy, as persisted.
#[derive(Debug, Clone)]
pub struct FirewallPolicyRecord {
    /// `FirewallPolicyResponse.FirewallPolicyName`.
    pub policy_name: String,
    /// `FirewallPolicyResponse.FirewallPolicyArn`.
    pub policy_arn: String,
    /// `FirewallPolicyResponse.FirewallPolicyId`.
    pub policy_id: String,
    /// The policy document itself, as `DescribeFirewallPolicy` returns it.
    pub policy: FirewallPolicy,
    /// `FirewallPolicyResponse.Description`.
    pub description: Option<String>,
    /// `FirewallPolicyResponse.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `FirewallPolicyResponse.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// A `ResourceStatus` member.
    pub status: String,
    /// How many firewalls are associated with this policy.
    pub number_of_associations: i32,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// `FirewallPolicyResponse.LastModifiedTime`.
    pub last_modified_time: SystemTime,
}

impl FirewallPolicyRecord {
    /// The `FirewallPolicyResponse` shape for this record.
    pub fn to_response(&self) -> FirewallPolicyResponse {
        FirewallPolicyResponse {
            firewall_policy_name: self.policy_name.clone(),
            firewall_policy_arn: self.policy_arn.clone(),
            firewall_policy_id: self.policy_id.clone(),
            description: self.description.clone(),
            firewall_policy_status: Some(self.status.clone()),
            tags: self.tags.clone(),
            number_of_associations: Some(self.number_of_associations),
            encryption_configuration: self.encryption_configuration.clone(),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            // The Consumed*Capacity members count capacity units drawn by the
            // rule groups the policy references. Rule groups are not implemented
            // (their operations still answer 501), so there is nothing to total
            // and the members are left absent rather than filled with a guess.
            ..FirewallPolicyResponse::default()
        }
    }

    /// The `FirewallPolicyMetadata` entry `ListFirewallPolicies` reports.
    pub fn to_metadata(&self) -> FirewallPolicyMetadata {
        FirewallPolicyMetadata {
            name: Some(self.policy_name.clone()),
            arn: Some(self.policy_arn.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-networkfirewall-networkfirewall-20201112. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkfirewallNetworkfirewall20201112State {
    /// Every created-and-not-deleted firewall, keyed by `FirewallName`.
    pub firewalls: HashMap<String, FirewallRecord>,
    /// `FirewallArn` -> `FirewallName`, so an operation resolves by either.
    pub firewall_arn_index: HashMap<String, String>,
    /// Every created-and-not-deleted policy, keyed by `FirewallPolicyName`.
    pub firewall_policies: HashMap<String, FirewallPolicyRecord>,
    /// `FirewallPolicyArn` -> `FirewallPolicyName`.
    pub firewall_policy_arn_index: HashMap<String, String>,
}

impl ComAmazonawsNetworkfirewallNetworkfirewall20201112State {
    /// The `FirewallName` the given name and/or ARN identifies.
    pub fn resolve_firewall(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, ResolveError> {
        resolve(&self.firewall_arn_index, name, arn, |key| {
            self.firewalls.contains_key(key)
        })
    }

    /// The `FirewallPolicyName` the given name and/or ARN identifies.
    pub fn resolve_firewall_policy(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, ResolveError> {
        resolve(&self.firewall_policy_arn_index, name, arn, |key| {
            self.firewall_policies.contains_key(key)
        })
    }
}

/// The shared name-or-ARN resolution rule: an ARN resolves through the index, a
/// name is used directly, both must agree, and the result must still exist.
fn resolve(
    arn_index: &HashMap<String, String>,
    name: Option<&str>,
    arn: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Result<String, ResolveError> {
    let from_arn = match arn {
        Some(arn) => Some(arn_index.get(arn).cloned().ok_or(ResolveError::NotFound)?),
        None => None,
    };
    let key = match (name, from_arn) {
        (Some(name), Some(from_arn)) if name != from_arn => return Err(ResolveError::Mismatch),
        (Some(name), _) => name.to_string(),
        (None, Some(from_arn)) => from_arn,
        (None, None) => return Err(ResolveError::NoIdentifier),
    };
    if exists(&key) {
        Ok(key)
    } else {
        Err(ResolveError::NotFound)
    }
}

/// Generate one operation's [`ResolveError`] -> modelled-error mapping.
///
/// The handlers that resolve a resource by name or ARN each own a DIFFERENT error
/// enum (`#[operation_error]` admits only the variants that operation's model
/// declares), so the mapping cannot be a single function. This macro keeps it a
/// single piece of wording: a request naming no resource at all, or naming two
/// different ones, is a bad request; one naming a resource that is not stored is
/// a miss.
///
/// Internal to this crate: `pub(crate) use` below keeps it off the crate's
/// public surface while still letting `src/ops/*` reach it by path.
macro_rules! impl_resolve_error {
    (
        $mapper:ident -> $error:ty, $modeled:ident,
        resource = $resource:literal, plural = $plural:literal,
        name_member = $name_member:literal, arn_member = $arn_member:literal
    ) => {
        /// Map a name/ARN resolution failure onto this operation's modelled errors.
        fn $mapper(error: $crate::state::ResolveError) -> $error {
            match error {
                $crate::state::ResolveError::NotFound => $modeled::ResourceNotFoundException(
                    $crate::ops::types::ResourceNotFoundException {
                        message: ::std::option::Option::Some(
                            concat!("The ", $resource, " does not exist.").to_string(),
                        ),
                    },
                ),
                $crate::state::ResolveError::NoIdentifier => $modeled::InvalidRequestException(
                    $crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            concat!(
                                "You must specify either ",
                                $name_member,
                                " or ",
                                $arn_member,
                                "."
                            )
                            .to_string(),
                        ),
                    },
                ),
                $crate::state::ResolveError::Mismatch => $modeled::InvalidRequestException(
                    $crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            concat!(
                                $name_member,
                                " and ",
                                $arn_member,
                                " identify different ",
                                $plural,
                                "."
                            )
                            .to_string(),
                        ),
                    },
                ),
            }
            .into()
        }
    };
}

pub(crate) use impl_resolve_error;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkfirewallNetworkfirewall20201112State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkfirewallNetworkfirewall20201112State>)
}
