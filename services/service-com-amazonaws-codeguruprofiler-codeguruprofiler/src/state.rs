// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeguruprofiler-codeguruprofiler service.
//!
//! One map of profiling groups, keyed by name — the service's only resource. Every
//! handler computes its response from what is stored here; nothing is canned.

use std::collections::{BTreeMap, HashMap, VecDeque};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AgentOrchestrationConfig, FindingsReportSummary, ProfilingGroupDescription, ProfilingStatus,
};

/// The region an ARN carries when the request is unsigned (so it has no SigV4
/// credential scope to read a region from). `emulator_arn`'s own documented default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and sigv4 signing name) of this service, as
/// `OPERATIONS.md` / the model's `aws.api#service` records it.
pub const ARN_NAMESPACE: &str = "codeguru-profiler";

/// `computePlatform` when `CreateProfilingGroup` does not specify one. The model's
/// doc for that member: "If not specified, Default is used."
pub const DEFAULT_COMPUTE_PLATFORM: &str = "Default";

/// The aggregation periods this service offers, as (the modelled `AggregationPeriod`
/// value, the period's length in WHOLE SECONDS): the three values the model's
/// `AggregationPeriod` enum declares (PT5M, PT1H, P1D), with the lengths the
/// `GetProfile` doc states — "Aggregated profiles are available with aggregation
/// periods of 5 minutes, 1 hour, and 1 day, aligned to UTC."
pub const AGGREGATION_PERIODS: &[(&str, i64)] = &[
    ("PT5M", 5 * 60),
    ("PT1H", 60 * 60),
    ("P1D", 24 * 60 * 60),
];

/// The length in seconds of a modelled `AggregationPeriod` value, or `None` when the
/// string is not one of [`AGGREGATION_PERIODS`], which covers every value the
/// generated `AGGREGATION_PERIOD_VALUES` lists.
pub fn aggregation_period_seconds(period: &str) -> Option<i64> {
    AGGREGATION_PERIODS
        .iter()
        .find(|(name, _)| *name == period)
        .map(|(_, seconds)| *seconds)
}

/// The start of the `period_seconds`-long aggregation window, aligned to the UTC
/// epoch, that contains the instant `at`. Alignment to UTC is what the `GetProfile`
/// doc states for every aggregation period, and the epoch is a whole number of
/// 5-minute, 1-hour and 1-day periods past UTC midnight, so flooring against it aligns
/// each of the three the same way.
///
/// Whole seconds, like [`instant_key`], rather than `f64` epoch seconds, whose
/// granularity at current instants is coarser than a microsecond; `div_euclid` floors
/// towards the past on both sides of the epoch. A period that is not positive names no
/// window, so the instant is its own window start.
pub fn aggregation_window_start(at: DateTime, period_seconds: i64) -> DateTime {
    if period_seconds <= 0 {
        return at;
    }
    DateTime::from_secs(at.secs().div_euclid(period_seconds) * period_seconds)
}

/// How many posted profiles one group retains. An emulator-chosen bound, not an AWS
/// duration: the real service expires profiles over time (the `GetProfile` doc:
/// "Because aggregated profiles expire over time GetProfile is not idempotent"), and
/// an in-memory emulator must bound what a caller can make it hold, so
/// [`ProfilingGroup::record_profile`] evicts the oldest profiles once a group holds
/// more than this.
pub const MAX_RETAINED_PROFILES_PER_GROUP: usize = 16;

/// How many profile BYTES one group retains, so a few large payloads are bounded as
/// well as many small ones. Emulator-chosen, for the same reason as
/// [`MAX_RETAINED_PROFILES_PER_GROUP`]; the newest profile is always kept even when it
/// alone is larger than this.
pub const MAX_RETAINED_PROFILE_BYTES_PER_GROUP: usize = 4 * 1024 * 1024;

