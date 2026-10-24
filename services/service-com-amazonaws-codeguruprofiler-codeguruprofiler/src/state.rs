// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeguruprofiler-codeguruprofiler service.
//!
//! One map of profiling groups, keyed by name — the service's only resource. Every
//! handler computes its response from what is stored here; nothing is canned.

use std::collections::{BTreeMap, HashMap, VecDeque};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    AgentOrchestrationConfig, Channel, FindingsReportSummary, NotificationConfiguration,
    ProfilingGroupDescription, ProfilingStatus,
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

/// How many anomaly-notification channels one profiling group holds. NOT an
/// emulator-chosen bound: the model's `Channels` list carries `@length(min: 1, max: 2)`
/// both on the `AddNotificationChannels` input and on the `NotificationConfiguration`
/// every notification operation returns, and `AddNotificationChannels`' own doc says
/// "Add up to 2 anomaly notifications channels for a profiling group". An add that
/// would push a group past this is refused with that operation's modelled
/// `ServiceQuotaExceededException`.
pub const MAX_NOTIFICATION_CHANNELS_PER_GROUP: usize = 2;

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

/// The hex-digit group lengths a `RevisionId` carries, as the model's `RevisionId`
/// pattern (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`) spells them
/// out: 32 lowercase hex characters in five hyphen-separated groups.
pub const REVISION_ID_GROUPS: &[usize] = &[8, 4, 4, 4, 12];

/// A `RevisionId` identifying one policy write: 32 lowercase hex characters grouped the
/// way [`REVISION_ID_GROUPS`] records the modelled pattern, which is the form the
/// `PutPermission` doc names ("A universally unique identifier (UUID) for the revision
/// of the policy").
///
/// Derived with SHA-256 (the `sha2` crate) from the write itself — the store's own
/// monotonically increasing mint count, the instant of the write, and the group and
/// action group being written. The mint count is what makes two writes of the same
/// grants carry different revisions, and it is store state a caller never sees, so a
/// revision cannot be computed from the request that produced it.
fn revision_id(mint: u64, at: DateTime, group_name: &str, action_group: &str) -> String {
    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, mint.to_be_bytes());
    ::sha2::Digest::update(&mut digest, at.secs().to_be_bytes());
    ::sha2::Digest::update(&mut digest, at.subsec_nanos().to_be_bytes());
    ::sha2::Digest::update(&mut digest, (group_name.len() as u64).to_be_bytes());
    ::sha2::Digest::update(&mut digest, group_name.as_bytes());
    ::sha2::Digest::update(&mut digest, action_group.as_bytes());
    let hex = ::hex::encode(::sha2::Digest::finalize(digest));

    // The first 32 hex characters of the digest, cut into the modelled groups.
    let mut remaining = &hex[..REVISION_ID_GROUPS.iter().sum::<usize>()];
    let mut revision = String::with_capacity(remaining.len() + REVISION_ID_GROUPS.len() - 1);
    for (index, group) in REVISION_ID_GROUPS.iter().enumerate() {
        if index > 0 {
            revision.push('-');
        }
        let (head, tail) = remaining.split_at(*group);
        revision.push_str(head);
        remaining = tail;
    }
    revision
}

/// The IAM policy-language version a resource-based policy document carries. The
/// version the service's own resource-policy document uses (the AWS CodeGuru Profiler
/// User Guide, "Resource-based policies in CodeGuru Profiler").
pub const POLICY_VERSION: &str = "2012-10-17";

/// The one action group a resource-based policy can grant, as the model's `ActionGroup`
/// enum declares it (`smithy_query(query="shape",shape="ActionGroup")`).
pub const ACTION_GROUP_AGENT_PERMISSIONS: &str = "agentPermissions";

/// Every action group a resource-based policy can hold a statement for: the values the
/// model's `ActionGroup` enum declares, as the generated
/// [`crate::ops::types::ACTION_GROUP_VALUES`] lists them (one, `agentPermissions`) — the
/// handlers refuse anything outside that list, so there is ONE list of them in this
/// crate and the bound below follows the model rather than a copy of it.
pub const MAX_POLICY_STATEMENTS: usize = crate::ops::types::ACTION_GROUP_VALUES.len();

