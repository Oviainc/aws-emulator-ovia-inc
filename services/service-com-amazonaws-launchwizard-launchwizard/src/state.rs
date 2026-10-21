// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-launchwizard-launchwizard service.
//!
//! This module owns everything the operation handlers share: the seeded workload
//! catalogue, the authoritative deployment store, the clock-owed status steps a
//! deployment still has to take, and the small projection/pagination helpers the
//! handlers in `ops/` call. `lib.rs` is generated, so a service cannot add a module
//! beside this one — shared service logic lives here rather than being copied into
//! each operation file.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::*;

/// The region an ARN carries when the request is unsigned and therefore names no
/// SigV4 credential scope to read it from (the emulator's default region, as
/// `emulator_server::emulator_arn` documents).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from `OPERATIONS.md` ("ARN namespace: launchwizard").
pub const ARN_SERVICE: &str = "launchwizard";

/// How long one clock-owed deployment status step takes.
///
/// Launch Wizard documents no duration for a deployment reaching `COMPLETED`, so
/// this span is CHOSEN: it is non-zero, so a caller that reads straight after
/// `CreateDeployment` still observes `CREATING`, and short enough that a caller
/// that waits observes the terminal status.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `DeploymentStatus` members this slice moves a deployment through. Every value is
/// one of `DEPLOYMENT_STATUS_VALUES` (the model's `DeploymentStatus` enum).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_CREATING`].
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETE_INITIATING: &str = "DELETE_INITIATING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETED: &str = "DELETED";

/// `EventStatus` members this slice records on a deployment event. Every value is one
/// of `EVENT_STATUS_VALUES` (the model's `EventStatus` enum).
pub const EVENT_CREATED: &str = "CREATED";
/// See [`EVENT_CREATED`].
pub const EVENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`EVENT_CREATED`].
pub const EVENT_COMPLETED: &str = "COMPLETED";

/// `WorkloadStatus` member the seeded catalogue entries carry, from
/// `WORKLOAD_STATUS_VALUES`.
pub const WORKLOAD_ACTIVE: &str = "ACTIVE";

/// How many deployment records this emulator keeps at once.
///
/// AWS publishes no deployment quota, so this is the EMULATOR's own bound, not a
/// claim about the service: the store lives for the life of the process and a caller
/// can create records in it, so it needs a ceiling. Reaching it first drops the
/// oldest already-deleted records (terminal, and retained only as a courtesy); a
/// create that still cannot fit is answered with the modelled
/// `ResourceLimitException`, which is exactly what Launch Wizard answers when a
/// deployment limit is exceeded.
pub const MAX_DEPLOYMENTS: usize = 200;

/// The page size a list operation uses when the caller names no `maxResults`.
///
/// The model caps `maxResults` at 100 (`@range(min: 1, max: 100)`), so one page
/// never carries more than that whether the caller asked for a size or not, and the
/// rest is reached through `nextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// A workload in the seeded, read-only catalogue `CreateDeployment` resolves against.
///
/// `patterns` holds only the deployment-pattern NAMES, because naming a pattern is
/// all the implemented slice does with them. The per-pattern metadata (display name,
/// status, versions, specification fields) belongs to the operations that report it —
/// `GetWorkloadDeploymentPattern`, `ListWorkloadDeploymentPatterns` and the
/// deployment-pattern-version operations — which are still scaffolded, and which
/// extend this struct when they are implemented.
#[derive(Debug, Clone)]
pub struct Workload {
    /// `WorkloadName`: len 1..100, `^[A-Za-z][a-zA-Z0-9-_]*$`.
    pub workload_name: String,
    /// `WorkloadDataSummary$displayName`.
    pub display_name: String,
    /// `WorkloadDataSummary$status`, one of `WORKLOAD_STATUS_VALUES`.
    pub status: String,
    /// `DeploymentPatternName`s this workload supports: len 1..256,
    /// `^[A-Za-z0-9][a-zA-Z0-9-]*$`.
    pub patterns: BTreeSet<String>,
}

impl Workload {
    /// This workload as the `WorkloadDataSummary` `ListWorkloads` returns.
    ///
    /// `accountConstraints` is left absent: the model documents it as the constraints
    /// on which kind of account may deploy the workload, and this emulator models no
    /// account constraints, so reporting none is the honest answer.
    pub fn to_summary(&self) -> WorkloadDataSummary {
        WorkloadDataSummary {
            workload_name: Some(self.workload_name.clone()),
            display_name: Some(self.display_name.clone()),
            status: Some(self.status.clone()),
            account_constraints: None,
        }
    }
}

