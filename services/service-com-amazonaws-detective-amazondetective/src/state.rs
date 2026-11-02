// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-detective-amazondetective service.
//!
//! The service's primary resource is the **behavior graph**. A graph is stored
//! exactly once, keyed by its own ARN — the identifier every operation names it by
//! — and it owns the member accounts invited into it, so `CreateMembers`,
//! `ListMembers` and `GetMembers` all read and write the one record. Deleting the
//! graph therefore drops its members and its tags with it, which is what
//! `DeleteGraph` documents ("removes the behavior graph from each member account's
//! list of behavior graphs").
//!
//! `tags` lives on the graph record, which is what `TagResource` writes and
//! `ListTagsForResource` reads, and so does the investigation record `StartInvestigation`
//! mints and `GetInvestigation` reads: the graph owns everything scoped to it, so
//! deleting the graph drops all of it at once.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    DatasourcePackageIngestDetail, Graph as GraphDetail, MemberDetail, MembershipDatasources,
    ResourceNotFoundException, TimestampForCollection, UnprocessedAccount,
};

/// Characters a behavior-graph id is built from.
///
/// Taken from the modelled `GraphArn` pattern, whose last segment is
/// `[abcdef\d]{32}`:
/// `smithy_query(query="shape",shape="GraphArn")`.
const GRAPH_ID_ALPHABET: &[u8] = b"0123456789abcdef";

/// Length of the id segment of a behavior-graph ARN, from the same pattern.
pub const GRAPH_ID_LEN: usize = 32;

/// The resource portion a behavior-graph ARN carries before its id, from the same
/// modelled pattern (`...:detective:<region>:<account>:graph:<id>`).
pub const GRAPH_ARN_RESOURCE_PREFIX: &str = "graph:";

/// The ARN service namespace for this service (`arnNamespace` / sigv4 name
/// `detective`).
pub const ARN_SERVICE: &str = "detective";

/// The region used when a request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Length of an Amazon Web Services account identifier, from the modelled
/// `AccountId` (length 12..12, pattern `^[0-9]+$`).
pub const ACCOUNT_ID_LEN: usize = 12;

/// The modelled `@pattern` of `AccountId`
/// (`smithy_query(query="shape",shape="AccountIdList")`): digits only, at least one.
pub const ACCOUNT_ID_PATTERN: &str = "^[0-9]+$";

/// The modelled `@pattern` of `GraphArn`
/// (`smithy_query(query="shape",shape="GraphArn")`), written once so every member schema
/// that captures it for the boundary walk refuses against the one expression.
pub const GRAPH_ARN_PATTERN: &str =
    r"^arn:aws[-\w]{0,10}?:detective:[-\w]{2,20}?:\d{12}?:graph:[abcdef\d]{32}?$";

/// The fewest account ids one member batch may name, from the modelled `AccountIdList`
/// (length 1..50), which `GetMembers` and `DeleteMembers` both bind their `AccountIds` to.
pub const MIN_ACCOUNT_IDS: usize = 1;

/// The most account ids one member batch may name, from the same bound.
pub const MAX_ACCOUNT_IDS: usize = 50;

/// The most graphs this emulator will hold.
///
/// Emulator resource guard, NOT an AWS quota. The account-level rule `CreateGraph`
/// documents ("An account can only be the administrator account for one behavior graph
/// within a Region. If the same account calls CreateGraph with the same administrator
/// account, it always returns the same behavior graph ARN") is enforced per
/// account+Region by [`ComAmazonawsDetectiveAmazondetectiveState::graph_of_administrator_in_region`],
/// so the store grows by at most one graph per Region; this cap is what keeps it finite
/// across Regions, and a create that would exceed it is refused with the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_GRAPHS: usize = 500;

/// The most member accounts one behavior graph may hold.
///
/// From `ServiceQuotaExceededException`'s own documentation: "This request cannot
/// be completed if it would cause the number of member accounts in the behavior
/// graph to exceed the maximum allowed. A behavior graph cannot have more than
/// 1,200 member accounts."
pub const MAX_MEMBERS_PER_GRAPH: usize = 1_200;

/// The largest page either list operation will return.
///
/// From the modelled `MemberResultsLimit` range 1..200, which both `ListGraphs` and
/// `ListMembers` bind their `MaxResults` to; it is also the page size used when a
/// request names none.
pub const MAX_PAGE_SIZE: usize = 200;

/// `MemberStatus$VERIFICATION_IN_PROGRESS`: Detective is checking that the account
/// id and email address given for the member match. The value is one of
/// `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_VERIFICATION_IN_PROGRESS: &str = "VERIFICATION_IN_PROGRESS";

/// `MemberStatus$INVITED`: verification passed and the invitation has been sent,
/// but the member has not responded yet. One of `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_INVITED: &str = "INVITED";

/// `MemberStatus$VERIFICATION_FAILED`: "For invited accounts only. Indicates that the
/// account and email address provided for the member account do not match, and
/// Detective did not send an invitation to the account." The other outcome of the
/// verification `CreateMembers` starts, and a terminal one. One of
/// `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_VERIFICATION_FAILED: &str = "VERIFICATION_FAILED";

/// `InvitationType$INVITATION`: the membership came from `CreateMembers` inviting
/// the account, not from an organization. One of `INVITATION_TYPE_VALUES`.
pub const INVITATION_TYPE_INVITATION: &str = "INVITATION";

/// How long after `CreateMembers` the verification it starts is due.
///
/// `MemberStatus$VERIFICATION_IN_PROGRESS` is a transient status Detective moves
/// out of by itself — "Detective is verifying that the account identifier and email
/// address provided for the member account match. If they do match, then Detective
/// sends the invitation" — so the move to its terminal status ([`MEMBER_STATUS_INVITED`]
/// when the pair matches, [`MEMBER_STATUS_VERIFICATION_FAILED`] when it does not) is
/// owed by the clock, not by any later call. AWS publishes no duration for it; this is
/// the span this emulator chooses, short enough not to slow a caller down and long
/// enough that a caller reading straight back still observes the transient status.
pub const MEMBER_VERIFICATION_MILLIS: i64 = 500;

/// `MemberStatus$ACCEPTED_BUT_DISABLED`: the member accepted the invitation but data
/// ingest is off, which is the one status `StartMonitoringMember` acts on ("Sends a
/// request to enable data ingest for a member account that has a status of
/// ACCEPTED_BUT_DISABLED"). One of `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_ACCEPTED_BUT_DISABLED: &str = "ACCEPTED_BUT_DISABLED";

