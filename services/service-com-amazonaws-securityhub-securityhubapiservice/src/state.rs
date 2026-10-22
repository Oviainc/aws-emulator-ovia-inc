// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityhub-securityhubapiservice service.
//!
//! Security Hub CSPM is subscribed per account **and** Region: `EnableSecurityHub`
//! creates the single `hub/default` resource for the caller's account/Region, and
//! `DisableSecurityHub` removes it together with the findings imported into it. The
//! state below therefore keys the hub on `(account id, Region)` and every finding on
//! `(account id, Region, ProductArn, Id)` — `ProductArn` + `Id` is the modelled
//! `AwsSecurityFindingIdentifier`, the identity `BatchUpdateFindings` addresses a
//! finding by, and the account/Region prefix is what makes "disable removes this
//! account's findings in this Region" expressible.
//!
//! A custom insight `CreateInsight` creates is keyed the same way — `(account id,
//! Region, InsightArn)` — because an insight is a saved query over exactly the
//! findings of the hub it was created in, so disabling the hub takes it with it.

use std::collections::{BTreeMap, HashMap};

use crate::ops::types::{
    AwsSecurityFinding, FindingHistoryUpdate, Severity, Standard, StandardsManagedBy,
};

/// The hub resource is one per account and Region, named `hub/default`.
pub type HubKey = (String, String);

/// A stored finding's identity: the hub it lives in, plus the modelled
/// `AwsSecurityFindingIdentifier` (`ProductArn` + `Id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FindingKey {
    pub account_id: String,
    pub region: String,
    pub product_arn: String,
    pub finding_id: String,
}

/// A member association's identity: the administrator hub it belongs to (the
/// account/Region pair that made the `CreateMembers` call), plus the member account
/// id — the identity every member operation addresses an association by.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemberKey {
    /// The administrator account the association belongs to.
    pub account_id: String,
    pub region: String,
    /// The member account id.
    pub member_account_id: String,
}

/// The member association `CreateMembers` creates and `GetMembers` reports.
#[derive(Debug, Clone)]
pub struct MemberAssociation {
    /// The email `AccountDetails$Email` carried, when the administrator supplied one.
    pub email: Option<String>,
    /// One of the documented `Member$MemberStatus` values.
    pub member_status: String,
    /// Set only once an invitation has been sent; `Member$InvitedAt` is "a timestamp
    /// for the date and time when the invitation was sent to the member account", so
    /// an association that was only created carries none.
    pub invited_at: Option<::aws_smithy_types::DateTime>,
    /// `Member$UpdatedAt`: when the member account was last updated.
    pub updated_at: ::aws_smithy_types::DateTime,
}

/// The `Hub` resource `EnableSecurityHub` creates and `DescribeHub` reports.
#[derive(Debug, Clone)]
pub struct Hub {
    pub hub_arn: String,
    /// ISO-8601 instant the account was subscribed; stable for the hub's lifetime.
    pub subscribed_at: String,
    pub auto_enable_controls: bool,
    pub control_finding_generator: String,
    pub enable_default_standards: bool,
    pub tags: HashMap<String, String>,
}

/// One finding change event, as `GetFindingHistory` reports it.
///
/// Appended by the two operations that write a finding — `BatchImportFindings` and
/// `BatchUpdateFindings`, the two values `FindingHistoryUpdateSourceType` models — so
/// the history is a record of writes that actually happened, never synthesized at
/// read time.
#[derive(Debug, Clone)]
pub struct FindingHistoryEvent {
    /// The instant the change was made, ISO-8601, as [`now_iso8601`] renders it.
    pub update_time: String,
    /// `True` only on the event that created the finding record.
    pub finding_created: bool,
    /// A `FindingHistoryUpdateSourceType`: which operation made the change.
    pub source_type: String,
    /// The identity that initiated the change: the product ARN for an import, the
    /// signing principal for an update.
    pub source_identity: String,
    /// The ASFF fields this event changed, with their old and new values.
    pub updates: Vec<FindingHistoryUpdate>,
    /// The write sequence of the change, the deterministic tie-breaker when two
    /// events carry the same `update_time`.
    pub seq: u64,
}

/// A custom action target's identity: the hub it lives in, plus the `Id` the
/// creator chose. `CreateActionTarget` documents `Id` as the identifier of the
/// custom action target, and the ARN it answers with is derived from it, so the two
/// cannot name different resources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionTargetKey {
    pub account_id: String,
    pub region: String,
    pub target_id: String,
}

/// A custom action target as `CreateActionTarget` stored it and
/// `DescribeActionTargets` reports it (the modelled `ActionTarget` members).
#[derive(Debug, Clone)]
pub struct ActionTargetRecord {
    pub action_target_arn: String,
    pub name: String,
    pub description: String,
}

/// A finding as `BatchImportFindings` stored it, mutated in place by
/// `BatchUpdateFindings`.
#[derive(Debug, Clone)]
pub struct StoredFinding {
    pub finding: AwsSecurityFinding,
    /// Monotonic sequence of the import that first created this record.
    pub imported_seq: u64,
    /// Monotonic sequence of the most recent write, used as the deterministic
    /// tie-breaker when two findings carry the same `UpdatedAt`.
    pub updated_seq: u64,
    /// The change events this record has seen, oldest first and bounded by
    /// [`MAX_HISTORY_EVENTS_PER_FINDING`] / [`MAX_HISTORY_BYTES_PER_FINDING`];
    /// `GetFindingHistory` reports it. Append through
    /// [`StoredFinding::record_history`], never by pushing, so the bound holds.
    pub history: Vec<FindingHistoryEvent>,
    /// The text [`StoredFinding::history`] currently holds, kept as a running total
    /// so the byte bound costs nothing to enforce.
    pub history_bytes: usize,
}