/// One event in a deployment's history, appended by the lifecycle calls (and by the
/// clock-owed steps those calls schedule) — never canned.
#[derive(Debug, Clone)]
pub struct DeploymentEvent {
    /// `DeploymentEventDataSummary$name`.
    pub name: String,
    /// `DeploymentEventDataSummary$description`.
    pub description: String,
    /// `DeploymentEventDataSummary$status`, one of `EVENT_STATUS_VALUES`.
    pub status: String,
    /// `DeploymentEventDataSummary$statusReason`: present only when the event carries
    /// one, which the happy path does not.
    pub status_reason: Option<String>,
    /// When the event happened.
    pub timestamp: SystemTime,
}

impl DeploymentEvent {
    /// A successful lifecycle event at `timestamp`.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        status: &str,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: status.to_string(),
            status_reason: None,
            timestamp,
        }
    }

    /// This event as the `DeploymentEventDataSummary` `ListDeploymentEvents` returns.
    ///
    /// `metadata` is left absent: the emulator attaches no provider metadata to an
    /// event, so there is nothing to report.
    pub fn to_summary(&self) -> DeploymentEventDataSummary {
        DeploymentEventDataSummary {
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            timestamp: Some(to_date_time(self.timestamp)),
            metadata: None,
        }
    }
}

/// A deployment status step the CLOCK owes, scheduled by the call that started it.
///
/// It is materialised by the first request at or after `due_at` — never by a sleep,
/// and never early: a caller acting immediately after `CreateDeployment` still sees
/// `CREATING` and a caller acting immediately after `DeleteDeployment` still sees
/// `DELETE_IN_PROGRESS`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant this step became owed.
    pub due_at: SystemTime,
    /// The `DeploymentStatus` the deployment takes when the step is materialised.
    pub status: String,
    /// The event appended when the step is materialised.
    pub event: DeploymentEvent,
}

/// A deployment: the service's primary resource and the authoritative record every
/// response field about it is computed from.
#[derive(Debug, Clone)]
pub struct Deployment {
    /// `DeploymentId`: len 2..128, `^[a-zA-Z0-9-]+$`.
    pub id: String,
    /// `DeploymentName` as submitted: len 1..50, `^[A-Za-z0-9_\.-]+$`.
    pub name: String,
    pub workload_name: String,
    pub pattern_name: String,
    /// One of `DEPLOYMENT_STATUS_VALUES`.
    pub status: String,
    /// `DeleteDeploymentOutput$statusReason` / `DeploymentData` has no reason in the
    /// happy path: the emulator reaches no failure status, so there is nothing to say.
    pub status_reason: Option<String>,
    /// The submitted `specifications` (the model marks the map `@sensitive`).
    pub specifications: BTreeMap<String, String>,
    /// `DeploymentData$resourceGroup`. Launch Wizard reports the Resource Groups group
    /// it created for the deployment; this emulator creates none, so it stays absent
    /// rather than naming a group that does not exist.
    pub resource_group: Option<String>,
    pub deployment_arn: String,
    /// Tags live here, on the deployment, keyed by nothing else — so the tagging
    /// operations can be layered on without a second store.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub deleted_at: Option<SystemTime>,
    pub events: Vec<DeploymentEvent>,
    /// Status steps the clock still owes, oldest due first.
    pub pending: VecDeque<PendingTransition>,
}

impl Deployment {
    /// Materialise every status step whose `due_at` has already passed.
    ///
    /// This is the one write a reporting operation is allowed to make: each step was
    /// owed at `due_at` whether or not anybody called, so applying it on read reports
    /// the state the clock already produced. The event timestamp and `modifiedAt` are
    /// the step's own due instant, not "now", so the record does not depend on when it
    /// was observed.
    pub fn materialize_due(&mut self, now: SystemTime) {
        while self.pending.front().is_some_and(|step| step.due_at <= now) {
            let Some(step) = self.pending.pop_front() else {
                break;
            };
            self.status = step.status;
            self.modified_at = step.due_at;
            if self.status == STATUS_DELETED {
                self.deleted_at = Some(step.due_at);
            }
            self.events.push(step.event);
        }
    }