/// `MemberStatus$ENABLED`: the member accepted the invitation and Detective is
/// ingesting its data — what `StartMonitoringMember` leaves behind. One of
/// `MEMBER_STATUS_VALUES`.
pub const MEMBER_STATUS_ENABLED: &str = "ENABLED";

/// Whether `status` is one a member account reached by ACCEPTING its invitation (or by
/// being enabled by the administrator), which is what makes the stored record a
/// membership of the behavior graph's data plane.
///
/// From `MemberDetail$Status`: `ENABLED` is "the member account currently contributes
/// data to the behavior graph. For invited accounts, the member account accepted the
/// invitation", and `ACCEPTED_BUT_DISABLED` is "the account accepted the invitation, or
/// was enabled by the Detective administrator account, but is prevented from
/// contributing data". The other three are all pre-acceptance: an invitation sent and
/// unanswered, a verification still running, and one that failed — of which the same
/// member documents that "the member cannot be added to the behavior graph". An
/// operation that acts on the membership itself therefore has nothing to act on unless
/// this holds.
pub fn is_accepted_membership(status: &str) -> bool {
    status == MEMBER_STATUS_ENABLED || status == MEMBER_STATUS_ACCEPTED_BUT_DISABLED
}

/// The message every refusal of a behavior graph this service does not hold carries.
///
/// Recorded behaviour: Detective answers a graph-scoped request naming a graph it
/// cannot resolve with `ResourceNotFoundException` and this one fixed sentence, which
/// does not echo the caller-supplied ARN back. It is therefore a constant rather than
/// a `format!`, and every operation that resolves a graph shares it.
pub const GRAPH_NOT_FOUND_MESSAGE: &str = "Graph not found";

/// The message the INVESTIGATION operations answer a behavior graph this service does
/// not hold with.
///
/// Recorded behaviour: the operations bound under this service's `/investigations/`
/// URIs word an unresolvable graph with this one fixed sentence — it names the input to
/// inspect and interpolates nothing from the request — while the member-administration
/// surface words the same condition as [`GRAPH_NOT_FOUND_MESSAGE`]. Each surface's text
/// is therefore written exactly once, here, and shared by every operation of that
/// surface that resolves a graph.
pub const INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE: &str =
    "Graph Not Found. Please check the graphArn provided";

/// The message every refusal of a MEMBERSHIP this service does not hold carries.
///
/// Recorded behaviour: whenever an operation cannot resolve the membership it was asked
/// about — the invitation addressed to the calling account, or the member account an
/// administrator named — Detective states it with this one fixed sentence, which
/// discloses neither the account nor whether the graph itself exists. The same sentence
/// is the `UnprocessedAccount$Reason` a batch reports for an account the graph holds no
/// membership for, so the 404 message and the unprocessed reason are the one text and
/// can never drift.
pub const NO_SUCH_MEMBERSHIP_MESSAGE: &str = "No such membership";

/// The message an operation that acts on a membership's data ingest answers a stored
/// record the member account has not ACCEPTED with.
///
/// Recorded behaviour: the graph does hold a record for the account, but the account
/// never accepted the invitation it was sent, so there is no ingest to act on — the
/// modelled `ResourceNotFoundException` again, with this fixed sentence, which names the
/// one fact that distinguishes it from [`NO_SUCH_MEMBERSHIP_MESSAGE`] and interpolates
/// nothing from the request.
pub const INVITATION_NOT_ACCEPTED_MESSAGE: &str = "invitation has not been accepted";

/// The message `ListDatasourcePackages` refuses a present `MaxResults` with.
///
/// Recorded behaviour: this report is not paginated — a request carrying ANY
/// `MaxResults` value is refused with the modelled `ValidationException`, and the
/// sentence interpolates nothing from the request. See
/// [`crate::ops::list_datasource_packages`].
pub const MAX_RESULTS_MUST_BE_NULL_MESSAGE: &str = "Max results must be null";

/// The most tag keys one behavior graph may hold.
///
/// AWS's per-resource tag quota is 50 keys, which the modelled `TagMap` length 1..50
/// states for ONE request — `TagResource` merges its keys into the map already stored
/// on the graph, so the bound has to be on the STORE, not on a request. A request that
/// would take the graph past it is refused rather than growing it, the way a create
/// past [`MAX_GRAPHS`] or [`MAX_MEMBERS_PER_GRAPH`] is.
pub const MAX_TAGS_PER_GRAPH: usize = 50;

/// The reserved tag-key prefix no caller may set.
///
/// Recorded behaviour: a tag key beginning with this prefix — compared
/// case-insensitively — is refused with error code `BadRequestException` at HTTP 400
/// before any tag is applied.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// What that refusal says. Recorded as an example, and input-independent: it names the
/// rule and echoes no key back.
pub const RESERVED_TAG_KEY_MESSAGE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// Length of the id segment of an investigation, from the modelled `InvestigationId`
/// (length 21..21, pattern `^[0-9]+$`).
pub const INVESTIGATION_ID_LEN: usize = 21;

/// `Status$RUNNING`: the investigation is still being assembled. One of
/// `STATUS_VALUES`.
pub const INVESTIGATION_STATUS_RUNNING: &str = "RUNNING";

/// `Status$SUCCESSFUL`: the investigation finished. One of `STATUS_VALUES`. Nothing
/// reaches it while no graph ingests account activity for a run to assemble a report
/// from — see [`Investigation::settle`].
pub const INVESTIGATION_STATUS_SUCCESSFUL: &str = "SUCCESSFUL";

/// `Status$FAILED`: the investigation's run did not complete. One of `STATUS_VALUES`.
pub const INVESTIGATION_STATUS_FAILED: &str = "FAILED";

/// `State$ACTIVE`: the investigation has not been archived. One of `STATE_VALUES`.
pub const INVESTIGATION_STATE_ACTIVE: &str = "ACTIVE";

/// `Severity$INFORMATIONAL`: the lowest modelled severity, and the one an
/// investigation that discovered no indicator of compromise carries.
///
/// `Severity` "is based on the likelihood and impact of the indicators of compromise
/// discovered in the investigation", and this emulator ingests no account activity, so
/// every investigation it runs discovers an empty indicator set — the severity that
/// computation yields is the lowest of `SEVERITY_VALUES`, never a drawn or invented
/// one.
pub const INVESTIGATION_SEVERITY_INFORMATIONAL: &str = "INFORMATIONAL";

