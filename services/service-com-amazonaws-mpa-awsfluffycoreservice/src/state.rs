// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mpa-awsfluffycoreservice service.
//!
//! The approval team is this service's primary resource, and this module is the one
//! place it is stored: `CreateApprovalTeam` mints a record here, and
//! `GetApprovalTeam` / `ListApprovalTeams` / `UpdateApprovalTeam` /
//! `StartActiveApprovalTeamDeletion` all compute their answers from it.
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

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::AccessDeniedException;

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
}

/// One policy in the catalog: everything `ListPolicies` reports for it.
#[derive(Debug, Clone)]
pub struct CatalogPolicy {
    /// The policy's unqualified ARN, in the form [`policy_arn`] mints.
    pub arn: String,
    pub name: String,
    pub policy_type: PolicyType,
    /// Version id that is this policy's default, as `Policy$DefaultVersion` reports it.
    pub default_version: i32,
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

/// The policies this store holds when it is first touched, as
/// `(category, name, type, default version)`.
///
/// The model fixes the SHAPE of an entry, and that is what this emulator computes its
/// answers from: the ARN form comes from `UnqualifiedPolicyArn`'s pattern (whose two
/// `[a-zA-Z0-9_\.-]` segments are the category and the name), the token from
/// `PolicyType`'s enum, and the version from `PolicyVersionId`'s `range(min: 1)`. The
/// entries themselves are AWS's own catalog, which no request can change and no model
/// member enumerates: this seeds the AWS-managed approval-team policy and invents no
/// others. Nothing is keyed to a particular entry — `ListPolicies` pages over whatever
/// the store holds, so an entry recorded later is reported exactly like a seeded one.
const SEEDED_POLICY_CATALOG: &[(&str, &str, PolicyType, i32)] = &[(
    "IdentityCenter",
    "MPAApprovalTeamPolicy",
    PolicyType::AwsManaged,
    FIRST_POLICY_VERSION,
)];

/// The catalog a fresh store starts with, keyed by ARN.
#[must_use]
pub fn seeded_policy_catalog() -> BTreeMap<String, CatalogPolicy> {
    SEEDED_POLICY_CATALOG
        .iter()
        .map(|&(category, name, policy_type, default_version)| {
            let policy = CatalogPolicy {
                arn: policy_arn(category, name),
                name: name.to_string(),
                policy_type,
                default_version,
            };
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
    /// operation in this model mints or removes one, and every reader answers out of
    /// this one map.
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
            policies: seeded_policy_catalog(),
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

    /// Every stored team in creation order — the stable ordering
    /// `ListApprovalTeams` pages over.
    #[must_use]
    pub fn teams_in_creation_order(&self) -> Vec<&ApprovalTeam> {
        let mut teams: Vec<&ApprovalTeam> = self.approval_teams.values().collect();
        teams.sort_by_key(|team| team.seq);
        teams
    }

    /// The catalog policy with this ARN.
    #[must_use]
    pub fn policy(&self, arn: &str) -> Option<&CatalogPolicy> {
        self.policies.get(arn)
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