/// How many change events one finding's history keeps. A history is append-only and
/// a caller may write the same finding without limit, so the store is bounded here:
/// the oldest events are dropped once it is full, which is also how the real service
/// ages a finding's history out. The bound is the emulator's own (AWS publishes a
/// retention window, not a count), chosen above `MAX_RESULTS_LIMIT` so a bounded
/// history still spans several pages.
pub const MAX_HISTORY_EVENTS_PER_FINDING: usize = 256;

/// How much event text one finding's history keeps. Each event carries the old and
/// new text of every field a write changed, which is caller-sized, so the count bound
/// alone does not bound memory: the oldest events are dropped until the retained text
/// fits this budget. The most recent event is always kept, however large it is, so a
/// write is never recorded as nothing.
pub const MAX_HISTORY_BYTES_PER_FINDING: usize = 128 * 1024;

/// The text one change event holds, as the byte bound counts it.
fn history_event_bytes(event: &FindingHistoryEvent) -> usize {
    let mut bytes = event.update_time.len() + event.source_type.len() + event.source_identity.len();
    for update in &event.updates {
        for value in [&update.updated_field, &update.old_value, &update.new_value] {
            bytes += value.as_ref().map_or(0, String::len);
        }
    }
    bytes
}

impl StoredFinding {
    /// A finding record as its creating import stores it, with that import as the
    /// first event of its history.
    pub fn new(finding: AwsSecurityFinding, seq: u64, created: FindingHistoryEvent) -> Self {
        let mut stored = Self {
            finding,
            imported_seq: seq,
            updated_seq: seq,
            history: Vec::new(),
            history_bytes: 0,
        };
        stored.record_history(created);
        stored
    }

    /// Append one change event, dropping the oldest events until the history is back
    /// inside both of its bounds. The newest event always survives, so the store is
    /// bounded without a write ever going unrecorded.
    pub fn record_history(&mut self, event: FindingHistoryEvent) {
        self.history_bytes += history_event_bytes(&event);
        self.history.push(event);
        while self.history.len() > 1
            && (self.history.len() > MAX_HISTORY_EVENTS_PER_FINDING
                || self.history_bytes > MAX_HISTORY_BYTES_PER_FINDING)
        {
            let dropped = self.history.remove(0);
            self.history_bytes = self
                .history_bytes
                .saturating_sub(history_event_bytes(&dropped));
        }
    }
}

/// A standards subscription's identity: the hub it belongs to plus the ARN of the
/// standard it subscribes to. One subscription per standard per account/Region, which
/// is what makes `BatchEnableStandards` of an already-enabled standard the same
/// resource rather than a second one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StandardsSubscriptionKey {
    pub account_id: String,
    pub region: String,
    pub standards_arn: String,
}

/// An enabled standard: the `StandardsSubscription` resource `EnableSecurityHub`'s
/// default standards (and, once implemented, `BatchEnableStandards`) create and
/// `GetEnabledStandards` reports.
#[derive(Debug, Clone)]
pub struct StandardsSubscriptionRecord {
    pub standards_subscription_arn: String,
    /// The `StandardsInput` key-value pairs the subscription was created with.
    pub standards_input: HashMap<String, String>,
    /// A `StandardsStatus` value: `PENDING` until the enabling call's transition
    /// falls due, then `READY`.
    pub standards_status: String,
    /// A `StandardsProvider` value (`AWS` or `Azure`) — the cloud whose resources the
    /// standard evaluates.
    pub provider: String,
    /// Whole nanoseconds since the epoch at which the `PENDING` -> `READY`
    /// transition the enabling call owes becomes due. A reporting operation
    /// materialises it once the clock passes this instant.
    pub ready_due_nanos: i128,
}

/// Per-service state for com-amazonaws-securityhub-securityhubapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// Custom insights, keyed by hub + the ARN `CreateInsight` minted.
    pub insights: HashMap<InsightKey, StoredInsight>,
    /// The subscribed hubs, one per `(account id, Region)`.
    pub hubs: HashMap<HubKey, Hub>,
    /// Imported findings, keyed by hub + `AwsSecurityFindingIdentifier`.
    pub findings: HashMap<FindingKey, StoredFinding>,
    /// Member associations, keyed by administrator account/Region + member account id.
    pub members: HashMap<MemberKey, MemberAssociation>,
    /// Custom action targets, keyed by hub + the creator-chosen `Id`.
    pub action_targets: HashMap<ActionTargetKey, ActionTargetRecord>,
    /// The enabled standards, keyed by hub + standard ARN.
    pub standards: HashMap<StandardsSubscriptionKey, StandardsSubscriptionRecord>,
    /// Source of [`StoredFinding::imported_seq`] / [`StoredFinding::updated_seq`].
    pub write_seq: u64,
}

impl ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// The next monotonic write sequence. Never zero, so a stored value is always
    /// distinguishable from `Default`.
    pub fn next_write_seq(&mut self) -> u64 {
        self.write_seq += 1;
        self.write_seq
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecurityhubSecurityhubapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecurityhubSecurityhubapiserviceState>)
}

/// The ARN namespace Security Hub signs and ARNs with (`awsService.arnNamespace`
/// / `sigv4Name` in this service's Smithy model).
pub const ARN_NAMESPACE: &str = "securityhub";