/// `EntityType$IAM_USER` / `EntityType$IAM_ROLE`: the two entity kinds Detective
/// investigates ("Detective supports IAM user ARNs and IAM role ARNs"). Both are in
/// `ENTITY_TYPE_VALUES`.
pub const ENTITY_TYPE_IAM_USER: &str = "IAM_USER";
/// See [`ENTITY_TYPE_IAM_USER`].
pub const ENTITY_TYPE_IAM_ROLE: &str = "IAM_ROLE";

/// How long after `StartInvestigation` the investigation it starts is due to finish.
///
/// `StartInvestigation` returns an investigation whose `Status` is the non-terminal
/// `RUNNING`, so the move to a terminal status is owed by the clock rather than by any
/// later call. AWS publishes no duration for it; this is the span this emulator
/// chooses, short enough not to slow a caller down and long enough that a caller
/// reading straight back still observes `RUNNING`.
pub const INVESTIGATION_RUN_MILLIS: i64 = 500;

/// The most investigations one behavior graph will hold.
///
/// Emulator resource guard, NOT an AWS quota: it is what keeps the store finite, and a
/// `StartInvestigation` that would exceed it is refused with the modelled
/// `ValidationException` rather than growing the store without bound.
pub const MAX_INVESTIGATIONS_PER_GRAPH: usize = 1_000;

/// The data source package every behavior graph must ingest.
///
/// Recorded behaviour: an `UpdateDatasourcePackages` request that omits it is refused,
/// naming it as the missing mandatory package, so it is the package `CreateGraph`
/// starts a graph with and the one an update may never switch off. One of
/// `DATASOURCE_PACKAGE_VALUES`.
pub const MANDATORY_DATASOURCE_PACKAGE: &str = "DETECTIVE_CORE";

/// `DatasourcePackageIngestState$STARTED`: Detective is ingesting this package. One of
/// `DATASOURCE_PACKAGE_INGEST_STATE_VALUES`.
pub const DATASOURCE_PACKAGE_STATE_STARTED: &str = "STARTED";

/// `DatasourcePackageIngestState$STOPPED`: ingest for this package was switched off by
/// an update that no longer named it. One of
/// `DATASOURCE_PACKAGE_INGEST_STATE_VALUES`.
pub const DATASOURCE_PACKAGE_STATE_STOPPED: &str = "STOPPED";

/// The data source package ingest records a behavior graph holds from the moment it is
/// created: one per modelled `DatasourcePackage`
/// (`smithy_query(query="shape",shape="DatasourcePackage")`), each
/// [`DATASOURCE_PACKAGE_STATE_STARTED`] as of `now_millis`.
///
/// Recorded behaviour: a graph nobody has run `UpdateDatasourcePackages` on reports an
/// ingest record for EVERY package the service models, each one started, so the optional
/// packages are ingesting from creation rather than absent from the report. The records
/// are only ever transitioned afterwards — `UpdateDatasourcePackages` moves a package's
/// state and appends to its state-change history, and never removes a key — so a client
/// can always tell a stopped package from one it has to ask about.
pub fn initial_datasource_packages(now_millis: i64) -> BTreeMap<String, DatasourcePackageIngest> {
    crate::ops::types::DATASOURCE_PACKAGE_VALUES
        .iter()
        .map(|package| {
            (
                (*package).to_string(),
                DatasourcePackageIngest::entering(DATASOURCE_PACKAGE_STATE_STARTED, now_millis),
            )
        })
        .collect()
}

/// One stored member account of a behavior graph: everything `MemberDetail`
/// reports for it, plus the instant the verification `CreateMembers` started is
/// due.
#[derive(Debug, Clone)]
pub struct Member {
    pub account_id: String,
    /// "The Amazon Web Services account root user email address for the member
    /// account", as the request that invited the account gave it. A membership nobody
    /// was invited into — the administrator's own — was never given one, so it has
    /// none to report.
    pub email_address: Option<String>,
    pub graph_arn: String,
    /// The account that administers the graph this membership belongs to.
    pub administrator_id: String,
    /// One of `MEMBER_STATUS_VALUES`.
    pub status: String,
    /// How this membership came about, as one of `INVITATION_TYPE_VALUES` — "the type
    /// of behavior graph membership", which is `INVITATION` for "an account that was
    /// invited to a behavior graph" and `ORGANIZATION` for an organization account.
    /// The administrator's own membership came about from neither, so it has no type to
    /// report; see [`Member::reported_invitation_type`] for WHEN a type that is stored
    /// is reported.
    pub invitation_type: Option<String>,
    /// "For invited accounts, the date and time that Detective sent the invitation to
    /// the account", and for a membership no invitation created the instant that
    /// membership came into being — for the administrator's own, the instant its graph
    /// was created.
    pub invited_time_millis: Option<i64>,
    /// "The date and time that the member account was last updated", which starts out
    /// as the instant the membership came into being.
    pub updated_time_millis: Option<i64>,
    /// When ingest for each data source package was last STARTED for this membership,
    /// keyed by package, in epoch milliseconds.
    ///
    /// Ingest is enabled per ACCOUNT as well as per graph: a package the graph ingests
    /// is only ingested for a membership once ingest for it has been started for that
    /// account, which is what `StartMonitoringMember` ("sends a request to enable data
    /// ingest for a member account") does. The mandatory package is started for every
    /// membership when the membership is created — it is the one package a graph may
    /// never switch off — and the administrator's own membership starts every package
    /// the graph was created ingesting, since the graph exists to ingest its
    /// administrator's data. This is the map `DatasourcePackageIngestHistory` reports,
    /// "details on when a data source package was added to a behavior graph", and half
    /// of what [`Graph::member_detail`] projects `DatasourcePackageIngestStates` from.
    pub package_started_millis: BTreeMap<String, i64>,
    /// One of `MEMBER_DISABLED_REASON_VALUES`, only while the membership is
    /// `ACCEPTED_BUT_DISABLED`.
    pub disabled_reason: Option<String>,
    /// The instant the move out of `VERIFICATION_IN_PROGRESS` — to `INVITED` or to
    /// `VERIFICATION_FAILED`, whichever [`member_pair_verifies`] decides — is due,
    /// while that move is still outstanding.
    pub verification_due_millis: Option<i64>,
}

