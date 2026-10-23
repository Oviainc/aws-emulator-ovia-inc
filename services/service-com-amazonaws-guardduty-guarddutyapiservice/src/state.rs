// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-guardduty-guarddutyapiservice service.
//!
//! GuardDuty's control plane is rooted at the *detector*: one per account per
//! Region. Everything the detector owns — its findings and its filters — hangs off
//! the [`Detector`] record, so `DeleteDetector` cascades by dropping one map entry.
//!
//! This module holds only plain records plus the helpers more than one operation
//! needs (id minting, the monotonic clock, finding attribute lookup, filtering and
//! sorting). Each operation builds its own modelled output in its own
//! `src/ops/<op>.rs`.

use std::collections::{BTreeMap, HashMap};

/// Per-service state for com-amazonaws-guardduty-guarddutyapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// Every persisted detector, keyed by its minted `DetectorId`.
    pub detectors: HashMap<String, Detector>,
    /// `ClientToken` -> `DetectorId`, so a replayed `CreateDetector` returns the
    /// detector the first call minted instead of refusing or minting a second one.
    /// Keyed by `(region, token)`: an idempotency token is scoped to the endpoint
    /// it was replayed against.
    pub client_tokens: HashMap<(String, String), String>,
    /// Last instant handed out by [`Self::tick_millis`], so two mutations in the
    /// same millisecond still get strictly increasing timestamps.
    last_millis: i64,
    /// Monotonic counter folded into every minted id.
    seq: u64,
}

impl ComAmazonawsGuarddutyGuarddutyapiserviceState {
    /// The current instant in epoch milliseconds, strictly greater than every
    /// instant this state handed out before.
    ///
    /// Call it only from a MUTATING operation: it is the "this resource changed
    /// now" clock, and a reporting operation must not advance it. Monotonicity is
    /// what makes `UpdatedAt > CreatedAt` observable for two changes that land in
    /// the same wall-clock millisecond.
    pub fn tick_millis(&mut self) -> i64 {
        let wall = now_millis();
        let next = if wall > self.last_millis {
            wall
        } else {
            self.last_millis.saturating_add(1)
        };
        self.last_millis = next;
        next
    }

    /// Mint a fresh 32-hex-character id, the shape GuardDuty uses for both
    /// `DetectorId` and a finding `Id`.
    ///
    /// The low 16 bits carry this state's own sequence counter, so two ids minted
    /// by the same state differ even if the hash above them repeats.
    pub fn mint_id(&mut self) -> String {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        let nanos = now_nanos();
        let hi = scramble(seq, nanos);
        let lo = scramble(nanos ^ GOLDEN_RATIO_64, hi ^ seq);
        let lo = (lo & !0xFFFF) | (seq & 0xFFFF);
        format!("{hi:016x}{lo:016x}")
    }

    /// The detector `region`'s endpoint serves for `id`, if any.
    ///
    /// A detector is a REGIONAL resource — "you must create a detector in each
    /// Region where you enable the service"
    /// (`smithy_query(query="doc",shape="CreateDetector")`) — so an id created
    /// through another Region's endpoint is not visible here.
    pub fn detector_in(&self, region: &str, id: &str) -> Option<&Detector> {
        self.detectors
            .get(id)
            .filter(|detector| detector.region == region)
    }

    /// [`Self::detector_in`], for a mutating operation.
    pub fn detector_in_mut(&mut self, region: &str, id: &str) -> Option<&mut Detector> {
        self.detectors
            .get_mut(id)
            .filter(|detector| detector.region == region)
    }

