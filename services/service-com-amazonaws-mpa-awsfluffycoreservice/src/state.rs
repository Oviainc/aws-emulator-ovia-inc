// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mpa-awsfluffycoreservice service.
//!
//! The approval team is this service's primary resource, and this module is the one
//! place it is stored: `CreateApprovalTeam` mints a record here, and
//! `GetApprovalTeam` / `ListApprovalTeams` / `UpdateApprovalTeam` /
//! `StartActiveApprovalTeamDeletion` all compute their answers from it.
//!
//! # Policies attached to a resource
//!
//! A team's `Policies` references are also the POLICIES ATTACHED to that team as a
//! resource, so the resource-policy reads (`GetResourcePolicy`) answer from the same
//! records rather than from a second copy, and resolve each reference through the
//! catalog below so that every member they report is the policy record's own:
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::attached_resource_policy`] turns one
//! attached `QualifiedPolicyArn` into the [`ResourcePolicy`] those reads report
//! ([`parse_policy_version_arn`] reading the ARN back, `$DEFAULT` resolving to the
//! policy's current default version), and
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_policies`] /
//! [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_policy`] resolve them for a
//! resource ARN.
//!
//! # The clock, and who materialises it
//!
//! Two changes are owed by the passage of time alone, so they are recorded as DUE
//! INSTANTS on the stored record and materialised by [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`],
//! which every handler calls before it reads or writes. Nothing sleeps.
//!
//! * An invited approver's response ([`INVITATION_RESPONSE_DELAY`]). A team whose every
//!   approver has accepted their invitation is `ACTIVE`; until then it is `PENDING`
//!   with status code `PENDING_ACTIVATION`, which is what AWS reports for a team whose
//!   approvers have not all answered.
//! * The end of a pending deletion window ([`DEFAULT_PENDING_WINDOW_DAYS`]). Once it
//!   elapses the team is gone, so `GetApprovalTeam` answers 404 and
//!   `ListApprovalTeams` omits it.
//!
//! # The policy catalog
//!
//! Policies are AWS's, not the caller's: no operation in this model creates, updates or
//! deletes one, so the catalog is SEEDED when this store is first touched
//! ([`Default`]) and `ListPolicies` reports whatever it then holds. See
//! [`seeded_policy_catalog`].
//!
//! Each catalog entry carries its VERSIONS ([`CatalogPolicy::versions`]), one of which is
//! the entry's default; `ListPolicyVersions` reports them for the policy its `PolicyArn`
//! names and `GetPolicyVersion` reports the one its `PolicyVersionArn` names, resolved
//! through [`ComAmazonawsMpaAwsfluffycoreserviceState::policy_version`]. A version is as
//! immutable as the policy that owns it — nothing in this model mints or retires one — so
//! no reader of the catalog writes to it.
//!
//! # Authorization
//!
//! This module also owns this service's authorization rules, so every handler states
//! them the same way and decides them BEFORE it resolves a resource:
//!
//! * [`authorize_resource_arn`] — an operation that identifies its resource by ARN is
//!   refused with `AccessDeniedException` (403) when that ARN is outside the caller's
//!   own account, whatever is stored under it.
//! * [`ComAmazonawsMpaAwsfluffycoreserviceState::is_management_account`] /
//!   [`not_management_account`] — an operation an organization restricts to its
//!   management account is refused the same way for any other caller.
//!
//! # How this service words a rejection
//!
//! A rejection's wording is part of this service's contract, so the ones MPA states with
//! a FIXED sentence are built here and nowhere else, and every handler raises them
//! through these constructors instead of formatting prose of its own:
//!
//! * [`resource_not_found`] — the one message every ARN-addressed resource lookup in
//!   this service answers with ([`RESOURCE_NOT_FOUND_MESSAGE`]).
//! * [`tagged_resource_not_found`] — the tag family's not-found, whose message IS the
//!   `ResourceArn` the request sent.
//! * [`invalid_arn`] / [`INVALID_ARN_MESSAGE`] — the one message a member that should
//!   carry an Amazon Resource Name and does not is refused with, raised by a handler for
//!   a member the model gives no ARN `@pattern` and by `crate::service`'s refusal seam
//!   ([`is_malformed_arn_violation`]) for the members it does.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{AccessDeniedException, ResourceNotFoundException, ValidationException};

/// Region stamped into a minted ARN when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The IAM action namespace every MPA action carries, from this service's
/// `arnNamespace` / `sigv4Name` (`smithy_query(query="service_info")`: both `mpa`).
/// It is what an authorization refusal names the attempted action with
/// (`mpa:GetApprovalTeam`).
pub const SERVICE_ACTION_PREFIX: &str = "mpa";

/// Longest `AccessDeniedException$Message` this service emits, from that member's
/// modelled `@length(max: 1000)`.
const MAX_ACCESS_DENIED_MESSAGE: usize = 1000;

/// How long after an approver is invited the emulator reports their invitation
/// response (and therefore how long a new team stays `PENDING`).
///
/// CHOSEN, not documented: a real approver answers an invitation whenever they
/// answer it. It sits inside the 250–750 ms window this project fixes for a
/// clock-owed transition, so a caller that reads the team back immediately still
/// observes the transient `PENDING` state and the errors that go with it.
pub const INVITATION_RESPONSE_DELAY: Duration = Duration::from_millis(500);

/// Days between the delete request and the team's removal when
/// `StartActiveApprovalTeamDeletion` omits `PendingWindowDays`.
///
/// `PendingWindowDays` is documented as "Number of days between when the team
/// approves the delete request and when the team is deleted"; AWS applies a 7-day
/// window when the request does not name one.
pub const DEFAULT_PENDING_WINDOW_DAYS: i32 = 7;

