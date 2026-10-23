// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkfirewall-networkfirewall-20201112 service.
//!
//! This module owns the service's persisted records and every projection from a
//! record onto a modelled shape, so each `src/ops/` handler only resolves,
//! mutates and projects — it never re-derives an ARN, an id or a status token.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Address, AnalysisResult, Attachment, EncryptionConfiguration, Firewall, FirewallMetadata,
    FirewallPolicy, FirewallPolicyMetadata, FirewallPolicyResponse, FirewallStatus, MatchAttributes,
    PerObjectStatus, PortRange, RuleGroup, RuleGroupResponse, SourceMetadata, SubnetMapping,
    SummaryConfiguration, SyncState, Tag,
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

/// `RuleGroupType` members (`smithy_query(query="shape",shape="RuleGroupType")`).
pub const RULE_GROUP_STATELESS: &str = "STATELESS";
/// See [`RULE_GROUP_STATELESS`].
pub const RULE_GROUP_STATEFUL: &str = "STATEFUL";
/// See [`RULE_GROUP_STATELESS`].
pub const RULE_GROUP_STATEFUL_DOMAIN: &str = "STATEFUL_DOMAIN";

/// `EncryptionType` members (`smithy_query(query="shape",shape="EncryptionType")`).
pub const ENCRYPTION_AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// `IdentifiedType` members (`smithy_query(query="shape",shape="IdentifiedType")`).
pub const IDENTIFIED_FORWARDING_ASYMMETRICALLY: &str = "STATELESS_RULE_FORWARDING_ASYMMETRICALLY";
/// See [`IDENTIFIED_FORWARDING_ASYMMETRICALLY`].
pub const IDENTIFIED_CONTAINS_TCP_FLAGS: &str = "STATELESS_RULE_CONTAINS_TCP_FLAGS";

/// The stateless standard actions that forward a packet on rather than dropping
/// it, and so are the ones the stateless analyzer looks at
/// (`RuleDefinition$Actions`: "aws:pass ... permits it to go to its intended
/// destination", "aws:forward_to_sfe ... forwards it to the stateful rule engine").
pub const STATELESS_FORWARDING_ACTIONS: [&str; 2] = ["aws:pass", "aws:forward_to_sfe"];

/// The encryption settings a resource reports, given what the request asked for.
///
/// `EncryptionConfiguration`'s own documentation states the default: "Your data
/// is encrypted by default with an Amazon Web Services owned key that Amazon Web
/// Services owns and manages for you", so an absent member means the AWS-owned
/// key — never "no encryption settings at all".
///
/// Both members are then reported: `KeyId` names the key protecting the
/// resource, and the identifier of the AWS-owned key is the
/// [`ENCRYPTION_AWS_OWNED_KMS_KEY`] token itself (the model does not state this,
/// so it is RECORDED behaviour: every recorded response for a resource created
/// without an `EncryptionConfiguration` carries
/// `{Type: "AWS_OWNED_KMS_KEY", KeyId: "AWS_OWNED_KMS_KEY"}`). A customer
/// managed key keeps the identifier the request gave, which is the only key id
/// the service can report for it.
pub fn effective_encryption_configuration(
    requested: Option<EncryptionConfiguration>,
) -> EncryptionConfiguration {
    let mut effective = requested.unwrap_or_else(|| EncryptionConfiguration {
        key_id: None,
        r#type: ENCRYPTION_AWS_OWNED_KMS_KEY.to_string(),
    });
    if effective.r#type == ENCRYPTION_AWS_OWNED_KMS_KEY && effective.key_id.is_none() {
        effective.key_id = Some(ENCRYPTION_AWS_OWNED_KMS_KEY.to_string());
    }
    effective
}

/// The tag collection a response reports for a resource the call just REMOVED
/// from state: the stored list, empty when the resource carries no tags.
///
/// A delete answers from the record it removed rather than echoing a request, so
/// every collection on that record is projected — a tag list with no entries is
/// reported as an empty list, not as an absent member. RECORDED behaviour: the
/// `DeleteFirewallPolicy` response for a policy created with no `Tags` carries
/// `Tags: []`, where the same policy's create response (which echoes the
/// request's own optional `Tags` member) carries none.
pub fn removed_resource_tags(stored: Option<Vec<Tag>>) -> Option<Vec<Tag>> {
    Some(stored.unwrap_or_default())
}

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

/// How many rule groups this emulator keeps. See [`MAX_STORED_FIREWALLS`].
pub const MAX_STORED_RULE_GROUPS: usize = 1_000;