    /// Mint an `InvestigationId`: the 32 hexadecimal characters [`Self::mint_id`]
    /// mints, grouped in the 8-4-4-4-12 form.
    ///
    /// `InvestigationId` is `len 1..64, pattern ^[a-fA-F0-9\-]+$`
    /// (`smithy_query(operation="CreateInvestigation")`) — hexadecimal plus
    /// hyphens — so the 36-character grouped form is a value of the member, and it
    /// is distinguishable at a glance from the bare-hex `DetectorId` / finding id.
    pub fn mint_investigation_id(&mut self) -> String {
        let hex = self.mint_id();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Set the archive state of the named findings of the detector `region`'s
    /// endpoint serves for `detector_id`, recording the instant of the write as the
    /// `UpdatedAt` of each finding it moves.
    ///
    /// `None` means that endpoint does not hold the detector, which is the caller's
    /// cue to refuse the request: nothing is written and the clock is not advanced,
    /// because the detector is resolved BEFORE [`Self::tick_millis`] runs.
    pub fn set_findings_archived_in(
        &mut self,
        region: &str,
        detector_id: &str,
        finding_ids: &[String],
        archived: bool,
    ) -> Option<()> {
        self.detector_in(region, detector_id)?;
        let now = self.tick_millis();
        self.detector_in_mut(region, detector_id)?
            .set_findings_archived(finding_ids, archived, now);
        Some(())
    }

    /// The ids of the detectors `region`'s endpoint serves, in a total order.
    pub fn detector_ids_in(&self, region: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .detectors
            .values()
            .filter(|detector| detector.region == region)
            .map(|detector| detector.id.clone())
            .collect();
        ids.sort();
        ids
    }
}

/// The 64-bit golden-ratio odd constant used to decorrelate the two hash inputs.
const GOLDEN_RATIO_64: u64 = 0x9E37_79B9_7F4A_7C15;

/// Hash two words into one, seeded by a freshly built [`std::collections::hash_map::RandomState`]
/// so ids are not predictable across runs.
fn scramble(a: u64, b: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(a);
    hasher.write_u64(b);
    hasher.finish()
}

/// The current instant in epoch milliseconds, straight off the wall clock.
///
/// A REPORTING operation reads the clock through this rather than through
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::tick_millis`]: it needs to know
/// what the clock has already made due without advancing the "this resource
/// changed now" counter.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0))
}

/// Render an epoch-millisecond instant the way GuardDuty renders `CreatedAt` /
/// `UpdatedAt`: an ISO-8601 instant in UTC with a fixed three-digit millisecond
/// field.
///
/// The width is fixed deliberately. RFC 3339 allows trimming trailing zeros from
/// the subsecond field, but then `…:21.62Z` and `…:21.621Z` no longer compare in
/// time order as text, and `CreatedAt`/`UpdatedAt` are modelled as `String` — text
/// is all a client has to order them by.
pub fn iso8601(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let subsec_millis = millis.rem_euclid(1000);
    let whole = ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    match whole.strip_suffix('Z') {
        Some(prefix) => format!("{prefix}.{subsec_millis:03}Z"),
        None => whole,
    }
}

/// `DetectorStatus` / `FeatureStatus` / `DataSourceStatus` — every one of the three
/// enums is `ENABLED | DISABLED` (`smithy_query(shape="DetectorStatus")`).
pub const ENABLED: &str = "ENABLED";
/// See [`ENABLED`].
pub const DISABLED: &str = "DISABLED";

/// Every member of the `DetectorFeature` enum, in model order
/// (`smithy_query(shape="DetectorFeature")`). These are the features a request may
/// configure: `DetectorFeatureConfiguration.Name` targets this enum.
pub const DETECTOR_FEATURES: [&str; 9] = [
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "RUNTIME_MONITORING",
    "AI_PROTECTION",
    "AI_ANALYST",
];

/// Runtime Monitoring: the feature `CreateDetector` documents as the exception to
/// its "all the optional features are enabled by default" default
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_RUNTIME_MONITORING: &str = "RUNTIME_MONITORING";

/// Runtime Monitoring already covers Amazon EKS, so the two cannot both be
/// requested: "Specifying both EKS Runtime Monitoring (EKS_RUNTIME_MONITORING) and
/// Runtime Monitoring (RUNTIME_MONITORING) will cause an error"
/// (`smithy_query(query="doc",shape="CreateDetector")`).
pub const FEATURE_EKS_RUNTIME_MONITORING: &str = "EKS_RUNTIME_MONITORING";

/// The feature that governs S3 data-event ingestion — the `DataSources.S3Logs`
/// view of the same switch.
pub const FEATURE_S3_DATA_EVENTS: &str = "S3_DATA_EVENTS";
/// The feature behind `DataSources.Kubernetes.AuditLogs`.
pub const FEATURE_EKS_AUDIT_LOGS: &str = "EKS_AUDIT_LOGS";
/// The feature behind `DataSources.MalwareProtection.ScanEc2InstanceWithFindings`.
pub const FEATURE_EBS_MALWARE_PROTECTION: &str = "EBS_MALWARE_PROTECTION";
/// The feature behind `DataSources.Database.Rds.DatabaseEvents`: RDS Protection
/// watches Amazon RDS login activity.
pub const FEATURE_RDS_LOGIN_EVENTS: &str = "RDS_LOGIN_EVENTS";

/// GuardDuty Investigation's own feature: "To use this operation, the AI_ANALYST
/// feature must be enabled on your detector"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const FEATURE_AI_ANALYST: &str = "AI_ANALYST";

/// The feature behind `DataSources.CloudTrail`: GuardDuty's analysis of CloudTrail
/// management events.
pub const FEATURE_CLOUD_TRAIL: &str = "CLOUD_TRAIL";
/// The feature behind `DataSources.DNSLogs`.
pub const FEATURE_DNS_LOGS: &str = "DNS_LOGS";
/// The feature behind `DataSources.FlowLogs`.
pub const FEATURE_FLOW_LOGS: &str = "FLOW_LOGS";

/// The optional features a bare `CreateDetector` leaves DISABLED — the ones a
/// detector reports only once a request asks for them.
///
/// Runtime Monitoring is the exception `CreateDetector` documents: "When you don't
/// specify any features, with an exception to RUNTIME_MONITORING, all the optional
/// features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). RDS Protection is opt-in
/// the same way: the recorded wire for a freshly created detector reports
/// `dataSources.database.rds.databaseEvents` DISABLED while every other data source
/// reports ENABLED.
pub const OPT_IN_FEATURES: [&str; 2] = [FEATURE_RUNTIME_MONITORING, FEATURE_RDS_LOGIN_EVENTS];

/// The three data sources GuardDuty always consumes for a detector — CloudTrail
/// management events, VPC flow logs and DNS logs.
///
/// They are reported as features (`DetectorFeatureResult` carries `CLOUD_TRAIL`,
/// `DNS_LOGS` and `FLOW_LOGS`, `smithy_query(shape="DetectorFeatureResult")`) but
/// are NOT members of `DetectorFeature` and have no `DataSourceConfigurations`
/// member, so no request can switch them off: they report `ENABLED` for the life of
/// the detector.
pub const FOUNDATIONAL_FEATURES: [&str; 3] =
    [FEATURE_CLOUD_TRAIL, FEATURE_DNS_LOGS, FEATURE_FLOW_LOGS];

/// Every feature a detector reports, in the order the service reports them: the
/// three foundational sources first, in the order `DataSourceConfigurationsResult`
/// lists them (`CloudTrail`, `DNSLogs`, `FlowLogs`), then every configurable
/// [`DETECTOR_FEATURES`] member in `DetectorFeature` model order.
///
/// The list is a total order over a SET, so a client may index it positionally.
pub fn reported_features() -> Vec<&'static str> {
    FOUNDATIONAL_FEATURES
        .iter()
        .chain(DETECTOR_FEATURES.iter())
        .copied()
        .collect()
}

/// The current GuardDuty finding schema version, the value every finding carries
/// in its required `SchemaVersion` member.
pub const FINDING_SCHEMA_VERSION: &str = "2.0";

/// The partition every emulated ARN lives in (`engine::emulator_arn` builds
/// `arn:aws:...`).
pub const PARTITION: &str = "aws";

// ---------------------------------------------------------------------------
// Client rejections
//
// GuardDuty classifies a rejection by the REQUEST, not by what the store holds:
// a value that is not a well-formed id for its member is a parameter-validation
// failure, a well-formed id this account/Region does not own is an ownership
// failure, and neither message quotes the caller's value or the endpoint's
// Region back at them. The messages are built here, once, so every operation
// rejects the same input the same way.
// ---------------------------------------------------------------------------

/// The wire name of the `DetectorId` member, as the request carries it
/// (`smithy_query(operation="GetDetector")` renders `DetectorId` with the
/// `jsonName`/URI label `detectorId`). It is what a rejection names.
pub const DETECTOR_ID_MEMBER: &str = "detectorId";

/// The wire name of the `NextToken` member every paginated operation here carries
/// (`smithy_query(operation="ListDetectors")`), for the same reason.
pub const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The wire name of the account-id member every members operation carries:
/// `AccountIds`' entries target `AccountId`
/// (`smithy_query(operation="DisassociateMembers")`), and `AccountDetail.AccountId`
/// renders as `accountId` (`smithy_query(shape="AccountDetail")`).
pub const ACCOUNT_ID_MEMBER: &str = "accountId";

/// The wire name of `AccountDetail`'s email member
/// (`smithy_query(shape="AccountDetail")` renders `Email` as `email`).
pub const EMAIL_MEMBER: &str = "email";

/// The length of a GuardDuty detector id: the 32 hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] mints, which is the
/// form every id this service hands out has. The model's own constraint
/// (`DetectorId`, len 1..300 — `smithy_query(shape="DetectorId")`) is far wider
/// than the id format, which is why a syntactically impossible id reaches the
/// handler at all instead of being refused at the boundary.
pub const DETECTOR_ID_LEN: usize = 32;

/// Is `id` a syntactically well-formed id of the kind this service MINTS — the 32
/// hexadecimal characters
/// [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] hands out?
///
/// Form only: a well-formed id need not exist. Case is not significant, so an id
/// a caller upper-cased is still well formed — and is then refused as not owned,
/// which is the ownership failure and not a parameter one.
pub fn is_minted_id(id: &str) -> bool {
    id.len() == DETECTOR_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Is `id` a syntactically well-formed `DetectorId`? See [`is_minted_id`].
pub fn is_detector_id(id: &str) -> bool {
    is_minted_id(id)
}

/// The message for a member whose VALUE cannot be a value of that member at all.
pub fn invalid_parameter_message(member: &str) -> String {
    format!("The request is rejected because the parameter {member} has an invalid value.")
}

/// The message for a well-formed id that this account does not own — an id
/// another account created, or one a delete removed. GuardDuty does not
/// distinguish "never existed" from "belongs to someone else": both are the same
/// refusal, which is also what keeps one account from probing another's ids.
pub fn not_owned_message(member: &str) -> String {
    format!(
        "The request is rejected because the input {member} is not owned by the current account."
    )
}

/// The message a second `CreateDetector` is refused with. "You can have only one
/// detector per account per Region"
/// (`smithy_query(query="doc",shape="CreateDetector")`); the Region is the one the
/// request was signed for, so the rejection does not name it.
pub const DETECTOR_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a detector already exists for the current account.";

/// The length of an AWS account id: the 12 characters `AccountId` is constrained
/// to (`len 12..12` — `smithy_query(shape="AccountId")`). The model constrains the
/// LENGTH only, so a 12-character value that is not an account id at all reaches
/// the handler instead of being refused at the boundary.
pub const ACCOUNT_ID_LEN: usize = 12;

/// Is `id` a syntactically well-formed account id — exactly [`ACCOUNT_ID_LEN`]
/// decimal digits?
///
/// Form only: a well-formed id need not name an account this administrator has a
/// member record for, which is a different question and a different answer (see
/// [`not_a_member_message`]).
pub fn is_account_id(id: &str) -> bool {
    id.len() == ACCOUNT_ID_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The administrator account the members of this emulator's detectors belong to:
/// the single account this emulator serves, which is the account every request it
/// answers is signed for.
///
/// It is NOT read from the request, so it is not caller input: a member's
/// `MasterId` / `AdministratorId` is the account that owns the detector the member
/// hangs off.
pub fn administrator_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// `UnprocessedAccount.Result` — "a reason why the account hasn't been processed"
/// (`smithy_query(shape="UnprocessedAccount")`) — for an account id the
/// administrator's detector holds no member record for.
///
/// A per-account failure is reported here rather than refusing the whole request:
/// the operations that take an `AccountIds` list answer 200 and report the accounts
/// they could not process, which is what `UnprocessedAccounts` is for.
pub fn not_a_member_message(administrator_account_id: &str) -> String {
    format!(
        "The request is rejected because the given account ID is not an associated member of account {administrator_account_id}."
    )
}

/// `UnprocessedAccount.Result` for an account `DisassociateMembers` was asked to
/// disassociate that is not ASSOCIATED with the administrator — one it holds no
/// member record for, and one whose relationship a disassociation already moved.
///
/// The condition is the association, not the record: the message states it ("the
/// latter is not yet associated to it"), and a disassociated member keeps its
/// record (see [`MemberRecord`]) while no longer being associated.
pub const NOT_ASSOCIATED_MESSAGE: &str = "The request is rejected because the current account cannot disassociate the given member account ID since the latter is not yet associated to it.";

/// `UnprocessedAccount.Result` for the administrator's OWN account id.
///
/// `CreateMembers` "creates member accounts of the current Amazon Web Services
/// account by specifying a list of Amazon Web Services account IDs"
/// (`smithy_query(query="doc",shape="CreateMembers")`): the current account is the
/// administrator in that relationship, so it cannot also be one of its own members.
pub const SELF_AS_MEMBER_MESSAGE: &str =
    "The request is rejected because the current account cannot be used as a member account.";

/// How many member accounts one administrator's detector holds.
///
/// GuardDuty's own quota is 5,000 member accounts per administrator account per
/// Region, and the store is bounded by it rather than by nothing: a member record
/// is keyed by a REQUEST-chosen account id, so without a bound a caller repeating
/// `CreateMembers` with fresh ids could grow the store without limit. Dropping a
/// member is not an option the way evicting an old finding is — the records are
/// retained on purpose (see [`MemberRecord`]) — so the quota is enforced by
/// REFUSING the accounts past it, which is what the real quota does.
pub const MAX_MEMBERS_PER_DETECTOR: usize = 5_000;

/// `UnprocessedAccount.Result` for an account that would take the administrator
/// past [`MAX_MEMBERS_PER_DETECTOR`] member accounts.
pub fn member_quota_message() -> String {
    format!(
        "The request is rejected because the current account cannot have more than {MAX_MEMBERS_PER_DETECTOR} member accounts."
    )
}

/// The lower bound on an `AccountDetail.Email`: "the email address must be a
/// minimum of 6 and a maximum of 64 characters long"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). The model
/// carries `len 1..64`, so only the maximum is refused at the boundary.
pub const EMAIL_MIN_LEN: usize = 6;

/// The characters the local name of an email address cannot contain, as the member
/// documents them: "whitespace, \" ' ( ) < > [ ] : ' , \\ | % &"
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`). Whitespace
/// is checked separately, since it is a class rather than a character.
pub const EMAIL_LOCAL_FORBIDDEN: [char; 14] = [
    '"', '\'', '(', ')', '<', '>', '[', ']', ':', ',', '\\', '|', '%', '&',
];

