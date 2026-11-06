// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-novaact-amazonnovaagentsdataplane service.
//!
//! The resource chain this service exposes is strictly nested — workflow definition ->
//! workflow run -> session -> act — so the store nests the same way and every child is
//! reached by resolving its parents first. That is what makes a child created under a
//! missing parent a `ResourceNotFoundException` instead of an orphan record.
//!
//! Besides the store itself this module holds the handful of helpers every handler of
//! this service needs (request region, ARN construction, the one place a
//! `ResourceNotFoundException` is filled in, idempotent-create bookkeeping, and
//! completing a deletion the clock has made due), so the per-operation files share
//! one implementation of each instead of seven copies.
//!
//! ONE PARTITION. The model scopes a workflow definition's name to an account AND a
//! region, and this emulator serves a single account ([`engine::EMULATOR_ACCOUNT_ID`]).
//! The store is keyed by name alone, so it behaves as that account's store in ONE region:
//! a definition created under one signing region resolves for a request signed for
//! another, and reports the ARN of the region it was created in (the ARN is stored once,
//! at create time, never rebuilt from the reading request). A per-region store would key
//! these maps by `(region, name)`; nothing in this slice depends on that choice, and
//! every operation here resolves names through [`ComAmazonawsNovaactAmazonnovaagentsdataplaneState::definition`],
//! so it is the one place a later slice would widen the key.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ActError, ClientInfo, ResourceNotFoundException, ToolSpec, TraceLocation, WorkflowExportConfig,
};

/// The AI models this service publishes. Service-managed, so the catalog is a published
/// fact rather than part of the store below — see the module's own docs.
pub mod model_catalog;

/// Ordering, paging and resumption tokens, shared by every `List*` operation of this
/// service so one implementation of that rule serves all of them.
pub mod paging;

/// The region an ARN carries when the request's SigV4 credential scope does not name a
/// usable one (an unsigned in-process call, say). The `@pattern` on every ARN shape in
/// this model requires a non-empty `[a-z0-9-]+` region segment, so there is no "no
/// region" ARN to fall back to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN service namespace (`arnNamespace` in the model).
pub const ARN_NAMESPACE: &str = "nova-act";

/// How long after `DeleteWorkflowDefinition` the definition stops resolving.
///
/// `DeleteWorkflowDefinition` answers 202 with status `DELETING`, a non-terminal status,
/// so the removal is a transition the clock owes rather than something the delete call
/// finishes inline: a caller that reads the definition straight after the delete still
/// observes `DELETING`, and one that reads it after the span observes the
/// `ResourceNotFoundException` the removal produces. The span is chosen (AWS documents
/// no duration for it) and is deliberately non-zero so the transient state is reachable.
pub const DELETION_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// This service's quota service code, as `ServiceQuotaExceededException` reports it.
pub const SERVICE_CODE: &str = "nova-act";

/// The most workflow definitions this store holds. Every create is one more entry that
/// is only released by a delete, so the count is capped: at `WorkflowDescription`'s
/// modelled maximum of 4,000 characters a full definition store is a few megabytes.
pub const MAX_WORKFLOW_DEFINITIONS: usize = 500;

/// The most workflow runs one definition holds.
pub const MAX_RUNS_PER_DEFINITION: usize = 100;

/// The most workflow runs the whole store holds. The per-parent cap alone would leave
/// the total free to grow with the number of definitions, so the total is capped too.
pub const MAX_WORKFLOW_RUNS: usize = 1_000;

/// The most sessions one run holds.
pub const MAX_SESSIONS_PER_RUN: usize = 100;

/// The most sessions the whole store holds.
pub const MAX_SESSIONS: usize = 2_000;

/// The most acts one session holds.
pub const MAX_ACTS_PER_SESSION: usize = 100;

/// The most acts the whole store holds. Acts are the heaviest record (`Task` is modelled
/// up to 10,000 characters and a `toolSpecs` list up to 100 entries), which is why this
/// total is the tightest of the three.
pub const MAX_ACTS: usize = 1_000;

/// The most idempotent-create replays remembered at once. Each distinct caller-supplied
/// `clientToken` adds one entry, so the map is capped and the oldest entry is evicted
/// first ([`ComAmazonawsNovaactAmazonnovaagentsdataplaneState::record_idempotent_create`]) —
/// a caller sending a fresh token every request cannot grow it without limit, and the
/// entry for a resource that has since been deleted cannot outlive the cap.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