/// The most principals one statement grants to: the maximum length the model's
/// `Principals` list declares (`smithy_query(query="shape",shape="Principals")`:
/// `length 1..50`). The validating request walk already refuses a longer list at the
/// boundary; the store holds to the same bound so no writer can grow a statement past
/// what the model allows.
pub const MAX_PRINCIPALS_PER_STATEMENT: usize = 50;

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
    /// This constructor and the two writers below are what the permission handlers'
    /// store writers —
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`] and
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`] — build
    /// a policy with, so every operation that returns `policy` renders the same
    /// document from the same grants.
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

    /// Drop `action_group`'s grant, reporting whether the policy held one. The
    /// revision is not touched here, for the same reason as
    /// [`ResourcePolicy::grant`]: the writer rotates it once for the change.
    pub fn revoke(&mut self, action_group: &str) -> bool {
        self.statements.remove(action_group).is_some()
    }

    /// The JSON-formatted resource-based policy document, as every operation that
    /// returns `policy` reports it: one `Allow` statement per stored action group,
    /// carrying that action group's statement id ([`action_group_statement_id`]), the
    /// principals that hold it, the actions the action group grants and `resource_arn` —
    /// the profiling group's own stored ARN.
    ///
    /// The document is an IAM policy document, so it is written the way an IAM policy
    /// document is written: a single principal is the ARN itself rather than a
    /// one-element list (both forms are accepted by a policy reader, and the single-ARN
    /// grant is reported in the scalar form), and the whole document is laid out by
    /// [`PolicyDocumentFormatter`]. The member ORDER is fixed by the structures below
    /// rather than by a `serde_json::Map`, whose iteration order depends on whether
    /// something in the build enabled the crate's `preserve_order` feature.
    pub fn document(&self, resource_arn: &str) -> String {
        let document = PolicyDocumentWire {
            version: POLICY_VERSION,
            statement: self
                .statements
                .iter()
                .map(|(action_group, principals)| PolicyStatementWire {
                    sid: action_group_statement_id(action_group),
                    effect: POLICY_EFFECT_ALLOW,
                    principal: PolicyPrincipalWire {
                        aws: PolicyPrincipalsWire::of(principals),
                    },
                    action: action_group_actions(action_group),
                    resource: resource_arn,
                })
                .collect(),
        };
        render_policy_document(&document)
    }
}

/// The `Effect` a stored grant carries: a resource-based policy holds the grants that
/// were made, so every statement it renders allows them.
const POLICY_EFFECT_ALLOW: &str = "Allow";

/// The suffix a policy statement's `Sid` carries past the action group it states: the
/// statement id of an action group's statement is the action group's own name plus
/// this, which is how a client locates its statement in the document again.
pub const STATEMENT_SID_SUFFIX: &str = "-statement";

/// The `Sid` the statement for `action_group` carries — the action group's statement
/// id. Applies to EVERY action group, so a document rendered for one the catalog does
/// not know still names its statement the same way.
pub fn action_group_statement_id(action_group: &str) -> String {
    format!("{action_group}{STATEMENT_SID_SUFFIX}")
}

/// One policy document, as the wire renders it. The field ORDER is the order the
/// document's members are written in, which is why these are structures and not
/// `serde_json::Value` maps.
#[derive(Debug, ::serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyDocumentWire<'a> {
    version: &'a str,
    statement: Vec<PolicyStatementWire<'a>>,
}

/// One `Allow` statement of a policy document: one stored action group's grant.
#[derive(Debug, ::serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyStatementWire<'a> {
    sid: String,
    effect: &'a str,
    principal: PolicyPrincipalWire,
    action: Vec<String>,
    resource: &'a str,
}

/// A statement's `Principal` block. `AWS` is spelled by the policy language, not by
/// Rust's casing rules, so it is renamed rather than derived.
#[derive(Debug, ::serde::Serialize)]
struct PolicyPrincipalWire {
    #[serde(rename = "AWS")]
    aws: PolicyPrincipalsWire,
}