/// Is `email` a valid `AccountDetail.Email`?
///
/// Every rule is one the member itself documents
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`), in the order
/// it lists them: 6..64 characters, 7-bit ASCII only, exactly one `@` separating
/// the local name from the domain name, no [`EMAIL_LOCAL_FORBIDDEN`] character or
/// whitespace and no leading dot in the local name, and a domain name of
/// `[a-zA-Z0-9]`, hyphen and dot only that neither begins nor ends with a dot or a
/// hyphen and contains at least one dot.
pub fn is_member_email(email: &str) -> bool {
    if email.len() < EMAIL_MIN_LEN || !email.is_ascii() {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let local_ok = !local.is_empty()
        && !local.starts_with('.')
        && !local
            .chars()
            .any(|c| c.is_whitespace() || EMAIL_LOCAL_FORBIDDEN.contains(&c));
    let domain_ok = !domain.is_empty()
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain.contains('.');
    local_ok && domain_ok
}

/// `Member.RelationshipStatus` for an account `CreateMembers` has recorded and no
/// invitation has been sent to yet.
///
/// The member targets a plain `String` — "the status of the relationship between
/// the member and the administrator"
/// (`smithy_query(query="doc",shape="Member",member="RelationshipStatus")`) — so
/// the values are GuardDuty's documented relationship statuses, which are
/// mixed-case words rather than the SCREAMING_CASE of this service's modelled
/// enums.
pub const RELATIONSHIP_CREATED: &str = "Created";

/// `Member.RelationshipStatus` for a member the administrator disassociated with
/// `DisassociateMembers`. See [`RELATIONSHIP_CREATED`] for where the values come
/// from.
pub const RELATIONSHIP_REMOVED: &str = "Removed";

/// The message a request is DENIED with when the account is not authorized on the
/// resource `member` names.
///
/// Like [`not_owned_message`], it names the member and not the caller's value: a
/// denial must not disclose whether the id names anything at all.
pub fn not_authorized_message(member: &str) -> String {
    format!(
        "The request is rejected because the current account is not authorized to perform this operation on the input {member}."
    )
}

/// Classify a `DetectorId` the store does not resolve: a value that is not a
/// detector id at all is a parameter failure, a well-formed one this
/// account/Region does not hold is an ownership failure.
pub fn detector_id_rejection(detector_id: &str) -> String {
    if is_detector_id(detector_id) {
        not_owned_message(DETECTOR_ID_MEMBER)
    } else {
        invalid_parameter_message(DETECTOR_ID_MEMBER)
    }
}

/// The wire name of the filter-name member on the operations that carry it as a
/// URI label — `GetFilter`, `UpdateFilter` and `DeleteFilter` bind
/// `FilterName` to `/detector/{DetectorId}/filter/{FilterName}`
/// (`smithy_query(operation="GetFilter")`).
pub const FILTER_NAME_MEMBER: &str = "filterName";

/// The wire name `CreateFilter` carries the filter name under: its `Name` member
/// has `jsonName` `name` (`smithy_query(operation="CreateFilter")`). A rejection
/// names the member AS THE REQUEST CARRIES IT, which is why creating and reading
/// a filter name it differently.
pub const CREATE_FILTER_NAME_MEMBER: &str = "name";

/// The `FilterName` length bounds (`smithy_query(shape="FilterName")`: len
/// 3..64). `GetFilter`/`UpdateFilter`/`DeleteFilter` take the name as a plain
/// `String` URI label with no constraint of its own, so a name outside these
/// bounds reaches the handler there and is classified by [`is_filter_name`].
pub const FILTER_NAME_MIN_LEN: usize = 3;
/// See [`FILTER_NAME_MIN_LEN`].
pub const FILTER_NAME_MAX_LEN: usize = 64;

/// Is `name` a syntactically well-formed `FilterName`?
///
/// "Valid characters include period (.), underscore (_), dash (-), and
/// alphanumeric characters. A whitespace is considered to be an invalid
/// character" (`smithy_query(query="doc",shape="CreateFilterRequest",member="Name")`),
/// within the modelled length bounds. Form only: a well-formed name need not
/// exist.
pub fn is_filter_name(name: &str) -> bool {
    (FILTER_NAME_MIN_LEN..=FILTER_NAME_MAX_LEN).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Classify a filter name the detector does not hold, the way
/// [`detector_id_rejection`] classifies a detector id: a value that cannot be a
/// filter name at all is a parameter failure, a well-formed one this account
/// does not hold is an ownership failure.
pub fn filter_name_rejection(name: &str) -> String {
    if is_filter_name(name) {
        not_owned_message(FILTER_NAME_MEMBER)
    } else {
        invalid_parameter_message(FILTER_NAME_MEMBER)
    }
}

/// The message a second `CreateFilter` under a name the detector already holds
/// is refused with. Creating is not updating — `UpdateFilter` is the operation
/// that changes an existing filter — and the rejection does not quote the name
/// back at the caller.
pub const FILTER_ALREADY_EXISTS_MESSAGE: &str =
    "The request is rejected because a filter with the same name already exists.";

/// "The maximum number of saved filters per Amazon Web Services account per
/// Region is 100" (`smithy_query(query="doc",shape="CreateFilter")`). A detector
/// is itself one per account per Region, so the per-detector count IS the
/// per-account-per-Region count.
pub const MAX_FILTERS_PER_ACCOUNT_PER_REGION: usize = 100;

/// The message a `CreateFilter` past [`MAX_FILTERS_PER_ACCOUNT_PER_REGION`] is
/// refused with.
pub const FILTER_QUOTA_EXCEEDED_MESSAGE: &str =
    "The request is rejected because the maximum number of saved filters per account per Region has been reached.";

/// The `FilterAction` a filter gets when the request names none: "Default: NOOP"
/// (`smithy_query(query="doc",shape="CreateFilterRequest",member="Action")`).
pub const FILTER_ACTION_NOOP: &str = "NOOP";

/// The `FilterVersion` a freshly created filter carries. `FilterVersion` is
/// `range min 1` (`smithy_query(shape="FilterVersion")`), so a filter nothing has
/// updated is at version 1.
pub const FILTER_INITIAL_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Modelled-constraint refusals
// ---------------------------------------------------------------------------

/// How GuardDuty states a refusal the request boundary made: a member the model
/// requires was absent, or a value fell outside a bound the model declares.
///
/// GuardDuty's model declares exactly two errors for every operation —
/// `BadRequestException` (`httpError` 400) and `InternalServerErrorException`
/// (500) (`smithy_query(query="service_info")` lists them as the service's common
/// errors) — and NO `ValidationException` shape. So a modelled-constraint
/// violation is the same refusal as any other unacceptable input, and is worded
/// the same way: the parameter at fault and the rule it broke, built from the
/// REQUEST, with the caller's value never quoted back.
///
/// `service.rs` routes the framework's pre-handler refusal through here, so a
/// constraint the model declares on ANY member of ANY operation in this service
/// is refused under the modelled error rather than under a shape no GuardDuty
/// client can catch.
pub fn constraint_rejection(violations: &[::aws_smithy_schema_server::Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    if clauses.is_empty() {
        // The framework describes every refusal it hands us with at least one
        // violation; an empty list has no member to name, so it falls back to the
        // service's parameter wording without one.
        return "The request is rejected because it contains an invalid parameter.".to_string();
    }
    clauses.join(" ")
}

/// One violation, as a GuardDuty sentence.
fn violation_clause(violation: &::aws_smithy_schema_server::Violation) -> String {
    use ::aws_smithy_schema_server::ViolationKind;

    // The path is already the member's wire spelling (lowerCamel, with
    // `.member` steps for collections), which is how the caller named it.
    let member = violation.path.as_str();
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => {
            return format!(
                "The request is rejected because the parameter {member} is required."
            )
        }
        ViolationKind::MinLength(min) => format!("it must have a length of at least {min}"),
        ViolationKind::MaxLength(max) => format!("it must have a length of at most {max}"),
        ViolationKind::MinValue(min) => format!("it must be greater than or equal to {min}"),
        ViolationKind::MaxValue(max) => format!("it must be less than or equal to {max}"),
        ViolationKind::Pattern(pattern) => {
            format!("it must satisfy the pattern {pattern}")
        }
        ViolationKind::EnumValue(allowed) => format!("it must be one of {}", allowed.join(", ")),
        ViolationKind::UniqueItems => "its values must be unique".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a class this service has no
        // specific wording for still states the member at fault.
        _ => return invalid_parameter_message(member),
    };
    format!(
        "The request is rejected because the parameter {member} has an invalid value: {requirement}."
    )
}

/// The modelled server fault a handler answers when the engine it was built over
/// holds no state for this service.
///
/// `Type` is "The error type" (`smithy_query(shape="InternalServerErrorException")`)
/// — the error class, the same value the body's `__type` discriminator carries.
pub fn state_unavailable() -> crate::ops::types::shared_types::InternalServerErrorException {
    crate::ops::types::shared_types::InternalServerErrorException {
        message: Some("the GuardDuty state store is unavailable".to_string()),
        r#type: Some("InternalServerErrorException".to_string()),
    }
}

/// A configured detector feature and when its status last changed.
#[derive(Debug, Clone)]
pub struct FeatureRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
    pub additional_configuration: Vec<AdditionalConfigRecord>,
}

/// A feature's nested additional configuration (e.g. a Runtime Monitoring agent
/// management switch).
#[derive(Debug, Clone)]
pub struct AdditionalConfigRecord {
    pub name: String,
    pub status: String,
    pub updated_at_millis: i64,
}

/// A GuardDuty detector: the account/Region-level resource the whole service hangs
/// off.
#[derive(Debug, Clone)]
pub struct Detector {
    pub id: String,
    /// The Region whose endpoint created this detector, and the only one that
    /// serves it.
    pub region: String,
    /// `DetectorStatus`: [`ENABLED`] or [`DISABLED`].
    pub status: String,
    pub service_role: String,
    /// The Malware Protection service-linked role, which is a DIFFERENT role from
    /// the detector-level [`Self::service_role`]:
    /// `MalwareProtectionConfigurationResult.ServiceRole` is "The GuardDuty Malware
    /// Protection service role"
    /// (`smithy_query(shape="MalwareProtectionConfigurationResult")`).
    pub malware_protection_service_role: String,
    pub finding_publishing_frequency: Option<String>,
    pub features: Vec<FeatureRecord>,
    pub tags: BTreeMap<String, String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// This detector's findings, keyed by finding id.
    pub findings: BTreeMap<String, FindingRecord>,
    /// This detector's filters, keyed by filter name. `DeleteDetector` drops them
    /// with the detector.
    pub filters: BTreeMap<String, FilterRecord>,
    /// This detector's investigations, keyed by investigation id. They hang off the
    /// detector like its findings and filters, so `DeleteDetector` drops them with
    /// it.
    pub investigations: BTreeMap<String, InvestigationRecord>,
    /// This detector's IPSets (trusted IP lists), keyed by the minted `IpSetId`.
    /// They are sub-resources of the detector — every IPSet operation is bound
    /// under `/detector/{DetectorId}/ipset`
    /// (`smithy_query(operation="GetIPSet")`) — so `DeleteDetector` drops them
    /// with it.
    pub ip_sets: BTreeMap<String, IpSetRecord>,
    /// The member accounts this detector's account administers, keyed by account
    /// id. `DeleteDetector` drops them with the detector, like the findings and
    /// filters above.
    pub members: BTreeMap<String, MemberRecord>,
}

/// A member account of the administrator that owns the detector this record hangs
/// off.
///
/// The record outlives the ASSOCIATION: when a member is disassociated "the member
/// account details obtained from the CreateMembers API, including the associated
/// email addresses, are retained. This is done so that the delegated administrator
/// can invoke the InviteMembers API without the need to invoke the CreateMembers
/// API again. To remove the details associated with a member account, the delegated
/// administrator must invoke the DeleteMembers API"
/// (`smithy_query(query="doc",shape="DisassociateMembers")`). So disassociating
/// moves [`Self::relationship_status`] and nothing else, and only `DeleteMembers`
/// drops the record.
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub account_id: String,
    /// The member account's email address, as the request that recorded the account
    /// gave it.
    pub email: String,
    /// `Member.RelationshipStatus`: [`RELATIONSHIP_CREATED`] or
    /// [`RELATIONSHIP_REMOVED`].
    pub relationship_status: String,
    /// When the relationship last changed, in epoch milliseconds — the member's
    /// `UpdatedAt`.
    pub updated_at_millis: i64,
}

impl MemberRecord {
    /// Is this member currently ASSOCIATED with the administrator?
    ///
    /// "Is a member account of this administrator" and "is associated with it" are
    /// two different questions, because the record is retained across a
    /// disassociation (see the type's documentation): `GetMembers` answers the
    /// first, `DisassociateMembers` acts on the second.
    pub fn is_associated(&self) -> bool {
        self.relationship_status != RELATIONSHIP_REMOVED
    }
}

/// The modelled `Member` a read reports for one stored member account.
///
/// `ListMembers` and `GetMembers` both report the `Members` list, so the projection
/// lives here once: two reads of one member must not disagree.
///
/// `MasterId` and `AdministratorId` are both "the administrator account ID"
/// (`smithy_query(shape="Member")`) — the same fact under the old and the current
/// name — so both carry the account that owns the detector this member hangs off.
pub fn member_view(
    member: &MemberRecord,
    administrator: &str,
) -> crate::ops::types::shared_types::Member {
    crate::ops::types::shared_types::Member {
        account_id: member.account_id.clone(),
        // The member account's own detector is not a resource this emulator holds;
        // the member is optional, so it is absent rather than invented.
        detector_id: None,
        master_id: administrator.to_string(),
        administrator_id: Some(administrator.to_string()),
        email: member.email.clone(),
        relationship_status: member.relationship_status.clone(),
        // No invitation is modelled yet (`InviteMembers` is still a scaffold), so
        // there is no instant at which one was sent.
        invited_at: None,
        updated_at: iso8601(member.updated_at_millis),
    }
}

impl Detector {
    /// The status of one feature, or [`DISABLED`] when the detector does not carry
    /// it.
    pub fn feature_status(&self, name: &str) -> &str {
        self.features
            .iter()
            .find(|f| f.name == name)
            .map_or(DISABLED, |f| f.status.as_str())
    }

    /// What one data source reports: the status of the feature that governs it and
    /// the instant that feature's status last changed, in epoch milliseconds.
    ///
    /// The status is the feature's OWN status, never a function of the detector's
    /// `Status` member: that member carries whether the detector is enabled, while
    /// the feature and data-source views carry the configuration, which a detector
    /// keeps across being disabled and enabled again.
    pub fn data_source(&self, feature: &str) -> (String, i64) {
        self.features.iter().find(|f| f.name == feature).map_or_else(
            || (DISABLED.to_string(), self.updated_at_millis),
            |f| (f.status.clone(), f.updated_at_millis),
        )
    }

    /// Set one feature's status, recording `now` as its `UpdatedAt`. Adds the
    /// feature when the detector does not carry it yet.
    pub fn set_feature(&mut self, name: &str, status: &str, now: i64) {
        if let Some(existing) = self.features.iter_mut().find(|f| f.name == name) {
            if existing.status != status {
                existing.status = status.to_string();
                existing.updated_at_millis = now;
            }
        } else {
            self.features.push(FeatureRecord {
                name: name.to_string(),
                status: status.to_string(),
                updated_at_millis: now,
                additional_configuration: Vec::new(),
            });
        }
    }

    /// Put every named finding this detector holds into the `archived` state,
    /// recording `now` as the `UpdatedAt` of each finding the call actually moves.
    ///
    /// Archiving and unarchiving are the two directions of ONE write over the
    /// `FindingIds` a request names (`smithy_query(operation="ArchiveFindings")` /
    /// `("UnarchiveFindings")`), so both go through here and the LAST call to land
    /// decides the state. A finding already in the requested state is left
    /// untouched — `UpdatedAt` included, which is what makes a repeated call a true
    /// no-op — and an id this detector does not hold moves nothing, the same way a
    /// findings read reports only the ids it holds.
    ///
    /// The archive state is a property of the RECORD, not an observation of the
    /// activity, so only [`FindingRecord::updated_at_millis`] moves:
    /// [`FindingRecord::event_last_seen_millis`] stays where the last observation
    /// left it.
    pub fn set_findings_archived(&mut self, finding_ids: &[String], archived: bool, now: i64) {
        for id in finding_ids {
            if let Some(finding) = self.findings.get_mut(id) {
                if finding.archived != archived {
                    finding.archived = archived;
                    finding.updated_at_millis = now;
                }
            }
        }
    }

    /// The investigation a create already minted for `token`, if any — the seam
    /// that makes `CreateInvestigation` idempotent under a replayed `ClientToken`.
    pub fn investigation_for_token(&self, token: &str) -> Option<&InvestigationRecord> {
        self.investigations
            .values()
            .find(|investigation| investigation.client_token.as_deref() == Some(token))
    }

    /// The member account ids this detector administers, in account-id order.
    ///
    /// `only_associated` narrows the answer to the members that are ASSOCIATED
    /// with the administrator ([`MemberRecord::is_associated`]), which is the
    /// distinction `ListMembers` exposes: "whether to only return associated
    /// members or to return all members (including members who haven't been
    /// invited yet or have been disassociated)"
    /// (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`).
    pub fn member_ids(&self, only_associated: bool) -> Vec<String> {
        self.members
            .values()
            .filter(|member| !only_associated || member.is_associated())
            .map(|member| member.account_id.clone())
            .collect()
    }

    /// How many investigations this detector holds that were initiated at or after
    /// `since`, in epoch milliseconds.
    pub fn investigations_started_since(&self, since: i64) -> usize {
        self.investigations
            .values()
            .filter(|investigation| investigation.started_at_millis >= since)
            .count()
    }

    /// The IPSet a create already minted for `token`, if any — the seam that makes
    /// `CreateIPSet` idempotent under a replayed `ClientToken`.
    pub fn ip_set_for_token(&self, token: &str) -> Option<&IpSetRecord> {
        self.ip_sets
            .values()
            .find(|ip_set| ip_set.client_token.as_deref() == Some(token))
    }
}