/// The highest `Capacity` a rule group may be created with here.
///
/// CHOSEN, not an AWS quota: the model puts NO range on `RuleCapacity`, and a
/// rule group's retained rules are bounded only by the capacity they consume
/// (`required_capacity` must not exceed `Capacity`), so capping the capacity is
/// what keeps one request from making a single retained record arbitrarily
/// large. It is far above any rule group an emulator serves, and a request past
/// it is refused with the modelled `LimitExceededException` ("Unable to perform
/// the operation because doing so would violate a limit setting").
pub const MAX_RULE_GROUP_CAPACITY: i32 = 30_000;

/// How many entries any one list inside a retained `FirewallPolicy` document may
/// carry.
///
/// CHOSEN, not an AWS quota: the model puts NO length constraint on the policy's
/// rule-group reference, default-action or custom-action lists, so one request
/// could otherwise make a single retained record arbitrarily large. AWS bounds
/// the same thing by rule CAPACITY (the `Consumed*RuleCapacity` members), which
/// this emulator totals from the rule groups a policy references (see
/// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::consumed_policy_capacity`]);
/// this entry count bounds the lists that carry no capacity of their own and is
/// far above any real policy. A request past it is refused with the modelled
/// `LimitExceededException`.
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

/// How many resource policies this emulator keeps.
///
/// A resource policy exists only for a resource that is still stored — the
/// attach path refuses an ARN that names none, and deleting a resource detaches
/// its policy — so this is exactly the number of resources that can carry one. A
/// request past it is refused rather than retained, so no sequence of calls can
/// grow the store beyond it even if that invariant is ever broken.
pub const MAX_STORED_RESOURCE_POLICIES: usize =
    MAX_STORED_FIREWALL_POLICIES + MAX_STORED_RULE_GROUPS;

/// The `RulesSource` member `group`'s rules were given in, for an error message
/// that names the offending part of the request. `rule_group_document_breach`
/// has already refused a document that does not set exactly one of them.
pub fn rules_source_member(group: &RuleGroup) -> &'static str {
    let source = &group.rules_source;
    if source.stateless_rules_and_custom_actions.is_some() {
        "StatelessRulesAndCustomActions"
    } else if source.stateful_rules.is_some() {
        "StatefulRules"
    } else if source.rules_source_list.is_some() {
        "RulesSourceList"
    } else {
        "RulesString"
    }
}

/// Why `group` is not a rule group document of type `group_type`, or `None` when
/// it is one.
///
/// Two documented rules: "Each rule group requires a single `RulesSource`. You
/// can use an instance of this for either stateless rules or stateful rules"
/// (`RulesSource`), so exactly one of the four `RulesSource` members is set and
/// the one that is set must belong to the half the `Type` names; and
/// "<RuleVariables> ... You can only use these for stateful rule groups"
/// (`RuleGroup$RuleVariables`).
pub fn rule_group_document_breach(group_type: &str, group: &RuleGroup) -> Option<String> {
    let source = &group.rules_source;
    let stateless = source.stateless_rules_and_custom_actions.is_some();
    let members = [
        ("RulesString", source.rules_string.is_some()),
        ("RulesSourceList", source.rules_source_list.is_some()),
        ("StatefulRules", source.stateful_rules.is_some()),
        ("StatelessRulesAndCustomActions", stateless),
    ];
    let given: Vec<&str> = members
        .into_iter()
        .filter_map(|(member, present)| present.then_some(member))
        .collect();
    if given.len() != 1 {
        return Some(format!(
            "RuleGroup.RulesSource must specify exactly one of RulesString, RulesSourceList, \
             StatefulRules or StatelessRulesAndCustomActions; this request specified {}.",
            if given.is_empty() {
                "none of them".to_string()
            } else {
                given.join(", ")
            }
        ));
    }
    if (group_type == RULE_GROUP_STATELESS) != stateless {
        return Some(format!(
            "A {group_type} rule group's RuleGroup.RulesSource must specify {}.",
            if group_type == RULE_GROUP_STATELESS {
                "StatelessRulesAndCustomActions"
            } else {
                "RulesString, RulesSourceList or StatefulRules"
            }
        ));
    }
    if group_type == RULE_GROUP_STATELESS && group.rule_variables.is_some() {
        return Some(
            "RuleGroup.RuleVariables can only be used in a stateful rule group.".to_string(),
        );
    }
    None
}

/// The actions a resource policy on a shared RULE GROUP may grant, from the AWS
/// `PutResourcePolicy` documentation's list of operations that may appear in the
/// statement for a rule group resource.
pub const RULE_GROUP_POLICY_ACTIONS: [&str; 3] = [
    "network-firewall:ListRuleGroups",
    "network-firewall:CreateFirewallPolicy",
    "network-firewall:UpdateFirewallPolicy",
];

/// The actions a resource policy on a shared FIREWALL POLICY may grant. See
/// [`RULE_GROUP_POLICY_ACTIONS`].
pub const FIREWALL_POLICY_POLICY_ACTIONS: [&str; 2] = [
    "network-firewall:AssociateFirewallPolicy",
    "network-firewall:ListFirewallPolicies",
];

