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

use crate::ops::types::{AwsSecurityFinding, Severity};

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

/// Per-service state for com-amazonaws-securityhub-securityhubapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSecurityhubSecurityhubapiserviceState {
    /// The subscribed hubs, one per `(account id, Region)`.
    pub hubs: HashMap<HubKey, Hub>,
    /// Imported findings, keyed by hub + `AwsSecurityFindingIdentifier`.
    pub findings: HashMap<FindingKey, StoredFinding>,
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