// ---------------------------------------------------------------------------
// IPSets (trusted IP lists)
// ---------------------------------------------------------------------------

/// `IpSetStatus`: the list is being activated, so GuardDuty is not using it yet.
/// One of `INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING
/// | DELETED` (`smithy_query(shape="IpSetStatus")`).
pub const IP_SET_ACTIVATING: &str = "ACTIVATING";

/// `IpSetStatus`: the list is uploaded and in use. See [`IP_SET_ACTIVATING`] for
/// the enum.
pub const IP_SET_ACTIVE: &str = "ACTIVE";

/// `IpSetStatus`: the list is being taken out of use.
pub const IP_SET_DEACTIVATING: &str = "DEACTIVATING";

/// `IpSetStatus`: the list is stored but GuardDuty is not using it.
pub const IP_SET_INACTIVE: &str = "INACTIVE";

/// How long an IPSet stays in a transitional [`IP_SET_ACTIVATING`] /
/// [`IP_SET_DEACTIVATING`] status before the CLOCK owes it the settled status.
///
/// GuardDuty documents no duration for activating a trusted IP list, so this
/// emulator CHOOSES one. It is deliberately non-zero: a caller that reads the
/// IPSet back immediately after creating it still observes the transient status.
pub const IP_SET_TRANSITION_MILLIS: i64 = 500;