/// The principals a statement grants to: an IAM policy document writes ONE principal
/// as the ARN itself and several as a list, so that is what is rendered.
#[derive(Debug, ::serde::Serialize)]
#[serde(untagged)]
enum PolicyPrincipalsWire {
    /// Exactly one principal: the ARN as a string scalar.
    One(String),
    /// Several (or, for a statement that somehow holds none, zero) principals.
    Several(Vec<String>),
}

impl PolicyPrincipalsWire {
    /// The rendered form of a stored principal set.
    fn of(principals: &[String]) -> Self {
        match principals {
            [only] => Self::One(only.clone()),
            several => Self::Several(several.to_vec()),
        }
    }
}

/// The JSON layout a resource-based policy document is written in: two-space
/// indentation, `" : "` between a member name and its value, and an array kept on one
/// line with a single space inside its brackets — the layout the policy documents this
/// surface returns carry.
///
/// This is `serde_json`'s OWN writer, configured through the one seam that fixes
/// layout ([`::serde_json::ser::Formatter`]): string escaping, number formatting and
/// UTF-8 handling all stay with `serde_json`.
///
/// Only the OBJECT hooks indent, because an array is written inline — which is the
/// layout, and which also means an array nested DIRECTLY inside another array would be
/// written `[ [ … ] ]` on one line, and an empty array as `[ ]`. The empty form is
/// reachable (an action group outside the catalog grants no actions, so its `Action` is
/// empty) and is written that way deliberately; an array directly inside an array is
/// not a shape any member of a policy document takes, and a member that ever took it
/// would have to be laid out here deliberately.
#[derive(Debug, Default)]
struct PolicyDocumentFormatter {
    /// How many OBJECTS deep the writer is — what the indentation counts. An array is
    /// written on one line, so it adds no depth.
    depth: usize,
    /// Whether the object open at each depth has written a member yet, innermost last:
    /// an object that wrote none closes as `{ }` rather than across two lines.
    object_wrote_member: Vec<bool>,
}

impl PolicyDocumentFormatter {
    /// A line break, then this depth's indentation.
    fn newline_indent<W>(&self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b"\n")?;
        for _ in 0..self.depth {
            writer.write_all(b"  ")?;
        }
        Ok(())
    }
}

impl ::serde_json::ser::Formatter for PolicyDocumentFormatter {
    fn begin_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth += 1;
        self.object_wrote_member.push(false);
        writer.write_all(b"{")
    }

    fn end_object<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        self.depth = self.depth.saturating_sub(1);
        if self.object_wrote_member.pop().unwrap_or(false) {
            self.newline_indent(writer)?;
        } else {
            writer.write_all(b" ")?;
        }
        writer.write_all(b"}")
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if !first {
            writer.write_all(b",")?;
        }
        if let Some(wrote) = self.object_wrote_member.last_mut() {
            *wrote = true;
        }
        self.newline_indent(writer)
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b" : ")
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        if !first {
            writer.write_all(b",")?;
        }
        writer.write_all(b" ")
    }

    fn end_array<W>(&mut self, writer: &mut W) -> ::std::io::Result<()>
    where
        W: ?Sized + ::std::io::Write,
    {
        writer.write_all(b" ]")
    }
}