/// How many profile BYTES the whole store retains, across every group. This is the
/// bound on what the service can be made to hold in total: the per-group caps alone
/// would still scale with the number of groups, so
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`] evicts the
/// oldest profile anywhere in the store until the total fits (never the profile just
/// posted). Emulator-chosen.
pub const MAX_RETAINED_PROFILE_BYTES: usize = 64 * 1024 * 1024;

/// How many profiling groups this emulator holds. Emulator-chosen, and reported with
/// the operation's own modelled `ServiceQuotaExceededException`, which says exactly
/// this: "You have exceeded your service quota. To perform the requested action,
/// remove some of the relevant resources, or use Service Quotas to request a service
/// quota increase." (The real service also caps profiling groups per account.)
pub const MAX_PROFILING_GROUPS: usize = 256;

/// The most tags one resource holds. AWS's general tagging limit, not an emulator
/// choice — "Maximum number of tags per resource: 50" in the AWS tagging quotas the
/// CodeGuru Profiler tagging operations follow — so a `TagResource` (or a create) that
/// would leave a group past it is refused with the operation's modelled
/// `ValidationException` instead of growing the store. `UntagResource` frees the budget
/// again, so this is a bound on what one resource holds, not on how often it is
/// re-tagged.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag KEY, in characters, and the shortest: AWS's tagging limits give a
/// key 1..128 characters. The model constrains `TagsMap`'s key with nothing
/// (`smithy_query(query="shape",shape="TagsMap")` -> `{key: String, value: String}`),
/// so the boundary validator clamps neither and [`validate_tags`] is what holds the
/// published range.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag VALUE, in characters: AWS's tagging limits give a value 0..256
/// characters (an empty value is a valid one). Same reason as [`MAX_TAG_KEY_CHARS`] for
/// enforcing it here.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The page size `ListProfilingGroups` uses when the caller gives no `maxResults`, so
/// one unparameterised request cannot materialise a response that grows with the
/// stored group count. Inside the modelled `maxResults` range (1..1000).
pub const DEFAULT_LIST_PAGE_SIZE: usize = 100;

/// The `AggregationPeriod` of a DAILY profile, as the model's `AggregationPeriod` enum
/// spells it (its values are "P1D", "PT1H", "PT5M" — see `AGGREGATION_PERIOD_VALUES`).
/// A findings report over a `P1D` profile is what `dailyReportsOnly` selects.
pub const DAILY_AGGREGATION_PERIOD: &str = "P1D";

/// How many findings reports one profiling group retains, so the report store is
/// bounded the way the profile store is. Emulator-chosen, like
/// [`MAX_RETAINED_PROFILES_PER_GROUP`]; the oldest analysed window is dropped first.
pub const MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP: usize = 64;

/// How many `PostAgentProfile` idempotency tokens one group remembers. Emulator-chosen,
/// and deliberately larger than [`MAX_RETAINED_PROFILES_PER_GROUP`] so every retained
/// profile's token is still recognised as a replay; the oldest token is forgotten
/// first, so caller-chosen token strings cannot accumulate without limit.
pub const MAX_RETAINED_PROFILE_TOKENS_PER_GROUP: usize = 64;

/// The IAM policy-language version a resource-based policy document carries. The
/// version the service's own resource-policy document uses (the AWS CodeGuru Profiler
/// User Guide, "Resource-based policies in CodeGuru Profiler").
pub const POLICY_VERSION: &str = "2012-10-17";

/// The one action group a resource-based policy can grant, as the model's `ActionGroup`
/// enum declares it (`smithy_query(query="shape",shape="ActionGroup")`).
pub const ACTION_GROUP_AGENT_PERMISSIONS: &str = "agentPermissions";

/// The API actions an action group grants, as IAM action strings
/// (`<arn namespace>:<operation>`). The model's `PutPermission` doc for `actionGroup`:
/// "One action group is supported, agentPermissions, which grants permission to perform
/// actions required by the profiling agent, ConfigureAgent and PostAgentProfile
/// permissions." An action group this service does not know grants nothing, so it
/// renders no actions rather than inventing some.
pub fn action_group_actions(action_group: &str) -> Vec<String> {
    match action_group {
        ACTION_GROUP_AGENT_PERMISSIONS => vec![
            format!("{ARN_NAMESPACE}:ConfigureAgent"),
            format!("{ARN_NAMESPACE}:PostAgentProfile"),
        ],
        _ => Vec::new(),
    }
}

/// A profiling group's resource-based policy: which principals hold which action
/// group, and the revision that identifies the policy's current content.
///
/// The policy is stored as the grants themselves, never as a document string, so every
/// reader ([`ResourcePolicy::document`]) renders the SAME document from the SAME grants
/// and the group's own stored ARN.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// `actionGroup` -> the principal ARNs that hold it, in the order they were
    /// granted. A `BTreeMap` so the rendered document's statement order is stable
    /// across reads.
    pub statements: BTreeMap<String, Vec<String>>,
    /// The policy's current revision, as the modelled `RevisionId`
    /// (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`). Rotated by
    /// whoever changes the grants, never by a read.
    pub revision_id: String,
}

impl ResourcePolicy {
    /// An empty policy at `revision_id`. A policy with no grants is not a policy: the
    /// writer that empties it clears the group's policy instead, and
    /// [`ProfilingGroup::resource_policy`] reports it as absent either way.
    ///
    /// This constructor and the two writers below exist for the permission handlers —
    /// `PutPermission` and `RemovePermission`, which are still scaffold stubs with their
    /// own open issue — so the operations that return `policy` all render the same
    /// document from the same grants. They are the writers, not dead code.
    pub fn new(revision_id: impl Into<String>) -> Self {
        Self {
            statements: BTreeMap::new(),
            revision_id: revision_id.into(),
        }
    }

    /// Grant `action_group` to exactly `principals`, REPLACING whatever principal set
    /// the action group held. The revision is NOT touched here: a writer that changes
    /// the grants rotates it once, with [`ResourcePolicy::set_revision`], however many
    /// action groups the one call rewrote.
    pub fn grant(&mut self, action_group: &str, principals: Vec<String>) {
        self.statements
            .insert(action_group.to_string(), principals);
    }

