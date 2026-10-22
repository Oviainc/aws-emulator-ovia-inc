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

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

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

/// The message a deployment lookup that finds nothing carries.
///
/// Launch Wizard's `ResourceNotFoundException` message is FIXED per kind of resource
/// looked up and never renders the identifier the caller asked for (the existence of
/// an id is not disclosed through the error channel), so every handler that resolves
/// a `deploymentId` answers with this constant rather than a per-request template.
/// This text is Launch Wizard's recorded wording for a deployment lookup.
pub const DEPLOYMENT_NOT_FOUND_MESSAGE: &str = "The specified deployment does not exist.";

/// The message a workload-catalogue lookup that finds nothing carries.
///
/// See [`DEPLOYMENT_NOT_FOUND_MESSAGE`]: identifier-free and fixed. Launch Wizard's
/// recorded wording for a workload lookup deliberately conflates "absent" with "you
/// may not see it", so a caller cannot probe the catalogue through the error channel.
pub const WORKLOAD_NOT_FOUND_MESSAGE: &str =
    "The specified workload does not exist or insufficient permissions.";

/// The message a deployment-pattern lookup that finds nothing carries.
///
/// Neither AWS's docs nor the recordings state the wording for a pattern lookup, so
/// this text is CHOSEN; what the contract requires, and what this constant exists to
/// hold, is that the message is fixed and identifier-free like the two above, which a
/// pattern name (caller input, and a probe of the catalogue) must not breach either.
pub const DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE: &str =
    "The specified deployment pattern does not exist or insufficient permissions.";

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