/// `document`, written out with [`PolicyDocumentFormatter`].
fn render_policy_document(document: &PolicyDocumentWire<'_>) -> String {
    let mut buffer: Vec<u8> = Vec::new();
    let mut serializer =
        ::serde_json::Serializer::with_formatter(&mut buffer, PolicyDocumentFormatter::default());
    // Infallible: the only fallible steps are the write — into an in-memory buffer —
    // and the two values `serde_json` refuses (a non-string map key, a non-finite
    // number), neither of which a policy document holds.
    ::serde::Serialize::serialize(document, &mut serializer)
        .expect("a policy document writes into an in-memory buffer");
    String::from_utf8(buffer).expect("serde_json writes UTF-8")
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

/// One anomaly-notification channel of a profiling group, as
/// `AddNotificationChannels` stored it.
///
/// `id` is a plain `String` rather than the modelled `Option<String>`: a STORED channel
/// always has an id — the one the request named, or the random UUID the model's
/// `Channel$id` doc says is used "when adding a channel to the notification
/// configuration if not specified in the request" — and
/// `RemoveNotificationChannel` addresses a channel by it.
#[derive(Debug, Clone)]
pub struct NotificationChannel {
    pub id: String,
    pub uri: String,
    pub event_publishers: Vec<String>,
}

impl NotificationChannel {
    /// This channel as the modelled shape every notification operation reports.
    pub fn to_modeled(&self) -> Channel {
        Channel {
            id: Some(self.id.clone()),
            uri: self.uri.clone(),
            event_publishers: self.event_publishers.clone(),
        }
    }
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
    /// This group's anomaly-notification channels, in the order they were added —
    /// what `GetNotificationConfiguration` reports, `AddNotificationChannels` appends
    /// to and `RemoveNotificationChannel` deletes from. Bounded at
    /// [`MAX_NOTIFICATION_CHANNELS_PER_GROUP`] by
    /// [`ProfilingGroup::add_notification_channels`], which is the only writer.
    pub notification_channels: Vec<NotificationChannel>,
    /// The group's resource-based policy, or `None` while it has none — a group is
    /// created without one, and the permission writers
    /// ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`] and
    /// [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`], which
    /// are what enforce the revision check) attach and clear it. It lives ON the group
    /// so deleting the group takes its policy with it and a re-created group starts
    /// with none. Read it through [`ProfilingGroup::resource_policy`].
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

    /// The notification configuration every notification operation answers with,
    /// built from the channels stored on THIS group.
    ///
    /// `channels` is always present, as an empty list when the group has none: that is
    /// what distinguishes a group whose notifications nobody has configured from one
    /// whose configuration is unknown (which is a 404 instead).
    pub fn notification_configuration(&self) -> NotificationConfiguration {
        NotificationConfiguration {
            channels: Some(
                self.notification_channels
                    .iter()
                    .map(NotificationChannel::to_modeled)
                    .collect(),
            ),
        }
    }

    /// Whether this group already holds a channel with `id`.
    pub fn has_notification_channel(&self, id: &str) -> bool {
        self.notification_channels
            .iter()
            .any(|channel| channel.id == id)
    }

    /// Append a batch of notification channels, ALL or nothing: `false` — and nothing
    /// stored — unless every channel fits, i.e. no channel repeats an id this group or
    /// an earlier channel of the batch already holds, and the group stays within
    /// [`MAX_NOTIFICATION_CHANNELS_PER_GROUP`]. The only writer, so the stored
    /// configuration never leaves what the model's `Channels` list allows and a refused
    /// add is never a partial one.
    pub fn add_notification_channels(&mut self, channels: Vec<NotificationChannel>) -> bool {
        if self.notification_channels.len() + channels.len() > MAX_NOTIFICATION_CHANNELS_PER_GROUP {
            return false;
        }
        let repeated = channels.iter().enumerate().any(|(index, channel)| {
            self.has_notification_channel(&channel.id)
                || channels[..index]
                    .iter()
                    .any(|earlier| earlier.id == channel.id)
        });
        if repeated {
            return false;
        }
        self.notification_channels.extend(channels);
        true
    }

    /// Remove the channel with `id`, answering whether there was one.
    pub fn remove_notification_channel(&mut self, id: &str) -> bool {
        let before = self.notification_channels.len();
        self.notification_channels
            .retain(|channel| channel.id != id);
        self.notification_channels.len() != before
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
    /// How many policy revisions this store has minted. Bumped by
    /// [`Self::mint_revision_id`], the only minter, and mixed into every revision it
    /// derives, so two writes of the same grants never carry the same revision.
    pub revisions_minted: u64,
    /// How many notification-channel ids this store has minted. Bumped by
    /// [`Self::mint_channel_id`], the only minter, and mixed into every id it derives,
    /// so two channels added in the same instant never carry the same id.
    pub channel_ids_minted: u64,
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

/// Why a policy write did not match the policy's current revision — every case a
/// conflict, since each says the caller is writing against a revision that is not the
/// one stored.
///
/// Each case carries the revision ids the conflict is ABOUT: the one the request named,
/// where it named one, and the one the stored policy carries, where there is a stored
/// policy. A revision-conflict refusal is self-healing only if it hands the caller the
/// revision to retry against, so the ids travel with the mismatch rather than being
/// left for a second `GetPolicy` round trip (see [`revision_conflict_message`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionMismatch {
    /// A policy already exists, so the write must name the revision it updates:
    /// `PutPermission`'s doc, "Subsequent calls must provide a revisionId to specify
    /// which revision of the resource-based policy to add the permissions to."
    Missing {
        /// The revision the stored policy carries — the one the next write must name.
        current: String,
    },
    /// The write named a revision, but the group has no policy yet:
    /// `PutPermission`'s doc, "The first time you call PutPermission on a profiling
    /// group, do not specify a revisionId because it doesn't have a resource-based
    /// policy."
    Unexpected {
        /// The revision the request named, which identifies no policy.
        requested: String,
    },
    /// The write named a revision other than the current one — a stale revision.
    Stale {
        /// The revision the request named, which the policy has moved past.
        requested: String,
        /// The revision the stored policy carries now — the one to retry against.
        current: String,
    },
}

/// Why a policy write would have left a policy larger than the service holds — the
/// store's own bounds on a resource-based policy, named so the refusal can say which
/// one was reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyLimit {
    /// The grant named more principals than [`MAX_PRINCIPALS_PER_STATEMENT`].
    Principals { requested: usize, limit: usize },
    /// The write would have left the policy holding statements for more action groups
    /// than [`MAX_POLICY_STATEMENTS`].
    Statements { requested: usize, limit: usize },
}

/// What a policy write ([`ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`]
/// or [`ComAmazonawsCodeguruprofilerCodeguruprofilerState::remove_permission`]) did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyWrite {
    /// Applied: the policy document as it now stands, and the revision it now carries.
    Written { policy: String, revision_id: String },
    /// Applied, and it left the group with NO resource-based policy: the removal took
    /// the policy's last statement, so there is no document to report and no revision —
    /// the group is back to the state it was created in, which is what `GetPolicy` then
    /// reports and what the next `PutPermission` writes against (naming no revision).
    Cleared,
    /// There is no such profiling group.
    UnknownGroup,
    /// The group has no resource-based policy at all, so there is nothing to remove
    /// from.
    NoPolicy,
    /// The group HAS a resource-based policy, but it holds no statement for the named
    /// action group — a different state from [`PolicyWrite::NoPolicy`], and a different
    /// thing to say about it.
    NoStatement,
    /// The write was refused against the stored revision; nothing changed.
    RevisionMismatch(RevisionMismatch),
    /// The write was refused because it would have left the policy past one of the
    /// store's bounds; nothing changed.
    LimitExceeded(PolicyLimit),
}