    /// Record `revision_id` as the policy's current revision. Only a change to the
    /// grants rotates it; a read reports what is stored.
    pub fn set_revision(&mut self, revision_id: impl Into<String>) {
        self.revision_id = revision_id.into();
    }

    /// The JSON-formatted resource-based policy document, as every operation that
    /// returns `policy` reports it: one `Allow` statement per stored action group,
    /// naming the principals that hold it, the actions the action group grants and
    /// `resource_arn` — the profiling group's own stored ARN.
    pub fn document(&self, resource_arn: &str) -> String {
        let statements: Vec<::serde_json::Value> = self
            .statements
            .iter()
            .map(|(action_group, principals)| {
                ::serde_json::json!({
                    "Sid": action_group,
                    "Effect": "Allow",
                    "Principal": { "AWS": principals },
                    "Action": action_group_actions(action_group),
                    "Resource": resource_arn,
                })
            })
            .collect();
        ::serde_json::json!({
            "Version": POLICY_VERSION,
            "Statement": statements,
        })
        .to_string()
    }
}

/// One agent profile as `PostAgentProfile` uploaded it. The bytes are kept verbatim:
/// the agent profile is an opaque payload (Amazon Ion or JSON) that `GetProfile`
/// hands back byte-identical.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub received_at: DateTime,
}

/// One recommendation ("findings") report: the result of analysing ONE profile window
/// of one group, which `ListFindingsReports` and `GetFindingsReportAccountSummary`
/// summarise as a `FindingsReportSummary`.
///
/// Reports are STORED, never derived from a request: this emulator runs no
/// recommendation analysis of its own, so the only way one enters the store is
/// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_findings_report`] — a
/// group that has had no analysis recorded against it holds no reports, and
/// `ListFindingsReports` answers it with an empty list rather than with invented
/// findings.
#[derive(Debug, Clone)]
pub struct FindingsReport {
    /// `FindingsReportSummary.id`: the report's UUID (the member's modelled pattern is
    /// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`).
    pub id: String,
    /// Start of the profile window the analysis covered.
    pub profile_start_time: DateTime,
    /// End of the profile window the analysis covered.
    pub profile_end_time: DateTime,
    /// `FindingsReportSummary.totalNumberOfFindings`: "The total number of different
    /// recommendations that were found by the analysis."
    pub total_number_of_findings: i32,
    /// The `AggregationPeriod` of the profile analysed — what `dailyReportsOnly`
    /// selects on. `FindingsReportSummary` has no member for it, so it is stored and
    /// never reported.
    pub aggregation_period: String,
}

impl FindingsReport {
    /// The `FindingsReportSummary` a list operation reports for this stored report.
    /// `profilingGroupName` comes from the stored group, not from the request that is
    /// asking for it.
    pub fn summary(&self, group_name: &str) -> FindingsReportSummary {
        FindingsReportSummary {
            id: Some(self.id.clone()),
            profiling_group_name: Some(group_name.to_string()),
            profile_start_time: Some(self.profile_start_time),
            profile_end_time: Some(self.profile_end_time),
            total_number_of_findings: Some(self.total_number_of_findings),
        }
    }

    /// Whether this is a report over a daily profile — the reports
    /// `dailyReportsOnly=true` restricts a listing to.
    pub fn is_daily(&self) -> bool {
        self.aggregation_period == DAILY_AGGREGATION_PERIOD
    }

    /// Whether the profile window this report analysed overlaps the closed interval
    /// `[start, end]` — the time range a listing asks about.
    pub fn overlaps(&self, start: DateTime, end: DateTime) -> bool {
        self.profile_end_time.as_secs_f64() >= start.as_secs_f64()
            && self.profile_start_time.as_secs_f64() <= end.as_secs_f64()
    }
}