/// Seconds in one day, the unit `PendingWindowDays` counts in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Largest pending deletion window this emulator will record, in days (~100 years).
///
/// `PendingWindowDays` carries no modelled `range`, and AWS's own maximum is not in the
/// model, so this is NOT a claim about what the real service accepts: it is the bound
/// past which the window's end would stop being a timestamp this service can report, the
/// response member being an `IsoTimestamp` (`@timestampFormat("date-time")`). A request
/// past it is refused rather than answered with a completion instant that cannot be
/// rendered.
pub const MAX_PENDING_WINDOW_DAYS: i32 = 36_500;

/// Largest number of approval teams the store will hold.
///
/// A ceiling, not a fidelity claim: nothing authenticates a caller of this emulator, so
/// without one a client looping on `CreateApprovalTeam` grows this process's heap (and
/// the per-request [`ComAmazonawsMpaAwsfluffycoreserviceState::materialize_due`] walk)
/// without bound. `CreateApprovalTeam` reports it through the
/// `ServiceQuotaExceededException` (402) the model already declares for it, and
/// `name_index` / `idempotency_tokens` are bounded with it because
/// [`ComAmazonawsMpaAwsfluffycoreserviceState::remove_team`] frees a team's entries in
/// both when the team goes.
pub const MAX_APPROVAL_TEAMS: usize = 1_000;

/// Largest page a paginated list operation of this service returns, from `MaxResults`'
/// modelled `range(min: 1, max: 20)` / `default: 20` — the same shape every one of them
/// binds (`ListApprovalTeams`, `ListPolicies`, …).
pub const MAX_LIST_PAGE: i32 = 20;

/// Largest number of tags one resource holds.
///
/// AWS's tagging limits allow 50 tags per resource, and this service's own model
/// corroborates that number: `UntagResource`'s `TagKeys` is `length(min: 0, max: 50)`
/// (`smithy_query(query="shape",shape="TagKeyList")`), so a single untag call can name
/// every key a resource is able to carry. `TagResource` reports a request that would
/// take a resource past it with the `TooManyTagsException` (400) the model declares for
/// exactly that condition.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Number of tags a resource holding `existing` would hold once `incoming` keys are
/// merged into it.
///
/// Tagging is a per-key upsert, so a key the resource already carries replaces its
/// value and adds nothing to the count; only keys it does not carry yet do.
///
/// The two borrows are independent: `existing` is only read, and only the keys
/// `incoming` adds are held, so `incoming` may be borrowed from anywhere.
#[must_use]
pub fn merged_tag_count<'a, I>(existing: &BTreeMap<String, String>, incoming: I) -> usize
where
    I: IntoIterator<Item = &'a str>,
{
    let added: ::std::collections::BTreeSet<&str> = incoming
        .into_iter()
        .filter(|key| !existing.contains_key(*key))
        .collect();
    existing.len() + added.len()
}

/// The pending deletion window `days` names, or `None` when `days` is not a day count
/// this service can turn into a duration.
#[must_use]
pub fn pending_window(days: i32) -> Option<Duration> {
    u64::try_from(days)
        .ok()?
        .checked_mul(SECONDS_PER_DAY)
        .map(Duration::from_secs)
}

/// The instant a pending deletion window of `days` started at `start` ends, or `None`
/// when `days` names no instant this service can record and report.
///
/// Every step is checked, so no request can make this arithmetic panic the handler.
#[must_use]
pub fn deletion_completion(start: SystemTime, days: i32) -> Option<SystemTime> {
    if !(1..=MAX_PENDING_WINDOW_DAYS).contains(&days) {
        return None;
    }
    start.checked_add(pending_window(days)?)
}

/// Status of an approval team, in the tokens `ApprovalTeamStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamStatus {
    Active,
    Inactive,
    Deleting,
    Pending,
}

impl TeamStatus {
    /// The wire token, as `ApprovalTeamStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deleting => "DELETING",
            Self::Pending => "PENDING",
        }
    }
}

/// Status code of an approval team, in the tokens `ApprovalTeamStatusCode` declares.
/// Only the codes this slice of the lifecycle can reach are modelled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamStatusCode {
    PendingActivation,
    DeletePendingApproval,
}

impl TeamStatusCode {
    /// The wire token, as `ApprovalTeamStatusCode` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::PendingActivation => "PENDING_ACTIVATION",
            Self::DeletePendingApproval => "DELETE_PENDING_APPROVAL",
        }
    }
}

/// Status of an approver's identity, in the tokens `IdentityStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproverIdentityStatus {
    Pending,
    Accepted,
    Rejected,
    Invalid,
}

impl ApproverIdentityStatus {
    /// The wire token, as `IdentityStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Accepted => "ACCEPTED",
            Self::Rejected => "REJECTED",
            Self::Invalid => "INVALID",
        }
    }
}

/// An approver's last activity, in the tokens `ApproverLastActivity` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproverLastActivity {
    Baselined,
    RespondedToInvitation,
    Voted,
}

impl ApproverLastActivity {
    /// The wire token, as `ApproverLastActivity` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Baselined => "BASELINED",
            Self::RespondedToInvitation => "RESPONDED_TO_INVITATION",
            Self::Voted => "VOTED",
        }
    }
}

/// How a team grants approval. Mirrors the `ApprovalStrategy` union, which today
/// declares the single `MofN` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredApprovalStrategy {
    MofN { min_approvals_required: i32 },
}

impl StoredApprovalStrategy {
    /// Minimum approvals (M) the strategy requires.
    #[must_use]
    pub fn min_approvals_required(self) -> i32 {
        match self {
            Self::MofN {
                min_approvals_required,
            } => min_approvals_required,
        }
    }
}

/// One approver on a team.
#[derive(Debug, Clone)]
pub struct Approver {
    /// Service-minted id for this participant, stable for as long as the approver
    /// stays on the team.
    pub approver_id: String,
    pub primary_identity_id: String,
    pub primary_identity_source_arn: String,
    pub primary_identity_status: ApproverIdentityStatus,
    pub response_time: Option<SystemTime>,
    pub last_activity: Option<ApproverLastActivity>,
    pub last_activity_time: Option<SystemTime>,
    /// Instant at which this approver's invitation response becomes due. `None`
    /// once it has been materialised.
    pub invitation_response_due: Option<SystemTime>,
}