/// The message a handler answers with when this service's store is unreachable,
/// kept in one place so every operation refuses identically.
pub const STATE_UNAVAILABLE_MESSAGE: &str = "the Security Hub CSPM state store is unavailable";

/// The message a hub-scoped operation refuses a caller with when the account is not
/// subscribed to Security Hub CSPM in the request's Region.
///
/// Only a HUB-SCOPED operation — one that reads or writes this account's hub,
/// findings, insights, action targets, members or standards subscriptions — has that
/// precondition. An operation that reports the service's own published metadata (the
/// standards catalog, the product catalog, the security-control definitions) answers
/// every caller, subscribed or not: those are the reads a client makes to discover
/// what it can enable, before any hub exists.
pub fn not_subscribed_message(account_id: &str, region: &str) -> String {
    format!("Account {account_id} is not subscribed to Security Hub CSPM in Region {region}")
}

/// How many custom action targets one hub (`(account, Region)`) may hold. The AWS
/// Security Hub quotas table publishes 100 custom actions per account per Region, and
/// `CreateActionTarget` models `LimitExceededException` for a create "beyond the
/// current Amazon Web Services account … limits"; the bound is also what keeps this
/// store from growing with the number of calls a caller makes.
pub const MAX_ACTION_TARGETS_PER_HUB: usize = 100;

/// The longest `Description` this store keeps for a custom action target. The model
/// documents no bound for that member, so the cap mirrors the one this crate already
/// applies to a finding's `Description` (`MAX_DESCRIPTION_LEN` in
/// `ops/batch_import_findings.rs`): one record's size must not grow with whatever a
/// caller sends.
pub const MAX_ACTION_TARGET_DESCRIPTION_LEN: usize = 1024;

/// How many member associations one hub (`(account, Region)`) may hold. The AWS
/// Security Hub quotas table publishes 1,000 member accounts per administrator
/// account, and `CreateMembers` models `LimitExceededException` for a create "beyond
/// the current Amazon Web Services account … limits"; the bound is also what keeps
/// this store from growing with the number of calls a caller makes.
pub const MAX_MEMBERS_PER_HUB: usize = 1_000;

/// The longest `Email` this store keeps for a member association. The model
/// documents no bound for `AccountDetails$Email`, so the cap is the longest address
/// SMTP itself allows (RFC 5321 §4.5.3.1.3 bounds a forward path at 256 octets
/// including the angle brackets, so 254 characters of address): one record's size
/// must not grow with whatever a caller sends.
pub const MAX_MEMBER_EMAIL_LEN: usize = 254;

/// How many accounts one member operation may address in a single call: the bound
/// the model already puts on `CreateMembers`' own batch (`AccountDetailsList`, "len
/// 1..50"), applied to every member operation's account list so neither the work one
/// call does under the state lock nor the response it produces grows with what a
/// caller sends. `GetMembers`' `AccountIdList` carries no `length` trait, so its
/// handler enforces this.
pub const MAX_MEMBER_BATCH: usize = 50;

/// A `NextToken` resuming a list at `offset`, carrying `prefix` so a token minted for
/// another list — or a value a caller invented — is rejected rather than read as an
/// offset. Opaque to the caller: the cursor is hex-encoded, so nothing in the
/// response invites arithmetic on it.
pub fn encode_list_token(prefix: &str, offset: usize) -> String {
    ::hex::encode(format!("{prefix}{offset}"))
}

/// The offset a `NextToken` resumes at, or `None` when it is not one the list named
/// by `prefix` minted.
pub fn decode_list_token(prefix: &str, token: &str) -> Option<usize> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(prefix)?.parse().ok()
}

/// The hub resource's fixed name: Security Hub CSPM creates exactly one hub per
/// account/Region and calls it `default`.
pub const HUB_RESOURCE: &str = "hub/default";

/// `EnableSecurityHub`'s documented default for `ControlFindingGenerator`: "For
/// accounts that aren't part of an organization, the default value of this field is
/// `SECURITY_CONTROL` if you enabled Security Hub CSPM on or after February 23,
/// 2023" (`smithy_query(query="doc", shape="EnableSecurityHubRequest",
/// member="ControlFindingGenerator")`). The emulator's clock is always after that
/// date, so this is the default for an omitted member.
pub const DEFAULT_CONTROL_FINDING_GENERATOR: &str = "SECURITY_CONTROL";

/// `EnableDefaultStandards`'s documented default: "If you don't provide a value for
/// `EnableDefaultStandards`, it is set to true."
pub const DEFAULT_ENABLE_DEFAULT_STANDARDS: bool = true;

/// A newly subscribed hub enables new controls of enabled standards automatically;
/// `UpdateSecurityHubConfiguration` is what turns it off, so a fresh hub reports
/// `true`.
pub const DEFAULT_AUTO_ENABLE_CONTROLS: bool = true;

/// One standard Security Hub CSPM supports, as the AWS Security Hub user guide's
/// "Security Hub standards reference" publishes it: the slug and version that make up
/// its ARNs, the display name and description `DescribeStandards` reports, the
/// `StandardsProvider` whose resources it evaluates, and whether `EnableSecurityHub`
/// enables it by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardDefinition {
    /// The `<slug>` in `standards/<slug>/v/<version>`.
    pub slug: &'static str,
    /// The `<version>` in `standards/<slug>/v/<version>`.
    pub version: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// A `StandardsProvider` value (`AWS` or `Azure`).
    pub provider: &'static str,
    /// `Standard$EnabledByDefault`: "When Security Hub CSPM is enabled using the
    /// `EnableSecurityHub` API operation, the standard is enabled by default unless
    /// `EnableDefaultStandards` is set to `false`" (`smithy_query(query="shape",
    /// shape="Standard")`, member `EnabledByDefault`).
    pub enabled_by_default: bool,
    /// The CIS AWS Foundations Benchmark v1.2.0 standard predates the per-Region
    /// `standards/` ARN form and keeps its partition-wide `arn:aws:securityhub:::ruleset/...`
    /// ARN; every other standard's ARN carries the Region.
    pub partition_wide_ruleset: bool,
}