/// How many IPSets one detector may hold.
///
/// `ListIPSets` reports a detector's IPSets in a single page: `IpSetIds` is
/// `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListIPSets")`), so a detector's whole collection
/// fits in one page of at most 50 ids. The same number BOUNDS the store: an IPSet
/// is retained until its detector is deleted and has no expiry of its own, so
/// without this maximum a caller could make the service retain records without
/// limit.
pub const IP_SETS_PER_DETECTOR: usize = 50;

/// The message a create past [`IP_SETS_PER_DETECTOR`] is refused with. Like every
/// other rejection here it names the resource class and the limit, never the
/// caller's values.
pub fn ip_set_limit_message() -> String {
    format!(
        "The request is rejected because the detector has reached the limit of {IP_SETS_PER_DETECTOR} IPSets."
    )
}

/// The wire name of the `IpSetId` member, as the request carries it
/// (`smithy_query(operation="GetIPSet")` binds it as the URI label `IpSetId`,
/// rendered `ipSetId` in the JSON body of the sibling operations). It is what a
/// rejection names.
pub const IP_SET_ID_MEMBER: &str = "ipSetId";

/// Classify an `IpSetId` the detector does not hold: a value that is not an id
/// this service mints at all is a parameter failure, a well-formed one the
/// detector does not hold is an ownership failure. Same split as
/// [`detector_id_rejection`], for the same reason — the class comes from the
/// REQUEST, not from the store.
pub fn ip_set_id_rejection(ip_set_id: &str) -> String {
    if is_minted_id(ip_set_id) {
        not_owned_message(IP_SET_ID_MEMBER)
    } else {
        invalid_parameter_message(IP_SET_ID_MEMBER)
    }
}

/// One stored IPSet (trusted IP list). Timestamps are epoch milliseconds, the
/// single source for the instants anything reports.
#[derive(Debug, Clone)]
pub struct IpSetRecord {
    /// The minted `IpSetId`.
    pub id: String,
    /// `Name`: "The user-friendly name for the IPSet"
    /// (`smithy_query(operation="GetIPSet")`), stored verbatim.
    pub name: String,
    /// `IpSetFormat`: the format of the file that contains the IPSet.
    pub format: String,
    /// `Location`: the URI of the file that contains the IPSet.
    pub location: String,
    /// `ExpectedBucketOwner`, which a read reports "only if it was provided
    /// during IPSet creation or update" (`smithy_query(operation="GetIPSet")`).
    pub expected_bucket_owner: Option<String>,
    /// The tags of the IPSet resource.
    pub tags: BTreeMap<String, String>,
    /// `IpSetStatus`, the status of the uploaded file.
    pub status: String,
    /// When the clock owes [`Self::status`] its settled value, while the current
    /// one is transitional. Absent once nothing is owed.
    pub status_due_millis: Option<i64>,
    /// The `ClientToken` the create carried, so a replay returns this same IPSet
    /// instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl IpSetRecord {
    /// An IPSet created at `now`.
    ///
    /// `Activate` is "a Boolean value that indicates whether GuardDuty is to start
    /// using the uploaded IPSet" (`smithy_query(operation="CreateIPSet")`): a list
    /// the caller asked to be used starts in the transitional
    /// [`IP_SET_ACTIVATING`] and the clock owes it [`IP_SET_ACTIVE`], while one the
    /// caller did not ask to be used is stored unused — [`IP_SET_INACTIVE`], which
    /// is settled, so nothing is owed on it.
    #[allow(clippy::too_many_arguments)]
    pub fn created(
        id: String,
        name: String,
        format: String,
        location: String,
        expected_bucket_owner: Option<String>,
        tags: BTreeMap<String, String>,
        activate: bool,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        let mut record = Self {
            id,
            name,
            format,
            location,
            expected_bucket_owner,
            tags,
            status: IP_SET_INACTIVE.to_string(),
            status_due_millis: None,
            client_token,
            created_at_millis: now,
            updated_at_millis: now,
        };
        record.set_activation(activate, now);
        record
    }

    /// Start the transition the caller's `Activate` asks for, from whatever status
    /// this IPSet currently has.
    ///
    /// Asking for the status it already has owes nothing, so a repeated request
    /// leaves the settled status alone instead of re-opening a transition.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        let (settled, transitional) = if activate {
            (IP_SET_ACTIVE, IP_SET_ACTIVATING)
        } else {
            (IP_SET_INACTIVE, IP_SET_DEACTIVATING)
        };
        if self.status == settled || self.status == transitional {
            return;
        }
        self.status = transitional.to_string();
        self.status_due_millis = Some(now.saturating_add(IP_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of a transitional status was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, which is why a
    /// reporting operation may apply it. It is applied exactly once: a settled
    /// status never moves on its own again.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.status_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        self.status = match self.status.as_str() {
            IP_SET_ACTIVATING => IP_SET_ACTIVE.to_string(),
            IP_SET_DEACTIVATING => IP_SET_INACTIVE.to_string(),
            // Nothing else is transitional, so nothing else is owed a move.
            other => other.to_string(),
        };
        self.status_due_millis = None;
    }
}

// ---------------------------------------------------------------------------
// ListMembers
// ---------------------------------------------------------------------------

/// The wire name of `ListMembers`'s `OnlyAssociated` query parameter
/// (`smithy_query(operation="ListMembers")`), for the same reason as
/// [`DETECTOR_ID_MEMBER`]: it is what a rejection names.
pub const ONLY_ASSOCIATED_MEMBER: &str = "onlyAssociated";