/// The two resource kinds Network Firewall can share through a resource policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedResource {
    /// A rule group, named by a `*-rulegroup/<name>` ARN.
    RuleGroup,
    /// A firewall policy, named by a `firewall-policy/<name>` ARN.
    FirewallPolicy,
}

impl SharedResource {
    /// The fixed action set a policy on this kind of resource may grant.
    pub fn permitted_actions(self) -> &'static [&'static str] {
        match self {
            Self::RuleGroup => &RULE_GROUP_POLICY_ACTIONS,
            Self::FirewallPolicy => &FIREWALL_POLICY_POLICY_ACTIONS,
        }
    }
}

/// What `arn` names, or `None` when it names a resource kind that cannot carry a
/// resource policy (a firewall, say).
pub fn shared_resource_kind(arn: &str) -> Option<SharedResource> {
    let resource = arn.split(':').next_back()?;
    let (resource_type, _) = resource.split_once('/')?;
    match resource_type {
        "firewall-policy" => Some(SharedResource::FirewallPolicy),
        "stateless-rulegroup" | "stateful-rulegroup" | "stateful-domain-rulegroup" => {
            Some(SharedResource::RuleGroup)
        }
        _ => None,
    }
}

/// Why `document` is not a resource policy this service accepts for a resource of
/// `kind`, or `None` when it validates.
///
/// A Network Firewall resource policy is validated against the RAM managed
/// permission for the shared resource's type, so the actions it grants must be
/// exactly that type's fixed set ([`SharedResource::permitted_actions`]): an
/// action outside the set and a missing one are the same failure, which is what
/// the service's "The supplied policy does not match RAM managed permissions."
/// rejection says. SET EQUALITY, not containment, is the rule here because the
/// document is compared with a managed permission rather than merely checked for
/// the permissions it carries: a subset does not match that permission, and an
/// action outside the set is one the documentation does not allow the statement
/// to name. The document itself must also be a policy: a JSON object with
/// at least one `Allow` statement that names a principal and an action.
pub fn resource_policy_breach(document: &str, kind: SharedResource) -> Option<String> {
    let parsed: ::serde_json::Value = match ::serde_json::from_str(document) {
        Ok(parsed) => parsed,
        Err(error) => return Some(format!("The policy is not valid JSON: {error}")),
    };
    if !parsed.is_object() {
        return Some("The policy must be a JSON object.".to_string());
    }
    let statements: Vec<&::serde_json::Value> = match parsed.get("Statement") {
        Some(::serde_json::Value::Array(statements)) => statements.iter().collect(),
        Some(statement) if statement.is_object() => vec![statement],
        _ => return Some("The policy must carry a Statement.".to_string()),
    };
    if statements.is_empty() {
        return Some("The policy's Statement must carry at least one statement.".to_string());
    }

    let mut granted: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for statement in statements {
        match statement.get("Effect").and_then(::serde_json::Value::as_str) {
            Some("Allow") => {}
            _ => {
                return Some(
                    "Every statement in the policy must have the Effect Allow.".to_string(),
                )
            }
        }
        if statement.get("Principal").is_none() {
            return Some("Every statement in the policy must name a Principal.".to_string());
        }
        match statement.get("Action") {
            Some(::serde_json::Value::String(action)) => {
                granted.insert(action.clone());
            }
            Some(::serde_json::Value::Array(actions)) if !actions.is_empty() => {
                for action in actions {
                    match action.as_str() {
                        Some(action) => {
                            granted.insert(action.to_string());
                        }
                        None => {
                            return Some(
                                "Every entry of a statement's Action must be a string.".to_string(),
                            )
                        }
                    }
                }
            }
            _ => return Some("Every statement in the policy must name an Action.".to_string()),
        }
    }

    let permitted = kind.permitted_actions();
    let unexpected: Vec<&str> = granted
        .iter()
        .map(String::as_str)
        .filter(|action| !permitted.contains(action))
        .collect();
    let missing: Vec<&str> = permitted
        .iter()
        .copied()
        .filter(|action| !granted.contains(*action))
        .collect();
    if unexpected.is_empty() && missing.is_empty() {
        return None;
    }
    Some(format!(
        "The supplied policy does not match RAM managed permissions. \
         Unexpected actions: [{}]; missing actions: [{}].",
        unexpected.join(", "),
        missing.join(", ")
    ))
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

/// The resource part of a rule group's ARN, and therefore the key its record is
/// stored under: `stateless-rulegroup/<name>` for a `STATELESS` rule group and
/// `stateful-rulegroup/<name>` for every stateful one.
///
/// The type is part of the resource path because `DescribeRuleGroup$Type` is
/// "required for requests that do not include the `RuleGroupARN`" — the ARN
/// alone identifies a rule group, so it must carry what the name alone does not.
/// Two rule groups therefore collide exactly when they share this path, which is
/// what makes a name reusable across the stateless and stateful halves of the
/// service.
pub fn rule_group_resource_path(group_type: &str, name: &str) -> String {
    let kind = if group_type == RULE_GROUP_STATELESS {
        "stateless-rulegroup"
    } else {
        "stateful-rulegroup"
    };
    format!("{kind}/{name}")
}

/// `arn:aws:network-firewall:<region>:<account>:<stateful|stateless>-rulegroup/<name>`.
pub fn rule_group_arn(region: &str, group_type: &str, name: &str) -> String {
    emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &rule_group_resource_path(group_type, name),
    )
}