impl Approver {
    /// A freshly invited approver: `PENDING` until the clock owes its response.
    #[must_use]
    pub fn invited(
        approver_id: String,
        primary_identity_id: String,
        primary_identity_source_arn: String,
        now: SystemTime,
    ) -> Self {
        Self {
            approver_id,
            primary_identity_id,
            primary_identity_source_arn,
            primary_identity_status: ApproverIdentityStatus::Pending,
            response_time: None,
            last_activity: None,
            last_activity_time: None,
            invitation_response_due: Some(now + INVITATION_RESPONSE_DELAY),
        }
    }
}

/// A stored approval team: everything the lifecycle operations report.
#[derive(Debug, Clone)]
pub struct ApprovalTeam {
    pub arn: String,
    pub name: String,
    pub description: String,
    pub approval_strategy: StoredApprovalStrategy,
    pub approvers: Vec<Approver>,
    /// The `PolicyArn` of each attached policy, in the order the request listed them.
    pub policies: Vec<String>,
    /// Wire version id: the decimal rendering of [`ApprovalTeam::version`].
    pub version_id: String,
    /// Monotonic version counter, bumped by every applied update.
    pub version: u64,
    pub status: TeamStatus,
    pub status_code: Option<TeamStatusCode>,
    pub status_message: Option<String>,
    pub creation_time: SystemTime,
    pub last_update_time: Option<SystemTime>,
    pub deletion_start_time: Option<SystemTime>,
    /// Instant at which the pending deletion window ends and the team is removed.
    pub deletion_completion_time: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// Creation order, the stable ordering `ListApprovalTeams` pages over.
    pub seq: u64,
}

impl ApprovalTeam {
    /// Record a new version of the team: bump the version id and the update time.
    pub fn bump_version(&mut self, now: SystemTime) {
        self.version += 1;
        self.version_id = self.version.to_string();
        self.last_update_time = Some(now);
    }

    /// Total number of approvers (N), as `NumberOfApprovers` reports it.
    #[must_use]
    pub fn number_of_approvers(&self) -> i32 {
        i32::try_from(self.approvers.len()).unwrap_or(i32::MAX)
    }
}

/// Longest category or name segment a policy ARN may carry, from the
/// `[a-zA-Z0-9_\.-]{1,1023}` segments `UnqualifiedPolicyArn`'s modelled pattern declares.
const MAX_POLICY_ARN_SEGMENT: usize = 1023;

/// The version segment that stands for "whichever version is this policy's default" —
/// the alternative `QualifiedPolicyArn`'s pattern allows beside a decimal version id
/// (`^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\.-]{1,1023}/[a-zA-Z0-9_\.-]{1,1023}/(?:[\d]+|\$DEFAULT)$`).
pub const DEFAULT_POLICY_VERSION: &str = "$DEFAULT";

/// Type of a policy, in the tokens `PolicyType` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyType {
    AwsManaged,
    AwsRam,
}

impl PolicyType {
    /// The wire token, as `PolicyType` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AwsManaged => "AWS_MANAGED",
            Self::AwsRam => "AWS_RAM",
        }
    }
    /// The policy type `token` names, or `None` when `PolicyType` declares no such
    /// value.
    #[must_use]
    pub fn from_wire(token: &str) -> Option<Self> {
        match token {
            "AWS_MANAGED" => Some(Self::AwsManaged),
            "AWS_RAM" => Some(Self::AwsRam),
            _ => None,
        }
    }
}

/// Status of one version of a policy, in the tokens `PolicyStatus` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyStatus {
    Attachable,
    Deprecated,
}

impl PolicyStatus {
    /// The wire token, as `PolicyStatus` spells it.
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Attachable => "ATTACHABLE",
            Self::Deprecated => "DEPRECATED",
        }
    }
}

/// The version of a policy a `QualifiedPolicyArn` names: an explicit
/// `PolicyVersionId`, or the policy's default version — the `$DEFAULT` alternative the
/// pattern allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyVersionSelector {
    Default,
    Id(i32),
}

/// The policy and version a `QualifiedPolicyArn` names, split into the unqualified ARN
/// the catalog is keyed by ([`policy_arn`]) and the version it selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyVersionRef {
    pub policy_arn: String,
    pub version: PolicyVersionSelector,
}

/// Whether `segment` fits the `[a-zA-Z0-9_\.-]{1,1023}` category/name segment
/// `UnqualifiedPolicyArn`'s pattern declares.
fn is_policy_arn_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_POLICY_ARN_SEGMENT
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