/// The standards this emulator knows, from the AWS Security Hub user guide's
/// "Security Hub standards reference" (names, ARNs, descriptions, the evaluated cloud
/// provider and the two standards enabled by default). `GetEnabledStandards` reports
/// the subscriptions created from these, so a standard absent here is one no operation
/// can enable.
///
/// Ordered by `Name`, which is the order `DescribeStandards` reports the catalog in,
/// so a page boundary falls in the same place on every call.
pub const STANDARDS_CATALOG: [StandardDefinition; 10] = [
    StandardDefinition {
        slug: "ai-security-best-practices",
        version: "1.0.0",
        name: "AI Security Best Practices v1.0.0",
        description: "The AI Security Best Practices standard is a set of automated security checks that detect when your artificial intelligence and machine learning workloads, and the resources they run on, do not align with security best practices.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        name: "AWS Foundational Security Best Practices v1.0.0",
        description: "The AWS Foundational Security Best Practices standard is a set of automated security checks that detect when AWS accounts and deployed resources do not align to security best practices. The standard is defined by AWS security experts. This curated set of controls helps improve your security posture in AWS, and cover AWS's most popular and foundational services.",
        provider: "AWS",
        enabled_by_default: true,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        name: "AWS Resource Tagging Standard v1.0.0",
        description: "The AWS Resource Tagging Standard is a set of automated controls that detect when deployed resources do not carry the tags your organization requires, so that you can keep your tagging strategy consistent.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        name: "Azure Foundational Security Best Practices v1.0.0",
        description: "The Azure Foundational Security Best Practices standard is a set of automated security checks that detect when your Microsoft Azure subscriptions and the resources deployed in them do not align to security best practices.",
        provider: "Azure",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        name: "CIS AWS Foundations Benchmark v1.2.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.2.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: true,
        // The recorded catalog reports this standard with the partition-wide
        // `ruleset/` ARN that predates the per-Region `standards/` form; every other
        // standard carries the Region.
        partition_wide_ruleset: true,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        name: "CIS AWS Foundations Benchmark v1.4.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.4.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        name: "CIS AWS Foundations Benchmark v3.0.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v3.0.0 is a set of security configuration best practices for AWS. This Security Hub standard automatically checks for your compliance readiness against a subset of CIS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "nist-800-53",
        version: "5.0.0",
        name: "NIST SP 800-53 Rev. 5",
        description: "NIST SP 800-53 Rev. 5 provides a catalog of security and privacy controls for information systems and organizations. This Security Hub standard automatically checks for your compliance readiness against a subset of NIST SP 800-53 Rev. 5 requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "pci-dss",
        version: "3.2.1",
        name: "PCI DSS v3.2.1",
        description: "The Payment Card Industry Data Security Standard (PCI DSS) v3.2.1 is an information security standard for entities that store, process, or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "pci-dss",
        version: "4.0.1",
        name: "PCI DSS v4.0.1",
        description: "The Payment Card Industry Data Security Standard (PCI DSS) v4.0.1 is an information security standard for entities that store, process, or transmit cardholder data. This Security Hub standard automatically checks for your compliance readiness against a subset of PCI DSS requirements.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
];

/// `StandardsStatus$PENDING`: "The standard is in the process of being enabled"
/// (`smithy_query(query="shape", shape="StandardsSubscription")`, member
/// `StandardsStatus`).
pub const STANDARDS_STATUS_PENDING: &str = "PENDING";

/// `StandardsStatus$READY`: "The standard is enabled."
pub const STANDARDS_STATUS_READY: &str = "READY";

/// `StandardsControlsUpdatable$NOT_READY_FOR_UPDATES`: "Controls in the standard
/// cannot be retrieved or configured" — what a subscription still being enabled
/// reports.
pub const STANDARDS_CONTROLS_NOT_READY: &str = "NOT_READY_FOR_UPDATES";

/// `StandardsControlsUpdatable$READY_FOR_UPDATES`: "Controls in the standard can be
/// retrieved and configured."
pub const STANDARDS_CONTROLS_READY: &str = "READY_FOR_UPDATES";

/// How long after the enabling call a standards subscription's `PENDING` -> `READY`
/// transition falls due. AWS publishes no duration for it, so this is a chosen span
/// inside the emulator's 250-750 ms band: short enough not to stall a caller, long
/// enough that a caller reading straight back still observes `PENDING`.
pub const STANDARDS_READY_AFTER_NANOS: i128 = 500_000_000;

/// The ARN of a standard itself. AWS's standard ARNs carry no account (and, for the
/// one partition-wide ruleset, no Region either), so they are not built through
/// `engine::emulator_arn`, which always stamps the emulator account.
pub fn standard_arn(region: &str, definition: &StandardDefinition) -> String {
    let (slug, version) = (definition.slug, definition.version);
    if definition.partition_wide_ruleset {
        format!("arn:aws:{ARN_NAMESPACE}:::ruleset/{slug}/v/{version}")
    } else {
        format!("arn:aws:{ARN_NAMESPACE}:{region}::standards/{slug}/v/{version}")
    }
}

/// The ARN of the caller's subscription to a standard:
/// `arn:aws:securityhub:<region>:<account>:subscription/<slug>/v/<version>`.
pub fn standards_subscription_arn(key: &HubKey, definition: &StandardDefinition) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        &key.1,
        &format!(
            "subscription/{}/v/{}",
            definition.slug, definition.version
        ),
    )
}

