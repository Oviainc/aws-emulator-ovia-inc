// SPDX-License-Identifier: CC0-1.0

//! The Spark Connect session surface of a cluster, one concern of this service's state.
//!
//! "Creates and starts a new Spark Connect session on the specified cluster. The cluster
//! must be in the RUNNING or WAITING state and have sessions enabled." (`StartSession`
//! doc, Smithy model.) Both halves of that sentence are rules this module states once, so
//! every session-scoped operation holds a cluster to the same ones:
//!
//! * sessions are enabled by the LAUNCH — "indicates whether Spark Connect sessions are
//!   enabled on the cluster. When set to `true`, you can start Spark Connect sessions using
//!   the `StartSession` operation" (`RunJobFlow$SessionEnabled` doc) — so a cluster launched
//!   without them carries none and never will ([`sessions_enabled`]), and an operation of
//!   that surface is refused rather than answered with an empty page
//!   ([`sessions_not_enabled`]);
//! * a cluster that is not up has nothing to start a session on
//!   ([`accepts_sessions_in_state`]).
//!
//! `StartSession` records a [`SessionRecord`] on the cluster ([`ClusterRecord::start_session`])
//! and the clock walks it through the states `Session$State` names
//! ([`SESSION_START_PROGRESSION`]); `GetSession` reports that record's wire shape and
//! `ListSessions` pages over them. The names here are re-exported by the state root, so
//! every caller keeps reaching them through `crate::state`.
//!
//! What this service does NOT hold is the session's data plane: no Spark Connect server is
//! run for a session, so `Session$ServerUrl` — "the Spark Connect server URL for the
//! session. Use this URL with the `Credentials` returned by `GetSessionEndpoint` to connect
//! directly to the session" — is reported by no session this emulator records, rather than
//! being given an address nothing answers on.

use super::*;

/// The request `StartSession` hands to [`ClusterRecord::start_session`]; the state root
/// imports the shapes it projects itself, and this is the one only this concern reaches.
use crate::ops::types::StartSessionInput;

/// `com.amazonaws.emr#SessionState`, for the states this emulator sets
/// (`smithy_query(query="shape", shape="SessionState")`).
pub mod session_state {
    /// "When a session is first created, it enters the SUBMITTED state."
    /// (`StartSessionOutput$State` doc, Smithy model.)
    pub const SUBMITTED: &str = "SUBMITTED";
    /// The cluster is starting the session.
    pub const STARTING: &str = "STARTING";
    /// The session has started: "the date and time that the session entered the
    /// `STARTED` state" is what `Session$StartedAt` reports.
    pub const STARTED: &str = "STARTED";
}

/// The states a session passes through, in order, from the request that created it to the
/// state it is started in.
///
/// The first is the one the model fixes ("when a session is first created, it enters the
/// SUBMITTED state", `StartSessionOutput$State` doc); each move after it is owed
/// [`SESSION_PHASE_SPAN`] after the one before, so a caller that reads the session right
/// after `StartSession` sees the state the call answered rather than the end of the walk.
pub const SESSION_START_PROGRESSION: &[&str] = &[
    session_state::SUBMITTED,
    session_state::STARTING,
    session_state::STARTED,
];

/// How long each phase of a session's start lasts.
///
/// CHOSEN, not documented, and the same span a cluster spends in each phase of coming up
/// ([`CLUSTER_PHASE_SPAN`]): long enough that a client polling `GetSession` straight after
/// `StartSession` observes the session before it has started. Stored as a due instant —
/// nothing sleeps.
pub const SESSION_PHASE_SPAN: Duration = CLUSTER_PHASE_SPAN;

/// How many sessions `ListSessions` reports in one page at most: the maximum the model
/// declares for the member a caller asks with (`MaxResultsNumber`, range 1..100), which the
/// listing holds a request to whatever it asked for.
pub const MAX_LIST_SESSIONS_RESULTS: usize = 100;

/// How many sessions one cluster holds at once.
///
/// CHOSEN, not documented: every session a cluster is asked for is retained on its record
/// and reported by every later `ListSessions`, so the stored list needs a ceiling of its
/// own — the same reasoning that bounds a cluster's instance fleets
/// ([`MAX_INSTANCE_FLEETS_PER_CLUSTER`]).
pub const MAX_SESSIONS_PER_CLUSTER: usize = 100;

/// The ARN of a session, under the cluster it belongs to:
/// `arn:aws:elasticmapreduce:<region>:<account>:cluster/<clusterId>/session/<sessionId>`.
///
/// AWS publishes no format for it, so the resource path is the QUALIFIED path Amazon EMR
/// states a session by in its own refusals (`<clusterId>/session/<sessionId>`, see
/// [`malformed_session_id_message`]), under this service's cluster resource type.
pub fn session_arn(region: &str, cluster_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_VENDOR,
        region,
        &format!("{CLUSTER_RESOURCE_TYPE}/{cluster_id}/session/{session_id}"),
    )
}