/// A profiling group, as Create/ConfigureAgent/PostAgentProfile left it.
#[derive(Debug, Clone)]
pub struct ProfilingGroup {
    pub name: String,
    pub arn: String,
    pub compute_platform: String,
    /// `agentOrchestrationConfig.profilingEnabled`.
    pub profiling_enabled: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub tags: HashMap<String, String>,
    /// `profilingStatus.latestAgentOrchestratedAt` — set by `ConfigureAgent`.
    pub latest_agent_orchestrated_at: Option<DateTime>,
    /// `profilingStatus.latestAgentProfileReportedAt` — set by `PostAgentProfile`.
    pub latest_agent_profile_reported_at: Option<DateTime>,
    /// Posted agent profiles, oldest FIRST — a `VecDeque`, so the oldest-first
    /// eviction the retention bound performs is structural and O(1). Bounded: at most
    /// [`MAX_RETAINED_PROFILES_PER_GROUP`] profiles and
    /// [`MAX_RETAINED_PROFILE_BYTES_PER_GROUP`] bytes are retained per group, and the
    /// store-wide [`MAX_RETAINED_PROFILE_BYTES`] budget bounds every group together —
    /// write them only through
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`], which is
    /// what applies those bounds.
    pub profiles: VecDeque<AgentProfile>,
    /// `PostAgentProfile` idempotency tokens already honoured for this group, oldest
    /// first. Bounded at [`MAX_RETAINED_PROFILE_TOKENS_PER_GROUP`] by
    /// [`ProfilingGroup::remember_profile_token`], which is the only writer.
    pub seen_profile_tokens: VecDeque<String>,
    /// The group's resource-based policy, or `None` while it has none — a group is
    /// created without one, and the permission writers attach and clear it. It lives
    /// ON the group so deleting the group takes its policy with it and a re-created
    /// group starts with none. Read it through [`ProfilingGroup::resource_policy`].
    pub policy: Option<ResourcePolicy>,
    /// The recommendation reports recorded against this group, oldest recorded FIRST.
    /// Bounded at [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`] by
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_findings_report`],
    /// which is the only writer.
    pub findings_reports: VecDeque<FindingsReport>,
}

impl ProfilingGroup {
    /// The `ProfilingGroupDescription` every READ of a stored profiling group answers with
    /// — `DescribeProfilingGroup`, `UpdateProfilingGroup` and the described form of
    /// `ListProfilingGroups` — built from what is stored, never from the request that is
    /// asking for it.
    ///
    /// These paths report the `profilingStatus` CONTAINER even while the group has no
    /// activity to report in it (its three members are each absent until the activity they
    /// name happens); the CREATE path is the one that omits the container, see
    /// [`ProfilingGroup::created_description`].
    pub fn description(&self) -> ProfilingGroupDescription {
        ProfilingGroupDescription {
            name: Some(self.name.clone()),
            agent_orchestration_config: Some(AgentOrchestrationConfig {
                profiling_enabled: self.profiling_enabled,
            }),
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            profiling_status: Some(self.profiling_status()),
            compute_platform: Some(self.compute_platform.clone()),
            tags: Some(self.tags.clone()),
        }
    }

    /// The description `CreateProfilingGroup` answers with: [`description`] with
    /// `profilingStatus` ABSENT while the group has no profiling activity.
    ///
    /// `profilingStatus` is not `@required`, and each of its members reports one thing that
    /// has HAPPENED to the group — the last time an agent pinged back, the last time a
    /// profile was received, the most recent aggregated profile. A create reports only what
    /// the create established, and nothing can have been reported against a group that is
    /// only now coming into existence, so the member is left out of that payload entirely
    /// rather than written as an empty object: the way a client tells "nothing has ever
    /// been reported" from "activity, reported here" is whether the key is there at all,
    /// and an empty object answers that question wrongly. A create replayed on its
    /// `clientToken` against a group that HAS since reported carries the status, because by
    /// then there is something to report.
    ///
    /// [`description`]: ProfilingGroup::description
    pub fn created_description(&self) -> ProfilingGroupDescription {
        ProfilingGroupDescription {
            profiling_status: self
                .has_profiling_activity()
                .then(|| self.profiling_status()),
            ..self.description()
        }
    }

    /// The group's `profilingStatus`: each member present only once the activity it names
    /// has happened.
    fn profiling_status(&self) -> ProfilingStatus {
        ProfilingStatus {
            latest_agent_profile_reported_at: self.latest_agent_profile_reported_at,
            // `latestAggregatedProfile` is ONE `AggregatedProfileTime`, i.e. one
            // aggregation period, while the service aggregates a group's posted
            // profiles at all three periods at once (see
            // [`ProfilingGroup::aggregated_profile_starts`], which derives the
            // aggregated profiles for the period a caller asks about). There is no
            // stored fact saying which period to report here, so this stays absent
            // rather than picking one arbitrarily.
            latest_aggregated_profile: None,
            latest_agent_orchestrated_at: self.latest_agent_orchestrated_at,
        }
    }

    /// Whether anything has ever been reported about this group's profiling: an agent ping
    /// (`ConfigureAgent`), a posted profile (`PostAgentProfile`) or an aggregated profile.
    /// This is what decides whether a create's payload carries a `profilingStatus` at all.
    fn has_profiling_activity(&self) -> bool {
        let status = self.profiling_status();
        status.latest_agent_profile_reported_at.is_some()
            || status.latest_agent_orchestrated_at.is_some()
            || status.latest_aggregated_profile.is_some()
    }

    /// Store a posted profile and record its receipt time, within this group's
    /// retention bound: once the group holds more than
    /// [`MAX_RETAINED_PROFILES_PER_GROUP`] profiles or
    /// [`MAX_RETAINED_PROFILE_BYTES_PER_GROUP`] bytes, the oldest are dropped until it
    /// does not. The profile just posted is never the one evicted, so a post is always
    /// readable back. Call it through
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`], which also
    /// applies the store-wide byte budget.
    fn record_profile(&mut self, profile: AgentProfile) {
        self.latest_agent_profile_reported_at = Some(profile.received_at);
        self.profiles.push_back(profile);
        let mut retained_bytes: usize = self
            .profiles
            .iter()
            .map(|profile| profile.bytes.len())
            .sum();
        while self.profiles.len() > 1
            && (self.profiles.len() > MAX_RETAINED_PROFILES_PER_GROUP
                || retained_bytes > MAX_RETAINED_PROFILE_BYTES_PER_GROUP)
        {
            match self.profiles.pop_front() {
                Some(evicted) => retained_bytes -= evicted.bytes.len(),
                None => break,
            }
        }
    }

    /// Remember a `PostAgentProfile` idempotency token, within
    /// [`MAX_RETAINED_PROFILE_TOKENS_PER_GROUP`]: `true` when the token is new (so the
    /// post must be stored), `false` when it is a replay of one still remembered.
    fn remember_profile_token(&mut self, token: &str) -> bool {
        if self.seen_profile_tokens.iter().any(|seen| seen == token) {
            return false;
        }
        while self.seen_profile_tokens.len() >= MAX_RETAINED_PROFILE_TOKENS_PER_GROUP {
            self.seen_profile_tokens.pop_front();
        }
        self.seen_profile_tokens.push_back(token.to_string());
        true
    }

    /// The start instants of the aggregated profiles this group holds for an
    /// aggregation period `period_seconds` long, ASCENDING and distinct.
    ///
    /// The `GetProfile` doc fixes the rule: "Amazon CodeGuru Profiler collects posted
    /// agent profiles for a profiling group into aggregated profiles", at periods
    /// "aligned to UTC". So each posted profile belongs to the one aligned window that
    /// contains its receipt time, and every window holding at least one posted profile
    /// is exactly one available aggregated profile, reported by its window start.
    pub fn aggregated_profile_starts(&self, period_seconds: i64) -> Vec<DateTime> {
        let mut starts: Vec<i64> = self
            .profiles
            .iter()
            .map(|profile| aggregation_window_start(profile.received_at, period_seconds).secs())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        starts.into_iter().map(DateTime::from_secs).collect()
    }

    /// This group's resource-based policy, or `None` when it has none. A policy with
    /// no grants left is no policy, so an emptied one reads as absent here too — which
    /// is what the operations returning `policy` must refuse on.
    pub fn resource_policy(&self) -> Option<&ResourcePolicy> {
        self.policy
            .as_ref()
            .filter(|policy| !policy.statements.is_empty())
    }

    /// The span the retained profiles inside `[start, end]` cover, as (earliest receipt
    /// time, latest receipt time), or `None` when this group retains no profile in that
    /// window. Both bounds are inclusive, and the answer is derived from the stored
    /// receipt times alone — this is what `GetRecommendations` reports as the span its
    /// analysis is about.
    ///
    /// Instants are compared through [`instant_key`], so two receipt times a fraction of
    /// a microsecond apart still order exactly.
    pub fn profile_span_in(&self, start: DateTime, end: DateTime) -> Option<(DateTime, DateTime)> {
        let (lower, upper) = (instant_key(start), instant_key(end));
        let mut span: Option<(DateTime, DateTime)> = None;
        for profile in &self.profiles {
            let at = instant_key(profile.received_at);
            if at < lower || at > upper {
                continue;
            }
            span = Some(match span {
                None => (profile.received_at, profile.received_at),
                Some((earliest, latest)) => (
                    if at < instant_key(earliest) {
                        profile.received_at
                    } else {
                        earliest
                    },
                    if at > instant_key(latest) {
                        profile.received_at
                    } else {
                        latest
                    },
                ),
            });
        }
        span
    }

    /// This group's recorded findings reports that a listing of `[start, end]` reports:
    /// those whose analysed profile window overlaps that range, and — when
    /// `daily_only` — only those over a daily (`P1D`) profile. Ordered by analysed
    /// window, oldest first, then by report id, so paging a listing visits every
    /// report exactly once however the store was written.
    pub fn findings_reports_in(
        &self,
        start: DateTime,
        end: DateTime,
        daily_only: bool,
    ) -> Vec<&FindingsReport> {
        let mut reports: Vec<&FindingsReport> = self
            .findings_reports
            .iter()
            .filter(|report| report.overlaps(start, end) && (!daily_only || report.is_daily()))
            .collect();
        reports.sort_by(|left, right| {
            left.profile_start_time
                .as_secs_f64()
                .total_cmp(&right.profile_start_time.as_secs_f64())
                .then_with(|| left.id.cmp(&right.id))
        });
        reports
    }

    /// The most recently received profile whose receipt time falls in
    /// `[start, end]` (either bound absent means unbounded).
    pub fn latest_profile_in(
        &self,
        start: Option<DateTime>,
        end: Option<DateTime>,
    ) -> Option<&AgentProfile> {
        self.profiles.iter().rfind(|profile| {
            let at = profile.received_at.as_secs_f64();
            start.is_none_or(|s| at >= s.as_secs_f64()) && end.is_none_or(|e| at <= e.as_secs_f64())
        })
    }
}