/// The `ValidationException` message every unusable `nextToken` is refused with.
///
/// A pagination token is OPAQUE — the caller may only hand back a token a previous
/// page gave it — so the refusal is one fixed sentence (recorded behaviour) that
/// states neither the rejected token nor which way it was unusable: a token this
/// service never issued and a token that no longer names a position in the result set
/// are the same refusal to a caller, and echoing the token back would put
/// caller-supplied input in the error channel.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "NextToken is not valid.";

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
    /// `WorkloadData$description`: what the workload deploys.
    pub description: String,
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

    /// This workload as the `WorkloadData` `GetWorkload` returns.
    ///
    /// The members `WorkloadData` shares with `WorkloadDataSummary` are read off the
    /// same catalogue entry [`Workload::to_summary`] reads, so the two reporting
    /// operations cannot disagree about one workload.
    ///
    /// `accountConstraints` is absent for the reason [`Workload::to_summary`] gives.
    /// `documentationUrl` and `iconUrl` are absent because this emulator hosts no
    /// workload document and no icon, so there is no URL to report; `statusMessage` is
    /// absent because an `ACTIVE` catalogue entry has nothing to say about its status.
    pub fn to_data(&self) -> WorkloadData {
        WorkloadData {
            workload_name: Some(self.workload_name.clone()),
            display_name: Some(self.display_name.clone()),
            status: Some(self.status.clone()),
            account_constraints: None,
            description: Some(self.description.clone()),
            documentation_url: None,
            icon_url: None,
            status_message: None,
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
///
/// `description` is the per-workload metadata `WorkloadData` (and therefore
/// `GetWorkload`) reports: what the entry's application deploys.
fn seeded_workloads() -> BTreeMap<String, Workload> {
    let catalogue = [
        (
            "SAP",
            "SAP",
            "Deploy SAP HANA and SAP NetWeaver applications on Amazon Web Services.",
            &["SapHanaSingle", "SapHanaMultiNode", "SapNWOnHanaHA"][..],
        ),
        (
            "SQLServer",
            "Microsoft SQL Server",
            "Deploy Microsoft SQL Server, standalone or highly available, on Amazon Web Services.",
            &["SqlServerSingle", "SqlServerHA"][..],
        ),
        (
            "ActiveDirectory",
            "Microsoft Active Directory",
            "Deploy AWS Managed Microsoft AD or self-managed Active Directory on Amazon Web \
             Services.",
            &["AWSManagedAD", "SelfManagedAD"][..],
        ),
    ];
    catalogue
        .into_iter()
        .map(|(workload_name, display_name, description, patterns)| {
            (
                workload_name.to_string(),
                Workload {
                    workload_name: workload_name.to_string(),
                    display_name: display_name.to_string(),
                    description: description.to_string(),
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
/// the operation's own `ValidationException` ([`INVALID_NEXT_TOKEN_MESSAGE`]); a token
/// this service did not issue, or one naming an item the result set no longer holds, is
/// a client error rather than a silent restart from the top.
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
            // A token is opaque, so a refusal says only that it is not valid: neither
            // which of the two ways it is unusable, nor the token itself.
            let cursor =
                decode_token(token).ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?;
            let found = items.iter().position(|item| cursor_of(item) == cursor);
            found
                .map(|index| index + 1)
                .ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?
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

// =============================================================================
// The `ValidationException` message a refused request carries
// =============================================================================

/// The text separating the member clause of a validation clause from the requirement
/// it failed, in the framework's own wording.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// How AWS states that a member's requirement is a COMPOSITION of several
/// constraints: the composed list follows this, bracketed.
const COMPOSED_MARKER: &str = "Member must satisfy constraint: ";

/// AWS's standard validation report for the modelled constraints a request failed at
/// the protocol boundary, as `src/service.rs` words this service's refusal with.
///
/// The report opens with HOW MANY members failed, then states one clause per failing
/// member: the member's dotted path and the requirement it failed. It does NOT echo
/// the rejected value — caller-supplied input is not replayed into the error channel —
/// and reports a `@pattern` as the expression itself, without the `^`/`$` the model
/// spells "whole value" with.
pub fn validation_report(violations: &[Violation]) -> String {
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let clause = validation_clause(violation);
        // AWS reports one error per failing MEMBER. Two violations of the same
        // collection member render the same clause (the composed constraint set of its
        // elements), so the second is not a second error.
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    let count = clauses.len();
    let noun = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {noun} detected: {}", clauses.join("; "))
}

/// One clause of [`validation_report`].
///
/// A violation found on a scalar ELEMENT of a list is reported at the COLLECTION
/// member, because the rule the element failed belongs to the collection's declared
/// element constraints; anything else is reported at the member itself.
fn validation_clause(violation: &Violation) -> String {
    collection_element_clause(violation).unwrap_or_else(|| scalar_clause(violation))
}

/// The clause for a member that failed one of its own constraints: the member and what
/// it failed, with no value echoed. An ABSENT member is still reported as the null it
/// was, which is the one "value" AWS states.
fn scalar_clause(violation: &Violation) -> String {
    let subject = if violation.value.is_none() {
        "Value null"
    } else {
        "Value"
    };
    format!(
        "{subject} at '{}' {CONSTRAINT_MARKER}{}",
        violation.path,
        requirement(violation)
    )
}

/// The clause for a scalar element of a list, or `None` when `violation` is not one.
///
/// It is reported at the collection member's path — never the element's position —
/// with the element shape's COMPLETE declared constraint set composed into one
/// requirement, whichever member of that set the element actually failed.
fn collection_element_clause(violation: &Violation) -> Option<String> {
    let collection = collection_of_element(&violation.path)?;
    Some(format!(
        "Value at '{collection}' {CONSTRAINT_MARKER}{COMPOSED_MARKER}[{}]",
        composed_requirements(collection, violation).join(", ")
    ))
}

/// The collection path a violation path's last segment is an element of, or `None`
/// when the path does not end in a collection step.
///
/// The validating walk spells a step from a list into its element
/// `<1-based-index>.member` (`aws_smithy_schema_server::constraints::path`), so
/// `filters.1.member.values.1.member` is an element of `filters.1.member.values`.
fn collection_of_element(path: &str) -> Option<&str> {
    let without_step = path.strip_suffix(".member")?;
    let (collection, index) = without_step.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.chars().all(|c| c.is_ascii_digit());
    indexed.then_some(collection)
}

/// The composed constraint list AWS states for an element of `collection`: the element
/// shape's own constraints, in the order AWS lists them (length ceiling, then length
/// floor, then pattern).
///
/// A list this service's inputs declare no element constraints for falls back to the
/// one requirement the walk found, stated in the same composed form, so a constraint
/// kind the framework adds later is reported rather than dropped.
fn composed_requirements(collection: &str, violation: &Violation) -> Vec<String> {
    match element_constraints(collection) {
        Some(constraints) => {
            let mut requirements = Vec::new();
            if let Some(max) = constraints.max_length {
                requirements.push(framework_requirement(&ViolationKind::MaxLength(max)));
            }
            if let Some(min) = constraints.min_length {
                requirements.push(framework_requirement(&ViolationKind::MinLength(min)));
            }
            if let Some(pattern) = constraints.pattern {
                requirements.push(pattern_requirement(&composed_pattern(pattern)));
            }
            requirements
        }
        None => vec![match &violation.kind {
            ViolationKind::Pattern(pattern) => pattern_requirement(&composed_pattern(pattern)),
            kind => framework_requirement(kind),
        }],
    }
}

/// The requirement a violation states, for a member reported on its own.
///
/// A `@pattern` is reported UNANCHORED, because the requirement is the expression a
/// value must match and the `^`/`$` are how the model spells "the whole value". Every
/// other constraint keeps the framework's own wording, read off the clause it builds
/// for this same violation, so a constraint kind added to the framework later is
/// reported rather than dropped.
fn requirement(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => pattern_requirement(unanchored(pattern)),
        kind => framework_requirement(kind),
    }
}

/// How a `@pattern` requirement is worded, whatever rendering of the expression it
/// states.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// `pattern` as a composed collection constraint states it: VERBATIM, anchors and all,
/// bracketed by `.*` on each side — the composition is the expression the collection's
/// own rendering has to satisfy, so the element's expression is embedded in it
/// (recorded behaviour).
fn composed_pattern(pattern: &str) -> String {
    format!(".*{pattern}.*")
}

/// `pattern` without its `^` prefix and `$` suffix.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The framework's own wording for `kind`'s requirement.
///
/// Read off a clause the framework builds for a violation of that kind with NO value
/// attached: a clause carrying the value would put caller-controlled text before the
/// marker, so a value containing the marker itself would split inside the value and
/// copy the rest of it into the report.
fn framework_requirement(kind: &ViolationKind) -> String {
    let clause = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
    }
    .message();
    match clause.split_once(CONSTRAINT_MARKER) {
        Some((_, requirement)) => requirement.to_string(),
        None => clause,
    }
}

/// The own constraints of the scalar ELEMENT of a constrained list, as AWS's composed
/// collection-constraint list states them.
#[derive(Debug, Clone, Copy)]
struct ElementConstraints {
    /// The element shape's `@length` maximum, if it declares one.
    max_length: Option<i64>,
    /// The element shape's `@length` minimum, if it declares one.
    min_length: Option<i64>,
    /// The element shape's `@pattern`, verbatim as the model spells it.
    pattern: Option<&'static str>,
}

/// Every list in this service's INPUTS whose element shape carries its own
/// constraints, keyed by the list member's dotted path with the collection steps
/// stripped ([`member_trail`]), so one entry covers that member however deeply it is
/// nested.
///
/// Source: `smithy_query(query="closure",operation="ListDeployments,UntagResource",
/// model_path="services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/\
/// com-amazonaws-launchwizard-launchwizard.json")`. Those two operations hold the only
/// input lists in the model whose element shape is constrained; every other input list
/// (`DeploymentFilterList`) carries a structure element whose own members are reported
/// under their own names.
const LIST_ELEMENT_CONSTRAINTS: &[(&str, ElementConstraints)] = &[
    // `DeploymentFilter$values` -> `DeploymentFilterValues`, element
    // `DeploymentFilterValue`: @length 1..100, @pattern `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`.
    (
        "filters.values",
        ElementConstraints {
            max_length: Some(100),
            min_length: Some(1),
            pattern: Some(r"(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)"),
        },
    ),
    // `UntagResourceInput$tagKeys` -> `TagKeyList`, element `TagKey`: @length 1..128,
    // @pattern `^[a-zA-Z0-9 _.:/=+@-]+$`.
    (
        "tagKeys",
        ElementConstraints {
            max_length: Some(128),
            min_length: Some(1),
            pattern: Some(r"^[a-zA-Z0-9 _.:/=+@-]+$"),
        },
    ),
];

/// The declared element constraints of the list at `collection_path`, if the model
/// gives its elements any.
fn element_constraints(collection_path: &str) -> Option<ElementConstraints> {
    let trail = member_trail(collection_path);
    LIST_ELEMENT_CONSTRAINTS
        .iter()
        .find(|(member, _)| *member == trail)
        .map(|(_, constraints)| *constraints)
}

/// `path` with every collection step removed, leaving the trail of MEMBER names:
/// `filters.1.member.values` -> `filters.values`.
fn member_trail(path: &str) -> String {
    let mut members: Vec<&str> = Vec::new();
    let mut after_index = false;
    for segment in path.split('.') {
        if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
            after_index = true;
            continue;
        }
        let collection_step = after_index && segment == "member";
        after_index = false;
        if !collection_step {
            members.push(segment);
        }
    }
    members.join(".")
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