impl Member {
    /// Materialise the verification the clock already made due.
    ///
    /// This is the ONLY write a reporting operation makes: the move was owed at
    /// `verification_due_millis` whether or not anybody called, so `UpdatedTime`
    /// carries that due instant rather than the time of the read, and the record a
    /// later caller sees is identical.
    ///
    /// The status it moves to is the OUTCOME of the verification, decided by
    /// [`member_pair_verifies`]: `INVITED` when the account identifier and the email
    /// address match, `VERIFICATION_FAILED` when they do not. Both are terminal, so a
    /// membership never sits in `VERIFICATION_IN_PROGRESS` once its due instant has
    /// passed. Each membership is settled on its own record, so a batch of accounts
    /// resolves independently.
    pub fn settle(&mut self, now_millis: i64) {
        if let Some(due) = self.verification_due_millis {
            if due <= now_millis {
                self.status = if member_pair_verifies(
                    &self.account_id,
                    self.email_address.as_deref().unwrap_or_default(),
                ) {
                    MEMBER_STATUS_INVITED.to_string()
                } else {
                    MEMBER_STATUS_VERIFICATION_FAILED.to_string()
                };
                self.updated_time_millis = Some(due);
                self.verification_due_millis = None;
            }
        }
    }

    /// Whether this is the membership a behavior graph holds for the account that
    /// ADMINISTERS it, rather than one of the member accounts.
    ///
    /// The administration surface tells the two apart: `DeleteMembers` documents that
    /// "an administrator account cannot use DeleteMembers to remove their own account
    /// from the behavior graph", and the operations an invited member account's
    /// membership is the subject of — rejecting the invitation that created it, enabling
    /// its data ingest — have no such membership to act on, because no invitation
    /// created this one. The reads report it like any other membership.
    pub fn is_administrators_own(&self) -> bool {
        self.account_id == self.administrator_id
    }

    /// The `InvitationType` this membership reports, which is none while Detective is
    /// still deciding the membership.
    ///
    /// Recorded behaviour: the type of a membership appears only once the verification
    /// `CreateMembers` starts has settled — the response that creates the membership,
    /// whose `Status` is still `VERIFICATION_IN_PROGRESS`, carries no type, and the
    /// reads that follow it do. A membership that came about from neither an invitation
    /// nor an organization reports none at any point (see [`Member::invitation_type`]).
    pub fn reported_invitation_type(&self) -> Option<String> {
        if self.status == MEMBER_STATUS_VERIFICATION_IN_PROGRESS {
            return None;
        }
        self.invitation_type.clone()
    }

    /// This membership as the wire shape reports it, carrying
    /// `datasource_package_ingest_states` as its `DatasourcePackageIngestStates` —
    /// "the state of a data source package for the behavior graph", which the graph
    /// this membership belongs to projects for it ([`Graph::member_detail`], the one
    /// caller of this).
    ///
    /// `MasterId` carries the same value as `AdministratorId`: the model deprecates
    /// it in favour of `AdministratorId` ("This property is deprecated. Use
    /// AdministratorId instead.") but still binds it, so a client reading the older
    /// member sees the administrator too. The volume members are left absent: nothing
    /// in this slice measures ingest volume, so reporting a number for them would be a
    /// fabrication.
    pub fn detail(
        &self,
        datasource_package_ingest_states: ::std::collections::HashMap<String, String>,
    ) -> MemberDetail {
        MemberDetail {
            account_id: Some(self.account_id.clone()),
            email_address: self.email_address.clone(),
            graph_arn: Some(self.graph_arn.clone()),
            master_id: Some(self.administrator_id.clone()),
            administrator_id: Some(self.administrator_id.clone()),
            status: Some(self.status.clone()),
            disabled_reason: self.disabled_reason.clone(),
            invited_time: self.invited_time_millis.map(millis_to_date_time),
            updated_time: self.updated_time_millis.map(millis_to_date_time),
            invitation_type: self.reported_invitation_type(),
            datasource_package_ingest_states: Some(datasource_package_ingest_states),
            ..Default::default()
        }
    }
}

/// One stored data source package of a behavior graph: whether Detective is ingesting
/// it, and when it last entered each ingest state it has been in.
#[derive(Debug, Clone)]
pub struct DatasourcePackageIngest {
    /// One of `DATASOURCE_PACKAGE_INGEST_STATE_VALUES`: the state ingest is in now.
    pub state: String,
    /// When this package last entered each ingest state, keyed by that state, in
    /// epoch milliseconds — the record the modelled `LastIngestStateChangeDates` map
    /// (`map<DatasourcePackageIngestState, TimestampForCollection>`) reports, so a
    /// package that has been started and later stopped keeps the instant of each.
    pub last_state_change_millis: BTreeMap<String, i64>,
}

impl DatasourcePackageIngest {
    /// A package that entered `state` at `now_millis`, for a graph that held no record
    /// of it before.
    pub fn entering(state: &str, now_millis: i64) -> Self {
        Self {
            state: state.to_string(),
            last_state_change_millis: [(state.to_string(), now_millis)].into_iter().collect(),
        }
    }

    /// Move this package into `state`, recording the instant it entered it.
    ///
    /// A package already in `state` keeps the instant it got there, so
    /// `LastIngestStateChange` reports when ingest actually changed rather than when it
    /// was last confirmed.
    pub fn enter(&mut self, state: &str, now_millis: i64) {
        if self.state == state {
            return;
        }
        self.state = state.to_string();
        self.last_state_change_millis
            .insert(state.to_string(), now_millis);
    }

    /// This package's history as the modelled `LastIngestStateChangeDates` reports it:
    /// one `TimestampForCollection` per ingest state the package has entered. The one
    /// renderer of that map, so `ListDatasourcePackages`' `DatasourcePackageIngestDetail`
    /// and `BatchGetGraphMemberDatasources`' `DatasourcePackageIngestHistory` cannot
    /// drift apart.
    pub fn state_change_dates(&self) -> ::std::collections::HashMap<String, TimestampForCollection>
    {
        self.last_state_change_millis
            .iter()
            .map(|(state, millis)| {
                (
                    state.clone(),
                    TimestampForCollection {
                        timestamp: Some(millis_to_date_time(*millis)),
                    },
                )
            })
            .collect()
    }

    /// This package as `ListDatasourcePackages`' `DatasourcePackageIngestDetail`
    /// reports it: its ingest state, and the instant it entered each state it has been
    /// in as a `date-time` timestamp.
    pub fn detail(&self) -> DatasourcePackageIngestDetail {
        DatasourcePackageIngestDetail {
            datasource_package_ingest_state: Some(self.state.clone()),
            last_ingest_state_change: Some(self.state_change_dates()),
        }
    }
}