/// Per-service state for com-amazonaws-codeguruprofiler-codeguruprofiler. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeguruprofilerCodeguruprofilerState {
    /// `profilingGroupName` -> the group.
    pub groups: HashMap<String, ProfilingGroup>,
    /// `CreateProfilingGroup`'s `clientToken` -> the name it created, so a retry of
    /// the same create is idempotent instead of a conflict. Bounded by `groups`: an
    /// entry is written only by a create that also added a group, and
    /// `DeleteProfilingGroup` removes every entry naming the group it deletes, so this
    /// map never holds more entries than there are live groups.
    pub create_tokens: HashMap<String, String>,
}

/// What [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::post_profile`] did with a
/// posted profile.
#[derive(Debug, PartialEq, Eq)]
pub enum PostedProfile {
    /// Stored against the group (within the retention bounds).
    Stored,
    /// A replay of a `profileToken` the group still remembers: nothing was stored.
    Replayed,
    /// There is no such profiling group.
    UnknownGroup,
}

impl ComAmazonawsCodeguruprofilerCodeguruprofilerState {
    /// Profiling-group names in a stable (lexicographic) order, so paging over
    /// `ListProfilingGroups` visits every group exactly once.
    pub fn sorted_group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.groups.keys().cloned().collect();
        names.sort();
        names
    }

    /// The stored group an ARN identifies, or `None` when this store holds no resource
    /// with that ARN. An ARN identifies ONE resource, so the stored ARN must match the
    /// one asked about in full: a group of the same name in another region, partition or
    /// account is a different resource and is not found here.
    ///
    /// Pair it with [`profiling_group_name_from_arn`], which separates "not a
    /// profiling-group ARN at all" (a bad request) from "no such profiling group"
    /// (a 404).
    pub fn group_by_arn(&self, arn: &str) -> Option<&ProfilingGroup> {
        let group = self.groups.get(profiling_group_name_from_arn(arn)?)?;
        (group.arn == arn).then_some(group)
    }

    /// The same resolution as [`group_by_arn`](Self::group_by_arn), for an operation
    /// that WRITES the resource it resolved. The rule lives in `group_by_arn` alone and
    /// is applied here through it, so the two cannot drift; only the borrow differs,
    /// which is why a reporting operation takes the shared one and physically cannot
    /// write.
    pub fn group_by_arn_mut(&mut self, arn: &str) -> Option<&mut ProfilingGroup> {
        let name = self.group_by_arn(arn)?.name.clone();
        self.groups.get_mut(&name)
    }

    /// Store a posted agent profile, honouring `profile_token` as an idempotency token
    /// and every retention bound: the per-group count and byte caps, and the
    /// store-wide [`MAX_RETAINED_PROFILE_BYTES`] budget, which evicts the oldest
    /// profile anywhere in the store — never the one just posted — until the total
    /// fits. The only way a handler stores a profile.
    pub fn post_profile(
        &mut self,
        group_name: &str,
        profile_token: Option<&str>,
        profile: AgentProfile,
    ) -> PostedProfile {
        let Some(group) = self.groups.get_mut(group_name) else {
            return PostedProfile::UnknownGroup;
        };
        if let Some(token) = profile_token {
            if !group.remember_profile_token(token) {
                return PostedProfile::Replayed;
            }
        }
        group.record_profile(profile);

        while self.retained_profile_bytes() > MAX_RETAINED_PROFILE_BYTES {
            let Some((name, index)) = self.oldest_evictable_profile(group_name) else {
                break;
            };
            if let Some(group) = self.groups.get_mut(&name) {
                group.profiles.remove(index);
            }
        }
        PostedProfile::Stored
    }

    /// Record one recommendation report against `group_name`, within
    /// [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`] (the oldest recorded report is
    /// dropped first, so the report store cannot grow without limit). `false` when
    /// there is no such group.
    ///
    /// The ONLY way a findings report enters this store. The emulator performs no
    /// recommendation analysis of profiles — nothing in it invents findings — so a
    /// group holds reports only where analysis was recorded for it, which is why
    /// `ListFindingsReports` answers a group that has had none with an empty list.
    pub fn record_findings_report(&mut self, group_name: &str, report: FindingsReport) -> bool {
        let Some(group) = self.groups.get_mut(group_name) else {
            return false;
        };
        while group.findings_reports.len() >= MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP {
            group.findings_reports.pop_front();
        }
        group.findings_reports.push_back(report);
        true
    }

    /// Profile bytes retained across every group — what
    /// [`MAX_RETAINED_PROFILE_BYTES`] bounds.
    pub fn retained_profile_bytes(&self) -> usize {
        self.groups
            .values()
            .flat_map(|group| group.profiles.iter())
            .map(|profile| profile.bytes.len())
            .sum()
    }

    /// The oldest retained profile anywhere in the store, as (group name, index),
    /// skipping the newest profile of `keep_newest_of` so a post is never evicted by
    /// its own budget pass.
    fn oldest_evictable_profile(&self, keep_newest_of: &str) -> Option<(String, usize)> {
        let mut oldest: Option<(String, usize, f64)> = None;
        for (name, group) in &self.groups {
            let protected = if name == keep_newest_of {
                group.profiles.len().saturating_sub(1)
            } else {
                usize::MAX
            };
            for (index, profile) in group.profiles.iter().enumerate() {
                if index == protected {
                    continue;
                }
                let at = profile.received_at.as_secs_f64();
                if oldest
                    .as_ref()
                    .is_none_or(|(_, _, candidate)| at < *candidate)
                {
                    oldest = Some((name.clone(), index, at));
                }
            }
        }
        oldest.map(|(name, index, _)| (name, index))
    }
}