/// Whether `arn` is an unqualified policy ARN in the form `UnqualifiedPolicyArn`'s
/// pattern fixes — the form [`policy_arn`] mints and the catalog is keyed by:
/// `arn:<partition>:mpa:::aws:policy/<Category>/<Name>`.
#[must_use]
pub fn is_policy_arn(arn: &str) -> bool {
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, the resource being
    // `aws:policy/<Category>/<Name>` — which carries a colon of its own, so the ARN
    // splits into SIX fields and no more.
    let mut fields = arn.splitn(6, ':');
    if fields.next() != Some("arn") {
        return false;
    }
    // `.{1,63}` in the pattern's partition position.
    if !fields
        .next()
        .is_some_and(|partition| !partition.is_empty() && partition.len() <= 63)
    {
        return false;
    }
    if fields.next() != Some(SERVICE_ACTION_PREFIX) {
        return false;
    }
    // A policy is an AWS-owned, global resource: `:::` in the pattern, so its region
    // and account fields are both empty.
    if fields.next() != Some("") || fields.next() != Some("") {
        return false;
    }
    let Some(path) = fields.next().and_then(|r| r.strip_prefix("aws:policy/")) else {
        return false;
    };
    let mut segments = path.split('/');
    let (Some(category), Some(name), None) = (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    is_policy_arn_segment(category) && is_policy_arn_segment(name)
}

/// The policy and version `arn` names, or `None` when it is not a policy-version ARN in
/// the grammar `QualifiedPolicyArn`'s pattern fixes (an unqualified policy ARN followed
/// by `/<PolicyVersionId>` or `/$DEFAULT`).
///
/// The inverse of [`policy_version_arn`], so what this service mints it also reads back.
#[must_use]
pub fn parse_policy_version_arn(arn: &str) -> Option<PolicyVersionRef> {
    let (policy_arn, version) = arn.rsplit_once('/')?;
    if !is_policy_arn(policy_arn) {
        return None;
    }
    let version = if version == DEFAULT_POLICY_VERSION {
        PolicyVersionSelector::Default
    } else {
        // `[\d]+` in the pattern, and `PolicyVersionId` is `range(min: 1)`, so a signed
        // or out-of-range value names no version.
        if !version.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let version_id: i32 = version.parse().ok()?;
        if version_id < FIRST_POLICY_VERSION {
            return None;
        }
        PolicyVersionSelector::Id(version_id)
    };
    Some(PolicyVersionRef {
        policy_arn: policy_arn.to_string(),
        version,
    })
}

/// A policy attached to a resource this store records, as the resource-policy reads
/// report it. Every member is read off the catalog entry the attachment resolves to, so
/// a reported resource policy can never disagree with the policy the catalog holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePolicy {
    /// ARN of the resource the policy is attached to.
    pub resource_arn: String,
    /// `UnqualifiedPolicyArn` of the policy itself.
    pub policy_arn: String,
    /// ARN of the version in effect — the version the attachment RESOLVES to, so an
    /// attachment of `$DEFAULT` reports the policy's default version by id.
    pub policy_version_arn: String,
    /// Id of that version.
    pub policy_version_id: i32,
    pub policy_name: String,
    pub policy_type: PolicyType,
    /// Document of the version in effect.
    pub document: String,
}

/// The document this service reports for version `version_id` of `policy`.
///
/// Its identity is read from the catalog: the statement id is built from the policy's
/// name and the version id, so every version of every policy has a document of its own
/// and the same version always reports the same one.
///
/// The document's CONTENTS are this emulator's and are NOT a claim about the text AWS
/// serves: AWS does not publish its MPA managed policy documents in this service's
/// model, and `PolicyDocument` carries no modelled structure beyond
/// `@length(max: 400000)`. What is reported is a syntactically valid document in the IAM
/// policy grammar that `PolicyVersion$Status`' own documentation points at, over this
/// service's own action namespace.
#[must_use]
pub fn policy_version_document(policy: &CatalogPolicy, version_id: i32) -> String {
    // An IAM `Sid` is alphanumeric, so the policy's name contributes only its
    // alphanumeric characters rather than the `_`, `.` and `-` its own pattern allows.
    let name: String = policy
        .name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let document = ::serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": format!("{name}v{version_id}"),
            "Effect": "Allow",
            "Action": format!("{SERVICE_ACTION_PREFIX}:*"),
            "Resource": "*",
        }],
    });
    document.to_string()
}

/// One version of a catalog policy: everything `ListPolicyVersions` reports for it that
/// is the VERSION's rather than the policy's.
///
/// The members a version shares with its policy — the policy ARN, the name and the
/// `PolicyType` — are not copied here: they live once on the owning
/// [`CatalogPolicy`], so a reader cannot report a version whose name disagrees with
/// its policy's.
#[derive(Debug, Clone)]
pub struct CatalogPolicyVersion {
    /// This version's id, as `PolicyVersionSummary$VersionId` reports it. At least
    /// [`FIRST_POLICY_VERSION`], from `PolicyVersionId`'s modelled `range(min: 1)`.
    pub version_id: i32,
    /// Whether this version may still be attached to a team.
    pub status: PolicyStatus,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
}

/// One policy in the catalog: everything `ListPolicies` reports for it, plus the
/// versions `ListPolicyVersions` reports.
#[derive(Debug, Clone)]
pub struct CatalogPolicy {
    /// The policy's unqualified ARN, in the form [`policy_arn`] mints.
    pub arn: String,
    pub name: String,
    pub policy_type: PolicyType,
    /// Version id that is this policy's default, as `Policy$DefaultVersion` reports it.
    pub default_version: i32,
    /// Every version of this policy, keyed by its version id — the stable, ascending
    /// ordering `ListPolicyVersions` pages over.
    pub versions: BTreeMap<i32, CatalogPolicyVersion>,
}

impl CatalogPolicy {
    /// A catalog policy holding one version, [`FIRST_POLICY_VERSION`], which is
    /// therefore also its default: the shape every seeded entry starts in.
    #[must_use]
    pub fn with_first_version(
        category: &str,
        name: &str,
        policy_type: PolicyType,
        created: SystemTime,
    ) -> Self {
        let mut policy = Self {
            arn: policy_arn(category, name),
            name: name.to_string(),
            policy_type,
            default_version: FIRST_POLICY_VERSION,
            versions: BTreeMap::new(),
        };
        policy.insert_version(CatalogPolicyVersion {
            version_id: FIRST_POLICY_VERSION,
            status: PolicyStatus::Attachable,
            creation_time: created,
            last_updated_time: created,
        });
        policy
    }

    /// Record a version of this policy, replacing any entry under the same id.
    pub fn insert_version(&mut self, version: CatalogPolicyVersion) {
        self.versions.insert(version.version_id, version);
    }

    /// This policy's version with id `version_id`.
    #[must_use]
    pub fn version(&self, version_id: i32) -> Option<&CatalogPolicyVersion> {
        self.versions.get(&version_id)
    }

    /// Whether `version_id` names the version this policy attaches by default.
    #[must_use]
    pub fn is_default_version(&self, version_id: i32) -> bool {
        version_id == self.default_version
    }

    /// The qualified ARN of this policy's version `version_id`.
    #[must_use]
    pub fn version_arn(&self, version_id: i32) -> String {
        policy_version_arn(&self.arn, version_id)
    }
}