/// The subscription an enabling call creates for one standard: `PENDING` now, with
/// the `READY` transition it owes due [`STANDARDS_READY_AFTER_NANOS`] later.
pub fn new_standards_subscription(
    key: &HubKey,
    definition: &StandardDefinition,
    standards_input: HashMap<String, String>,
) -> (StandardsSubscriptionKey, StandardsSubscriptionRecord) {
    (
        StandardsSubscriptionKey {
            account_id: key.0.clone(),
            region: key.1.clone(),
            standards_arn: standard_arn(&key.1, definition),
        },
        StandardsSubscriptionRecord {
            standards_subscription_arn: standards_subscription_arn(key, definition),
            standards_input,
            standards_status: STANDARDS_STATUS_PENDING.to_string(),
            provider: definition.provider.to_string(),
            ready_due_nanos: now_nanos() + STANDARDS_READY_AFTER_NANOS,
        },
    )
}

/// Materialise, for one hub, every standards-subscription transition the clock has
/// already made due: a `PENDING` subscription whose due instant has passed is
/// `READY`, whether or not anybody called. A reporting operation calls this before it
/// reads, so the status it reports is the status the clock owes.
pub fn settle_standards(
    store: &mut ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
) {
    let now = now_nanos();
    for (subscription_key, record) in &mut store.standards {
        if subscription_key.account_id != key.0 || subscription_key.region != key.1 {
            continue;
        }
        if record.standards_status == STANDARDS_STATUS_PENDING && now >= record.ready_due_nanos {
            record.standards_status = STANDARDS_STATUS_READY.to_string();
        }
    }
}

/// Whether controls of a subscription in this `StandardsStatus` can be retrieved and
/// configured: only an enabled (`READY`) standard exposes its controls.
pub fn standards_controls_updatable(standards_status: &str) -> &'static str {
    if standards_status == STANDARDS_STATUS_READY {
        STANDARDS_CONTROLS_READY
    } else {
        STANDARDS_CONTROLS_NOT_READY
    }
}

/// The only schema version the `AwsSecurityFinding` format defines: "The schema
/// version that a finding is formatted for. The value is `2018-10-08`"
/// (`smithy_query(query="shape", shape="AwsSecurityFinding")`, member
/// `SchemaVersion`).
pub const FINDING_SCHEMA_VERSION: &str = "2018-10-08";

/// The `RecordState` an ingested finding starts in: "ACTIVE - The finding must be
/// investigated or remediated" is the live state, `ARCHIVED` the other
/// (`smithy_query(query="shape", shape="RecordState")`). Security Hub populates it,
/// so a provider that submits none still gets one.
pub const DEFAULT_RECORD_STATE: &str = "ACTIVE";

/// The `Workflow.Status` an ingested finding starts in: "NEW - The initial state of a
/// finding, before it is reviewed" (`smithy_query(query="doc", shape="Workflow",
/// member="Status")`).
pub const DEFAULT_WORKFLOW_STATUS: &str = "NEW";

/// The Region an emulated request is for: the SigV4 credential scope's Region when
/// the request is signed, else the emulator's default Region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region an unsigned request is attributed to. Emulators default to
/// `us-east-1` (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The account the emulator presents for every caller.
pub fn request_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The hub key for the request `ctx` belongs to.
pub fn hub_key(ctx: &smithy_server_runtime::OperationContext<'_>) -> HubKey {
    (request_account_id(), request_region(ctx))
}

/// The key of a member association of the administrator hub `hub` — the single
/// definition of how every member operation addresses an association.
pub fn member_key(hub: &HubKey, member_account_id: &str) -> MemberKey {
    MemberKey {
        account_id: hub.0.clone(),
        region: hub.1.clone(),
        member_account_id: member_account_id.to_string(),
    }
}

/// An Amazon Web Services account id is exactly 12 decimal digits (the
/// `AwsSecurityFinding$AwsAccountId` length constraint: "Length Constraints: 12").
/// The `AccountId` shape itself carries no `length`/`pattern` trait, so the
/// framework's boundary validation cannot refuse a malformed id and each member
/// operation checks it.
pub const ACCOUNT_ID_LEN: usize = 12;

/// Whether `candidate` is a well-formed Amazon Web Services account id.
pub fn is_account_id(candidate: &str) -> bool {
    candidate.len() == ACCOUNT_ID_LEN && candidate.bytes().all(|byte| byte.is_ascii_digit())
}

// The documented `Member$MemberStatus` values (`smithy_query(query="shape",
// shape="Member")`, member `MemberStatus`).

/// "`Created` - Indicates that the administrator account added the member account,
/// but has not yet invited the member account."
pub const MEMBER_STATUS_CREATED: &str = "Created";
/// "`Invited` - Indicates that the administrator account invited the member account.
/// The member account has not yet responded to the invitation."
pub const MEMBER_STATUS_INVITED: &str = "Invited";
/// "`Enabled` - Indicates that the member account is currently active."
pub const MEMBER_STATUS_ENABLED: &str = "Enabled";
/// "`Removed` - Indicates that the administrator account disassociated the member
/// account."
pub const MEMBER_STATUS_REMOVED: &str = "Removed";
/// "`Resigned` - Indicates that the member account disassociated themselves from the
/// administrator account."
pub const MEMBER_STATUS_RESIGNED: &str = "Resigned";
/// "`Deleted` - Indicates that the administrator account deleted the member account."
pub const MEMBER_STATUS_DELETED: &str = "Deleted";

