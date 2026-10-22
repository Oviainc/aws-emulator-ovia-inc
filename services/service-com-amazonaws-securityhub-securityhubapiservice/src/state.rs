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

use std::collections::HashMap;

use crate::ops::types::{AwsSecurityFinding, Severity, Standard, StandardsManagedBy};

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
    /// The subscribed hubs, one per `(account id, Region)`.
    pub hubs: HashMap<HubKey, Hub>,
    /// Imported findings, keyed by hub + `AwsSecurityFindingIdentifier`.
    pub findings: HashMap<FindingKey, StoredFinding>,
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
/// "Security Hub standards reference" (names, ARNs and the two standards enabled by
/// default). `GetEnabledStandards` reports the subscriptions created from these, so a
/// standard absent here is one no operation can enable.
pub const STANDARDS_CATALOG: [StandardDefinition; 7] = [
    StandardDefinition {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        name: "AWS Foundational Security Best Practices v1.0.0",
        description: "The AWS Foundational Security Best Practices standard is a set of automated security checks that detect when AWS accounts and deployed resources do not align with security best practices.",
        provider: "AWS",
        enabled_by_default: true,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        name: "CIS AWS Foundations Benchmark v1.2.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.2.0 is a set of security configuration best practices for AWS.",
        provider: "AWS",
        enabled_by_default: true,
        partition_wide_ruleset: true,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "1.4.0",
        name: "CIS AWS Foundations Benchmark v1.4.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v1.4.0 is a set of security configuration best practices for AWS.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "cis-aws-foundations-benchmark",
        version: "3.0.0",
        name: "CIS AWS Foundations Benchmark v3.0.0",
        description: "The Center for Internet Security (CIS) AWS Foundations Benchmark v3.0.0 is a set of security configuration best practices for AWS.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "nist-800-53",
        version: "5.0.0",
        name: "NIST SP 800-53 Rev. 5",
        description: "NIST SP 800-53 Rev. 5 provides a catalog of security and privacy controls for information systems and organizations.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "pci-dss",
        version: "3.2.1",
        name: "PCI DSS v3.2.1",
        description: "The Payment Card Industry Data Security Standard (PCI DSS) v3.2.1 is an information security standard for entities that store, process, or transmit cardholder data.",
        provider: "AWS",
        enabled_by_default: false,
        partition_wide_ruleset: false,
    },
    StandardDefinition {
        slug: "aws-resource-tagging-standard",
        version: "1.0.0",
        name: "AWS Resource Tagging Standard v1.0.0",
        description: "The AWS Resource Tagging Standard is a set of automated security checks that detect when deployed resources do not carry the tags your organization requires.",
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

/// `arn:aws:securityhub:<region>:<account>:hub/default` for a hub key.
pub fn hub_arn(key: &HubKey) -> String {
    // `emulator_arn` stamps the canonical emulator account id; the key's account is
    // that same id, so the two cannot drift.
    ::engine::emulator_arn(ARN_NAMESPACE, &key.1, HUB_RESOURCE)
}

/// The current instant in the ISO-8601 (RFC-3339) form every Security Hub
/// timestamp member uses.
pub fn now_iso8601() -> String {
    let now = ::std::time::SystemTime::now();
    let since_epoch = now
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let dt = ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    );
    dt.fmt(::aws_smithy_types::date_time::Format::DateTime)
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
    let dt = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(i128::from(dt.secs()) * 1_000_000_000 + i128::from(dt.subsec_nanos()))
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

/// The company every standard in [`STANDARDS_CATALOG`] is managed by: "For existing
/// standards, the value is equal to `Amazon Web Services`"
/// (`smithy_query(query="shape", shape="StandardsManagedBy")`, member `Company`).
pub const STANDARDS_MANAGED_BY_COMPANY: &str = "Amazon Web Services";

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