/// A bound a create would have exceeded, and the quota vocabulary for reporting it.
///
/// Each variant is one cap above: a per-parent one, or the store-wide one for that kind
/// of record. A handler turns it into the error ITS model declares for a refused create
/// — `ServiceQuotaExceededException` where the operation lists it, and otherwise a
/// conflict with the parent's current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    WorkflowDefinitions,
    RunsPerDefinition,
    WorkflowRuns,
    SessionsPerRun,
    Sessions,
    ActsPerSession,
    Acts,
}

impl Quota {
    /// The value of the cap this variant names.
    pub fn limit(self) -> usize {
        match self {
            Self::WorkflowDefinitions => MAX_WORKFLOW_DEFINITIONS,
            Self::RunsPerDefinition => MAX_RUNS_PER_DEFINITION,
            Self::WorkflowRuns => MAX_WORKFLOW_RUNS,
            Self::SessionsPerRun => MAX_SESSIONS_PER_RUN,
            Self::Sessions => MAX_SESSIONS,
            Self::ActsPerSession => MAX_ACTS_PER_SESSION,
            Self::Acts => MAX_ACTS,
        }
    }

    /// The quota code a refusal names, so a caller can tell the per-parent cap from the
    /// store-wide one.
    pub fn code(self) -> &'static str {
        match self {
            Self::WorkflowDefinitions => "workflow-definitions-per-account",
            Self::RunsPerDefinition => "workflow-runs-per-workflow-definition",
            Self::WorkflowRuns => "workflow-runs-per-account",
            Self::SessionsPerRun => "sessions-per-workflow-run",
            Self::Sessions => "sessions-per-account",
            Self::ActsPerSession => "acts-per-session",
            Self::Acts => "acts-per-account",
        }
    }

    /// The kind of resource that could not be created.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::WorkflowDefinitions => "workflow-definition",
            Self::RunsPerDefinition | Self::WorkflowRuns => "workflow-run",
            Self::SessionsPerRun | Self::Sessions => "session",
            Self::ActsPerSession | Self::Acts => "act",
        }
    }

    /// Why the create was refused, naming the quota and its value.
    pub fn message(self, resource_id: &str) -> String {
        format!(
            "cannot create another {}: the {} quota of {} is exhausted for {resource_id}",
            self.resource_type(),
            self.code(),
            self.limit()
        )
    }
}

/// A resource the request named that this store does not hold.
///
/// Carries what `ResourceNotFoundException` needs, so each handler can turn one into its
/// own modeled error through [`Self::not_found`] without re-deriving any of it.
///
/// `resourceId` is "the identifier of the resource that wasn't found", and the identifier
/// a resource of this service HAS is its ARN — the one its own `arn` / `workflowRunArn`
/// member reports — not the path segment that names it under its parent. A missing
/// resource has no stored record to read a region from (that is what makes it missing),
/// so its ARN is qualified with the requesting credential scope's region, exactly as the
/// ARN of a resource this request created would be; hence the `region` argument rather
/// than a stored string.
#[derive(Debug, Clone)]
pub struct MissingResource {
    /// The kind of resource, as `resourceType` reports it.
    pub resource_type: String,
    /// The resource part of this resource's ARN (everything after the account segment),
    /// nested under its parents the way the model nests them. Its last segment is the
    /// path identifier the request named the resource by.
    resource_path: String,
}

impl MissingResource {
    fn new(resource_type: &str, resource_path: String) -> Self {
        Self {
            resource_type: resource_type.to_string(),
            resource_path,
        }
    }

    /// This resource's ARN in `region`, which is what `resourceId` carries.
    pub fn resource_id(&self, region: &str) -> String {
        resource_arn(region, &self.resource_path)
    }

    /// Why the lookup failed: the KIND of resource that was not found, as a sentence.
    ///
    /// It interpolates nothing from the request: the identifier the caller asked for is
    /// what `resourceId` carries, parseable, and a client reading `message` gets the one
    /// thing the prose adds — which kind of resource in the chain was missing. Derived
    /// from `resourceType` for every kind rather than written out per operation.
    pub fn message(&self) -> String {
        let kind = self.resource_type.replace('-', " ");
        let mut sentence = String::with_capacity(kind.len() + " not found".len());
        let mut characters = kind.chars();
        if let Some(first) = characters.next() {
            sentence.extend(first.to_uppercase());
            sentence.push_str(characters.as_str());
        }
        sentence.push_str(" not found");
        sentence
    }