/// How Amazon EMR refuses a session operation on a cluster that was not launched with
/// sessions enabled.
///
/// Recorded behavior for the text: fixed apart from the cluster id, which is the
/// `ClusterId` the request sent, quoted. The sentence also states what to do about it —
/// sessions are a property of the LAUNCH, so the cluster cannot be changed into one that
/// carries them.
pub fn sessions_not_enabled_message(cluster_id: &str) -> String {
    format!(
        "Sessions are not enabled on cluster '{cluster_id}'. Set SessionEnabled to true \
         when creating the cluster to enable sessions."
    )
}

/// The refusal every operation of the session surface answers a cluster without sessions
/// with: that sentence under the client error those operations model.
///
/// No `ErrorCode`: Amazon EMR publishes no discriminator for this class. Stated once here
/// because `StartSession` and `ListSessions` answer the SAME refusal — the listing refuses
/// rather than reporting an empty `Sessions` list, because a cluster that carries no
/// session surface is not a cluster with no sessions on it.
pub fn sessions_not_enabled(cluster_id: &str) -> InvalidRequestException {
    InvalidRequestException {
        error_code: None,
        message: Some(sessions_not_enabled_message(cluster_id)),
    }
}

/// How `StartSession` refuses a cluster that is not up.
///
/// AWS publishes no wording for it, so the text is built the way this service words its
/// other state refusals: the cluster the request named, the state it is actually in, and
/// the states the operation accepts ("the cluster must be in the RUNNING or WAITING
/// state", `StartSession` doc).
pub fn cluster_not_running_for_session_message(cluster_id: &str, state: &str) -> String {
    format!(
        "Cluster '{cluster_id}' is in state {state}; a session can only be started on a \
         cluster in the {} or {} state.",
        cluster_state::RUNNING,
        cluster_state::WAITING
    )
}

/// Whether a cluster in `state` can be asked for a session: "the cluster must be in the
/// RUNNING or WAITING state" (`StartSession` doc, Smithy model).
pub fn accepts_sessions_in_state(state: &str) -> bool {
    state == cluster_state::RUNNING || state == cluster_state::WAITING
}

/// How `ListSessions` refuses a `NextToken` it did not mint: the member, the value the
/// request sent and that it is not valid — the shape every continuation-token refusal of
/// this service takes ([`malformed_marker_message`]), under the member this listing pages
/// on.
pub fn malformed_next_token_message(next_token: &str) -> String {
    format!("NextToken {}", not_valid_message(next_token))
}

/// The refusal `ListSessions` answers a `NextToken` it did not mint with: that sentence
/// under the continuation-token discriminator ([`error_code::MALFORMED_MARKER`]), the code
/// every paged listing of this service publishes for a page token that cannot work.
pub fn malformed_next_token(next_token: &str) -> InvalidRequestException {
    InvalidRequestException {
        error_code: Some(error_code::MALFORMED_MARKER.to_string()),
        message: Some(malformed_next_token_message(next_token)),
    }
}

/// The state a session's clock owes it, and when that comes due.
#[derive(Debug, Clone)]
pub struct PendingSessionState {
    /// The instant the state below was owed, stored rather than slept for.
    pub due: SystemTime,
    pub state: String,
}

/// One persisted Spark Connect session: the wire `Session` every read of it reports, the
/// state the clock still owes it, and the token the request that created it carried.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session: Session,
    pub pending_state: Option<PendingSessionState>,
    /// "A unique, case-sensitive identifier that you provide to ensure the idempotency of
    /// the request. If you retry a request that completed successfully using the same
    /// client request token, the service returns the original response without performing
    /// the operation again." (`StartSessionInput$ClientRequestToken` doc, Smithy model.)
    pub client_request_token: Option<String>,
}

impl SessionRecord {
    /// Materialise every state the clock already made due, and nothing else.
    ///
    /// A session moves one state at a time, so a read that comes several spans late applies
    /// each of them in turn and reports the one the clock has actually reached — the answer
    /// does not depend on how often anybody looked. `UpdatedAt` is "the date and time that
    /// the session was last updated" (`Session$UpdatedAt` doc), which is the instant the
    /// move was owed at rather than the instant somebody read it.
    pub fn advance_to(&mut self, now: SystemTime) {
        while let Some(pending) = self.pending_state.clone() {
            if now < pending.due {
                return;
            }
            self.pending_state = None;
            self.session.state = pending.state.clone();
            self.session.updated_at = Some(timestamp(pending.due));
            if pending.state == session_state::STARTED {
                self.session.started_at = Some(timestamp(pending.due));
            }
            if let Some(next) = next_session_state(&pending.state) {
                self.pending_state = Some(PendingSessionState {
                    due: pending.due + SESSION_PHASE_SPAN,
                    state: next.to_string(),
                });
            }
        }
    }
}