/// The capacity the rules in `group` consume, by the calculation
/// `CreateRuleGroupRequest$Capacity` documents.
///
/// For a stateless rule group it is "the sum of the capacity requirements of the
/// individual rules", where one rule's requirement is the product of its match
/// settings' values and a setting that is absent (which matches anything) counts
/// as 1. For a stateful rule group it is "the number of individual rules": one
/// per `StatefulRules` entry, one per rule line of a Suricata `RulesString`, or
/// one per domain in a `RulesSourceList`.
pub fn required_capacity(group_type: &str, group: &RuleGroup) -> i32 {
    let source = &group.rules_source;
    let total: i64 = if group_type == RULE_GROUP_STATELESS {
        source
            .stateless_rules_and_custom_actions
            .as_ref()
            .map(|stateless| {
                stateless
                    .stateless_rules
                    .iter()
                    .map(|rule| {
                        let attributes = &rule.rule_definition.match_attributes;
                        let settings = [
                            attributes.sources.as_ref().map_or(0, Vec::len),
                            attributes.destinations.as_ref().map_or(0, Vec::len),
                            attributes.source_ports.as_ref().map_or(0, Vec::len),
                            attributes.destination_ports.as_ref().map_or(0, Vec::len),
                            attributes.protocols.as_ref().map_or(0, Vec::len),
                            attributes.tcp_flags.as_ref().map_or(0, Vec::len),
                        ];
                        settings
                            .into_iter()
                            .map(|elements| i64::try_from(elements.max(1)).unwrap_or(i64::MAX))
                            .fold(1i64, i64::saturating_mul)
                    })
                    .fold(0i64, i64::saturating_add)
            })
            .unwrap_or_default()
    } else {
        let from_rules = source.stateful_rules.as_ref().map_or(0, Vec::len);
        let from_string = source
            .rules_string
            .as_deref()
            .map_or(0, |rules| suricata_rule_lines(rules).count());
        let from_domains = source
            .rules_source_list
            .as_ref()
            .map_or(0, |list| list.targets.len());
        i64::try_from(from_rules + from_string + from_domains).unwrap_or(i64::MAX)
    };
    i32::try_from(total).unwrap_or(i32::MAX)
}