    /// The modeled `ResourceNotFoundException` for a request signed in `region`: the ONE
    /// place this service fills that error's members in.
    pub fn not_found(&self, region: &str) -> ResourceNotFoundException {
        ResourceNotFoundException {
            message: self.message(),
            resource_id: self.resource_id(region),
            resource_type: self.resource_type.clone(),
        }
    }
}

/// `WorkflowDefinitionStatus`: the two values the model's enum declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowDefinitionStatus {
    Active,
    Deleting,
}

impl WorkflowDefinitionStatus {
    /// The value as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// `WorkflowRunStatus`: the five values the model's enum declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Deleting,
}

impl WorkflowRunStatus {
    /// The value as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::TimedOut => "TIMED_OUT",
            Self::Deleting => "DELETING",
        }
    }

    /// Whether the run has finished: only a finished run has an `endedAt`.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::TimedOut)
    }
}

/// `ActStatus`: the six values the model's enum declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActStatus {
    Running,
    PendingClientAction,
    PendingHumanAction,
    Succeeded,
    Failed,
    TimedOut,
}

impl ActStatus {
    /// Every status the model declares, in the ORDER it declares them
    /// (`smithy_query(query="shape",shape="ActStatus")`). It is the one list
    /// [`Self::from_wire`] reads and the order a refusal lists the set in; the integration
    /// suites check its length against the value list generated from the model, so a model
    /// that grows a value does not leave it behind.
    pub const ALL: [Self; 6] = [
        Self::Running,
        Self::PendingClientAction,
        Self::PendingHumanAction,
        Self::Succeeded,
        Self::Failed,
        Self::TimedOut,
    ];

    /// The value as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Running => "RUNNING",
            Self::PendingClientAction => "PENDING_CLIENT_ACTION",
            Self::PendingHumanAction => "PENDING_HUMAN_ACTION",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::TimedOut => "TIMED_OUT",
        }
    }

    /// The status a request named, or `None` for a value the model's enum does not
    /// declare. Read off [`Self::ALL`] through [`Self::as_wire`], so no second spelling of
    /// any value exists to drift from the wire form.
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_wire() == value)
    }

    /// Whether the act has FINISHED: it ran to the end, it failed, or it ran out of time.
    /// Only an act in one of those statuses has an `endedAt`, and it is the last status
    /// the act takes.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::TimedOut)
    }
}

/// A stored workflow definition, the root of the resource chain.
#[derive(Debug)]
pub struct WorkflowDefinition {
    pub name: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub description: Option<String>,
    pub export_config: Option<WorkflowExportConfig>,
    pub status: WorkflowDefinitionStatus,
    /// When `DeleteWorkflowDefinition` set `status` to `DELETING`, the instant the
    /// removal becomes due. `None` while the definition is `ACTIVE`.
    pub deletion_due_at: Option<SystemTime>,
    /// The request that created it, so an identical repeat of an `@idempotent` create is
    /// a replay while a different body for the same name is a conflict.
    pub create_fingerprint: String,
    pub runs: BTreeMap<String, WorkflowRun>,
    /// Creation order, so a later `List*` can honour `sortOrder` without a redesign.
    pub insertion_seq: u64,
}

impl WorkflowDefinition {
    /// The run `run_id` names, or the description of what was missing.
    pub fn run(&self, run_id: &str) -> Result<&WorkflowRun, MissingResource> {
        self.runs.get(run_id).ok_or_else(|| {
            MissingResource::new("workflow-run", workflow_run_path(&self.name, run_id))
        })
    }

    /// The run `run_id` names, mutably.
    pub fn run_mut(&mut self, run_id: &str) -> Result<&mut WorkflowRun, MissingResource> {
        let definition_name = self.name.clone();
        self.runs.get_mut(run_id).ok_or_else(|| {
            MissingResource::new(
                "workflow-run",
                workflow_run_path(&definition_name, run_id),
            )
        })
    }
}