/// How GuardDuty refuses an `AccountDetail.Email` that is not formatted the way
/// [`is_member_email`]'s documented rules require: the service's own wording for
/// that rule, which names the member it refuses rather than echoing the value.
pub const MEMBER_EMAIL_FORMAT_MESSAGE: &str =
    "The request failed because provided `emailId` was formatted incorrectly.";

// ---------------------------------------------------------------------------
// Investigations
// ---------------------------------------------------------------------------

/// `InvestigationStatus`, the status a created investigation starts in: the
/// analysis is under way. One of `RUNNING | COMPLETED | FAILED`
/// (`smithy_query(shape="InvestigationStatus")`).
pub const INVESTIGATION_RUNNING: &str = "RUNNING";

/// The `InvestigationStatus` an investigation reaches when its analysis could not
/// be performed. See [`INVESTIGATION_RUNNING`] for the enum.
pub const INVESTIGATION_FAILED: &str = "FAILED";

/// How long a created investigation stays [`INVESTIGATION_RUNNING`] before the
/// clock owes it a terminal status.
///
/// GuardDuty documents no duration for an investigation, so this emulator CHOOSES
/// one. It is deliberately non-zero: a caller that reads the investigation back
/// immediately after creating it still observes the transient `RUNNING` status.
pub const INVESTIGATION_TRANSITION_MILLIS: i64 = 500;

/// The window the per-day investigation limit is counted over: "you can initiate
/// up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATION_DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// How many investigations an account may initiate per day: "During the preview,
/// you can initiate up to 10 investigations per account per day"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_DAY: usize = 10;

/// How many investigations an account may initiate in total: "with a total limit
/// of 100 investigations per account"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATIONS_PER_ACCOUNT: usize = 100;

/// The Regions GuardDuty Investigation is available in, as `CreateInvestigation`
/// and `GetInvestigation` both list them: "US East (N. Virginia), US East (Ohio),
/// US West (Oregon), Canada (Central), Europe (Frankfurt), Europe (Ireland),
/// Europe (London), Europe (Paris), Europe (Stockholm), and Asia Pacific (Tokyo)"
/// (`smithy_query(query="doc",shape="CreateInvestigation")`), in that order.
pub const INVESTIGATION_REGIONS: [&str; 10] = [
    "us-east-1",
    "us-east-2",
    "us-west-2",
    "ca-central-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "ap-northeast-1",
];

/// Is GuardDuty Investigation available through `region`'s endpoint? Decided from
/// the documented [`INVESTIGATION_REGIONS`] catalogue, not from what the store
/// happens to hold.
pub fn investigations_available_in(region: &str) -> bool {
    INVESTIGATION_REGIONS.contains(&region)
}

/// The message a request to a Region the feature is not available in is refused
/// with. It does not name the caller's Region: the endpoint already carries it.
pub const INVESTIGATION_REGION_MESSAGE: &str =
    "The request is rejected because GuardDuty Investigation is not available in the current Region.";

/// `CloudProvider`, the only value the enum carries: "The cloud provider.
/// Currently, only AWS is supported" (`smithy_query(shape="CloudDetails")`).
pub const CLOUD_PROVIDER: &str = "AWS";

/// The message `GetInvestigation` answers an `InvestigationId` this detector does
/// not hold with. GuardDuty does not echo the caller's id back, so this names the
/// resource class only.
pub const INVESTIGATION_NOT_FOUND_MESSAGE: &str =
    "The request is rejected because the requested investigation could not be found.";

/// The message `CreateInvestigation` is denied with when the detector does not
/// have the AI_ANALYST feature enabled — the feature the operation requires
/// (`smithy_query(query="doc",shape="CreateInvestigation")`).
pub const INVESTIGATION_FEATURE_DENIED_MESSAGE: &str =
    "The request is rejected because the AI_ANALYST feature is not enabled for the detector.";

/// The message a request past the documented per-day investigation limit is
/// refused with.
pub fn investigation_daily_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_DAY} investigations per day."
    )
}

/// The message a request past the documented total investigation limit is refused
/// with.
pub fn investigation_total_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {INVESTIGATIONS_PER_ACCOUNT} investigations."
    )
}

/// Why an investigation this emulator accepted ends [`INVESTIGATION_FAILED`].
///
/// A GuardDuty investigation is produced by Amazon Bedrock models over the
/// account's findings (`smithy_query(query="doc",shape="CreateInvestigation")`).
/// This emulator has no such analysis engine, so it does NOT synthesise a summary,
/// a risk level or a confidence: the run it cannot perform ends in the modelled
/// failure state, carrying the reason it could not run.
pub const INVESTIGATION_ENGINE_UNAVAILABLE: &str = "The investigation analysis engine is not available in this emulator, so no investigation summary was produced.";

/// One stored investigation. Timestamps are epoch milliseconds, the single source
/// for the instants the wire carries.
#[derive(Debug, Clone)]
pub struct InvestigationRecord {
    pub id: String,
    /// `TriggerPrompt`: what the caller asked to be investigated, stored verbatim.
    pub trigger_prompt: String,
    /// `TriggeredBy`: "The account that initiated the investigation"
    /// (`smithy_query(shape="Investigation")`).
    pub triggered_by: String,
    /// The Region the investigation was initiated through.
    pub region: String,
    /// The `ClientToken` the create carried, so a replay returns this same
    /// investigation instead of initiating a second one.
    pub client_token: Option<String>,
    /// `InvestigationStatus`.
    pub status: String,
    pub started_at_millis: i64,
    /// When the clock owes this investigation its terminal status. See
    /// [`INVESTIGATION_TRANSITION_MILLIS`].
    pub terminal_due_millis: i64,
    /// When it reached a terminal status, once it has.
    pub ended_at_millis: Option<i64>,
    /// `Error`: "Details about the error if the investigation status is FAILED"
    /// (`smithy_query(shape="Investigation")`).
    pub error: Option<String>,
}