impl ComAmazonawsCodeguruprofilerCodeguruprofilerState {
    /// The `RevisionId` the next policy write carries: the store's mint count is bumped
    /// first, so every revision this store hands out is distinct even when two writes
    /// grant exactly the same principals. The ONLY minter.
    fn mint_revision_id(&mut self, group_name: &str, action_group: &str) -> String {
        self.revisions_minted = self.revisions_minted.wrapping_add(1);
        revision_id(self.revisions_minted, now(), group_name, action_group)
    }

    /// The `ChannelId` the next notification channel the caller did not name one for
    /// carries: the store's own mint count is bumped first, so every id this store
    /// hands out is distinct even for two channels added in the same instant. The ONLY
    /// minter.
    pub fn mint_channel_id(&mut self, group_name: &str) -> String {
        self.channel_ids_minted = self.channel_ids_minted.wrapping_add(1);
        channel_id(self.channel_ids_minted, now(), group_name)
    }

    /// Grant `action_group`'s permissions to exactly `principals` on `group_name`,
    /// under optimistic locking on `revision_id`.
    ///
    /// The principal set REPLACES whatever that action group last granted (the write
    /// is a `PUT` of the action group's statement, not a merge), duplicate principals
    /// in one request collapse to one, and a successful write mints a new revision —
    /// so the revision a caller holds is good for exactly one write.
    ///
    /// Bounded like every other store in this module: a grant to more than
    /// [`MAX_PRINCIPALS_PER_STATEMENT`] principals, or one that would leave the policy
    /// holding more than [`MAX_POLICY_STATEMENTS`] statements, is refused with
    /// [`PolicyWrite::LimitExceeded`] and changes nothing, so no sequence of writes can
    /// grow one policy without limit.
    pub fn put_permission(
        &mut self,
        group_name: &str,
        action_group: &str,
        principals: &[String],
        revision_id: Option<&str>,
    ) -> PolicyWrite {
        let Some(group) = self.groups.get(group_name) else {
            return PolicyWrite::UnknownGroup;
        };
        if principals.len() > MAX_PRINCIPALS_PER_STATEMENT {
            return PolicyWrite::LimitExceeded(PolicyLimit::Principals {
                requested: principals.len(),
                limit: MAX_PRINCIPALS_PER_STATEMENT,
            });
        }
        // A policy with no grants is no policy (`ProfilingGroup::resource_policy`), so
        // a write against one is a FIRST write and must name no revision.
        match (group.resource_policy(), revision_id) {
            (None, Some(named)) => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Unexpected {
                    requested: named.to_string(),
                });
            }
            (Some(policy), None) => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Missing {
                    current: policy.revision_id.clone(),
                });
            }
            (Some(policy), Some(named)) if policy.revision_id != named => {
                return PolicyWrite::RevisionMismatch(RevisionMismatch::Stale {
                    requested: named.to_string(),
                    current: policy.revision_id.clone(),
                });
            }
            _ => {}
        }

        // How many statements the policy would hold once this one is written: rewriting
        // a statement the policy already holds adds none.
        let would_hold = group
            .resource_policy()
            .map(|policy| {
                policy.statements.len() + usize::from(!policy.statements.contains_key(action_group))
            })
            .unwrap_or(1);
        if would_hold > MAX_POLICY_STATEMENTS {
            return PolicyWrite::LimitExceeded(PolicyLimit::Statements {
                requested: would_hold,
                limit: MAX_POLICY_STATEMENTS,
            });
        }

        // A grant is a SET of principals: the request's order is kept, and a principal
        // it names twice is granted once. The `seen` set is what makes that one pass
        // over the list rather than one scan of the kept principals per principal.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut granted: Vec<String> = Vec::with_capacity(principals.len());
        for principal in principals {
            if seen.insert(principal.as_str()) {
                granted.push(principal.clone());
            }
        }

        // The write is applied from here on, so this is where the revision it carries
        // is minted.
        let revision_id = self.mint_revision_id(group_name, action_group);
        let Some(group) = self.groups.get_mut(group_name) else {
            // Unreachable: the group was resolved above, under the same `&mut self`.
            return PolicyWrite::UnknownGroup;
        };
        let arn = group.arn.clone();
        let mut policy = group
            .policy
            .take()
            .unwrap_or_else(|| ResourcePolicy::new(&revision_id));
        policy.grant(action_group, granted);
        policy.set_revision(&revision_id);
        let document = policy.document(&arn);
        group.policy = Some(policy);
        PolicyWrite::Written {
            policy: document,
            revision_id,
        }
    }

    /// Remove `action_group`'s statement from `group_name`'s policy, under the same
    /// optimistic locking as [`Self::put_permission`]: the revision is checked first,
    /// so a stale revision is answered as a conflict whatever else the request names.
    ///
    /// Removing the last statement leaves the group with NO resource-based policy at
    /// all, which is [`PolicyWrite::Cleared`]: there is then no document to report and
    /// no revision to report, because none is stored — a revision a caller could not
    /// write against is not handed out. A removal that leaves other statements behind
    /// is [`PolicyWrite::Written`], carrying the policy as it then stands and the
    /// revision the change minted.
    pub fn remove_permission(
        &mut self,
        group_name: &str,
        action_group: &str,
        revision_id: &str,
    ) -> PolicyWrite {
        // The checks first, under a shared borrow, so the revision is minted only for a
        // removal that is actually applied.
        let Some(group) = self.groups.get(group_name) else {
            return PolicyWrite::UnknownGroup;
        };
        // An emptied policy is no policy, exactly as `resource_policy` reports it, so
        // there is nothing to remove from.
        let Some(policy) = group.resource_policy() else {
            return PolicyWrite::NoPolicy;
        };
        if policy.revision_id != revision_id {
            return PolicyWrite::RevisionMismatch(RevisionMismatch::Stale {
                requested: revision_id.to_string(),
                current: policy.revision_id.clone(),
            });
        }
        if !policy.statements.contains_key(action_group) {
            return PolicyWrite::NoStatement;
        }

        // Whether this removal takes the policy's LAST statement: then the group is
        // left with no policy, so nothing is rendered and no revision is minted.
        if policy.statements.len() == 1 {
            let Some(group) = self.groups.get_mut(group_name) else {
                // Unreachable: the group was resolved above, under the same `&mut self`.
                // Reported as the absence rather than as a clear that did not happen.
                return PolicyWrite::UnknownGroup;
            };
            group.policy = None;
            return PolicyWrite::Cleared;
        }

        let revision_id = self.mint_revision_id(group_name, action_group);
        let Some(group) = self.groups.get_mut(group_name) else {
            // Unreachable: the group was resolved above, under the same `&mut self`.
            return PolicyWrite::UnknownGroup;
        };
        let Some(policy) = group.policy.as_mut() else {
            // Unreachable for the same reason: the policy was read above.
            return PolicyWrite::NoPolicy;
        };
        policy.revoke(action_group);
        policy.set_revision(&revision_id);
        // The policy as it stands after the removal, which still holds at least one
        // statement — the emptying case returned above.
        let document = policy.document(&group.arn);
        PolicyWrite::Written {
            policy: document,
            revision_id,
        }
    }

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