/// The message a 404 about missing profile DATA carries — a FIXED service text, with no
/// substitution from the request at all. Both "no such group" and "the group holds no
/// profile in this range" are `ResourceNotFoundException` (404), so the message is the
/// only thing a caller can tell them apart by: the group-missing refusals name the group
/// ([`group_not_found_message`], [`profiling_group_record_not_found_message`]) and the
/// no-data refusal states this constant instead.
pub const NO_PROFILES_IN_TIME_RANGE: &str = "No profiles found for the requested time range!";

/// How CodeGuru Profiler words a refusal about the profiling-group RECORD ITSELF — the
/// collision a create hits and the absence a delete hits: the group is named together
/// with the account it would live in, because a profiling-group name identifies a group
/// only WITHIN one account, so the account is part of saying which group collided or was
/// missing. `outcome` completes the sentence ("already exists.", "not found.").
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries.
fn profiling_group_record_message(name: &str, outcome: &str) -> String {
    format!(
        "ProfilingGroup (name: {name}, accountId: {}) {outcome}",
        ::engine::emulator_account_id()
    )
}

/// The `ConflictException` message for a create whose `profilingGroupName` is already
/// taken: the record-level form of [`profiling_group_record_message`].
pub fn profiling_group_already_exists_message(name: &str) -> String {
    profiling_group_record_message(name, "already exists.")
}