    /// True once the deployment has reached `DELETED`.
    pub fn is_deleted(&self) -> bool {
        self.status == STATUS_DELETED
    }

    /// True while a delete is under way or finished — the states in which another
    /// `DeleteDeployment` must not restart anything.
    pub fn is_deleting_or_deleted(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_DELETE_INITIATING | STATUS_DELETE_IN_PROGRESS | STATUS_DELETED
        )
    }

    /// This deployment as the `DeploymentData` `GetDeployment` returns.
    pub fn to_data(&self) -> DeploymentData {
        DeploymentData {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            workload_name: Some(self.workload_name.clone()),
            pattern_name: Some(self.pattern_name.clone()),
            status: Some(self.status.clone()),
            created_at: Some(to_date_time(self.created_at)),
            modified_at: Some(to_date_time(self.modified_at)),
            specifications: Some(to_hash_map(&self.specifications)),
            resource_group: self.resource_group.clone(),
            deleted_at: self.deleted_at.map(to_date_time),
            tags: (!self.tags.is_empty()).then(|| to_hash_map(&self.tags)),
            deployment_arn: Some(self.deployment_arn.clone()),
        }
    }

    /// This deployment as the `DeploymentDataSummary` `ListDeployments` returns.
    pub fn to_summary(&self) -> DeploymentDataSummary {
        DeploymentDataSummary {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            workload_name: Some(self.workload_name.clone()),
            pattern_name: Some(self.pattern_name.clone()),
            status: Some(self.status.clone()),
            created_at: Some(to_date_time(self.created_at)),
            modified_at: Some(to_date_time(self.modified_at)),
        }
    }
}

/// Per-service state for com-amazonaws-launchwizard-launchwizard. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsLaunchwizardLaunchwizardState {
    /// The seeded, read-only workload catalogue, keyed by `workloadName`.
    pub workloads: BTreeMap<String, Workload>,
    /// The authoritative deployment store, keyed by `deploymentId`.
    pub deployments: BTreeMap<String, Deployment>,
    /// Sequence behind [`ComAmazonawsLaunchwizardLaunchwizardState::mint_deployment_id`].
    pub next_id_seq: u64,
}

impl Default for ComAmazonawsLaunchwizardLaunchwizardState {
    fn default() -> Self {
        Self {
            workloads: seeded_workloads(),
            deployments: BTreeMap::new(),
            next_id_seq: 1,
        }
    }
}

impl ComAmazonawsLaunchwizardLaunchwizardState {
    /// Mint the next `deploymentId`.
    ///
    /// The shape is `DeploymentId`: len 2..128, `^[a-zA-Z0-9-]+$`. The sequence makes
    /// it unique within the process and keeps the lexical order of the ids the same as
    /// the order the deployments were created in.
    pub fn mint_deployment_id(&mut self) -> String {
        let seq = self.next_id_seq;
        self.next_id_seq += 1;
        format!("lwd-{seq:08}")
    }

    /// Materialise the clock-owed steps of every stored deployment. Call this first in
    /// every handler, so each one sees the same, current state.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for deployment in self.deployments.values_mut() {
            deployment.materialize_due(now);
        }
    }

    /// True when a deployment that has not been deleted already carries `name`.
    ///
    /// Derived from the store rather than from a second index, so the two cannot
    /// drift: a deleted deployment keeps its record (and its name on it) but no longer
    /// holds the name against a new deployment.
    pub fn live_name_taken(&self, name: &str) -> bool {
        self.deployments
            .values()
            .any(|d| d.name == name && !d.is_deleted())
    }

    /// Every stored deployment in creation order, oldest first — the order
    /// `ListDeployments` pages over.
    pub fn deployments_in_order(&self) -> Vec<&Deployment> {
        let mut all: Vec<&Deployment> = self.deployments.values().collect();
        all.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        all
    }

    /// Make room for one more deployment, reporting whether there is any.
    ///
    /// See [`MAX_DEPLOYMENTS`]: already-deleted records are dropped oldest-first to
    /// fit a new one, and `false` means even that was not enough, which the caller
    /// answers with the modelled `ResourceLimitException`.
    pub fn make_room_for_a_deployment(&mut self) -> bool {
        while self.deployments.len() >= MAX_DEPLOYMENTS {
            let oldest_deleted = self
                .deployments_in_order()
                .into_iter()
                .find(|deployment| deployment.is_deleted())
                .map(|deployment| deployment.id.clone());
            match oldest_deleted {
                Some(id) => {
                    self.deployments.remove(&id);
                }
                None => return false,
            }
        }
        true
    }
}