/// Whether a member status means the association is no longer in force, so
/// `CreateMembers` re-creates it rather than leaving it alone: "`CreateMembers` is
/// only used in the following cases: … The account was disassociated or deleted in
/// Security Hub CSPM" (`smithy_query(query="operation", operation="CreateMembers")`).
pub fn member_status_is_dissolved(status: &str) -> bool {
    matches!(
        status,
        MEMBER_STATUS_REMOVED | MEMBER_STATUS_RESIGNED | MEMBER_STATUS_DELETED
    )
}

/// The resource prefix a custom action target's ARN carries:
/// `arn:aws:securityhub:<region>:<account>:action/custom/<Id>` — the form the
/// model's own `ActionTargetArn` examples carry (every one of them matches
/// `"ActionTargetArn":"arn:aws:securityhub:<region>:<account>:action/custom/<Id>"`
/// in this service's `smithy-model/`).
pub const ACTION_TARGET_RESOURCE_PREFIX: &str = "action/custom/";

/// The ARN `CreateActionTarget` answers with for `target_id` in `region`.
pub fn action_target_arn(region: &str, target_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ACTION_TARGET_RESOURCE_PREFIX}{target_id}"),
    )
}

/// `arn:aws:securityhub:<region>:<account>:hub/default` for a hub key.
pub fn hub_arn(key: &HubKey) -> String {
    // `emulator_arn` stamps the canonical emulator account id; the key's account is
    // that same id, so the two cannot drift.
    ::engine::emulator_arn(ARN_NAMESPACE, &key.1, HUB_RESOURCE)
}

/// The current instant as a modelled `Timestamp` — the type every `date-time`
/// member of this service (`Member$UpdatedAt`, `Member$InvitedAt`, …) carries.
pub fn now_date_time() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The current instant in the ISO-8601 (RFC-3339) form every Security Hub
/// timestamp member uses.
pub fn now_iso8601() -> String {
    now_date_time()
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| String::new())
}

/// The current instant in whole nanoseconds since the epoch, the unit
/// [`parse_iso8601_nanos`] answers in, so a relative date filter compares in one
/// unit.
pub fn now_nanos() -> i128 {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i128::try_from(since_epoch.as_nanos()).unwrap_or(i128::MAX)
}

/// Parse an ISO-8601 (RFC-3339) timestamp into whole nanoseconds since the epoch,
/// for ordering and date filtering. `None` when the text is not a valid instant.
pub fn parse_iso8601_nanos(text: &str) -> Option<i128> {
    Some(datetime_nanos(&parse_iso8601(text)?))
}

/// Parse an ISO-8601 (RFC-3339) timestamp into the modelled `Timestamp` type, for a
/// response member the model types as a timestamp rather than a string. `None` when
/// the text is not a valid instant.
pub fn parse_iso8601(text: &str) -> Option<::aws_smithy_types::DateTime> {
    ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()
}

/// An instant in the whole nanoseconds since the epoch that [`parse_iso8601_nanos`]
/// and [`now_nanos`] answer in, so a bound a caller sent as a modelled `Timestamp`
/// compares against a stored timestamp in one unit.
pub fn datetime_nanos(instant: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(instant.secs()) * 1_000_000_000 + i128::from(instant.subsec_nanos())
}

/// The greatest number of tags one Security Hub resource carries: the modelled
/// `TagMap`'s own `length` maximum (`smithy_query(query="shape", shape="TagMap")`
/// reports `length {min: 1, max: 50}`), which is the shape `ListTagsForResource`
/// reports a resource's whole tag set in.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Whether `arn` is a well-formed ARN: the six colon-separated ARN fields
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a non-empty
/// partition, service and resource (Region and account are empty on the
/// service-owned ARNs Security Hub mints, e.g. a product ARN).
///
/// The modelled `ResourceArn` pattern (`^arn:aws:securityhub:`) only pins the
/// prefix, so the boundary validator admits a truncated ARN; that is a bad input
/// parameter, not a missing resource.
pub fn is_well_formed_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The tag set of the resource `resource_arn` names, for reading, as seen by the
/// caller whose account and Region are `key`. `None` when that caller has no
/// resource carrying that ARN.
///
/// Tagging addresses a resource by ARN, and Security Hub CSPM is subscribed per
/// account and Region behind per-Region endpoints, so the search is scoped to the
/// requesting account/Region: an ARN naming another Region's resource resolves to
/// nothing here, which is the 404 the call owes it. Every resource kind this crate
/// persists and the model gives a tag set is looked up here — today that is the
/// `hub/default` resource `EnableSecurityHub` creates; a resource kind whose create
/// operation is still a stub has nothing in state to find.
pub fn resource_tags<'a>(
    store: &'a ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
    resource_arn: &str,
) -> Option<&'a HashMap<String, String>> {
    store
        .hubs
        .get(key)
        .filter(|hub| hub.hub_arn == resource_arn)
        .map(|hub| &hub.tags)
}