impl InvestigationRecord {
    /// An investigation initiated at `now`, still [`INVESTIGATION_RUNNING`].
    pub fn started(
        id: String,
        trigger_prompt: String,
        triggered_by: String,
        region: String,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        Self {
            id,
            trigger_prompt,
            triggered_by,
            region,
            client_token,
            status: INVESTIGATION_RUNNING.to_string(),
            started_at_millis: now,
            terminal_due_millis: now.saturating_add(INVESTIGATION_TRANSITION_MILLIS),
            ended_at_millis: None,
            error: None,
        }
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of `RUNNING` was owed at [`Self::terminal_due_millis`] whether
    /// or not anybody called, which is why a reporting operation may apply it. It
    /// is applied exactly once: a terminal investigation never moves again.
    pub fn settle(&mut self, now: i64) {
        if self.status != INVESTIGATION_RUNNING || now < self.terminal_due_millis {
            return;
        }
        self.status = INVESTIGATION_FAILED.to_string();
        self.ended_at_millis = Some(self.terminal_due_millis);
        self.error = Some(INVESTIGATION_ENGINE_UNAVAILABLE.to_string());
    }
}

/// A saved finding filter. `CreateFilter` writes it, `GetFilter` reports it and
/// `DeleteDetector`'s cascade drops it with its detector; `ListFilters` /
/// `UpdateFilter` / `DeleteFilter` are still scaffolded.
#[derive(Debug, Clone, Default)]
pub struct FilterRecord {
    pub name: String,
    pub description: Option<String>,
    /// `FilterAction`: `ARCHIVE` or [`FILTER_ACTION_NOOP`].
    pub action: Option<String>,
    pub rank: Option<i32>,
    /// The criteria the filter selects findings by, kept as the modelled shape so
    /// a read reports exactly what the create stored.
    pub finding_criteria: crate::ops::types::FindingCriteria,
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` the create carried, so a replayed `CreateFilter`
    /// returns the filter the first call saved instead of refusing the name.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `FilterVersion`: 1 for a filter nothing has updated.
    pub version: i64,
}

/// One stored finding. Timestamps are kept as epoch milliseconds — the single
/// source for both the ISO-8601 strings the wire carries and the numeric
/// comparisons `FindingCriteria` makes.
#[derive(Debug, Clone)]
pub struct FindingRecord {
    pub id: String,
    pub arn: String,
    pub account_id: String,
    pub region: String,
    pub partition: String,
    pub schema_version: String,
    pub r#type: String,
    pub severity: f64,
    pub confidence: Option<f64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub resource_type: String,
    pub detector_id: String,
    pub resource_role: String,
    /// `Service.Count`: "The total count of the occurrences of this finding type"
    /// (`smithy_query(shape="Service")`).
    pub count: i32,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
    /// `Service.EventLastSeen`: when the ACTIVITY this finding reports was last
    /// observed, which is a different clock from [`Self::updated_at_millis`] — the
    /// instant the finding RECORD last changed. A recurrence of the activity moves
    /// both; a change to the record's own metadata (its archive state, its
    /// feedback) moves at most the record's clock, because nothing was observed.
    pub event_last_seen_millis: i64,
    pub archived: bool,
    /// True for a finding `CreateSampleFindings` generated.
    pub sample: bool,
    /// `Service.UserFeedback` — "Feedback that was submitted about the finding"
    /// (`smithy_query(shape="Service")`): the `Feedback` value of the latest
    /// `UpdateFindingsFeedback` call that named this finding, and absent until one
    /// does.
    pub user_feedback: Option<String>,
    /// The `Comments` submitted beside [`Self::user_feedback`] — "additional feedback
    /// about the GuardDuty findings"
    /// (`smithy_query(operation="UpdateFindingsFeedback")`). The member is
    /// `@sensitive` and no modelled read reports it (`Service` has no comments
    /// member), so it is kept with the feedback it arrived with rather than published.
    pub feedback_comments: Option<String>,
}

/// One finding attribute, resolved for comparison or sorting.
#[derive(Debug, Clone, PartialEq)]
pub enum Attribute {
    Text(String),
    Number(f64),
}

impl Attribute {
    /// The textual form `Equals` / `NotEquals` compare against: GuardDuty's
    /// `findingCriteria` carries every value as a string, including numeric ones.
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => format_number(*n),
        }
    }

    /// The numeric form the ordering conditions compare against, when the
    /// attribute has one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Text(_) => None,
            Self::Number(n) => Some(*n),
        }
    }
}

/// Render a finding's numeric attribute without a trailing `.0`, so `severity`
/// equality against the string `"8"` behaves the way it does on the real service.
fn format_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The finding attribute paths this emulator can filter and sort on, as the
/// `Finding` shape's `jsonName`s name them
/// (`smithy_query(shape="Finding")`), plus the nested paths GuardDuty's own filter
/// documentation uses.
pub const FINDING_ATTRIBUTES: [&str; 16] = [
    "accountId",
    "arn",
    "confidence",
    "createdAt",
    "description",
    "id",
    "partition",
    "region",
    "resource.resourceType",
    "schemaVersion",
    "service.archived",
    "service.detectorId",
    "severity",
    "title",
    "type",
    "updatedAt",
];

/// Can this emulator evaluate `path` as a finding attribute? Decided from the
/// catalogue rather than from a sample record, so an empty result set refuses an
/// attribute path exactly as a populated one does.
pub fn supports_attribute(path: &str) -> bool {
    FINDING_ATTRIBUTES.contains(&path)
}

/// Resolve one finding attribute by its wire path, or `None` when this emulator
/// does not model that path.
///
/// `updatedAt` is accepted as well as `createdAt`; both are compared as epoch
/// milliseconds by the ordering conditions and as ISO-8601 text by the equality
/// ones, which is how GuardDuty's own filters treat them.
pub fn finding_attribute(finding: &FindingRecord, path: &str) -> Option<Attribute> {
    let attr = match path {
        "accountId" => Attribute::Text(finding.account_id.clone()),
        "arn" => Attribute::Text(finding.arn.clone()),
        "confidence" => Attribute::Number(finding.confidence.unwrap_or(0.0)),
        "createdAt" => Attribute::Number(finding.created_at_millis as f64),
        "updatedAt" => Attribute::Number(finding.updated_at_millis as f64),
        "description" => Attribute::Text(finding.description.clone().unwrap_or_default()),
        "id" => Attribute::Text(finding.id.clone()),
        "partition" => Attribute::Text(finding.partition.clone()),
        "region" => Attribute::Text(finding.region.clone()),
        "resource.resourceType" => Attribute::Text(finding.resource_type.clone()),
        "schemaVersion" => Attribute::Text(finding.schema_version.clone()),
        "service.archived" => Attribute::Text(finding.archived.to_string()),
        "service.detectorId" => Attribute::Text(finding.detector_id.clone()),
        "severity" => Attribute::Number(finding.severity),
        "title" => Attribute::Text(finding.title.clone().unwrap_or_default()),
        "type" => Attribute::Text(finding.r#type.clone()),
        _ => return None,
    };
    Some(attr)
}

/// A finding attribute rendered for EQUALITY comparison. `createdAt` /
/// `updatedAt` compare as their ISO-8601 text, every other attribute as its own
/// textual form.
fn equality_text(finding: &FindingRecord, path: &str) -> Option<String> {
    match path {
        "createdAt" => Some(iso8601(finding.created_at_millis)),
        "updatedAt" => Some(iso8601(finding.updated_at_millis)),
        _ => finding_attribute(finding, path).map(|a| a.as_text()),
    }
}

/// Does `finding` satisfy one `Condition` on `path`?
///
/// `Err(path)` means the emulator does not model that attribute path, which the
/// caller turns into a refusal rather than silently returning every finding.
/// Within one condition every populated operator must hold, which is how
/// GuardDuty composes a criterion.
pub fn condition_matches(
    finding: &FindingRecord,
    path: &str,
    condition: &crate::ops::types::Condition,
) -> Result<bool, String> {
    if !supports_attribute(path) {
        return Err(path.to_string());
    }
    let Some(attr) = finding_attribute(finding, path) else {
        return Err(path.to_string());
    };
    let text = equality_text(finding, path).unwrap_or_else(|| attr.as_text());

    // `Equals`/`NotEquals` and the deprecated `Eq`/`Neq` are the same comparison;
    // a list holds alternatives, so `Equals` passes when ANY entry matches and
    // `NotEquals` passes only when NONE does.
    for list in [condition.equals.as_ref(), condition.eq.as_ref()].into_iter().flatten() {
        if !list.is_empty() && !list.contains(&text) {
            return Ok(false);
        }
    }
    for list in [condition.not_equals.as_ref(), condition.neq.as_ref()]
        .into_iter()
        .flatten()
    {
        if list.contains(&text) {
            return Ok(false);
        }
    }

    // The ordering conditions only apply to an attribute with a numeric form.
    let bounds: [(Option<f64>, Ordering); 8] = [
        (condition.greater_than.map(|v| v as f64), Ordering::Gt),
        (condition.gt.map(f64::from), Ordering::Gt),
        (condition.greater_than_or_equal.map(|v| v as f64), Ordering::Gte),
        (condition.gte.map(f64::from), Ordering::Gte),
        (condition.less_than.map(|v| v as f64), Ordering::Lt),
        (condition.lt.map(f64::from), Ordering::Lt),
        (condition.less_than_or_equal.map(|v| v as f64), Ordering::Lte),
        (condition.lte.map(f64::from), Ordering::Lte),
    ];
    for (bound, ordering) in bounds {
        let Some(bound) = bound else { continue };
        let Some(value) = attr.as_number() else {
            return Err(path.to_string());
        };
        let holds = match ordering {
            Ordering::Gt => value > bound,
            Ordering::Gte => value >= bound,
            Ordering::Lt => value < bound,
            Ordering::Lte => value <= bound,
        };
        if !holds {
            return Ok(false);
        }
    }

    // `Matches` / `NotMatches` compare the attribute against a pattern in which
    // `*` stands for any run of characters; like the equality lists, a list holds
    // alternatives.
    if let Some(patterns) = condition.matches.as_ref() {
        if !patterns.is_empty() && !patterns.iter().any(|p| wildcard_matches(p, &text)) {
            return Ok(false);
        }
    }
    if let Some(patterns) = condition.not_matches.as_ref() {
        if patterns.iter().any(|p| wildcard_matches(p, &text)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Does `value` match `pattern`, where `*` in the pattern stands for any run of
/// characters (including none)?
fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return pattern == value;
    };
    if !value.starts_with(first) {
        return false;
    }
    let mut rest = &value[first.len()..];
    let segments: Vec<&str> = segments.collect();
    let Some((last, middle)) = segments.split_last() else {
        // No `*` at all: the pattern is a literal.
        return rest.is_empty();
    };
    for segment in middle {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// Which way an ordering condition compares.
enum Ordering {
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Does `finding` satisfy every criterion in `criteria`? An absent or empty
/// criteria map matches every finding.
pub fn finding_matches(
    finding: &FindingRecord,
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<bool, String> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(true);
    };
    for (path, condition) in criterion {
        if !condition_matches(finding, path, condition)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Refuse a `FindingCriteria` this emulator cannot evaluate, naming the attribute
/// path.
///
/// Checked BEFORE any finding is looked at, so whether a criterion is refused does
/// not depend on how many findings happen to be stored: a detector with none must
/// answer an unevaluable criterion exactly as a populated one does.
pub fn validate_finding_criteria(
    criteria: Option<&crate::ops::types::FindingCriteria>,
) -> Result<(), String> {
    let Some(criterion) = criteria.and_then(|c| c.criterion.as_ref()) else {
        return Ok(());
    };
    for (path, condition) in criterion {
        if !supports_attribute(path) {
            return Err(path.clone());
        }
        let orders_numerically = condition.greater_than.is_some()
            || condition.greater_than_or_equal.is_some()
            || condition.less_than.is_some()
            || condition.less_than_or_equal.is_some()
            || condition.gt.is_some()
            || condition.gte.is_some()
            || condition.lt.is_some()
            || condition.lte.is_some();
        if orders_numerically && !attribute_is_numeric(path) {
            return Err(path.clone());
        }
    }
    Ok(())
}

/// Which finding attributes an ordering condition can compare. `createdAt` /
/// `updatedAt` order as epoch milliseconds, the way GuardDuty's own filters compare
/// them.
fn attribute_is_numeric(path: &str) -> bool {
    matches!(path, "confidence" | "createdAt" | "severity" | "updatedAt")
}

/// Order `findings` for `ListFindings` / `GetFindings`.
///
/// With no `SortCriteria` the newest finding comes first, with the finding id
/// breaking ties so the order is total and a page token stays meaningful.
/// `Err(attribute)` means the emulator does not model that sort attribute.
pub fn sort_findings(
    findings: &mut [&FindingRecord],
    sort: Option<&crate::ops::types::SortCriteria>,
) -> Result<(), String> {
    let attribute = sort
        .and_then(|s| s.attribute_name.as_deref())
        .filter(|a| !a.is_empty());
    let descending = match sort.and_then(|s| s.order_by.as_deref()) {
        Some("ASC") => false,
        Some("DESC") => true,
        // No explicit order: newest-first for the default attribute, ascending for
        // an attribute the caller named.
        _ => attribute.is_none(),
    };
    let attribute = attribute.unwrap_or("createdAt");
    if !supports_attribute(attribute) {
        return Err(attribute.to_string());
    }

    findings.sort_by(|a, b| {
        let key = |f: &FindingRecord| match finding_attribute(f, attribute) {
            Some(Attribute::Number(n)) => (n, String::new()),
            Some(Attribute::Text(t)) => (0.0, t),
            None => (0.0, String::new()),
        };
        let (an, at) = key(a);
        let (bn, bt) = key(b);
        let primary = an
            .partial_cmp(&bn)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| at.cmp(&bt));
        let primary = if descending { primary.reverse() } else { primary };
        primary.then_with(|| a.id.cmp(&b.id))
    });
    Ok(())
}

/// Take one page out of an ordered key list.
///
/// `token` is the number of entries already returned, as a decimal string — the
/// only thing a caller may do with it is hand it back. `Err(token)` means the token
/// was not one this service issued: either it is not a decimal offset at all, or it
/// points past the end of the list, which this service never hands out (it answers
/// the last page with no `NextToken`).
pub fn paginate(
    keys: &[String],
    max_results: Option<i32>,
    token: Option<&str>,
    page_limit: usize,
) -> Result<(Vec<String>, Option<String>), String> {
    let start = match token.filter(|t| !t.is_empty()) {
        Some(t) => {
            let start = t.parse::<usize>().map_err(|_| t.to_string())?;
            // A token this service issued always names an entry that exists: it is
            // emitted only while `consumed < keys.len()`.
            if start >= keys.len() {
                return Err(t.to_string());
            }
            start
        }
        None => 0,
    };
    let limit = max_results
        .and_then(|m| usize::try_from(m).ok())
        .filter(|m| *m > 0)
        .unwrap_or(page_limit)
        .min(page_limit);
    let page: Vec<String> = keys.iter().skip(start).take(limit).cloned().collect();
    let consumed = start.saturating_add(page.len());
    let next = if consumed < keys.len() {
        Some(consumed.to_string())
    } else {
        None
    };
    Ok((page, next))
}

/// The feature set `CreateDetector` gives a detector when the request names no
/// features.
///
/// Every configurable feature is enabled except the [`OPT_IN_FEATURES`]: "When you
/// don't specify any features, with an exception to RUNTIME_MONITORING, all the
/// optional features are enabled by default"
/// (`smithy_query(query="doc",shape="CreateDetector")`). The three
/// [`FOUNDATIONAL_FEATURES`] are not optional and are always enabled.
///
/// The records are created in [`reported_features`] order, which is the order a
/// read reports them in.
pub fn default_features(now: i64) -> Vec<FeatureRecord> {
    reported_features()
        .into_iter()
        .map(|name| FeatureRecord {
            name: name.to_string(),
            status: if OPT_IN_FEATURES.contains(&name) {
                DISABLED.to_string()
            } else {
                ENABLED.to_string()
            },
            updated_at_millis: now,
            additional_configuration: Vec::new(),
        })
        .collect()
}

/// Refuse a `Features` list this service cannot accept, with the message the
/// refusal carries.
///
/// Two rules, both from `CreateDetector`'s own documentation: a feature name must
/// be a `DetectorFeature` member, and EKS Runtime Monitoring and Runtime
/// Monitoring cannot both be requested because Runtime Monitoring already includes
/// Amazon EKS.
pub fn validate_feature_configurations(
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
) -> Result<(), String> {
    let Some(features) = features else {
        return Ok(());
    };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        if !DETECTOR_FEATURES.contains(&name) {
            return Err(format!(
                "Features: '{name}' is not a supported detector feature. Supported features are {}.",
                DETECTOR_FEATURES.join(", ")
            ));
        }
        if let Some(status) = feature.status.as_deref() {
            if status != ENABLED && status != DISABLED {
                return Err(format!(
                    "Features: '{status}' is not a supported status for feature '{name}'. Supported values are {ENABLED}, {DISABLED}."
                ));
            }
        }
    }
    let named = |wanted: &str| {
        features
            .iter()
            .any(|f| f.name.as_deref() == Some(wanted) && f.status.as_deref() != Some(DISABLED))
    };
    if named(FEATURE_EKS_RUNTIME_MONITORING) && named(FEATURE_RUNTIME_MONITORING) {
        return Err(format!(
            "Features: {FEATURE_EKS_RUNTIME_MONITORING} and {FEATURE_RUNTIME_MONITORING} cannot both be enabled because {FEATURE_RUNTIME_MONITORING} already includes threat detection for Amazon EKS resources."
        ));
    }
    Ok(())
}

/// Write a `Features` list onto `detector`. A feature the list does not name keeps
/// the status it already had.
pub fn apply_feature_configurations(
    detector: &mut Detector,
    features: Option<&Vec<crate::ops::types::DetectorFeatureConfiguration>>,
    now: i64,
) {
    let Some(features) = features else { return };
    for feature in features {
        let Some(name) = feature.name.as_deref() else {
            continue;
        };
        let status = feature.status.as_deref().unwrap_or(ENABLED);
        detector.set_feature(name, status, now);
        if let Some(additional) = feature.additional_configuration.as_ref() {
            let configured: Vec<AdditionalConfigRecord> = additional
                .iter()
                .filter_map(|cfg| {
                    cfg.name.as_deref().map(|name| AdditionalConfigRecord {
                        name: name.to_string(),
                        status: cfg.status.as_deref().unwrap_or(ENABLED).to_string(),
                        updated_at_millis: now,
                    })
                })
                .collect();
            if let Some(record) = detector.features.iter_mut().find(|f| f.name == name) {
                record.additional_configuration = configured;
            }
        }
    }
}

/// Write the deprecated `DataSources` view onto `detector`.
///
/// `DataSources` is "deprecated, use Features instead"
/// (`smithy_query(operation="CreateDetector")`), so each data source switch is
/// stored as the feature that governs it — one source of truth, read back through
/// either view.
pub fn apply_data_sources(
    detector: &mut Detector,
    data_sources: Option<&crate::ops::types::DataSourceConfigurations>,
    now: i64,
) {
    let Some(data_sources) = data_sources else {
        return;
    };
    let status = |enable: bool| if enable { ENABLED } else { DISABLED };
    if let Some(s3) = data_sources.s3_logs.as_ref() {
        detector.set_feature(FEATURE_S3_DATA_EVENTS, status(s3.enable), now);
    }
    if let Some(kubernetes) = data_sources.kubernetes.as_ref() {
        detector.set_feature(
            FEATURE_EKS_AUDIT_LOGS,
            status(kubernetes.audit_logs.enable),
            now,
        );
    }
    if let Some(enable) = data_sources
        .malware_protection
        .as_ref()
        .and_then(|m| m.scan_ec2_instance_with_findings.as_ref())
        .and_then(|s| s.ebs_volumes)
    {
        detector.set_feature(FEATURE_EBS_MALWARE_PROTECTION, status(enable), now);
    }
}

/// The AWS Region a request was signed for, which is the Region the resources it
/// creates live in. Falls back to the emulator's default Region for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGuarddutyGuarddutyapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGuarddutyGuarddutyapiserviceState>)
}