/// One stored investigation of a behavior graph: everything `GetInvestigation`
/// reports for it, plus the instant its run is due to finish.
#[derive(Debug, Clone)]
pub struct Investigation {
    pub investigation_id: String,
    pub graph_arn: String,
    pub entity_arn: String,
    /// One of `ENTITY_TYPE_VALUES`, resolved from `entity_arn` by [`entity_type_of`].
    pub entity_type: String,
    pub created_time_millis: i64,
    pub scope_start_time_millis: i64,
    pub scope_end_time_millis: i64,
    /// One of `STATUS_VALUES`.
    pub status: String,
    /// One of `SEVERITY_VALUES`.
    pub severity: String,
    /// One of `STATE_VALUES`.
    pub state: String,
    /// The instant the `RUNNING` -> `SUCCESSFUL` move is due, while that move is still
    /// outstanding.
    pub completion_due_millis: Option<i64>,
}

impl Investigation {
    /// Materialise the completion the clock already made due.
    ///
    /// The same single write [`Member::settle`] makes, for the same reason: the move
    /// was owed at `completion_due_millis` whether or not anybody called, so the record
    /// a later caller sees is identical to this one.
    ///
    /// The status it moves to is the OUTCOME of the run, which `Status` reports ("the
    /// status based on the completion status of the investigation"). An investigation is
    /// a report assembled from the activity the behavior graph INGESTED for the entity
    /// over the scope window; this emulator ingests no account activity into any graph,
    /// so a run has nothing to assemble a report from and does not complete — the status
    /// its completion yields is [`INVESTIGATION_STATUS_FAILED`], never a success claimed
    /// for a report nothing produced.
    pub fn settle(&mut self, now_millis: i64) {
        if let Some(due) = self.completion_due_millis {
            if due <= now_millis {
                self.status = INVESTIGATION_STATUS_FAILED.to_string();
                self.completion_due_millis = None;
            }
        }
    }
}

/// One stored behavior graph: its ARN, when it was created, its tags and the
/// member accounts invited into it.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub arn: String,
    pub created_time_millis: i64,
    /// The account that created the graph, and so administers it.
    pub administrator_id: String,
    /// Keyed by tag key, so `ListTagsForResource` will walk a stable order.
    pub tags: BTreeMap<String, String>,
    /// Keyed by account id, so `ListMembers` walks a stable, total order and a page
    /// token can name the next account.
    pub members: BTreeMap<String, Member>,
    /// Keyed by investigation id, so `ListInvestigations` will walk a stable order.
    pub investigations: BTreeMap<String, Investigation>,
    /// Keyed by data source package, so `ListDatasourcePackages` will walk a stable
    /// order. [`initial_datasource_packages`] seeds it with every modelled package;
    /// `UpdateDatasourcePackages` transitions those records and adds no key.
    pub datasource_packages: BTreeMap<String, DatasourcePackageIngest>,
}

impl Graph {
    /// A behavior graph `administrator_id` has just created, holding what every graph
    /// holds from the moment it exists: an ingest record per modelled data source
    /// package ([`initial_datasource_packages`]) and the ENABLED self-membership of the
    /// account that administers it ([`Graph::administrator_membership`]).
    pub fn created_by(
        arn: String,
        administrator_id: String,
        created_time_millis: i64,
        tags: BTreeMap<String, String>,
    ) -> Self {
        let datasource_packages = initial_datasource_packages(created_time_millis);
        let members = [(
            administrator_id.clone(),
            Self::administrator_membership(
                &arn,
                &administrator_id,
                &datasource_packages,
                created_time_millis,
            ),
        )]
        .into_iter()
        .collect();
        Self {
            arn,
            created_time_millis,
            administrator_id,
            tags,
            members,
            investigations: BTreeMap::new(),
            datasource_packages,
        }
    }

    /// The membership a behavior graph holds for the account that administers it.
    ///
    /// Recorded behaviour: the administrator account is a member of its own behavior
    /// graph — `ListMembers` reports it beside the accounts that were invited — and that
    /// membership is `ENABLED` ("the member account currently contributes data to the
    /// behavior graph"), which is the status of the account whose data the graph is
    /// built from. Nobody invited it and no organization enrolled it, so it carries
    /// neither a root-user email address nor an invitation type, and its two instants
    /// are the one instant the membership came into being: the graph's own creation.
    /// `DeleteMembers` cannot remove it ("an administrator account cannot use
    /// DeleteMembers to remove their own account from the behavior graph"), so a graph
    /// can never be left without it.
    pub fn administrator_membership(
        graph_arn: &str,
        administrator_id: &str,
        datasource_packages: &BTreeMap<String, DatasourcePackageIngest>,
        created_time_millis: i64,
    ) -> Member {
        Member {
            account_id: administrator_id.to_string(),
            email_address: None,
            graph_arn: graph_arn.to_string(),
            administrator_id: administrator_id.to_string(),
            status: MEMBER_STATUS_ENABLED.to_string(),
            invitation_type: None,
            invited_time_millis: Some(created_time_millis),
            updated_time_millis: Some(created_time_millis),
            disabled_reason: None,
            verification_due_millis: None,
            // The graph is created to ingest its administrator's data, so ingest of
            // every package the graph was created ingesting is started for this
            // membership at that same instant.
            package_started_millis: datasource_packages
                .iter()
                .filter(|(_, ingest)| ingest.state == DATASOURCE_PACKAGE_STATE_STARTED)
                .map(|(package, _)| (package.clone(), created_time_millis))
                .collect(),
        }
    }

    /// This graph as `ListGraphs`' `Graph` shape reports it.
    pub fn detail(&self) -> GraphDetail {
        GraphDetail {
            arn: Some(self.arn.clone()),
            created_time: Some(millis_to_date_time(self.created_time_millis)),
        }
    }

    /// The `DatasourcePackageIngestStates` one membership of this graph reports: one
    /// entry per package the graph holds a record for, carrying the state ingest of that
    /// package is in FOR THAT MEMBERSHIP.
    ///
    /// Ingest of a package runs for an account when the graph is ingesting the package
    /// and ingest of it has been started for that account ([`Member::package_started_millis`]);
    /// anything else is `STOPPED`. No package is ever left out, so a client can tell a
    /// package whose ingest is stopped for a member from one the graph does not know.
    pub fn member_ingest_states(&self, member: &Member) -> ::std::collections::HashMap<String, String> {
        self.datasource_packages
            .iter()
            .map(|(package, ingest)| {
                let started = ingest.state == DATASOURCE_PACKAGE_STATE_STARTED
                    && member.package_started_millis.contains_key(package);
                let state = if started {
                    DATASOURCE_PACKAGE_STATE_STARTED
                } else {
                    DATASOURCE_PACKAGE_STATE_STOPPED
                };
                (package.clone(), state.to_string())
            })
            .collect()
    }