/// Version id a policy's first version carries, from `PolicyVersionId`'s modelled
/// `range(min: 1)`.
pub const FIRST_POLICY_VERSION: i32 = 1;

/// ARN of a policy, in the form `UnqualifiedPolicyArn`'s pattern fixes:
/// `arn:aws:mpa:::aws:policy/<category>/<name>`.
///
/// A policy is an AWS-owned resource, so the ARN's region and account fields are EMPTY —
/// which is why `engine::emulator_arn`, which stamps this emulator's region and account
/// into both, cannot mint one and this builds the ARN itself.
#[must_use]
pub fn policy_arn(category: &str, name: &str) -> String {
    format!("arn:aws:mpa:::aws:policy/{category}/{name}")
}

/// ARN of ONE VERSION of a policy, in the form `QualifiedPolicyArn`'s pattern fixes:
/// the policy's own unqualified ARN followed by the version id
/// (`arn:aws:mpa:::aws:policy/<category>/<name>/<version id>`).
#[must_use]
pub fn policy_version_arn(policy_arn: &str, version_id: i32) -> String {
    format!("{policy_arn}/{version_id}")
}

/// The policies this store holds when it is first touched, as
/// `(category, name, type)`.
///
/// The model fixes the SHAPE of an entry, and that is what this emulator computes its
/// answers from: the ARN form comes from `UnqualifiedPolicyArn`'s pattern (whose two
/// `[a-zA-Z0-9_\.-]` segments are the category and the name), the token from
/// `PolicyType`'s enum, and the version from `PolicyVersionId`'s `range(min: 1)`. The
/// entries themselves are AWS's own catalog, which no request can change and no model
/// member enumerates: this seeds the AWS-managed approval-team policy and invents no
/// others, each with the one version `FIRST_POLICY_VERSION` names. Nothing is keyed to a
/// particular entry — `ListPolicies` pages over whatever the store holds and
/// `ListPolicyVersions` over whatever versions an entry holds, so an entry or a version
/// recorded later is reported exactly like a seeded one.
const SEEDED_POLICY_CATALOG: &[(&str, &str, PolicyType)] =
    &[("IdentityCenter", "MPAApprovalTeamPolicy", PolicyType::AwsManaged)];

/// The catalog a fresh store starts with, keyed by ARN. `created` is the instant the
/// store came into being, which is when its seeded policies and their versions did.
#[must_use]
pub fn seeded_policy_catalog(created: SystemTime) -> BTreeMap<String, CatalogPolicy> {
    SEEDED_POLICY_CATALOG
        .iter()
        .map(|&(category, name, policy_type)| {
            let policy = CatalogPolicy::with_first_version(category, name, policy_type, created);
            (policy.arn.clone(), policy)
        })
        .collect()
}