/// The message a bound-refused policy write carries, for the `ValidationException` the
/// write answers with: which bound the write would have passed, and by how much. The
/// bounds themselves are the store's ([`MAX_PRINCIPALS_PER_STATEMENT`] is the modelled
/// `Principals` maximum, [`MAX_POLICY_STATEMENTS`] the number of modelled action
/// groups), so the text states the member and the constraint the request missed.
pub fn policy_limit_message(group_name: &str, limit: &PolicyLimit) -> String {
    match limit {
        PolicyLimit::Principals { requested, limit } => format!(
            "principals must name at most {limit} roles or users, got {requested}"
        ),
        PolicyLimit::Statements { requested, limit } => format!(
            "the resource-based policy of profiling group {group_name} holds at most \
             {limit} action group statements, and this write would leave it holding \
             {requested}"
        ),
    }
}

/// Why a revision-checked policy write was refused, for the `ConflictException` the
/// write answers with. The status (409) and the code are the contract; this text is
/// built from the request and the stored resource, so it STATES both revisions the
/// conflict is about: the revision the request named (where it named one) and the
/// revision the stored policy carries (where there is one) — which is the revision the
/// caller retries against, so a conflict resolves without a second `GetPolicy`.
///
/// Only the `Stale` sentence is a RECORDED one. The other two arms are this emulator's
/// own wording around the same two facts: a recording that shows how the service words
/// them is the authority over this text, not a regression against it.
pub fn revision_conflict_message(group_name: &str, mismatch: &RevisionMismatch) -> String {
    match mismatch {
        RevisionMismatch::Missing { current } => format!(
            "profiling group {group_name} already has a resource-based policy; name its \
             current revision to write to it, RevisionId: {current}"
        ),
        RevisionMismatch::Unexpected { requested } => format!(
            "profiling group {group_name} has no resource-based policy, so the revision: \
             {requested} names none; do not name a revisionId when creating one"
        ),
        RevisionMismatch::Stale { requested, current } => format!(
            "Policy has been updated since the revision: {requested}, new RevisionId: \
             {current}"
        ),
    }
}