    /// One membership of this graph as `MemberDetail` reports it: the stored record
    /// plus its per-package ingest states. The ONE projection of a member, so
    /// `CreateMembers`, `GetMembers` and `ListMembers` cannot report a member
    /// differently.
    pub fn member_detail(&self, member: &Member) -> MemberDetail {
        member.detail(self.member_ingest_states(member))
    }

    /// One membership of this graph as `MembershipDatasources` reports it.
    ///
    /// Every account whose data a behavior graph ingests holds a membership in it: the
    /// accounts invited into the graph, and the ADMINISTRATOR account, which
    /// `CreateGraph` "creates a new behavior graph for ... and sets as the administrator
    /// account" and whose own membership the graph holds from the moment it exists. An
    /// account with no membership is one the graph holds no data source package
    /// information for at all, so there is nothing to project for it.
    ///
    /// What it reports is an ADDITION history: `DatasourcePackageIngestHistory` is
    /// "details on when a data source package was added to a behavior graph", so each
    /// package the membership ingests carries the instant ingest of it was started for
    /// that membership and no other state's instant.
    pub fn membership_datasources(&self, member: &Member) -> MembershipDatasources {
        MembershipDatasources {
            account_id: Some(member.account_id.clone()),
            graph_arn: Some(self.arn.clone()),
            datasource_package_ingest_history: Some(
                member
                    .package_started_millis
                    .iter()
                    .map(|(package, millis)| {
                        (
                            package.clone(),
                            [(
                                DATASOURCE_PACKAGE_STATE_STARTED.to_string(),
                                TimestampForCollection {
                                    timestamp: Some(millis_to_date_time(*millis)),
                                },
                            )]
                            .into_iter()
                            .collect(),
                        )
                    })
                    .collect(),
            ),
        }
    }

    /// Materialise every member verification the clock already made due. See
    /// [`Member::settle`] for why a reporting operation is allowed to do this.
    pub fn settle_members(&mut self, now_millis: i64) {
        for member in self.members.values_mut() {
            member.settle(now_millis);
        }
    }

    /// Materialise every investigation completion the clock already made due. See
    /// [`Member::settle`] for why a reporting operation is allowed to do this.
    pub fn settle_investigations(&mut self, now_millis: i64) {
        for investigation in self.investigations.values_mut() {
            investigation.settle(now_millis);
        }
    }
}

/// Per-service state for com-amazonaws-detective-amazondetective. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDetectiveAmazondetectiveState {
    /// Every behavior graph, keyed by its own ARN. A `BTreeMap` so `ListGraphs`
    /// walks a stable, total order and a page token can name the next ARN.
    pub graphs: BTreeMap<String, Graph>,
    /// Mixed into each generated graph id so two creates in the same nanosecond
    /// differ.
    id_counter: u64,
}

impl ComAmazonawsDetectiveAmazondetectiveState {
    /// The graph `arn` names, for a reporting operation.
    pub fn graph(&self, arn: &str) -> Option<&Graph> {
        self.graphs.get(arn)
    }

    /// The graph `arn` names, for an operation the spec says mutates it.
    pub fn graph_mut(&mut self, arn: &str) -> Option<&mut Graph> {
        self.graphs.get_mut(arn)
    }

    /// Store a newly created graph.
    pub fn insert_graph(&mut self, graph: Graph) {
        self.graphs.insert(graph.arn.clone(), graph);
    }

    /// The one behavior graph `administrator_id` administers in `region`, if it has
    /// one.
    ///
    /// `CreateGraph` documents the rule this answers: "An account can only be the
    /// administrator account for one behavior graph within a Region. If the same
    /// account calls CreateGraph with the same administrator account, it always returns
    /// the same behavior graph ARN. It does not create a new behavior graph." So this is
    /// the identity of "this account's graph here", and `CreateGraph` answers with it
    /// rather than minting a second one.
    ///
    /// The lookup walks the stored graphs rather than a second index, so a graph
    /// `DeleteGraph` removed stops being the account's graph with no further
    /// bookkeeping, and the next `CreateGraph` in that Region mints a fresh one.
    pub fn graph_of_administrator_in_region(
        &self,
        administrator_id: &str,
        region: &str,
    ) -> Option<&Graph> {
        self.graphs.values().find(|graph| {
            graph.administrator_id == administrator_id && arn_region(&graph.arn) == Some(region)
        })
    }

    /// Remove the graph `arn` names, with its members and its tags. `None` when
    /// nothing matched.
    pub fn remove_graph(&mut self, arn: &str) -> Option<Graph> {
        self.graphs.remove(arn)
    }

    /// A 32-character lowercase-hex graph id no stored graph in `region` is using.
    pub fn fresh_graph_arn(&mut self, region: &str) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let seed = now_nanos() ^ self.id_counter.rotate_left(17);
            let candidate = graph_arn(region, &graph_id_from_seed(seed));
            if !self.graphs.contains_key(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded hex counters are also `[abcdef\d]{32}`, and there are always
        // more of them than there are stored graphs.
        (0u64..=self.graphs.len() as u64)
            .map(|n| graph_arn(region, &format!("{n:032x}")))
            .find(|arn| !self.graphs.contains_key(arn))
            .unwrap_or_else(|| graph_arn(region, &format!("{:032x}", self.graphs.len())))
    }