/// A stored workflow run: one execution instance of a definition.
#[derive(Debug)]
pub struct WorkflowRun {
    pub run_id: String,
    /// The definition this run belongs to, so the run can name its own place in the
    /// resource chain (its children's ARNs nest under it).
    pub definition_name: String,
    pub arn: String,
    pub status: WorkflowRunStatus,
    pub started_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub model_id: String,
    pub log_group_name: Option<String>,
    pub client_info: ClientInfo,
    pub create_fingerprint: String,
    pub sessions: BTreeMap<String, Session>,
    pub insertion_seq: u64,
}

impl WorkflowRun {
    /// The session `session_id` names.
    pub fn session(&self, session_id: &str) -> Result<&Session, MissingResource> {
        self.sessions.get(session_id).ok_or_else(|| {
            MissingResource::new(
                "session",
                session_path(&self.definition_name, &self.run_id, session_id),
            )
        })
    }

    /// The session `session_id` names, mutably.
    pub fn session_mut(&mut self, session_id: &str) -> Result<&mut Session, MissingResource> {
        let path = session_path(&self.definition_name, &self.run_id, session_id);
        self.sessions
            .get_mut(session_id)
            .ok_or_else(|| MissingResource::new("session", path))
    }
}

/// A stored session: conversation state within a run, holding its acts.
#[derive(Debug)]
pub struct Session {
    pub session_id: String,
    pub created_at: SystemTime,
    pub acts: BTreeMap<String, Act>,
    pub insertion_seq: u64,
}

/// A stored act: one AI task within a session.
#[derive(Debug)]
pub struct Act {
    pub act_id: String,
    pub task: String,
    pub tool_specs: Vec<ToolSpec>,
    pub status: ActStatus,
    pub error: Option<ActError>,
    pub created_at: SystemTime,
    /// The instant the act finished, which only an act in a terminal status has: it is
    /// what `ActSummary`'s `endedAt` reports, and `None` while the act is still running or
    /// waiting on an action. The status transition that ends an act is what sets it
    /// ([`Act::move_to`]).
    pub ended_at: Option<SystemTime>,
    /// Where this act's trace is stored, as `ActSummary`'s `traceLocation` reports it.
    /// `None` until something records a trace for the act.
    pub trace_location: Option<TraceLocation>,
    pub insertion_seq: u64,
}

impl Act {
    /// Move the act to `status` as of `now`, keeping the instant the status implies in
    /// step: an act that has just reached a terminal status ended at `now`, and one moved
    /// back to a status that leaves it under way has not ended.
    ///
    /// The ONE place an act's status is written, so `endedAt` cannot drift from the status
    /// it belongs to. A move to the status the act already holds is no move at all: it
    /// leaves the instant the act finished where it was, which is what a repeat of an
    /// `@idempotent` update must observe.
    pub fn move_to(&mut self, status: ActStatus, now: SystemTime) {
        if self.status == status {
            return;
        }
        self.status = status;
        self.ended_at = if status.is_terminal() { Some(now) } else { None };
    }
}

/// What an `@idempotencyToken` already created, so repeating the call replays it.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The request the token was first seen with. A token reused with a different
    /// request is a conflict, not a replay.
    pub fingerprint: String,
    /// The identifier the first call returned: the definition name, or the generated
    /// run / session / act id. The replay re-reads the resource through it, so the
    /// replayed response still comes from state.
    pub resource_id: String,
}

/// Per-service state for com-amazonaws-novaact-amazonnovaagentsdataplane. A handler reaches it through [`service_state`].
///
/// Every map here is BOUNDED: the definition store by [`MAX_WORKFLOW_DEFINITIONS`], each
/// nested map by its per-parent and store-wide caps (checked through [`Self::check_definition_quota`]
/// and friends before any insert), and the idempotency map by [`MAX_IDEMPOTENCY_ENTRIES`]
/// with oldest-first eviction. Nothing a caller can repeat grows this store without limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsNovaactAmazonnovaagentsdataplaneState {
    /// Definitions by name — the model requires a name unique per account and region.
    pub workflow_definitions: BTreeMap<String, WorkflowDefinition>,
    /// `(operation, clientToken)` -> what that token created.
    idempotent_creates: HashMap<String, IdempotentCreate>,
    /// The order [`Self::idempotent_creates`]' keys were recorded in, so the oldest is
    /// the one evicted once the map is full.
    idempotency_order: VecDeque<String>,
    /// Runs held across every definition, maintained by [`Self::insert_run`] and the
    /// deletions [`Self::complete_due_deletions`] performs.
    stored_runs: usize,
    /// Sessions held across every run.
    stored_sessions: usize,
    /// Acts held across every session.
    stored_acts: usize,
    /// Monotonic creation counter handed to every record's `insertion_seq`.
    insertion_seq: u64,
    /// Monotonic counter mixed into every minted id, so two ids minted inside one clock
    /// tick still differ. Only [`Self::mint_uuid`] reads it.
    mint_counter: u64,
}