/// The workload catalogue the emulator seeds at first touch.
///
/// Launch Wizard's catalogue is service-owned (there is no API to create a workload),
/// so the emulator seeds it. The entries are the Launch Wizard applications AWS
/// documents — SAP, Microsoft SQL Server and Microsoft Active Directory — with the
/// deployment patterns each of those applications offers. The model constrains the
/// shapes of these values: `WorkloadName` is `^[A-Za-z][a-zA-Z0-9-_]*$` (len 1..100)
/// and `DeploymentPatternName` is `^[A-Za-z0-9][a-zA-Z0-9-]*$` (len 1..256).
fn seeded_workloads() -> BTreeMap<String, Workload> {
    let catalogue = [
        (
            "SAP",
            "SAP",
            &["SapHanaSingle", "SapHanaMultiNode", "SapNWOnHanaHA"][..],
        ),
        (
            "SQLServer",
            "Microsoft SQL Server",
            &["SqlServerSingle", "SqlServerHA"][..],
        ),
        (
            "ActiveDirectory",
            "Microsoft Active Directory",
            &["AWSManagedAD", "SelfManagedAD"][..],
        ),
    ];
    catalogue
        .into_iter()
        .map(|(workload_name, display_name, patterns)| {
            (
                workload_name.to_string(),
                Workload {
                    workload_name: workload_name.to_string(),
                    display_name: display_name.to_string(),
                    status: WORKLOAD_ACTIVE.to_string(),
                    patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
                },
            )
        })
        .collect()
}

/// The region to build a new resource's ARN in: the request's SigV4 credential-scope
/// region, or [`DEFAULT_REGION`] when the request is unsigned.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a deployment: `arn:aws:launchwizard:<region>:<account>:deployment/<id>`.
pub fn deployment_arn(region: &str, deployment_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("deployment/{deployment_id}"))
}

/// A `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A stored map as the `HashMap` the generated shapes carry.
fn to_hash_map(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Encode an opaque pagination token over the cursor it resumes after.
///
/// The cursor is hex-encoded so a caller cannot read a position out of it or
/// construct one: the token is only ever a value a previous page handed back.
/// `PaginationToken` is len 1..2048, which hex of a deployment id or a workload name
/// stays well inside.
pub fn encode_token(cursor: &str) -> String {
    ::hex::encode(format!("v1:{cursor}"))
}

/// The cursor an opaque token carries, or `None` when it was not issued by
/// [`encode_token`].
fn decode_token(token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("v1:").map(ToString::to_string)
}

/// One page of `items`: the items after the one `next_token` names, capped at
/// `max_results`, plus the token for the following page when items remain.
///
/// `cursor_of` names an item's stable cursor, so a page resumes exactly where the
/// previous one ended rather than at a remembered offset. The error is the message for
/// the operation's own `ValidationException`; a token this service did not issue, or
/// one naming an item the result set no longer holds, is a client error rather than a
/// silent restart from the top.
pub fn paginate<T>(
    items: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    cursor_of: impl Fn(&T) -> String,
) -> Result<(Vec<T>, Option<String>), String> {
    let total = items.len();
    let start = match next_token {
        None => 0,
        Some(token) => {
            let cursor = decode_token(token)
                .ok_or_else(|| format!("nextToken '{token}' is not a valid pagination token."))?;
            let found = items.iter().position(|item| cursor_of(item) == cursor);
            found.map(|index| index + 1).ok_or_else(|| {
                format!("nextToken '{token}' does not name a position in these results.")
            })?
        }
    };
    // `maxResults` is `@range(min: 1, max: 100)`, so the boundary validator has already
    // refused a value outside that range; absent, a page carries at most
    // `DEFAULT_PAGE_SIZE` so no single response grows with the store.
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max.max(0) as usize);
    let page: Vec<T> = items.into_iter().skip(start).take(limit).collect();
    let next = if start + page.len() < total {
        page.last().map(|item| encode_token(&cursor_of(item)))
    } else {
        None
    };
    Ok((page, next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLaunchwizardLaunchwizardState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLaunchwizardLaunchwizardState>)
}