    /// A 21-digit investigation id no investigation of the graph `graph_arn` names is
    /// using, or `None` when that graph already holds
    /// [`MAX_INVESTIGATIONS_PER_GRAPH`].
    pub fn fresh_investigation_id(&mut self, graph_arn: &str) -> Option<String> {
        if self
            .graphs
            .get(graph_arn)
            .is_some_and(|graph| graph.investigations.len() >= MAX_INVESTIGATIONS_PER_GRAPH)
        {
            return None;
        }
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let candidate = investigation_id_from_seed(now_nanos() ^ self.id_counter.rotate_left(29));
            let taken = self
                .graphs
                .get(graph_arn)
                .is_some_and(|graph| graph.investigations.contains_key(&candidate));
            if !taken {
                return Some(candidate);
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: a decimal
        // counter padded to the modelled length is also `[0-9]{21}`, and there are
        // always more of them than the graph holds investigations.
        let held = self
            .graphs
            .get(graph_arn)
            .map_or(0, |graph| graph.investigations.len() as u64);
        (0u64..=held)
            .map(|n| format!("{:0width$}", n + 1, width = INVESTIGATION_ID_LEN))
            .find(|candidate| {
                self.graphs
                    .get(graph_arn)
                    .is_none_or(|graph| !graph.investigations.contains_key(candidate))
            })
    }
}

/// The refusal every operation answers a behavior graph this service does not hold
/// with: the modelled 404, carrying [`GRAPH_NOT_FOUND_MESSAGE`] rather than anything
/// built from the request.
pub fn graph_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(GRAPH_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The refusal the DELETE of a behavior graph this service does not hold answers with:
/// the same modelled 404, with no message member at all.
///
/// Recorded behaviour: the operations that read, administer or scope work to a graph
/// state [`GRAPH_NOT_FOUND_MESSAGE`] when they cannot resolve one, but the recorded
/// delete of a graph that is already gone carries no explanatory text — the member is
/// optional in the model, and there is nothing this service publishes for that
/// condition, so none is invented. See [`crate::ops::delete_graph`].
pub fn graph_not_found_unstated() -> ResourceNotFoundException {
    ResourceNotFoundException { message: None }
}

/// The refusal the investigation operations answer a behavior graph this service does
/// not hold with: the same modelled 404, carrying that surface's own
/// [`INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE`].
pub fn investigation_graph_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVESTIGATION_GRAPH_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The refusal every operation answers a membership this service does not hold with:
/// the modelled 404, carrying [`NO_SUCH_MEMBERSHIP_MESSAGE`] rather than anything built
/// from the request.
pub fn no_such_membership() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NO_SUCH_MEMBERSHIP_MESSAGE.to_string()),
    }
}

/// The refusal an operation that acts on a membership's data ingest answers a stored
/// record the account has not accepted with: the same modelled 404, carrying
/// [`INVITATION_NOT_ACCEPTED_MESSAGE`].
pub fn invitation_not_accepted() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(INVITATION_NOT_ACCEPTED_MESSAGE.to_string()),
    }
}

/// Whether `key` begins with the reserved [`RESERVED_TAG_KEY_PREFIX`], compared
/// case-insensitively the way the recorded refusal describes ("Customers cannot set
/// tag keys starting with aws: (case insensitive)").
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// How `CreateMembers` states a member of one of its `Accounts` entries failing its
/// modelled `@pattern`: a fixed sentence naming the member in the API-level spelling
/// the model uses, with neither the offending value nor its position in the list.
///
/// Recorded behaviour: two different 12-character non-numeric `AccountId`s produce the
/// one same message, so the message is the rule rather than a report of the input. The
/// one place it is written, so the operation's own check and the refusal the framework
/// hands back cannot drift apart.
pub fn account_member_pattern_message(member: &str) -> String {
    format!("Account.{member} did not match the pattern.")
}

/// How this service states the ELEMENTS of a list member of an operation's input failing
/// that element shape's modelled `@pattern`: one clause naming the shape that declares
/// the list and the list member itself, then the constraint class — with neither the
/// offending value nor the position it was sent in.
///
/// The same rule [`account_member_pattern_message`] states one step out: a `@pattern`
/// failure is reported against the shape that DECLARES the constrained member, in the
/// API-level spelling the model uses, and interpolates nothing from the request. Where the
/// constrained value is a list ELEMENT rather than a member of a structure, the clause
/// names the declaring shape and the list member and says it is that member's own members
/// that failed. Recorded behaviour: a 12-character non-numeric `AccountId` in a member
/// batch is refused with exactly this sentence, which is why the message is the rule
/// rather than a report of the input.
///
/// Both variable parts are read off the model-derived schemas rather than written out:
/// `input` is the operation's input schema and `member` the list member's.
pub fn list_members_pattern_message(
    input: &::aws_smithy_schema::Schema,
    member: &::aws_smithy_schema::Schema,
) -> String {
    format!(
        "{}.{} members did not match the pattern.",
        input.shape_id().shape_name(),
        member.member_name().unwrap_or_default(),
    )
}

/// How `UpdateDatasourcePackages` states requested packages outside the
/// `DatasourcePackage` value set: by listing them, which is the only place the service
/// names which ones it did not recognise.
pub fn unknown_datasource_packages_message(packages: &[&str]) -> String {
    format!(
        "UpdateDatasourcePackages request includes unknown datasource package(s): [{}]",
        packages.join(", ")
    )
}

/// How `UpdateDatasourcePackages` states a request that leaves out a package the graph
/// must ingest — the companion of
/// [`unknown_datasource_packages_message`], recorded for a request omitting
/// [`MANDATORY_DATASOURCE_PACKAGE`].
pub fn missing_datasource_packages_message(packages: &[&str]) -> String {
    format!(
        "UpdateDatasourcePackages request missing mandatory datasource package(s): [{}]",
        packages.join(", ")
    )
}

/// The `EntityType` the entity ARN `arn` names, or `None` when it names neither of the
/// two kinds Detective investigates.
///
/// From `StartInvestigation$EntityArn`: "The unique Amazon Resource Name (ARN).
/// Detective supports IAM user ARNs and IAM role ARNs." An assumed-role session ARN
/// names a role, so it resolves to `IAM_ROLE` too.
pub fn entity_type_of(arn: &str) -> Option<&'static str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let (kind, rest) = resource.split_once('/')?;
    if rest.is_empty() {
        return None;
    }
    match kind {
        "user" => Some(ENTITY_TYPE_IAM_USER),
        "role" | "assumed-role" => Some(ENTITY_TYPE_IAM_ROLE),
        _ => None,
    }
}