/// The state that follows `state` in the start progression, or `None` for one that ends it
/// and for one the progression does not name.
///
/// The ONE reading of what a session's clock owes it next, so nothing can drift apart from
/// [`SESSION_START_PROGRESSION`].
pub fn next_session_state(state: &str) -> Option<&'static str> {
    let position = SESSION_START_PROGRESSION
        .iter()
        .position(|step| *step == state)?;
    SESSION_START_PROGRESSION.get(position + 1).copied()
}

impl ClusterRecord {
    /// Whether this cluster carries the Spark Connect session surface at all.
    ///
    /// "Indicates whether Spark Connect sessions are enabled on the cluster"
    /// (`RunJobFlow$SessionEnabled` doc, Smithy model): a launch that left the member out
    /// asked for no sessions, so only a cluster launched with it true carries them.
    pub fn sessions_enabled(&self) -> bool {
        self.session_enabled == Some(true)
    }

    /// The session `session_id` of this cluster, if it has one.
    pub fn session(&self, session_id: &str) -> Option<&SessionRecord> {
        self.sessions
            .iter()
            .find(|record| record.session.id == session_id)
    }

    /// The session `session_id` of this cluster, for an operation that changes it.
    pub fn session_mut(&mut self, session_id: &str) -> Option<&mut SessionRecord> {
        self.sessions
            .iter_mut()
            .find(|record| record.session.id == session_id)
    }

    /// The session this cluster already started for the client request token `token`, if
    /// the token names one.
    ///
    /// This is what makes `StartSession` idempotent on the token the model gives it: a
    /// repeat of a request that succeeded is answered with the session the first one
    /// created, and starts no second session. A request carrying no token names no earlier
    /// session.
    pub fn session_for_token(&self, token: Option<&str>) -> Option<&SessionRecord> {
        let token = token?;
        self.sessions.iter().find(|record| {
            record
                .client_request_token
                .as_deref()
                .is_some_and(|held| held == token)
        })
    }

    /// Record the session `StartSession` asked this cluster for, and answer it.
    ///
    /// The session starts in the state the model fixes for a new one (`SUBMITTED`) and the
    /// clock owes it the rest of the progression ([`SESSION_START_PROGRESSION`]). What the
    /// request supplied is held as supplied — its name, its execution role, its engine
    /// configuration overrides, its monitoring configuration, its idle timeout and its tags
    /// — and what belongs to the CLUSTER is read off this record, so a session reports the
    /// release its cluster runs rather than one its request named.
    pub fn start_session(
        &mut self,
        session_id: String,
        arn: String,
        input: StartSessionInput,
        now: SystemTime,
    ) -> &SessionRecord {
        let record = SessionRecord {
            session: Session {
                id: session_id,
                cluster_id: self.id.clone(),
                name: input.name,
                arn,
                state: session_state::SUBMITTED.to_string(),
                state_change_reason: None,
                release_label: self.release_label.clone(),
                execution_role_arn: input.execution_role_arn,
                account_id: Some(::engine::emulator_account_id().to_string()),
                created_at: Some(timestamp(now)),
                updated_at: Some(timestamp(now)),
                // "The date and time that the session entered the STARTED state"
                // (`Session$StartedAt` doc): a session that has just been submitted has
                // entered none of the states those three members report.
                started_at: None,
                ended_at: None,
                idle_since: None,
                engine_configurations: input.engine_configurations,
                monitoring_configuration: input.monitoring_configuration,
                session_idle_timeout_in_minutes: input.session_idle_timeout_in_minutes,
                // No Spark Connect server is run for this session (see the module docs), so
                // the URL a connect client would dial is reported by nothing here.
                server_url: None,
                // Reported only when the session actually carries tags, the way a studio's
                // are ([`TagSetMut::Optional`]).
                tags: input.tags.filter(|tags| !tags.is_empty()),
            },
            pending_state: next_session_state(session_state::SUBMITTED).map(|next| {
                PendingSessionState {
                    due: now + SESSION_PHASE_SPAN,
                    state: next.to_string(),
                }
            }),
            client_request_token: input.client_request_token,
        };
        self.sessions.push(record);
        self.sessions
            .last()
            .expect("the session was just pushed onto this cluster")
    }
}

impl ComAmazonawsEmrElasticmapreduceState {
    /// A fresh session id: [`SESSION_ID_PREFIX`] followed by uppercase alphanumerics,
    /// minted from the same monotonic, zero-padded sequence every other id in this service
    /// is ([`base36_id`]).
    pub fn mint_session_id(&mut self) -> String {
        self.next_session_seq += 1;
        format!("{SESSION_ID_PREFIX}{}", base36_id(self.next_session_seq))
    }
}