/// The individual rules in a Suricata flat-format rules string: one per line,
/// skipping blank lines and `#` comments, which carry no rule.
pub fn suricata_rule_lines(rules: &str) -> impl Iterator<Item = &str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// The stateless rule-group analyzer's findings for `group`.
///
/// `AnalysisResult` documents the analyzer as running over STATELESS rule groups
/// and `IdentifiedType` documents exactly the two configurations it reports, each
/// for rules "with the action `pass` or `forward`":
///
/// * `STATELESS_RULE_FORWARDING_ASYMMETRICALLY` — "the rule's set of source IP
///   addresses or their associated port numbers, don't match the set of
///   destination IP addresses or their associated port numbers", mitigated by
///   there being "an existing return path". A rule is therefore reported unless
///   some forwarding rule in the group matches the mirrored traffic (its sources
///   and ports are this rule's destinations and ports, and vice versa); a rule
///   that names no addresses or ports matches anything in both directions and so
///   mirrors itself.
/// * `STATELESS_RULE_CONTAINS_TCP_FLAGS` — a forwarding rule "contains TCP
///   flags", whose mitigation is to "remove unnecessary TCP flag inspections
///   from the rules".
///
/// `IdentifiedRuleIds` is "the priority number of the stateless rules identified
/// in the analysis", so each finding lists the priorities it was raised for, in
/// the order the rules were given.
///
/// The pass is linear in the number of rules (the return paths are indexed once)
/// and the caller runs it OUTSIDE the state lock, so one request's analysis
/// neither scales faster than its own input nor blocks another caller's.
pub fn analyze_stateless_rule_group(group: &RuleGroup) -> Vec<AnalysisResult> {
    let Some(stateless) = group.rules_source.stateless_rules_and_custom_actions.as_ref() else {
        return Vec::new();
    };
    let forwarding: Vec<_> = stateless
        .stateless_rules
        .iter()
        .filter(|rule| {
            rule.rule_definition
                .actions
                .iter()
                .any(|action| STATELESS_FORWARDING_ACTIONS.contains(&action.as_str()))
        })
        .collect();

    // Indexed once, so looking a rule's return path up is a hash lookup rather
    // than a scan of the other rules: the whole pass costs one unit of work per
    // rule, whatever a caller sends.
    let directions: Vec<RuleDirection> = forwarding
        .iter()
        .map(|rule| RuleDirection::of(&rule.rule_definition.match_attributes))
        .collect();
    let indexed: HashSet<&RuleDirection> = directions.iter().collect();

    let mut asymmetric = Vec::new();
    let mut with_tcp_flags = Vec::new();
    for (rule, direction) in forwarding.iter().zip(&directions) {
        if !indexed.contains(&direction.reversed()) {
            asymmetric.push(rule.priority.to_string());
        }
        if rule
            .rule_definition
            .match_attributes
            .tcp_flags
            .as_ref()
            .is_some_and(|flags| !flags.is_empty())
        {
            with_tcp_flags.push(rule.priority.to_string());
        }
    }

    let mut results = Vec::new();
    if !asymmetric.is_empty() {
        results.push(AnalysisResult {
            identified_rule_ids: Some(asymmetric),
            identified_type: Some(IDENTIFIED_FORWARDING_ASYMMETRICALLY.to_string()),
            analysis_detail: Some(
                "The rule forwards traffic asymmetrically: no rule in this rule group \
                 matches the return traffic for the addresses and ports it matches."
                    .to_string(),
            ),
        });
    }
    if !with_tcp_flags.is_empty() {
        results.push(AnalysisResult {
            identified_rule_ids: Some(with_tcp_flags),
            identified_type: Some(IDENTIFIED_CONTAINS_TCP_FLAGS.to_string()),
            analysis_detail: Some(
                "The rule inspects TCP flags, which are not consistent in the forward and \
                 return directions of a connection."
                    .to_string(),
            ),
        });
    }
    results
}

/// The traffic one stateless rule matches, as comparable sets.
///
/// Two rules describe the same traffic when their sets are equal whatever order
/// the match settings were written in, and a rule's return path is this with the
/// source and destination halves swapped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuleDirection {
    /// `MatchAttributes.Sources`, as address definitions.
    sources: Vec<String>,
    /// `MatchAttributes.Destinations`, as address definitions.
    destinations: Vec<String>,
    /// `MatchAttributes.SourcePorts`, as `from:to` ranges.
    source_ports: Vec<String>,
    /// `MatchAttributes.DestinationPorts`, as `from:to` ranges.
    destination_ports: Vec<String>,
}

impl RuleDirection {
    /// The traffic `attributes` matches.
    fn of(attributes: &MatchAttributes) -> Self {
        Self {
            sources: address_set(attributes.sources.as_ref()),
            destinations: address_set(attributes.destinations.as_ref()),
            source_ports: port_set(attributes.source_ports.as_ref()),
            destination_ports: port_set(attributes.destination_ports.as_ref()),
        }
    }

    /// The same traffic flowing the other way: the return path a symmetric rule
    /// group also matches.
    fn reversed(&self) -> Self {
        Self {
            sources: self.destinations.clone(),
            destinations: self.sources.clone(),
            source_ports: self.destination_ports.clone(),
            destination_ports: self.source_ports.clone(),
        }
    }
}

/// The address definitions of a match setting as a set, so two settings compare
/// equal when they match the same addresses whatever order they were given in.
/// An absent setting is the empty set, which is how "matches with any address" is
/// written.
fn address_set(addresses: Option<&Vec<Address>>) -> Vec<String> {
    let mut definitions: Vec<String> = addresses
        .map(|addresses| {
            addresses
                .iter()
                .map(|address| address.address_definition.clone())
                .collect()
        })
        .unwrap_or_default();
    definitions.sort();
    definitions.dedup();
    definitions
}

/// The port ranges of a match setting as a set. See [`address_set`].
fn port_set(ports: Option<&Vec<PortRange>>) -> Vec<String> {
    let mut ranges: Vec<String> = ports
        .map(|ports| {
            ports
                .iter()
                .map(|range| format!("{}:{}", range.from_port, range.to_port))
                .collect()
        })
        .unwrap_or_default();
    ranges.sort();
    ranges.dedup();
    ranges
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

/// Why a `RuleGroupName`/`Type`/`RuleGroupArn` triple did not name a stored rule
/// group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleGroupLookupError {
    /// A name was given without a `Type` and without an ARN, so the request
    /// names at most half of a rule group's identity.
    TypeRequired,
    /// The triple was well formed but did not resolve.
    Resolve(ResolveError),
}