/// The ARN of the behavior graph `id` in `region`.
pub fn graph_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{GRAPH_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The account that administers the graphs this emulator serves: the one canonical
/// emulator account every ARN and every account field carries.
pub fn administrator_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The account whose credentials every request this emulator serves is signed with.
///
/// This emulator serves exactly ONE account, so the value is the same one
/// [`administrator_account_id`] answers. The two names are kept apart because the
/// operations read them for different reasons: a graph's administrator is a property of
/// the stored graph, while this is who is calling — which is what an invitee-side
/// operation such as `RejectInvitation` resolves its own membership by.
pub fn caller_account_id() -> String {
    administrator_account_id()
}

/// The root-user email address registered for the account `account_id`, or `None` when
/// this emulator holds no registration for it.
///
/// An account's root-user email address lives in the account directory, not in
/// Detective: `MemberDetail$EmailAddress` is "The Amazon Web Services account root user
/// email address for the member account", and the only place to check a given one
/// against is that directory. This emulator serves exactly ONE account
/// ([`caller_account_id`]) and registers no email address for it or for any other, so
/// the answer here is `None` for every account \u2014 returning an address for one would be
/// inventing a registration that does not exist.
pub fn registered_root_email(account_id: &str) -> Option<String> {
    let _ = account_id;
    None
}

/// Whether the account identifier and email address one `CreateMembers` entry carried
/// match, which is the question `MemberStatus$VERIFICATION_IN_PROGRESS` describes
/// Detective answering: "Detective is verifying that the account identifier and email
/// address provided for the member account match. If they do match, then Detective
/// sends the invitation. If the email address and account identifier don't match, then
/// the member cannot be added to the behavior graph."
///
/// The pair matches when the account named by `account_id` has a registered root-user
/// email address equal to `email_address` \u2014 compared case-insensitively, as email
/// domains and the addresses AWS reports are. [`registered_root_email`] is the one
/// source of that address, so an account this emulator holds no registration for can
/// never match, and its membership settles at [`MEMBER_STATUS_VERIFICATION_FAILED`]:
/// "the account and email address provided for the member account do not match, and
/// Detective did not send an invitation to the account."
pub fn member_pair_verifies(account_id: &str, email_address: &str) -> bool {
    registered_root_email(account_id)
        .is_some_and(|registered| registered.eq_ignore_ascii_case(email_address))
}

/// The Region field of the ARN `arn`, or `None` when it carries no such field.
///
/// Every behavior-graph ARN this service mints names the Region it was created in
/// (`arn:aws:detective:<region>:<account>:graph:<id>`), which is what makes the stored
/// graphs themselves the index of "this account's graph in this Region" \u2014 see
/// [`ComAmazonawsDetectiveAmazondetectiveState::graph_of_administrator_in_region`].
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3)
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only `[-\w]{2,20}` in that field, so anything else is
/// refused here and the default is used instead: a generated ARN can never carry
/// text the model does not admit.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            (2..=20).contains(&region.len())
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `arn` is shaped like a behavior-graph ARN this service mints.
///
/// The modelled `GraphArn` pattern is already enforced on every input member that
/// targets it, so this is only used to check a value the model cannot police — the
/// ARN carried inside an opaque pagination token.
pub fn is_graph_arn(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(ARN_SERVICE), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let partition_ok = partition
        .strip_prefix("aws")
        .is_some_and(|rest| rest.len() <= 10 && rest.bytes().all(|b| b == b'-' || b == b'_' || b.is_ascii_alphanumeric()));
    let region_ok = (2..=20).contains(&region.len())
        && region
            .bytes()
            .all(|b| b == b'-' || b == b'_' || b.is_ascii_alphanumeric());
    let account_ok = account.len() == ACCOUNT_ID_LEN && account.bytes().all(|b| b.is_ascii_digit());
    let id_ok = resource
        .strip_prefix(GRAPH_ARN_RESOURCE_PREFIX)
        .is_some_and(|id| {
            id.len() == GRAPH_ID_LEN && id.bytes().all(|b| GRAPH_ID_ALPHABET.contains(&b))
        });
    partition_ok && region_ok && account_ok && id_ok
}

/// Whether `account_id` is shaped like the modelled `AccountId` (12 digits).
pub fn is_account_id(account_id: &str) -> bool {
    account_id.len() == ACCOUNT_ID_LEN && matches_account_id_pattern(account_id)
}

/// Whether `value` satisfies the modelled `AccountId` `@pattern`
/// ([`ACCOUNT_ID_PATTERN`], digits only and at least one), independent of its `@length`:
/// the two are separate constraints and this service states their refusals differently.
pub fn matches_account_id_pattern(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

/// The page size to serve, whatever the request asked for.
///
/// The size is the server's own decision — clamped into `1..=MAX_PAGE_SIZE` — so no
/// allocation a list handler makes is sized by caller input.
pub fn page_size(requested: Option<i32>) -> usize {
    requested.map_or(MAX_PAGE_SIZE, |value| {
        value.clamp(1, MAX_PAGE_SIZE as i32) as usize
    })
}

/// Encode an opaque pagination token naming the next item to return.
///
/// Hex rather than base64 so the token carries no character a protocol would have
/// to escape; it is opaque to a caller either way, and the longest key it wraps (a
/// graph ARN) renders well inside `PaginationToken`'s 1..1024 length bound.
pub fn encode_page_token(key: &str) -> String {
    ::hex::encode(key)
}

/// The key a pagination token names, or `None` when the token is not one this
/// service issued. The caller's own token text is never echoed back.
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Why an account a batch named is reported unprocessed when the behavior graph ingests
/// no data for it at all: it neither administers the graph nor holds a membership in it,
/// so the graph stores no data source package information of it to report.
pub const NOT_INGESTED_FOR_REASON: &str = "The account is not an account of the behavior graph.";

/// Why the account that administers a behavior graph is reported unprocessed when a
/// member-removal batch names it: `DeleteMembers` documents that "an administrator
/// account cannot use DeleteMembers to remove their own account from the behavior graph.
/// To disable a behavior graph, the administrator account uses the DeleteGraph API
/// method."
pub const ADMINISTRATOR_NOT_REMOVABLE_REASON: &str =
    "The account administers this behavior graph and cannot be removed from it; use \
     DeleteGraph to disable the behavior graph.";

/// An `UnprocessedAccount` naming `account_id` and why it was not processed.
pub fn unprocessed(account_id: &str, reason: String) -> UnprocessedAccount {
    UnprocessedAccount {
        account_id: Some(account_id.to_string()),
        reason: Some(reason),
    }
}

/// Now, in milliseconds since the Unix epoch — the resolution the ISO8601
/// `date-time` timestamps in this model carry ("For example,
/// `2021-08-18T16:35:56.284Z`").
pub fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// An epoch-millisecond instant as the modelled `date-time` timestamp.
pub fn millis_to_date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// One `[abcdef\d]{32}` graph id from `seed`.
fn graph_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..GRAPH_ID_LEN)
        .map(|_| {
            let draw = splitmix64(&mut state);
            GRAPH_ID_ALPHABET[(draw % GRAPH_ID_ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// One `[0-9]{21}` investigation id from `seed`, as the modelled `InvestigationId`
/// spells one. The leading digit is drawn from `1..=9` so the id reads as a number of
/// its modelled length rather than a zero-padded shorter one.
fn investigation_id_from_seed(seed: u64) -> String {
    let mut state = seed;
    (0..INVESTIGATION_ID_LEN)
        .map(|position| {
            let draw = splitmix64(&mut state);
            let digit = if position == 0 {
                1 + draw % 9
            } else {
                draw % 10
            };
            char::from(b'0' + digit as u8)
        })
        .collect()
}

/// SplitMix64: the id generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDetectiveAmazondetectiveState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDetectiveAmazondetectiveState>)
}