impl ComAmazonawsNovaactAmazonnovaagentsdataplaneState {
    /// The next creation-order stamp.
    pub fn next_insertion_seq(&mut self) -> u64 {
        self.insertion_seq += 1;
        self.insertion_seq
    }

    /// A freshly minted identifier in the lowercase hyphenated form every `UuidString`
    /// member of this model is constrained to
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), carrying the
    /// version and variant nibbles of a random (version 4) UUID.
    ///
    /// Minted the way this workspace's other services mint server-side ids (see
    /// `service-com-amazonaws-pinpoint-pinpoint`'s `mint_id`): SHA-256 over a monotonic
    /// counter and the clock, so two ids minted inside one clock tick — or after the
    /// clock steps back — still differ, and a caller cannot read a sequence number out
    /// of an id and predict the next one. The hash comes from `sha2` and the hex from
    /// `hex`, the crates this package already uses; neither is reimplemented here.
    pub fn mint_uuid(&mut self) -> String {
        self.mint_counter = self.mint_counter.wrapping_add(1);
        let seed = format!("{}:{}", self.mint_counter, nanos_since_epoch());
        let digest = Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Version 4 and the RFC 4122 variant, so the value is a well-formed random UUID
        // rather than 32 arbitrary hex characters wearing the hyphens.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let rendered = ::hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &rendered[0..8],
            &rendered[8..12],
            &rendered[12..16],
            &rendered[16..20],
            &rendered[20..32]
        )
    }

    /// Materialise every deletion the clock has already made due.
    ///
    /// Called at the start of every handler, read or write: it finishes work that was
    /// owed at `now` whether or not anybody called, so a reporting operation running it
    /// changes nothing a later caller could not already have observed. Removing a
    /// definition releases everything stored under it, counters included, so the store
    /// shrinks again rather than only ever growing.
    pub fn complete_due_deletions(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .workflow_definitions
            .iter()
            .filter(|(_, definition)| {
                definition.deletion_due_at.is_some_and(|instant| instant <= now)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            let Some(definition) = self.workflow_definitions.remove(&name) else {
                continue;
            };
            for run in definition.runs.values() {
                for session in run.sessions.values() {
                    self.stored_acts = self.stored_acts.saturating_sub(session.acts.len());
                }
                self.stored_sessions = self.stored_sessions.saturating_sub(run.sessions.len());
            }
            self.stored_runs = self.stored_runs.saturating_sub(definition.runs.len());
        }
    }

    /// Runs held across every definition.
    pub fn stored_runs(&self) -> usize {
        self.stored_runs
    }

    /// Sessions held across every run.
    pub fn stored_sessions(&self) -> usize {
        self.stored_sessions
    }

    /// Acts held across every session.
    pub fn stored_acts(&self) -> usize {
        self.stored_acts
    }

    /// Whether another definition fits, naming the quota it would exceed.
    pub fn check_definition_quota(&self) -> Result<(), Quota> {
        if self.workflow_definitions.len() >= MAX_WORKFLOW_DEFINITIONS {
            return Err(Quota::WorkflowDefinitions);
        }
        Ok(())
    }

    /// Whether another run fits under `definition_name`, store-wide cap first.
    pub fn check_run_quota(&self, definition_name: &str) -> Result<(), Quota> {
        if self.stored_runs >= MAX_WORKFLOW_RUNS {
            return Err(Quota::WorkflowRuns);
        }
        let held = self
            .workflow_definitions
            .get(definition_name)
            .map_or(0, |definition| definition.runs.len());
        if held >= MAX_RUNS_PER_DEFINITION {
            return Err(Quota::RunsPerDefinition);
        }
        Ok(())
    }

    /// Whether another session fits under that run, store-wide cap first.
    pub fn check_session_quota(
        &self,
        definition_name: &str,
        run_id: &str,
    ) -> Result<(), Quota> {
        if self.stored_sessions >= MAX_SESSIONS {
            return Err(Quota::Sessions);
        }
        let held = self
            .definition(definition_name)
            .and_then(|definition| definition.run(run_id))
            .map_or(0, |run| run.sessions.len());
        if held >= MAX_SESSIONS_PER_RUN {
            return Err(Quota::SessionsPerRun);
        }
        Ok(())
    }

    /// Whether another act fits under that session, store-wide cap first.
    pub fn check_act_quota(
        &self,
        definition_name: &str,
        run_id: &str,
        session_id: &str,
    ) -> Result<(), Quota> {
        if self.stored_acts >= MAX_ACTS {
            return Err(Quota::Acts);
        }
        let held = self
            .definition(definition_name)
            .and_then(|definition| definition.run(run_id))
            .and_then(|run| run.session(session_id))
            .map_or(0, |session| session.acts.len());
        if held >= MAX_ACTS_PER_SESSION {
            return Err(Quota::ActsPerSession);
        }
        Ok(())
    }

    /// Store `run` under its definition, keeping the run counter in step.
    ///
    /// The insert goes through here rather than through `definition.runs` directly so the
    /// bound [`Self::check_run_quota`] enforces cannot drift from what is held.
    pub fn insert_run(
        &mut self,
        definition_name: &str,
        run: WorkflowRun,
    ) -> Result<(), MissingResource> {
        let definition = self.definition_mut(definition_name)?;
        if definition.runs.insert(run.run_id.clone(), run).is_none() {
            self.stored_runs += 1;
        }
        Ok(())
    }

    /// Store `session` under its run, keeping the session counter in step.
    pub fn insert_session(
        &mut self,
        definition_name: &str,
        run_id: &str,
        session: Session,
    ) -> Result<(), MissingResource> {
        let run = self
            .definition_mut(definition_name)?
            .run_mut(run_id)?;
        if run
            .sessions
            .insert(session.session_id.clone(), session)
            .is_none()
        {
            self.stored_sessions += 1;
        }
        Ok(())
    }

    /// Store `act` under its session, keeping the act counter in step.
    pub fn insert_act(
        &mut self,
        definition_name: &str,
        run_id: &str,
        session_id: &str,
        act: Act,
    ) -> Result<(), MissingResource> {
        let session = self
            .definition_mut(definition_name)?
            .run_mut(run_id)?
            .session_mut(session_id)?;
        if session.acts.insert(act.act_id.clone(), act).is_none() {
            self.stored_acts += 1;
        }
        Ok(())
    }

    /// The act those four identifiers name, mutably: every parent in the chain is resolved
    /// first, so an act reached through here is never one under a parent this store does
    /// not hold, and whichever link is missing is what the refusal reports.
    pub fn act_mut(
        &mut self,
        definition_name: &str,
        run_id: &str,
        session_id: &str,
        act_id: &str,
    ) -> Result<&mut Act, MissingResource> {
        let path = act_path(definition_name, run_id, session_id, act_id);
        let session = self
            .definition_mut(definition_name)?
            .run_mut(run_id)?
            .session_mut(session_id)?;
        session
            .acts
            .get_mut(act_id)
            .ok_or_else(|| MissingResource::new("act", path))
    }

    /// The definition `name` names.
    pub fn definition(&self, name: &str) -> Result<&WorkflowDefinition, MissingResource> {
        self.workflow_definitions.get(name).ok_or_else(|| {
            MissingResource::new("workflow-definition", workflow_definition_path(name))
        })
    }

    /// The definition `name` names, mutably.
    pub fn definition_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut WorkflowDefinition, MissingResource> {
        self.workflow_definitions.get_mut(name).ok_or_else(|| {
            MissingResource::new("workflow-definition", workflow_definition_path(name))
        })
    }

    /// What `token` already created for `operation`, if anything.
    pub fn idempotent_create(&self, operation: &str, token: &str) -> Option<&IdempotentCreate> {
        self.idempotent_creates
            .get(&idempotency_key(operation, token))
    }

    /// How many idempotent-create replays are remembered: at most
    /// [`MAX_IDEMPOTENCY_ENTRIES`].
    pub fn remembered_idempotent_creates(&self) -> usize {
        self.idempotent_creates.len()
    }

    /// Record what `token` created for `operation`, so a repeat replays it.
    ///
    /// Evicts the oldest remembered token once [`MAX_IDEMPOTENCY_ENTRIES`] are held, so a
    /// caller sending a fresh token every request cannot grow this map without limit —
    /// including with tokens whose resource has since been deleted.
    pub fn record_idempotent_create(
        &mut self,
        operation: &str,
        token: &str,
        fingerprint: String,
        resource_id: String,
    ) {
        let key = idempotency_key(operation, token);
        if self
            .idempotent_creates
            .insert(
                key.clone(),
                IdempotentCreate {
                    fingerprint,
                    resource_id,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotent_creates.remove(&oldest);
            }
        }
    }
}