/// The `ResourceNotFoundException` message for a delete of a group that is not there:
/// the record-level form of [`profiling_group_record_message`].
pub fn profiling_group_record_not_found_message(name: &str) -> String {
    profiling_group_record_message(name, "not found.")
}

/// The `ResourceNotFoundException` message every OTHER group-scoped operation answers a
/// name it cannot resolve with: the service states that it cannot carry out the operation
/// asked of it, naming the group that was not there. The apostrophe is the typographic
/// one (U+2019) the service's own text uses, not an ASCII `'`.
///
/// This is the refusal of an operation ON a group, as distinct from a refusal about the
/// group RECORD (create's conflict, delete's absence), which names the account too —
/// see [`profiling_group_record_not_found_message`].
pub fn group_not_found_message(name: &str) -> String {
    format!("CodeGuru Profiler can\u{2019}t complete the operation because {name} was not found.")
}

/// The resource-type prefix a profiling-group ARN carries in its resource field, as
/// [`profiling_group_arn`] mints it: `profilingGroup/<name>`.
pub const PROFILING_GROUP_RESOURCE_PREFIX: &str = "profilingGroup/";

/// The profiling-group name a `ProfilingGroupArn` names, or `None` when the string is
/// not a profiling-group ARN of this service.
///
/// `ProfilingGroupArn` carries an `aws.api#arnReference` to
/// `AWS::CodeGuruProfiler::ProfilingGroup` and no length or pattern constraint, so the
/// boundary validator accepts any string and the SHAPE of the ARN is the handler's to
/// check. What an ARN IS — the six fields, the literal `arn`, the non-empty partition,
/// vendor and resource — is read by the tree's one parse,
/// [`service_common::arn::parse_of`], narrowed to this service's
/// [`ARN_NAMESPACE`]; only the part that is specific to this service is decided here:
/// its resources are the profiling groups, so a resource field that is not a
/// `profilingGroup/<name>` names nothing a tagging operation can act on.
pub fn profiling_group_name_from_arn(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    let name = parsed
        .resource
        .strip_prefix(PROFILING_GROUP_RESOURCE_PREFIX)?;
    // A profiling-group name is one path segment (the modelled `ProfilingGroupName`
    // pattern is `^[\w-]+$`), so an empty name or a further `/` is not one of this
    // service's resources. `parse_of` keeps the resource field's own colons, so this
    // check is on the name as the ARN carries it.
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name)
}