/// What a request that gives a rule group's name but neither its `Type` nor its
/// ARN is refused with: `DescribeRuleGroup$Type` (and the same member on every
/// other rule-group operation) is "required for requests that do not include the
/// RuleGroupARN".
pub const TYPE_REQUIRED_MESSAGE: &str =
    "Type is required for requests that do not include the RuleGroupArn.";

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
    ///
    /// `consumed` is the capacity the rule groups this policy references reserve;
    /// it is read from the rule-group store, so it is passed in rather than held
    /// on the record (see
    /// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::consumed_policy_capacity`]).
    pub fn to_response(&self, consumed: ConsumedPolicyCapacity) -> FirewallPolicyResponse {
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
            consumed_stateless_rule_capacity: Some(consumed.stateless),
            consumed_stateful_rule_capacity: Some(consumed.stateful),
            consumed_stateful_domain_capacity: Some(consumed.stateful_domain),
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

/// The capacity the rule groups a firewall policy references reserve, as the
/// `FirewallPolicyResponse` `Consumed*` members report it.
///
/// `CreateRuleGroupRequest$Capacity` documents the reservation: "When you
/// reference a rule group from a firewall policy, Network Firewall reserves this
/// capacity for the rule group."
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConsumedPolicyCapacity {
    /// `ConsumedStatelessRuleCapacity`.
    pub stateless: i32,
    /// `ConsumedStatefulRuleCapacity`.
    pub stateful: i32,
    /// `ConsumedStatefulDomainCapacity` — "the total number of domain name
    /// specifications across all domain list rule groups in the firewall policy
    /// that use the `stateful-domain-rulegroup` resource type", which is the
    /// consumed (not the declared) capacity of those rule groups.
    pub stateful_domain: i32,
}

/// A rule group, as persisted. Every member `RuleGroupResponse` reports is read
/// back from here.
#[derive(Debug, Clone)]
pub struct RuleGroupRecord {
    /// `RuleGroupResponse.RuleGroupName`.
    pub rule_group_name: String,
    /// `RuleGroupResponse.RuleGroupArn`.
    pub rule_group_arn: String,
    /// `RuleGroupResponse.RuleGroupId`.
    pub rule_group_id: String,
    /// `RuleGroupResponse.Type`, a `RuleGroupType` member.
    pub rule_group_type: String,
    /// `RuleGroupResponse.Capacity`: fixed at creation.
    pub capacity: i32,
    /// The rule group document itself, as `DescribeRuleGroup` returns it.
    pub rule_group: RuleGroup,
    /// `RuleGroupResponse.Description`.
    pub description: Option<String>,
    /// `RuleGroupResponse.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `RuleGroupResponse.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `RuleGroupResponse.SourceMetadata`.
    pub source_metadata: Option<SourceMetadata>,
    /// `RuleGroupResponse.SummaryConfiguration`.
    pub summary_configuration: Option<SummaryConfiguration>,
    /// A `ResourceStatus` member.
    pub status: String,
    /// How many firewall policies use this rule group.
    pub number_of_associations: i32,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// `RuleGroupResponse.LastModifiedTime`.
    pub last_modified_time: SystemTime,
}

impl RuleGroupRecord {
    /// The `RuleGroupResponse` shape for this record.
    ///
    /// `AnalysisResults` is left to the caller: the analyzer runs only when the
    /// request sets `AnalyzeRuleGroup`, and its findings are computed from the
    /// rules rather than stored.
    pub fn to_response(&self) -> RuleGroupResponse {
        RuleGroupResponse {
            rule_group_name: self.rule_group_name.clone(),
            rule_group_arn: self.rule_group_arn.clone(),
            rule_group_id: self.rule_group_id.clone(),
            r#type: Some(self.rule_group_type.clone()),
            capacity: Some(self.capacity),
            consumed_capacity: Some(required_capacity(&self.rule_group_type, &self.rule_group)),
            description: self.description.clone(),
            rule_group_status: Some(self.status.clone()),
            tags: self.tags.clone(),
            number_of_associations: Some(self.number_of_associations),
            encryption_configuration: self.encryption_configuration.clone(),
            source_metadata: self.source_metadata.clone(),
            summary_configuration: self.summary_configuration.clone(),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            // `SnsTopic` records changes to a MANAGED rule group, which only
            // Amazon Web Services and its partners publish; a rule group created
            // through this API has no such topic, so the member stays absent.
            ..RuleGroupResponse::default()
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
    /// The resource policy attached to a shared rule group or firewall policy,
    /// keyed by that resource's ARN. An absent entry is what makes
    /// `DescribeResourcePolicy` answer `ResourceNotFoundException`.
    ///
    /// One entry can exist only for a resource this state still holds:
    /// `PutResourcePolicy` refuses an ARN that names no stored resource, and
    /// [`Self::detach_resource_policy`] drops the entry when that resource is
    /// deleted. The map therefore never holds more entries than
    /// [`MAX_STORED_FIREWALL_POLICIES`] + [`MAX_STORED_RULE_GROUPS`], which
    /// [`MAX_STORED_RESOURCE_POLICIES`] states as the ceiling the attach path
    /// checks.
    pub resource_policies: HashMap<String, String>,
    /// Every created-and-not-deleted rule group, keyed by the ARN resource path
    /// [`rule_group_resource_path`] builds, so the stateless and the stateful
    /// half of the service each have their own name space.
    pub rule_groups: HashMap<String, RuleGroupRecord>,
    /// `RuleGroupArn` -> the [`rule_groups`](Self::rule_groups) key.
    pub rule_group_arn_index: HashMap<String, String>,
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

    /// The [`rule_groups`](Self::rule_groups) key a request's `RuleGroupName`,
    /// `Type` and `RuleGroupArn` identify.
    ///
    /// A rule group's name is unique only within its type, so the store is keyed
    /// by the resource path [`rule_group_resource_path`] builds from both and
    /// `Type` "is required for requests that do not include the RuleGroupARN"
    /// (`DescribeRuleGroup$Type`). With an ARN in hand the name adds nothing the
    /// ARN does not already carry, so a typeless name is resolved through the ARN
    /// and then checked against the record rather than ignored.
    ///
    /// Every rule-group operation that takes this triple resolves it here, so one
    /// identification rule serves the read, the update and the delete alike.
    pub fn resolve_rule_group_request(
        &self,
        name: Option<&str>,
        group_type: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, RuleGroupLookupError> {
        let resource_path = match (name, group_type) {
            (Some(name), Some(group_type)) => Some(rule_group_resource_path(group_type, name)),
            (Some(_), None) if arn.is_none() => {
                return Err(RuleGroupLookupError::TypeRequired);
            }
            (_, _) => None,
        };
        let key = resolve(
            &self.rule_group_arn_index,
            resource_path.as_deref(),
            arn,
            |key| self.rule_groups.contains_key(key),
        )
        .map_err(RuleGroupLookupError::Resolve)?;
        if resource_path.is_none()
            && name.is_some_and(|name| {
                self.rule_groups
                    .get(&key)
                    .is_some_and(|record| record.rule_group_name != name)
            })
        {
            return Err(RuleGroupLookupError::Resolve(ResolveError::Mismatch));
        }
        Ok(key)
    }

    /// Why a `FirewallPolicy` document's rule-group references cannot be served,
    /// as a message naming the first offending reference, or `None` when every
    /// one of them resolves.
    ///
    /// Two documented rules, both of them `InvalidRequestException`'s ("Your
    /// request references an ARN that is malformed, or corresponds to a resource
    /// that isn't valid in the context of the request"):
    ///
    /// * a reference names a rule group that EXISTS — the reference is that rule
    ///   group's ARN (`StatelessRuleGroupReference$ResourceArn` is "the Amazon
    ///   Resource Name (ARN) of the stateless rule group"), so an ARN no stored
    ///   rule group carries names no resource at all; and
    /// * it names one of the KIND its list is for: `StatelessRuleGroupReferences`
    ///   are "references to the stateless rule groups that are used in the
    ///   policy" and a `StatefulRuleGroupReference$ResourceArn` is "the ARN of
    ///   the stateful rule group", so a stateless reference naming a stateful
    ///   rule group (or the reverse) is a resource that is not valid in the
    ///   context of the request.
    ///
    /// This reads the store, so a caller runs it in the same critical section it
    /// inserts the policy in: a reference cannot be resolved and then go stale
    /// before the record lands.
    pub fn policy_reference_breach(&self, policy: &FirewallPolicy) -> Option<String> {
        let stateless = policy
            .stateless_rule_group_references
            .iter()
            .flatten()
            .map(|reference| {
                (
                    "StatelessRuleGroupReferences",
                    &reference.resource_arn,
                    false,
                )
            });
        let stateful = policy
            .stateful_rule_group_references
            .iter()
            .flatten()
            .map(|reference| ("StatefulRuleGroupReferences", &reference.resource_arn, true));

        for (member, arn, wants_stateful) in stateless.chain(stateful) {
            let Some(record) = self
                .rule_group_arn_index
                .get(arn)
                .and_then(|key| self.rule_groups.get(key))
            else {
                return Some(format!(
                    "FirewallPolicy.{member} references a rule group that does not exist, \
                     parameter: [{arn}]"
                ));
            };
            // Every type other than STATELESS is a stateful rule group, which is
            // the same split the store's keys are built on.
            let is_stateful = record.rule_group_type != RULE_GROUP_STATELESS;
            if is_stateful != wants_stateful {
                return Some(format!(
                    "FirewallPolicy.{member} must reference a {} rule group, but [{arn}] is a \
                     {} rule group.",
                    if wants_stateful {
                        "stateful"
                    } else {
                        RULE_GROUP_STATELESS
                    },
                    record.rule_group_type
                ));
            }
        }
        None
    }

    /// Drop the resource policy attached to `resource_arn`, if any.
    ///
    /// Called wherever a shareable resource is removed: the policy is reachable
    /// only through that resource's own ARN, so an entry left behind would be
    /// unreadable state that nothing can ever delete.
    pub fn detach_resource_policy(&mut self, resource_arn: &str) {
        self.resource_policies.remove(resource_arn);
    }

    /// The capacity the rule groups `policy` references reserve. A reference to a
    /// rule group this emulator does not hold reserves nothing.
    ///
    /// The three members are deliberately NOT in one unit, because the model
    /// does not define them in one unit: `ConsumedStatelessRuleCapacity` and
    /// `ConsumedStatefulRuleCapacity` count the capacity a reference RESERVES, so
    /// a referenced rule group contributes its declared `Capacity`
    /// (`CreateRuleGroupRequest$Capacity`: "when you reference a rule group from
    /// a firewall policy, Network Firewall reserves this capacity for the rule
    /// group"), while `ConsumedStatefulDomainCapacity` is defined as a count of
    /// domain name specifications ("the total number of domain name
    /// specifications across all domain list rule groups in the firewall policy
    /// that use the `stateful-domain-rulegroup` resource type"), which is a
    /// domain-list group's CONSUMED capacity, not its declared one.
    ///
    /// A domain-list reference is therefore counted in the domain member ONLY,
    /// not in `stateful` as well: the model singles those rule groups out as a
    /// class of their own, measured in domain name specifications, so adding
    /// their declared capacity to the stateful total would report one reference
    /// under two members in two different units.
    pub fn consumed_policy_capacity(&self, policy: &FirewallPolicy) -> ConsumedPolicyCapacity {
        let mut consumed = ConsumedPolicyCapacity::default();
        for arn in policy_rule_group_references(policy) {
            let Some(record) = self
                .rule_group_arn_index
                .get(arn)
                .and_then(|key| self.rule_groups.get(key))
            else {
                continue;
            };
            match record.rule_group_type.as_str() {
                // Reserved: the declared capacity.
                RULE_GROUP_STATELESS => {
                    consumed.stateless = consumed.stateless.saturating_add(record.capacity);
                }
                // Counted in domain name specifications: the capacity the domain
                // list consumes.
                RULE_GROUP_STATEFUL_DOMAIN => {
                    consumed.stateful_domain = consumed.stateful_domain.saturating_add(
                        required_capacity(&record.rule_group_type, &record.rule_group),
                    );
                }
                // Reserved: the declared capacity.
                _ => {
                    consumed.stateful = consumed.stateful.saturating_add(record.capacity);
                }
            }
        }
        consumed
    }
}

/// Every rule-group ARN a firewall policy document references, stateless first.
fn policy_rule_group_references(policy: &FirewallPolicy) -> impl Iterator<Item = &str> {
    policy
        .stateless_rule_group_references
        .iter()
        .flatten()
        .map(|reference| reference.resource_arn.as_str())
        .chain(
            policy
                .stateful_rule_group_references
                .iter()
                .flatten()
                .map(|reference| reference.resource_arn.as_str()),
        )
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

/// Generate one rule-group operation's [`RuleGroupLookupError`] ->
/// modelled-error mapping, on top of the [`ResolveError`] mapper
/// [`impl_resolve_error`] generated for the same operation.
///
/// Every operation that identifies a rule group by `RuleGroupName`/`Type`/
/// `RuleGroupArn` shares both refusals — a name without a type or an ARN is a bad
/// request, everything else is the ordinary name/ARN resolution — and each still
/// owns a different error enum, so this is the same per-operation expansion.
///
/// Internal to this crate: `pub(crate) use` below keeps it off the crate's
/// public surface while still letting `src/ops/*` reach it by path.
macro_rules! impl_rule_group_lookup_error {
    ($mapper:ident -> $error:ty, $modeled:ident, resolve = $resolve_mapper:ident) => {
        /// Map a rule-group lookup failure onto this operation's modelled errors.
        fn $mapper(error: $crate::state::RuleGroupLookupError) -> $error {
            match error {
                $crate::state::RuleGroupLookupError::TypeRequired => {
                    <$error>::from($modeled::InvalidRequestException(
                        $crate::ops::types::InvalidRequestException {
                            message: ::std::option::Option::Some(
                                $crate::state::TYPE_REQUIRED_MESSAGE.to_string(),
                            ),
                        },
                    ))
                }
                $crate::state::RuleGroupLookupError::Resolve(error) => $resolve_mapper(error),
            }
        }
    };
}

pub(crate) use impl_rule_group_lookup_error;

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