/// The tag set of the resource `resource_arn` names, for writing — the mutable twin
/// of [`resource_tags`], so `TagResource` and `UntagResource` resolve an ARN exactly
/// as `ListTagsForResource` does.
pub fn resource_tags_mut<'a>(
    store: &'a mut ComAmazonawsSecurityhubSecurityhubapiserviceState,
    key: &HubKey,
    resource_arn: &str,
) -> Option<&'a mut HashMap<String, String>> {
    store
        .hubs
        .get_mut(key)
        .filter(|hub| hub.hub_arn == resource_arn)
        .map(|hub| &mut hub.tags)
}

/// The company every standard in [`STANDARDS_CATALOG`] is managed by. The model's
/// prose says "For existing standards, the value is equal to `Amazon Web Services`"
/// (`smithy_query(query="shape", shape="StandardsManagedBy")`, member `Company`), but
/// the service answers the short company identifier `AWS` on the wire, which is what
/// a client comparing `StandardsManagedBy.Company` sees; the wire form wins over the
/// doc's expansion of it.
pub const STANDARDS_MANAGED_BY_COMPANY: &str = "AWS";

/// The product every standard in [`STANDARDS_CATALOG`] is managed by: "the value is
/// equal to the Amazon Web Services service that manages the standard"
/// (`smithy_query(query="shape", shape="StandardsManagedBy")`, member `Product`).
pub const STANDARDS_MANAGED_BY_PRODUCT: &str = "Security Hub";

/// [`STANDARDS_CATALOG`] as the modelled `Standard` list `DescribeStandards`
/// reports, with every ARN built for `region`.
///
/// Every published standard is managed by Amazon Web Services, so each entry
/// carries the same `StandardsManagedBy` pair.
pub fn standards_catalog(region: &str) -> Vec<Standard> {
    STANDARDS_CATALOG
        .iter()
        .map(|definition| Standard {
            standards_arn: Some(standard_arn(region, definition)),
            name: Some(definition.name.to_string()),
            description: Some(definition.description.to_string()),
            enabled_by_default: Some(definition.enabled_by_default),
            provider: Some(definition.provider.to_string()),
            standards_managed_by: Some(StandardsManagedBy {
                company: Some(STANDARDS_MANAGED_BY_COMPANY.to_string()),
                product: Some(STANDARDS_MANAGED_BY_PRODUCT.to_string()),
            }),
        })
        .collect()
}

/// The documented `Normalized` value for a `SeverityLabel`: "If you provide `Label`
/// and don't provide `Normalized`, then `Normalized` is set automatically as
/// follows: INFORMATIONAL - 0, LOW - 1, MEDIUM - 40, HIGH - 70, CRITICAL - 90"
/// (`smithy_query(query="doc", shape="Severity", member="Normalized")`).
pub fn normalized_for_label(label: &str) -> Option<i32> {
    match label {
        "INFORMATIONAL" => Some(0),
        "LOW" => Some(1),
        "MEDIUM" => Some(40),
        "HIGH" => Some(70),
        "CRITICAL" => Some(90),
        _ => None,
    }
}

/// The documented `SeverityLabel` for a `Normalized` value: "If you provide
/// `Normalized` and don't provide `Label`, `Label` is set automatically as follows:
/// 0 - INFORMATIONAL, 1–39 - LOW, 40–69 - MEDIUM, 70–89 - HIGH, 90–100 - CRITICAL"
/// (`smithy_query(query="doc", shape="SeverityUpdate", member="Normalized")`).
pub fn label_for_normalized(normalized: i32) -> Option<&'static str> {
    match normalized {
        0 => Some("INFORMATIONAL"),
        1..=39 => Some("LOW"),
        40..=69 => Some("MEDIUM"),
        70..=89 => Some("HIGH"),
        90..=100 => Some("CRITICAL"),
        _ => None,
    }
}

/// Fill in whichever of `Label` / `Normalized` the submitter left out, from the
/// other, per the two documented mappings above. A severity that carries both, or
/// neither, is left exactly as submitted.
pub fn complete_severity(severity: &mut Severity) {
    match (severity.label.clone(), severity.normalized) {
        (Some(_), Some(_)) | (None, None) => {}
        (Some(label), None) => severity.normalized = normalized_for_label(&label),
        (None, Some(normalized)) => {
            severity.label = label_for_normalized(normalized).map(str::to_string);
        }
    }
}

/// `FindingHistoryUpdateSource.Type` for a change `BatchImportFindings` made; one of
/// the two values `FindingHistoryUpdateSourceType` models.
pub const HISTORY_SOURCE_BATCH_IMPORT_FINDINGS: &str = "BATCH_IMPORT_FINDINGS";

/// `FindingHistoryUpdateSource.Type` for a change `BatchUpdateFindings` made.
pub const HISTORY_SOURCE_BATCH_UPDATE_FINDINGS: &str = "BATCH_UPDATE_FINDINGS";

/// The top-level `AwsSecurityFinding` members that carry a timestamp, which
/// `GetFindingHistory` excludes from a change event: "The history includes changes
/// made to any fields in the Amazon Web Services Security Finding Format (ASFF)
/// except top-level timestamp fields, such as the CreatedAt and UpdatedAt fields"
/// (`smithy_query(query="operation", operation="GetFindingHistory")` docs). These are
/// every member of `AwsSecurityFinding` whose documentation calls it a timestamp
/// (`smithy_query(query="shape", shape="AwsSecurityFinding")`).
pub const HISTORY_EXCLUDED_FIELDS: [&str; 5] = [
    "CreatedAt",
    "UpdatedAt",
    "FirstObservedAt",
    "LastObservedAt",
    "ProcessedAt",
];