/// One instant as an EXACTLY comparable key: whole seconds and sub-second nanoseconds.
///
/// `DateTime` carries nanosecond resolution, which an `f64` of current epoch seconds
/// cannot represent (its granularity there is coarser than a microsecond), so two
/// instants a fraction of a microsecond apart would compare equal. Ordering and
/// inclusive-bound tests on stored timestamps go through this instead.
pub fn instant_key(at: DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// The ARN of a profiling group:
/// `arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/<name>`.
pub fn profiling_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{PROFILING_GROUP_RESOURCE_PREFIX}{name}"),
    )
}

/// How the AGENT plane says that the profiling group a request names does not exist: its
/// message is account-qualified, naming the account the group would belong to as well as
/// the group — a different sentence from the one every other operation states
/// ([`group_not_found_message`]).
///
/// The agent plane is `ConfigureAgent` and `PostAgentProfile` — the two operations the
/// model's own `ActionGroup` enum names as the agent APIs ("Permission group type for Agent
/// APIs - ConfigureAgent, PostAgentProfile",
/// `smithy_query(query="shape", shape="com.amazonaws.codeguruprofiler#ActionGroup")`). The
/// account is the emulator's canonical one, the same account every ARN this service mints
/// carries.
pub fn agent_group_not_found_message(name: &str) -> String {
    format!(
        "Profiling group for {}/{name} does not exist!",
        ::engine::emulator_account_id()
    )
}

/// Why a caller-supplied `resourceArn` that names no profiling group of this service is
/// refused on the INPUT: it names the member and the form the member must take, so the
/// caller can fix the request from the message alone.
pub fn resource_arn_validation_message(resource_arn: &str) -> String {
    format!(
        "Value '{resource_arn}' at 'resourceArn' failed to satisfy constraint: Member \
         must be the ARN of a CodeGuru Profiler profiling group, \
         arn:<partition>:{ARN_NAMESPACE}:<region>:<account>:{PROFILING_GROUP_RESOURCE_PREFIX}<name>"
    )
}

/// Refuse a caller-supplied tag map no AWS-tagged resource can hold: more than
/// [`MAX_TAGS_PER_RESOURCE`] entries, a key outside 1..[`MAX_TAG_KEY_CHARS`]
/// characters, or a value longer than [`MAX_TAG_VALUE_CHARS`].
///
/// `Ok(())` when every entry is inside those limits; otherwise the message naming the
/// member (`member`, the request member the map arrived in), the entry and the
/// constraint it failed, which each handler lifts into its own modelled
/// `ValidationException`. Every path that WRITES a resource's tags checks this BEFORE
/// storing anything, so a refused request leaves the stored map exactly as it was and
/// no request can grow the store past the published limits.
pub fn validate_tags(member: &str, tags: &HashMap<String, String>) -> Result<(), String> {
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "Value at '{member}' failed to satisfy constraint: Member must have length \
             less than or equal to {MAX_TAGS_PER_RESOURCE}"
        ));
    }
    // Lexicographic order, so the entry a message blames is the same one on every run
    // whatever order the request's map was read in.
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort();
    for key in keys {
        let key_chars = key.chars().count();
        if !(1..=MAX_TAG_KEY_CHARS).contains(&key_chars) {
            return Err(format!(
                "Value '{key}' at '{member}' failed to satisfy constraint: Map keys must \
                 have length between 1 and {MAX_TAG_KEY_CHARS}"
            ));
        }
        let value = &tags[key];
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return Err(format!(
                "Value at '{member}.{key}' failed to satisfy constraint: Member must have \
                 length less than or equal to {MAX_TAG_VALUE_CHARS}"
            ));
        }
    }
    Ok(())
}

/// How many tags `stored` would hold once `applied` has been merged into it: a key
/// `stored` already carries is re-tagged rather than added, so only a NEW key grows the
/// map. What [`MAX_TAGS_PER_RESOURCE`] is checked against before a merge is written.
pub fn merged_tag_count(
    stored: &HashMap<String, String>,
    applied: &HashMap<String, String>,
) -> usize {
    stored.len() + applied.keys().filter(|key| !stored.contains_key(*key)).count()
}

/// Why a merge that would leave a resource past [`MAX_TAGS_PER_RESOURCE`] is refused:
/// the member it arrived in, how many tags it would leave, and the limit. Beside
/// [`merged_tag_count`] and worded like [`validate_tags`], so every tag refusal of this
/// service speaks with one voice.
pub fn too_many_tags_message(member: &str, resulting: usize) -> String {
    format!(
        "Value at '{member}' failed to satisfy constraint: applying these tags would \
         leave {resulting} tags on the resource, past the maximum of \
         {MAX_TAGS_PER_RESOURCE} tags per resource"
    )
}

/// The region this request was signed for, defaulting to [`DEFAULT_REGION`] when the
/// request carries no SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the wire timestamp type every modelled timestamp member uses.
pub fn now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeguruprofilerCodeguruprofilerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>)
}