fn idempotency_key(operation: &str, token: &str) -> String {
    format!("{operation}{FIELD_SEPARATOR}{token}")
}

/// Separator for the request fingerprints below: a unit-separator control byte, which
/// none of the constrained input members this service models can contain.
const FIELD_SEPARATOR: char = '\u{1f}';

/// A canonical description of a create request's members.
///
/// Compared only for equality — between the request in hand and the one a `clientToken`
/// (or a definition name) was first seen with — so a repeat of the same call is told
/// apart from a different call wearing the same token.
pub fn fingerprint(fields: &[String]) -> String {
    fields.join(&FIELD_SEPARATOR.to_string())
}

/// A fingerprint field for an optional string member.
pub fn field(value: Option<&String>) -> String {
    value.cloned().unwrap_or_default()
}

/// A fingerprint field for a modeled structure or list, via its `Debug` form, which is
/// derived straight from the members and so is stable for equal values.
pub fn structural_field<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

/// The region to build this request's ARNs in: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`].
///
/// A scope region that is not a syntactic AWS region is ignored rather than interpolated:
/// every ARN shape in this model constrains the region segment to `[a-z0-9-]+`.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The resource part of a workflow definition's ARN: `workflow-definition/<name>`, as
/// `WorkflowDefinitionArn`'s pattern spells it.
pub fn workflow_definition_path(name: &str) -> String {
    format!("workflow-definition/{name}")
}

