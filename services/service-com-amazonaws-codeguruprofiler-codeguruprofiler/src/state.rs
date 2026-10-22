// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeguruprofiler-codeguruprofiler service.
//!
//! One map of profiling groups, keyed by name — the service's only resource. Every
//! handler computes its response from what is stored here; nothing is canned.

use std::collections::{HashMap, VecDeque};

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
    /// The recommendation reports recorded against this group, oldest recorded FIRST.
    /// Bounded at [`MAX_RETAINED_FINDINGS_REPORTS_PER_GROUP`] by
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::record_findings_report`],
    /// which is the only writer.
    pub findings_reports: VecDeque<FindingsReport>,
}

impl ProfilingGroup {
    /// The `ProfilingGroupDescription` every profiling-group operation returns, built
    /// from what is stored — never from the request that is asking for it.
    pub fn description(&self) -> ProfilingGroupDescription {
        ProfilingGroupDescription {
            name: Some(self.name.clone()),
            agent_orchestration_config: Some(AgentOrchestrationConfig {
                profiling_enabled: self.profiling_enabled,
            }),
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            profiling_status: Some(ProfilingStatus {
                latest_agent_profile_reported_at: self.latest_agent_profile_reported_at,
                // Aggregating posted agent profiles into an aggregated profile is not
                // implemented in this slice, so there is no aggregated-profile time to
                // report. It stays absent rather than being invented.
                latest_aggregated_profile: None,
                latest_agent_orchestrated_at: self.latest_agent_orchestrated_at,
            }),
            compute_platform: Some(self.compute_platform.clone()),
            tags: Some(self.tags.clone()),
        }
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

/// The ARN of a profiling group:
/// `arn:aws:codeguru-profiler:<region>:<account>:profilingGroup/<name>`.
pub fn profiling_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("profilingGroup/{name}"))
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