/// Per-service state for com-amazonaws-mpa-awsfluffycoreservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMpaAwsfluffycoreserviceState {
    /// Every live team, keyed by its ARN.
    pub approval_teams: BTreeMap<String, ApprovalTeam>,
    /// Team name -> ARN. Backs the `ConflictException` a duplicate name earns.
    pub name_index: BTreeMap<String, String>,
    /// `ClientToken` -> ARN. Backs the idempotent replay of `CreateApprovalTeam`.
    pub idempotency_tokens: BTreeMap<String, String>,
    /// The account that is the MANAGEMENT account of the organization this store
    /// records, if any.
    ///
    /// Multi-party approval is an AWS Organizations feature, and the operations
    /// that set an organization's approval teams up are restricted to its
    /// management account — a call from a member or standalone account is refused
    /// with `AccessDeniedException` (403). This emulator presents a single account
    /// ([`caller_account_id`]) and runs no Organizations control plane, so there is
    /// no organization and no management account to record: the field is `None` and
    /// every caller is a standalone account. Read it through
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::is_management_account`], which
    /// therefore answers `false` for every caller until something records one.
    pub organization_management_account_id: Option<String>,
    /// The policy catalog, keyed by each policy's ARN — the stable ordering
    /// `ListPolicies` pages over.
    ///
    /// Seeded by [`Default`] from [`seeded_policy_catalog`]: a policy is AWS's, so no
    /// operation in this model mints or removes one or one of its versions, and every
    /// reader — `ListPolicies` and `ListPolicyVersions` alike — answers out of this one
    /// map.
    pub policies: BTreeMap<String, CatalogPolicy>,
    /// Source of [`ApprovalTeam::seq`].
    next_team_seq: u64,
    /// Source of [`Approver::approver_id`].
    next_approver_seq: u64,
}

impl Default for ComAmazonawsMpaAwsfluffycoreserviceState {
    /// A fresh store: no team, no organization, and the policy catalog seeded — the
    /// engine builds this service's state with `Default` on first touch, so this is the
    /// only place the catalog comes into being.
    fn default() -> Self {
        Self {
            approval_teams: BTreeMap::new(),
            name_index: BTreeMap::new(),
            idempotency_tokens: BTreeMap::new(),
            organization_management_account_id: None,
            policies: seeded_policy_catalog(SystemTime::now()),
            next_team_seq: 0,
            next_approver_seq: 0,
        }
    }
}

impl ComAmazonawsMpaAwsfluffycoreserviceState {
    /// Whether `account_id` is the management account of the organization this store
    /// records. False whenever no management account is recorded, which is the only
    /// state this emulator reaches on its own: an organization-only operation is then
    /// available to nobody.
    #[must_use]
    pub fn is_management_account(&self, account_id: &str) -> bool {
        self.organization_management_account_id.as_deref() == Some(account_id)
    }

    /// Apply every change the clock already owes at `now`, so each handler reads a
    /// store that is current. Idempotent, and a no-op when nothing is due.
    ///
    /// Only clock-owed changes happen here: a deletion window that has run out, and
    /// an invitation response that has come due. Nothing a caller has to ask for.
    pub fn materialize_due(&mut self, now: SystemTime) {
        // A team whose pending deletion window has elapsed is gone.
        let elapsed: Vec<String> = self
            .approval_teams
            .values()
            .filter(|team| {
                team.deletion_completion_time
                    .is_some_and(|due| due <= now)
            })
            .map(|team| team.arn.clone())
            .collect();
        for arn in elapsed {
            self.remove_team(&arn);
        }

        for team in self.approval_teams.values_mut() {
            for approver in &mut team.approvers {
                if approver.primary_identity_status != ApproverIdentityStatus::Pending {
                    continue;
                }
                let Some(due) = approver.invitation_response_due else {
                    continue;
                };
                if due > now {
                    continue;
                }
                // The response is stamped with the instant it was DUE, not with
                // `now`, so a materialised value does not move between reads.
                approver.primary_identity_status = ApproverIdentityStatus::Accepted;
                approver.response_time = Some(due);
                approver.last_activity = Some(ApproverLastActivity::RespondedToInvitation);
                approver.last_activity_time = Some(due);
                approver.invitation_response_due = None;
            }

            // A team is active once every approver has accepted their invitation. A team
            // with NO approvers never activates: `Approvers` is `length(1..20)`, so such
            // a team cannot be created, and a vacuous `all()` must not promote one.
            if team.status == TeamStatus::Pending
                && !team.approvers.is_empty()
                && team
                    .approvers
                    .iter()
                    .all(|a| a.primary_identity_status == ApproverIdentityStatus::Accepted)
            {
                team.status = TeamStatus::Active;
                team.status_code = None;
            }
        }
    }

    /// Drop a team and everything that indexes it.
    pub fn remove_team(&mut self, arn: &str) {
        if let Some(team) = self.approval_teams.remove(arn) {
            if self.name_index.get(&team.name).is_some_and(|a| a == arn) {
                self.name_index.remove(&team.name);
            }
        }
        self.idempotency_tokens.retain(|_, stored| stored != arn);
    }

    /// The stored team with this ARN.
    #[must_use]
    pub fn team(&self, arn: &str) -> Option<&ApprovalTeam> {
        self.approval_teams.get(arn)
    }

    /// The stored team with this ARN, for update.
    pub fn team_mut(&mut self, arn: &str) -> Option<&mut ApprovalTeam> {
        self.approval_teams.get_mut(arn)
    }

    /// The tags of the resource `arn` names, or `None` when this store holds no
    /// resource under that ARN.
    ///
    /// The one place a tagging operation resolves a resource ARN, so `TagResource` and
    /// `ListTagsForResource` agree on which resources exist and read the same map. The
    /// approval team is the only resource this service stores, so an ARN naming
    /// anything else is not found; when another resource is added here, both operations
    /// pick it up from this one function.
    #[must_use]
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.team(arn).map(|team| &team.tags)
    }

    /// Whether this store records a RESOURCE under `arn`.
    ///
    /// The one place an operation addressing a resource by ARN decides whether it
    /// exists, so the tagging and the resource-policy operations agree on it. The
    /// approval team is the only resource this service stores, so an ARN naming anything
    /// else names no resource; when another resource type is added here, every operation
    /// that resolves a resource ARN picks it up from this one function.
    #[must_use]
    pub fn records_resource(&self, arn: &str) -> bool {
        self.team(arn).is_some()
    }

    /// The tags of the resource `arn` names, for update. Same resolution as
    /// [`ComAmazonawsMpaAwsfluffycoreserviceState::resource_tags`].
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        self.team_mut(arn).map(|team| &mut team.tags)
    }

    /// Every stored team in creation order — the stable ordering
    /// `ListApprovalTeams` pages over.
    #[must_use]
    pub fn teams_in_creation_order(&self) -> Vec<&ApprovalTeam> {
        let mut teams: Vec<&ApprovalTeam> = self.approval_teams.values().collect();
        teams.sort_by_key(|team| team.seq);
        teams
    }

    /// Every policy attached to the resource `resource_arn` names, in attachment
    /// order, or `None` when this store records NO resource under that ARN — which is
    /// what tells a missing resource from a resource with no policy of the name asked
    /// for.
    ///
    /// An approval team is the only resource this store records, and a team's attached
    /// policies are the `Policies` references it carries, so the resource-policy reads
    /// and `GetApprovalTeam$Policies` report one and the same attachment list. Each
    /// reference is resolved through the CATALOG, so every member reported comes from
    /// the policy record itself; a reference the catalog does not hold names no policy
    /// and is reported by nothing.
    #[must_use]
    pub fn resource_policies(&self, resource_arn: &str) -> Option<Vec<ResourcePolicy>> {
        let team = self.team(resource_arn)?;
        Some(
            team.policies
                .iter()
                .filter_map(|attached| self.attached_resource_policy(resource_arn, attached))
                .collect(),
        )
    }

    /// The policy of this name and type attached to the resource `resource_arn` names,
    /// or `None` when no resource is recorded under that ARN or none of its attached
    /// policies carries that name and type.
    #[must_use]
    pub fn resource_policy(
        &self,
        resource_arn: &str,
        policy_name: &str,
        policy_type: PolicyType,
    ) -> Option<ResourcePolicy> {
        self.resource_policies(resource_arn)?
            .into_iter()
            .find(|policy| policy.policy_name == policy_name && policy.policy_type == policy_type)
    }

    /// The resource policy an attachment of `attached_arn` to `resource_arn` reports,
    /// or `None` when the attachment names no policy version the catalog holds.
    ///
    /// `PolicyReference$PolicyArn` is a `QualifiedPolicyArn`, so an attachment names a
    /// policy and one of its versions: the version id when it names one, and the
    /// policy's current default version when it names `$DEFAULT`.
    #[must_use]
    pub fn attached_resource_policy(
        &self,
        resource_arn: &str,
        attached_arn: &str,
    ) -> Option<ResourcePolicy> {
        // The one resolver for a qualified policy ARN, so an attachment and
        // `GetPolicyVersion` name the same version — and the same document — for the
        // same ARN.
        let (policy, version) = self.policy_version(attached_arn)?;
        Some(ResourcePolicy {
            resource_arn: resource_arn.to_string(),
            policy_arn: policy.arn.clone(),
            policy_version_arn: policy.version_arn(version.version_id),
            policy_version_id: version.version_id,
            policy_name: policy.name.clone(),
            policy_type: policy.policy_type,
            document: policy_version_document(policy, version.version_id),
        })
    }

    /// The catalog policy with this ARN.
    #[must_use]
    pub fn policy(&self, arn: &str) -> Option<&CatalogPolicy> {
        self.policies.get(arn)
    }

    /// The catalog policy and the one of its versions that a `QualifiedPolicyArn` names,
    /// or `None` when `arn` is outside that grammar or the catalog holds no such version.
    ///
    /// The grammar is [`parse_policy_version_arn`]'s, so every reader of a qualified
    /// policy ARN — the resource-policy reads and `GetPolicyVersion` alike — resolves one
    /// through this one function and accepts exactly the ARNs the model's pattern fixes.
    /// `$DEFAULT` is resolved against the policy's RECORDED default, so such an ARN names
    /// whichever version is the default when it is read.
    #[must_use]
    pub fn policy_version(&self, arn: &str) -> Option<(&CatalogPolicy, &CatalogPolicyVersion)> {
        let reference = parse_policy_version_arn(arn)?;
        let policy = self.policy(&reference.policy_arn)?;
        let version_id = match reference.version {
            PolicyVersionSelector::Default => policy.default_version,
            PolicyVersionSelector::Id(version_id) => version_id,
        };
        policy.version(version_id).map(|version| (policy, version))
    }

    /// Record a policy in the catalog, replacing any entry under the same ARN.
    pub fn insert_policy(&mut self, policy: CatalogPolicy) {
        self.policies.insert(policy.arn.clone(), policy);
    }

    /// Store a team and index it by name.
    ///
    /// Replacing a team already stored under this ARN drops the name entry the previous
    /// record held, so the index can never point at a name no stored team carries.
    pub fn insert_team(&mut self, team: ApprovalTeam) {
        if let Some(previous) = self.approval_teams.get(&team.arn) {
            if previous.name != team.name
                && self
                    .name_index
                    .get(&previous.name)
                    .is_some_and(|arn| arn == &team.arn)
            {
                self.name_index.remove(&previous.name);
            }
        }
        self.name_index.insert(team.name.clone(), team.arn.clone());
        self.approval_teams.insert(team.arn.clone(), team);
    }

    /// The next creation-order sequence number.
    pub fn next_team_seq(&mut self) -> u64 {
        self.next_team_seq += 1;
        self.next_team_seq
    }

    /// A fresh, never-reused approver id.
    pub fn mint_approver_id(&mut self) -> String {
        self.next_approver_seq += 1;
        format!("approver-{}", self.next_approver_seq)
    }
}

/// The `(PrimaryIdentityId, PrimaryIdentitySourceArn)` pair that `approvers` names more
/// than once, rendered for a refusal message, or `None` when every pair is distinct.
///
/// One identity cannot hold two seats on a team: a repeat would inflate the approver
/// count (N) that `MofN` is measured against without adding anyone who can approve.
#[must_use]
pub fn repeated_identity<'a, I>(approvers: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut seen = ::std::collections::BTreeSet::new();
    for (identity_id, source_arn) in approvers {
        if !seen.insert((identity_id, source_arn)) {
            return Some(format!("{identity_id} from {source_arn}"));
        }
    }
    None
}

/// ARN of an approval team, in the form `ApprovalTeamArn`'s pattern fixes:
/// `arn:aws:mpa:<region>:<account>:approval-team/<name>`.
#[must_use]
pub fn team_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("mpa", region, &format!("approval-team/{name}"))
}

/// The account every caller of this emulator authenticates as: every request is
/// signed with the emulator's own credentials, so the caller's account is
/// [`::engine::emulator_account_id`].
#[must_use]
pub fn caller_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The ARN of the identity this emulator's callers authenticate as, as an
/// authorization refusal names the principal it refused.
#[must_use]
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The account id an ARN embeds: its fifth colon-separated field, as every ARN
/// grammar places it (`arn:<partition>:<service>:<region>:<account>:<resource>`).
#[must_use]
pub fn arn_account_id(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// Whether `arn` is a well-formed Amazon Resource Name:
/// `arn:<partition>:<service>:<region>:<account>:<resource>` — six colon-separated
/// fields with a non-empty partition, service and resource.
///
/// Region and account are empty on a global resource's ARN (an AWS-managed MPA policy,
/// `arn:aws:mpa:::aws:policy/...`, for one), so neither is required.
#[must_use]
pub fn is_well_formed_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let prefix = fields.next() == Some("arn");
    let partition = fields.next().is_some_and(|field| !field.is_empty());
    let service = fields.next().is_some_and(|field| !field.is_empty());
    let region = fields.next().is_some();
    let account = fields.next().is_some();
    let resource = fields.next().is_some_and(|field| !field.is_empty());
    prefix && partition && service && region && account && resource
}

/// Whether `arn` names a resource in the caller's own account.
#[must_use]
pub fn is_caller_account_arn(arn: &str) -> bool {
    arn_account_id(arn) == Some(caller_account_id())
}

/// Authorize a request that identifies a resource by ARN: a resource outside the
/// caller's own account is not one the caller can act on, so the request is refused
/// BEFORE the resource is resolved — the answer to a foreign-account ARN never
/// depends on whether anything is stored under it.
///
/// `action` is the operation's name (`Self::NAME` in a handler), which the refusal
/// names as the IAM action `mpa:<action>`.
pub fn authorize_resource_arn(action: &str, arn: &str) -> Result<(), AccessDeniedException> {
    if is_caller_account_arn(arn) {
        return Ok(());
    }
    Err(cross_account_access_denied(action, arn))
}

/// The `AccessDeniedException` payload for a resource ARN outside the caller's
/// account. It is built from the request — the caller principal, the attempted
/// action and the ARN asked for — and says nothing about the resource, which is
/// never looked up.
#[must_use]
pub fn cross_account_access_denied(action: &str, arn: &str) -> AccessDeniedException {
    AccessDeniedException {
        message: bounded_message(format!(
            "User: {} is not authorized to perform: {SERVICE_ACTION_PREFIX}:{action} on \
             resource: {arn} because no resource-based policy allows the \
             {SERVICE_ACTION_PREFIX}:{action} action",
            caller_identity_arn()
        )),
    }
}

/// The `AccessDeniedException` payload for an operation an organization restricts to
/// its management account, called by an account that is not it.
///
/// The sentence is the one AWS answers this condition with; the requirement is the
/// 403 and the `AccessDeniedException` code, and the refusal names no value from the
/// request because the decision does not depend on one.
#[must_use]
pub fn not_management_account() -> AccessDeniedException {
    AccessDeniedException {
        message: "Action can only be performed by an organization\u{2019}s management account."
            .to_string(),
    }
}

/// The one message every resource-lookup failure of this service answers with.
///
/// `ResourceNotFoundException$Message` is `@required` with `@length(max: 1000)` and the
/// model fixes no text for it, so the rule is RECORDED behaviour: four ARN-addressed
/// operations (`GetApprovalTeam`, `StartActiveApprovalTeamDeletion`, `ListPolicyVersions`,
/// `GetResourcePolicy`) answer one and the same sentence for an ARN that resolves to
/// nothing, independent of the operation and of the ARN supplied. It is therefore a
/// SERVICE CONSTANT: the refusal names no value the request carried.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource does not exist.";

/// The `ResourceNotFoundException` an ARN-addressed lookup answers with when nothing is
/// recorded under the ARN it was given: 404, and [`RESOURCE_NOT_FOUND_MESSAGE`].
///
/// The tag family is the one exception — see [`tagged_resource_not_found`].
#[must_use]
pub fn resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: RESOURCE_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// The `ResourceNotFoundException` the TAG FAMILY (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) answers with for a `ResourceArn` no stored resource carries.
///
/// Its message is BUILT FROM THE REQUEST and is the `ResourceArn` member verbatim, with
/// no surrounding prose: on this family the not-found message IS the identifier of the
/// resource that was not found, which is what a caller reads out of it. `ResourceArn` is
/// `@length(max: 1000)` here — exactly the bound on `ResourceNotFoundException$Message`
/// — so the value always fits the message it becomes and is never cut.
#[must_use]
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: resource_arn.to_string(),
    }
}

/// The one message this service refuses a malformed Amazon Resource Name with.
///
/// `ValidationException$Message` is `@required` and the model fixes no text for it, so
/// the rule is RECORDED behaviour: a value that is not a well-formed MPA ARN is refused
/// with 400 `ValidationException` and this one sentence, on every member that carries an
/// ARN (`GetPolicyVersion$PolicyVersionArn`, and the `ResourceArn` of the tag family,
/// `GetResourcePolicy` and `ListResourcePolicies`). It names neither the member nor the
/// grammar: the malformed-ARN class is one service-level rule, not a per-member
/// constraint report.
pub const INVALID_ARN_MESSAGE: &str = "Invalid Resource Arn";

/// The `ValidationException` (400) a member that should carry an ARN and does not is
/// refused with: [`INVALID_ARN_MESSAGE`].
#[must_use]
pub fn invalid_arn() -> ValidationException {
    ValidationException {
        message: INVALID_ARN_MESSAGE.to_string(),
    }
}

/// Whether `pattern` is an ARN grammar.
///
/// Every ARN-valued member of this model anchors its `@pattern` on the literal `arn:`
/// prefix an Amazon Resource Name starts with — `ApprovalTeamArn`
/// (`^arn:aws(-[^:]+)?:mpa:…:approval-team/…$`), `SessionArn` and `QualifiedPolicyArn`
/// (`^arn:.{1,63}:mpa:::aws:policy/…$`), from
/// `smithy_query(query="shape",shape="ApprovalTeamArn,SessionArn,QualifiedPolicyArn")` —
/// and no member whose value is not an ARN does. So the prefix is what tells a failed
/// ARN grammar from any other failed pattern, without naming a member.
#[must_use]
pub fn is_arn_pattern(pattern: &str) -> bool {
    pattern.starts_with("^arn:")
}

/// Whether `violation` is a value that should have been an ARN failing the ARN grammar
/// its member's `@pattern` declares — the malformed-ARN class this service answers with
/// [`invalid_arn`] rather than with the framework's per-member constraint report.
#[must_use]
pub fn is_malformed_arn_violation(violation: &::aws_smithy_schema_server::Violation) -> bool {
    matches!(
        &violation.kind,
        ::aws_smithy_schema_server::ViolationKind::Pattern(pattern) if is_arn_pattern(pattern)
    )
}

/// `message` cut to the `AccessDeniedException$Message` bound
/// ([`MAX_ACCESS_DENIED_MESSAGE`]) on a character boundary, so a long resource ARN
/// (`ApprovalTeamArn` is `length(20..2048)`) cannot push the refusal past the length
/// its own member declares.
fn bounded_message(message: String) -> String {
    if message.len() <= MAX_ACCESS_DENIED_MESSAGE {
        return message;
    }
    let mut end = MAX_ACCESS_DENIED_MESSAGE;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_string()
}

/// The region to stamp into a minted ARN: the one this request's SigV4 credential
/// scope names, or [`DEFAULT_REGION`] when it names none the ARN grammar accepts.
#[must_use]
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` fits the `[a-z0-9-]{1,20}` region segment every `mpa` ARN
/// pattern declares.
fn is_arn_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 20
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMpaAwsfluffycoreserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMpaAwsfluffycoreserviceState>)
}