/// The resource part of a workflow run's ARN. `WorkflowRunArn`'s pattern nests the run
/// under its definition's path, which is the nesting every resource below follows too.
pub fn workflow_run_path(definition_name: &str, run_id: &str) -> String {
    format!(
        "{}/workflow-run/{run_id}",
        workflow_definition_path(definition_name)
    )
}

/// The resource part of a session's ARN.
///
/// The model declares no `SessionArn` shape — no operation returns a session ARN — so the
/// form here extends the run's modelled path with the session's own segment, exactly as
/// `WorkflowRunArn` extends `WorkflowDefinitionArn`. It is what names a session in this
/// service's resource chain, which is what an error reporting a missing session needs.
pub fn session_path(definition_name: &str, run_id: &str, session_id: &str) -> String {
    format!(
        "{}/session/{session_id}",
        workflow_run_path(definition_name, run_id)
    )
}

/// The resource part of an act's ARN.
///
/// The model declares no `ActArn` shape, so this extends the session's path with the act's
/// own segment exactly as [`session_path`] extends the run's: it is what names an act in
/// this service's resource chain, which is what an error reporting a missing act needs.
pub fn act_path(definition_name: &str, run_id: &str, session_id: &str, act_id: &str) -> String {
    format!(
        "{}/act/{act_id}",
        session_path(definition_name, run_id, session_id)
    )
}

/// `arn:aws:nova-act:<region>:<account>:<resource_path>` — this service's ARN for one of
/// its resources, from the path helpers above.
pub fn resource_arn(region: &str, resource_path: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, resource_path)
}

/// `arn:aws:nova-act:<region>:<account>:workflow-definition/<name>`.
pub fn workflow_definition_arn(region: &str, name: &str) -> String {
    resource_arn(region, &workflow_definition_path(name))
}

/// `arn:aws:nova-act:<region>:<account>:workflow-definition/<name>/workflow-run/<id>`.
pub fn workflow_run_arn(region: &str, name: &str, run_id: &str) -> String {
    resource_arn(region, &workflow_run_path(name, run_id))
}

/// Nanoseconds since the Unix epoch, saturating at 0 if the host clock predates it.
fn nanos_since_epoch() -> u128 {
    SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNovaactAmazonnovaagentsdataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNovaactAmazonnovaagentsdataplaneState>)
}