/// The IAM user name the emulator attributes every signed request to, and the one
/// `GetCallerIdentity` answers with: see `AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`
/// ("The identity of the ambient caller: the IAM user whose long-term key this
/// emulator publishes").
pub const CALLER_USER_NAME: &str = "emulator";

/// The IAM principal a request is attributed to, which is what names the customer
/// that called `BatchUpdateFindings` in a change event's `UpdateSource.Identity`.
///
/// The emulator publishes ONE long-term identity for every signed request, and STS's
/// `GetCallerIdentity` is where a caller reads it; a finding change event names the
/// caller with that same ARN rather than minting a second principal for the one
/// credential.
pub fn caller_arn() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{CALLER_USER_NAME}"))
}

/// Each top-level ASFF field of a finding, keyed by the `AwsSecurityFinding` member
/// name, rendered as the text a change event reports for it.
///
/// The finding is rendered with the protocol's own JSON codec, so the keys are the
/// modelled ASFF field names however a client sent them, and a field this emulator
/// never special-cases is still accounted for. A change event names "The ASFF field
/// that changed" (`smithy_query(query="doc", shape="FindingHistoryUpdate",
/// member="UpdatedField")`), and an ASFF field is one member of the finding: a scalar
/// renders as its own text, and a structure, list or map renders as its JSON, so one
/// changed field is one entry whatever its shape. An absent or empty field is no entry
/// at all: `FindingHistoryUpdate`'s `OldValue` / `NewValue` are `NonEmptyString`.
fn finding_field_values(finding: &AwsSecurityFinding) -> BTreeMap<String, String> {
    use ::aws_smithy_schema::codec::{Codec, FinishSerializer};
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    let mut values = BTreeMap::new();
    if serializer
        .write_struct(&AwsSecurityFinding::SCHEMA, finding)
        .is_err()
    {
        return values;
    }
    let Ok(rendered) = ::serde_json::from_slice::<::serde_json::Value>(&serializer.finish()) else {
        return values;
    };
    let ::serde_json::Value::Object(fields) = rendered else {
        return values;
    };
    for (name, value) in fields {
        match value {
            ::serde_json::Value::Null => {}
            ::serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    values.insert(name, text);
                }
            }
            other => {
                let rendered = other.to_string();
                // An empty list or map is no value, like an absent member: clearing a
                // collection reads as a field that no longer has a value.
                if rendered != "[]" && rendered != "{}" {
                    values.insert(name, rendered);
                }
            }
        }
    }
    values
}

/// The fields a finding was created with, as the creating event's `Updates`: one
/// entry per ASFF field the import set, each with no prior value, with the top-level
/// timestamp members ([`HISTORY_EXCLUDED_FIELDS`]) left out.
pub fn finding_created_fields(finding: &AwsSecurityFinding) -> Vec<FindingHistoryUpdate> {
    finding_field_values(finding)
        .into_iter()
        .filter(|(field, _)| !HISTORY_EXCLUDED_FIELDS.contains(&field.as_str()))
        .map(|(field, value)| FindingHistoryUpdate {
            updated_field: Some(field),
            old_value: None,
            new_value: Some(value),
        })
        .collect()
}

/// The ASFF fields that differ between a finding before and after a write, as a
/// change event's `Updates`: one entry per field whose value changed, in field-name
/// order, with the top-level timestamp members ([`HISTORY_EXCLUDED_FIELDS`]) left out.
pub fn finding_changes(
    before: &AwsSecurityFinding,
    after: &AwsSecurityFinding,
) -> Vec<FindingHistoryUpdate> {
    let before_values = finding_field_values(before);
    let after_values = finding_field_values(after);
    let mut changes = Vec::new();
    let mut fields: Vec<&String> = before_values.keys().chain(after_values.keys()).collect();
    fields.sort_unstable();
    fields.dedup();
    for field in fields {
        if HISTORY_EXCLUDED_FIELDS.contains(&field.as_str()) {
            continue;
        }
        let old_value = before_values.get(field);
        let new_value = after_values.get(field);
        if old_value == new_value {
            continue;
        }
        changes.push(FindingHistoryUpdate {
            updated_field: Some(field.clone()),
            old_value: old_value.cloned(),
            new_value: new_value.cloned(),
        });
    }
    changes
}

/// A custom insight's identity: the hub it lives in, plus the ARN `CreateInsight`
/// minted for it. The ARN alone would be unique, but the account/Region prefix is
/// what makes "disable removes this account's insights in this Region" expressible,
/// exactly as it does for [`FindingKey`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InsightKey {
    pub account_id: String,
    pub region: String,
    pub insight_arn: String,
}

/// A custom insight as `CreateInsight` stored it: the three members the modelled
/// `Insight` shape carries beyond its ARN. `GetInsightResults` evaluates it against
/// the hub's findings.
///
/// `Filters` names its type by path rather than through this module's `use`, so
/// adding an insight store costs no line in an import list every new operation of
/// this service also edits.
#[derive(Debug, Clone)]
pub struct StoredInsight {
    pub name: String,
    pub filters: crate::ops::types::AwsSecurityFindingFilters,
    pub group_by_attribute: String,
}

/// `arn:aws:securityhub:<region>:<account>:insight/<account>/custom/<id>` — the ARN
/// Security Hub CSPM mints for a CUSTOM insight (the insights a caller creates,
/// as opposed to the service's own managed ones).
pub fn custom_insight_arn(key: &HubKey, insight_id: &str) -> String {
    // `emulator_arn` stamps the canonical emulator account id; the key's account is
    // that same id, so the two cannot drift.
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        &key.1,
        &format!("insight/{}/custom/{insight_id}", key.0),
    )
}