/// The `ResourceNotFoundException` message a policy-scoped operation answers a group
/// that EXISTS but has no resource-based policy with: the group as the request named it,
/// and the account the lookup was scoped to — a profiling-group name identifies a group
/// only within one account, so the account is part of saying whose view of the group had
/// no policy.
///
/// The account is this emulator's own ([`::engine::emulator_account_id`]), which is the
/// account every resource it mints lives in — the same one the group's ARN carries. This
/// is distinct from [`group_not_found_message`], which answers a group that is not there
/// at all.
pub fn policy_not_found_message(group_name: &str) -> String {
    format!(
        "No policy attached to profiling group name: {group_name} in accountId: {}; ",
        ::engine::emulator_account_id()
    )
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

/// How the POLICY plane says that the profiling group a request names does not exist:
/// `GetPolicy` states the LOOKUP it performed — the account the lookup was scoped to and
/// the name it looked for — rather than the sentence every other group-scoped operation
/// states ([`group_not_found_message`]).
///
/// Why this operation has a form of its own: both "no such group" and "the group has no
/// resource-based policy" are 404 `ResourceNotFoundException` here, so the message is the
/// only thing a caller can tell them apart by — this one says the GROUP lookup found
/// nothing, [`policy_not_found_message`] says the group was found and holds no policy.
///
/// `PutPermission` and `RemovePermission` keep [`group_not_found_message`] for a group
/// that is not there: their own wording for that case is not recorded anywhere, and a
/// per-operation sentence this service has never been observed to emit would be invented
/// rather than emulated.
pub fn policy_group_not_found_message(name: &str) -> String {
    format!(
        "No profiling group with accountId: {}; name: {name} was found.",
        ::engine::emulator_account_id()
    )
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

/// A fresh notification-channel id for a channel the request did not name one for.
///
/// The model's `ChannelId` carries the SAME pattern as `RevisionId`
/// (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`) and `Channel$id`'s
/// doc says "A random UUID for channelId is used when adding a channel to the
/// notification configuration if not specified in the request" — so the id is derived
/// exactly the way this crate derives its other UUID-shaped id, through
/// [`revision_id`]: SHA-256 over the store's own monotonically increasing mint count,
/// the instant of the write and the group the channel is added to, cut into the five
/// lowercase-hex groups [`REVISION_ID_GROUPS`] records. The mint count is store state a
/// caller never sees, so a channel id cannot be computed from the request that produced
/// it, and two channels added in the same instant still differ.
fn channel_id(mint: u64, at: DateTime, group_name: &str) -> String {
    revision_id(mint, at, group_name, CHANNEL_ID_SCOPE)
}

/// What [`channel_id`] mixes into its digest in place of an action group, so a channel
/// id and a policy revision minted from the same count and instant differ.
const CHANNEL_ID_SCOPE: &str = "notificationChannel";

/// The ARN namespace (`aws.api#service$arnNamespace`) of Amazon SNS, the vendor field
/// of a topic ARN.
pub const SNS_ARN_NAMESPACE: &str = "sns";

/// Whether `uri` is an Amazon SNS topic ARN, which is what a channel uri must be:
/// `ChannelUri` is an `@arnReference` and its model doc says "Currently we only support
/// SNS channels and thus required to be an ARN".
///
/// The ARN itself is read by `service_common::arn`, the one caller-supplied-ARN parse
/// in this tree (six fields, a literal `arn`, a non-empty partition, vendor and
/// resource; region and account may be empty, as a partition-global ARN writes them);
/// only the vendor this service accepts is decided here.
pub fn is_sns_topic_arn(uri: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(uri, SNS_ARN_NAMESPACE)
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
