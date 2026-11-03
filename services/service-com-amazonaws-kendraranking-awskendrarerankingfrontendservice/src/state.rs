// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kendraranking-awskendrarerankingfrontendservice service.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::constraints::path;
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    CapacityUnitsConfiguration, ConflictException, ResourceNotFoundException,
    ResourceUnavailableException, ServiceQuotaExceededException, Tag, ValidationException,
};

/// The `RescoreExecutionPlanStatus` value a plan that is ready to serve `Rescore`
/// carries ("When the value is `ACTIVE`, the rescore execution plan is ready for
/// use." — `RescoreExecutionPlanSummary$Status` in the model).
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The `RescoreExecutionPlanStatus` value a plan carries while the service is still
/// provisioning a freshly created plan (`CREATING` is one of the five values
/// `RescoreExecutionPlanStatus` models). Creating a rescore execution plan is
/// asynchronous: the plan is reported in this non-terminal status first and only then
/// becomes [`STATUS_ACTIVE`], which is what makes polling
/// `DescribeRescoreExecutionPlan` until `ACTIVE` the documented way to use one
/// (`DescribeRescoreExecutionPlanResponse$Status`: "When the value is ACTIVE, the
/// rescore execution plan is ready for use.").
pub const STATUS_CREATING: &str = "CREATING";

/// How this service states that a rescore execution plan id names no plan it holds.
///
/// A CONSTANT, request-independent sentence: the service names the resource class that
/// was not found and never echoes the identifier it was asked for, so the same text
/// answers an id that never existed and an id whose plan was deleted.
pub const PLAN_NOT_FOUND_MESSAGE: &str = "The specified Resource Execution Plan ID does not exist.";

/// How this service states that a plan exists but is not [`STATUS_ACTIVE`] yet, so it
/// cannot serve the data plane. Request-independent, like [`PLAN_NOT_FOUND_MESSAGE`].
pub const PLAN_NOT_ACTIVE_MESSAGE: &str =
    "Rescore Execution Plan status is invalid, must be in ACTIVE state";

/// How this service states that a `Rescore` document carries neither form of its body
/// text.
///
/// A CONSTANT, request-independent sentence naming the members the document must
/// choose between, like [`PLAN_NOT_FOUND_MESSAGE`]: the rule is about the document's
/// shape, so nothing of the caller's values belongs in it.
pub const DOCUMENT_BODY_REQUIRED_MESSAGE: &str = "Document must have a Body or TokenizedBody.";

/// How this service states that a `Rescore` document gave ONE of its fields in BOTH of
/// the two forms it may be given in, with `text_member` and `tokenized_member` the pair
/// the document had to choose between.
///
/// The sentence is the model's own words for this rule ("You must choose to provide
/// `Body` or `TokenizedBody`. You cannot provide both." — `Document$TokenizedBody`, and
/// the same sentence for the title on `Document$TokenizedTitle`), and it is
/// request-independent for the same reason [`DOCUMENT_BODY_REQUIRED_MESSAGE`] is: the
/// rule is about the document's SHAPE, so the two member names are all it needs and
/// nothing of the caller's values belongs in it. The member names come from the model,
/// not from the request.
pub fn document_member_choice_message(text_member: &str, tokenized_member: &str) -> String {
    format!(
        "You must choose to provide {text_member} or {tokenized_member}. \
         You cannot provide both."
    )
}

/// The modelled `ValidationException` a document carrying both forms of one field is
/// refused with — the ONE place that message is built, so every such field pair is
/// worded identically, exactly as [`plan_not_found`] does for the id-addressed lookups.
pub fn document_member_choice(
    text_member: &str,
    tokenized_member: &str,
) -> ValidationException {
    ValidationException {
        message: Some(document_member_choice_message(
            text_member,
            tokenized_member,
        )),
    }
}

/// The modelled `ResourceNotFoundException` for a plan id the store does not hold —
/// the ONE place that message is built, so every plan-lookup path states it
/// identically. It reaches the wire with this service's client-error status (see
/// [`crate::scaffolded::wire_status`]), not with the `@httpError 404` its shape is
/// annotated with.
pub fn plan_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(PLAN_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// How this service states that an ARN names no resource it holds, with `resource_arn`
/// the ARN the caller addressed.
///
/// Unlike [`PLAN_NOT_FOUND_MESSAGE`], this sentence NAMES the identifier it was asked
/// for: an ARN-addressed call's subject is the ARN itself, so the refusal quotes it and
/// then states what to check.
pub fn resource_unavailable_message(resource_arn: &str) -> String {
    format!(
        "Resource {resource_arn} was not found. Please check you have provided the \
         correct resource information and try again."
    )
}

/// The modelled `ResourceUnavailableException` for an ARN the store resolves to no plan
/// — the ONE place that message is built, so every ARN-addressed path states it
/// identically, exactly as [`plan_not_found`] does for the id-addressed ones. It reaches
/// the wire with this service's client-error status (see
/// [`crate::scaffolded::wire_status`]), not with the `@httpError 404` its shape is
/// annotated with.
pub fn resource_unavailable(resource_arn: &str) -> ResourceUnavailableException {
    ResourceUnavailableException {
        message: Some(resource_unavailable_message(resource_arn)),
    }
}

/// The modelled `ValidationException` a data-plane call against a plan that is not
/// [`STATUS_ACTIVE`] is refused with.
pub fn plan_not_active() -> ValidationException {
    ValidationException {
        message: Some(PLAN_NOT_ACTIVE_MESSAGE.to_string()),
    }
}

/// How the refusal of a mutating call names what the caller asked to do to the plan: the
/// past participle a `DeleteRescoreExecutionPlan` call puts in
/// [`plan_mutation_conflict_message`].
pub const MUTATION_DELETED: &str = "deleted";

/// [`MUTATION_DELETED`] for an `UpdateRescoreExecutionPlan` call.
pub const MUTATION_UPDATED: &str = "updated";

/// How this service states that a plan is mid-transition, so it cannot take the mutating
/// call that reached it: `plan_id` is the plan the call addressed, `status` the
/// `RescoreExecutionPlanStatus` value it carries now, and `verb` what the caller asked to
/// do to it ([`MUTATION_DELETED`], [`MUTATION_UPDATED`]).
///
/// The sentence names the plan, the status that refuses the call and the only status in
/// which the call would have been served, so every mutating operation states the same
/// rule and differs only in the verb.
pub fn plan_mutation_conflict_message(plan_id: &str, status: &str, verb: &str) -> String {
    format!(
        "Rescore execution plan {plan_id} is {status} and can only be {verb} while it is \
         {STATUS_ACTIVE}."
    )
}

/// The modelled `ConflictException` a mutating call against a plan that is not
/// [`STATUS_ACTIVE`] is refused with — the ONE place that message is built, so every
/// mutating path states it identically, exactly as [`plan_not_found`] does for the
/// id-addressed lookups. Reached through
/// [`settle_for_mutation`](RescoreExecutionPlan::settle_for_mutation), which is also the
/// one place the predicate behind it lives.
pub fn plan_mutation_conflict(plan_id: &str, status: &str, verb: &str) -> ConflictException {
    ConflictException {
        message: Some(plan_mutation_conflict_message(plan_id, status, verb)),
    }
}

// ---------------------------------------------------------------------------
// The validation report this service answers a constraint violation with.
//
// Three rules, all this service's own rendering of the standard AWS report, and
// all applied to EVERY constraint class and every operation (the boundary
// validator reaches them through `service.rs`'s `override_refusal`):
//
// * a clause names WHERE the offending value was and WHAT it had to satisfy, and
//   never renders the value itself;
// * a `@pattern` requirement quotes the expression WITHOUT the whole-value `^` / `$`
//   pair the model writes around it, which is the expression the service advertises —
//   and, where the expression anchors its own branches, as the model declares it (see
//   `unanchored`);
// * the whole report fits the modelled length of the `Message` member that carries
//   it (see `MAX_ERROR_MESSAGE_CHARS`), however many positions the request offended
//   in.
// ---------------------------------------------------------------------------

/// The error code every refusal this service rewords answers with: the same code the
/// modelled `ValidationException` (HTTP 400) already carries, so rewording the body
/// never moves a client off the code it branches on.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text [`Violation::message`] puts between the offending value's position and the
/// requirement it failed.
///
/// The LAST occurrence is the real separator: a requirement never contains this text,
/// while a caller-supplied value interpolated before it might.
const CONSTRAINT_SEPARATOR: &str = " failed to satisfy constraint: ";

/// A `@pattern` expression as this service advertises it: without the whole-value
/// `^` / `$` pair the model writes around an expression to make the match total.
///
/// The pair is what the MODEL adds, so dropping it is the inverse of that wrapping — and
/// only where the expression inside it carries no anchor of its own. An expression whose
/// branches anchor THEMSELVES is already a whole-value expression before any wrapping:
/// its first and last characters are two different branches' anchors, so dropping them
/// states a requirement of neither branch, leaving an alternative that is a bare
/// end-of-input anchor every value satisfies while the branch the other anchor belonged
/// to goes unstated. Such an expression is quoted as the model declares it, so whatever
/// the clause advertises is an expression a caller can apply to the value that was
/// refused. A `$` escaped by a backslash is a literal dollar sign, not the end anchor,
/// so nothing is dropped for it either.
///
/// The rule is the shared rendering's, CALLED rather than restated here, so this service
/// and every other one wording its own refusals cannot disagree on it.
fn unanchored(pattern: &str) -> String {
    ::service_common::validation::unanchored_keeping_inner_anchors(pattern)
}

/// One clause of the validation report: WHERE the offending value was, and WHAT it had
/// to satisfy. The value itself is never part of a clause.
pub fn clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// The number of characters a message this service answers an error with may carry.
///
/// Every modelled error of this service carries exactly one `Message` member, and every
/// one of them targets `ErrorMessage`, which the model bounds at
/// `@length(min: 1, max: 2048)`. A longer message is outside the shape the model
/// publishes, so no message built here may exceed it — characters, not bytes, because a
/// `@length` on a string counts characters.
pub const MAX_ERROR_MESSAGE_CHARS: usize = 2048;

/// What a validation report puts between two clauses.
const CLAUSE_SEPARATOR: &str = "; ";

/// The sentence a report of `count` clauses opens with, up to and including the space
/// before its first clause.
fn report_prefix(count: usize) -> String {
    match count {
        1 => "1 validation error detected: ".to_string(),
        many => format!("{many} validation errors detected: "),
    }
}

/// How many of `clauses`, taken in order, one report may state and still fit
/// [`MAX_ERROR_MESSAGE_CHARS`].
///
/// The number of offending positions is CALLER-CONTROLLED: one well-formed request may
/// carry hundreds of list elements, each of which can contribute a clause, so a report
/// stating every one of them amplifies a request into a message many times longer than
/// the member that has to carry it may be. The clauses it states are therefore the
/// LEADING ones — the report begins at the first offending position, in request order —
/// and the count its sentence states is the count of exactly those, so the sentence stays
/// literally true of the report carrying it and the format stays AWS's, with no
/// truncation marker. The boundary validator bounds the violations it records in the same
/// way.
///
/// At least one clause is always stated: `ErrorMessage` is modelled `min: 1`, so a report
/// stating no clause at all is as far outside the shape as one past the maximum. One
/// clause always fits in practice — a clause renders a member path and the requirement it
/// failed, never the caller's value, and the framework elides a value it does echo.
fn reported_clause_count(clauses: &[String]) -> usize {
    let separator = CLAUSE_SEPARATOR.chars().count();
    let mut stated = 0;
    let mut clause_chars = 0;
    for (index, clause) in clauses.iter().enumerate() {
        clause_chars += clause.chars().count() + if index == 0 { 0 } else { separator };
        if report_prefix(index + 1).chars().count() + clause_chars > MAX_ERROR_MESSAGE_CHARS {
            break;
        }
        stated = index + 1;
    }
    // A report of one clause is stated whole even where it is the only one that does not
    // fit, for the `min: 1` reason above.
    stated.max(clauses.len().min(1))
}

/// The standard AWS validation report around `clauses`: the violation count, then every
/// clause joined with `; ` — bounded by [`MAX_ERROR_MESSAGE_CHARS`], which
/// [`reported_clause_count`] turns into the number of clauses this report states.
pub fn validation_report(clauses: &[String]) -> String {
    let stated = reported_clause_count(clauses);
    format!(
        "{}{}",
        report_prefix(stated),
        clauses[..stated].join(CLAUSE_SEPARATOR)
    )
}

/// The clause one [`Violation`] states under this service's rules.
///
/// The requirement is lifted out of the framework's OWN rendering of the violation
/// rather than re-spelled here, so every constraint class is worded exactly as the
/// boundary validator words it; only the `@pattern` expression is rewritten, and only to
/// drop its anchors. What the clause drops is the rendering of the value.
fn violation_clause(violation: &Violation) -> String {
    let advertised = match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(unanchored(pattern)),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        },
        _ => violation.clone(),
    };
    let rendered = advertised.message();
    match rendered.rsplit_once(CONSTRAINT_SEPARATOR) {
        // A member that was ABSENT has no value to omit, and the framework's own clause
        // already says exactly that (`Value null at '<member>' …`), so it stands. A value
        // the walk withheld was still sent, so its clause states no value either.
        Some((_, requirement)) if violation.value.is_some() || violation.sensitive => {
            clause(&violation.path, requirement)
        }
        _ => rendered,
    }
}

/// The report for every constraint violation the boundary validator found, in the order
/// it found them. This is what `service.rs`'s `override_refusal` answers with.
pub fn refusal_report(violations: &[Violation]) -> String {
    let clauses: Vec<String> = violations.iter().map(violation_clause).collect();
    validation_report(&clauses)
}

/// The report for one member that failed `kind`, for a handler stating a constraint the
/// model cannot express itself.
pub fn constraint_report(member: &str, kind: ViolationKind) -> String {
    validation_report(&[violation_clause(&Violation {
        path: member.to_string(),
        kind,
        // A PRESENT value selects the value-omitting clause rather than the `Value null`
        // one an absent member gets. What it holds is irrelevant: the value never
        // reaches the body.
        value: Some(String::new()),
        sensitive: false,
    })])
}

/// The `RescoreExecutionPlanStatus` value a plan carries while the service
/// re-provisions it for an `UpdateRescoreExecutionPlan` call (`UPDATING` is one of the
/// five values `RescoreExecutionPlanStatus` models).
pub const STATUS_UPDATING: &str = "UPDATING";

/// The `RescoreExecutionPlanStatus` value a plan carries while the service tears it
/// down for a `DeleteRescoreExecutionPlan` call (`DELETING` is one of the five values
/// `RescoreExecutionPlanStatus` models). Deleting a plan is asynchronous like creating
/// and updating one: the plan is reported in this non-terminal status first, every
/// mutating call is refused while it is, and only once the teardown is due does the id
/// stop resolving.
pub const STATUS_DELETING: &str = "DELETING";

/// How long a plan the service put into a transient status stays there before the
/// clock owes it what comes next — a freshly created plan's `CREATING` -> `ACTIVE`, an
/// updated plan's `UPDATING` -> `ACTIVE`, and a deleted plan's `DELETING` -> gone
/// alike.
///
/// The model publishes no duration for any of those transitions and the service does
/// the work after the call has returned, so this span is the emulator's own. It is never
/// zero: a caller acting straight after the call observes the transient status — and the
/// refusals it causes — exactly as it would on the real service. Nothing sleeps for it:
/// the due instant is stored and read back through
/// [`status_at`](RescoreExecutionPlan::status_at) and
/// [`teardown_is_complete`](RescoreExecutionPlan::teardown_is_complete).
pub const TRANSIENT_STATUS_MILLIS: i64 = 500;

/// A status the clock owes a plan: the value the plan takes once `due_millis` has
/// passed, with no further call needed to make it due.
#[derive(Debug, Clone)]
pub struct PendingStatus {
    /// The `RescoreExecutionPlanStatus` value the plan settles into.
    pub status: String,
    /// The instant the settled status comes due, Unix epoch milliseconds.
    pub due_millis: i64,
}

impl PendingStatus {
    /// `settled`, due [`TRANSIENT_STATUS_MILLIS`] after `now_millis` — the one place the
    /// transient span is turned into a due instant.
    pub fn due_after_transient_span(settled: &str, now_millis: i64) -> Self {
        Self {
            status: settled.to_string(),
            due_millis: now_millis.saturating_add(TRANSIENT_STATUS_MILLIS),
        }
    }
}

/// The signing name / ARN namespace of this service, as the model's
/// `aws.api#service` trait states it (see `OPERATIONS.md`).
pub const ARN_NAMESPACE: &str = "kendra-ranking";

/// The region a request carries no SigV4 credential scope for. The emulator's
/// documented default region (see `emulator_arn` in `vendor/server/src/lib.rs`:
/// "`region` is caller-supplied (emulators default to `us-east-1`)").
pub const DEFAULT_REGION: &str = "us-east-1";

/// One rescore execution plan, exactly as `CreateRescoreExecutionPlan` persisted
/// it. `DescribeRescoreExecutionPlan` and `ListRescoreExecutionPlans` read their
/// whole answer from here.
#[derive(Debug, Clone)]
pub struct RescoreExecutionPlan {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The capacity configuration this plan was CONFIGURED with, or `None` for a plan
    /// no call ever gave one: `CapacityUnits` is modelled optional with no `@default`
    /// on both the create request and the describe response, so a plan the caller never
    /// configured has no configuration to report and a reader leaves the member unset.
    /// Writing a configuration here is what an explicit `CapacityUnits` on a create or
    /// an update does; nothing else synthesizes one.
    pub capacity_units: Option<CapacityUnitsConfiguration>,
    /// Creation instant, Unix epoch milliseconds (the model types `CreatedAt` as a
    /// timestamp; milliseconds keep the stored value independent of the wire form).
    pub created_at_millis: i64,
    /// Last-update instant, Unix epoch milliseconds.
    pub updated_at_millis: i64,
    /// A `RescoreExecutionPlanStatus` value. Read it through
    /// [`status_at`](RescoreExecutionPlan::status_at), never directly: a transition the
    /// clock already owes is held in `pending_status` until some call materialises it.
    pub status: String,
    /// The status the clock owes this plan, while a transition is in flight; `None`
    /// once the plan has settled.
    pub pending_status: Option<PendingStatus>,
    /// The instant this plan's teardown is finished, Unix epoch milliseconds, for a plan
    /// a `DeleteRescoreExecutionPlan` call put into [`STATUS_DELETING`]; `None` for a
    /// plan no delete has started.
    ///
    /// Read it through
    /// [`teardown_is_complete`](RescoreExecutionPlan::teardown_is_complete): until that
    /// instant the plan still exists and still reports its `DELETING` status, and from
    /// it the clock owes its removal, which
    /// [`complete_due_teardowns`](ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState::complete_due_teardowns)
    /// materialises.
    pub deletion_due_millis: Option<i64>,
    /// Set only for a plan whose status is `FAILED`; absent otherwise.
    pub error_message: Option<String>,
    /// This plan's tags, keyed by [`normalized_tag_key`] so one key holds at most one
    /// tag and a lookup costs no scan of the set. Read it through
    /// [`tag_list`](RescoreExecutionPlan::tag_list) and write it through
    /// [`put_tag`](RescoreExecutionPlan::put_tag) /
    /// [`remove_tag`](RescoreExecutionPlan::remove_tag), never directly: those keep the
    /// key normalisation and the reported order. Bounded by [`MAX_TAGS_PER_PLAN`].
    pub tags: BTreeMap<String, StoredTag>,
    /// How many distinct tag keys this plan has ever taken; the sequence number
    /// [`StoredTag::order`] comes from.
    pub tags_written: u64,
}

/// One stored tag, with the position it holds in the order
/// [`tag_list`](RescoreExecutionPlan::tag_list) reports.
#[derive(Debug, Clone)]
pub struct StoredTag {
    /// The sequence number this key was first written under, so replacing a tag's
    /// value keeps that key's place in the reported order.
    pub order: u64,
    /// The tag as the caller wrote it, with the key in the caller's own spelling.
    pub tag: Tag,
}

impl RescoreExecutionPlan {
    /// Add `tag`, replacing the value of a tag this plan already carries under the
    /// same key: "A tag key and value ... Keys are not case sensitive and must be
    /// unique." (`Tag$Key` in the model), so one key identifies at most one tag and a
    /// repeated write is a replacement rather than a second entry.
    ///
    /// The replacement keeps the position of the tag it replaced, so the order
    /// `ListTagsForResource` reports is the order the keys were first written.
    ///
    /// The caller bounds the set FIRST with
    /// [`tag_count_after`](RescoreExecutionPlan::tag_count_after): this method only
    /// writes.
    pub fn put_tag(&mut self, tag: Tag) {
        let key = normalized_tag_key(&tag.key);
        match self.tags.get_mut(&key) {
            Some(stored) => stored.tag = tag,
            None => {
                self.tags_written = self.tags_written.wrapping_add(1);
                let order = self.tags_written;
                self.tags.insert(key, StoredTag { order, tag });
            }
        }
    }

    /// Drop the tag held under `key`, if any, and report whether one was there. "If a
    /// tag key does not exist on the resource, it is ignored."
    /// (`UntagResourceRequest$TagKeys` in the model).
    pub fn remove_tag(&mut self, key: &str) -> bool {
        self.tags.remove(&normalized_tag_key(key)).is_some()
    }

    /// This plan's tags, in the order their keys were first written — the order
    /// `ListTagsForResource` reports. At most [`MAX_TAGS_PER_PLAN`] entries.
    pub fn tag_list(&self) -> Vec<Tag> {
        let mut stored: Vec<&StoredTag> = self.tags.values().collect();
        stored.sort_by_key(|entry| entry.order);
        stored.iter().map(|entry| entry.tag.clone()).collect()
    }

    /// How many distinct tag keys this plan would hold if `incoming` were written: the
    /// keys it already holds plus the ones `incoming` introduces. A caller checks this
    /// against [`MAX_TAGS_PER_PLAN`] BEFORE any [`put_tag`](Self::put_tag), so a
    /// request past the ceiling is refused whole rather than half-applied.
    pub fn tag_count_after(&self, incoming: &[Tag]) -> usize {
        let introduced: BTreeSet<String> = incoming
            .iter()
            .map(|tag| normalized_tag_key(&tag.key))
            .filter(|key| !self.tags.contains_key(key))
            .collect();
        self.tags.len().saturating_add(introduced.len())
    }

    /// This plan's status as of `now_millis`: the pending status once the clock owes it,
    /// otherwise the stored one.
    ///
    /// A reporting operation reads the status through this, so a transition that came
    /// due is visible without the read having to write anything.
    pub fn status_at(&self, now_millis: i64) -> &str {
        match &self.pending_status {
            Some(pending) if now_millis >= pending.due_millis => pending.status.as_str(),
            _ => self.status.as_str(),
        }
    }

    /// Materialise a transition the clock already owes, so a mutating call decides
    /// against the same status a reader sees rather than a stale one.
    pub fn settle_status(&mut self, now_millis: i64) {
        if let Some(pending) = self.pending_status.take() {
            if now_millis >= pending.due_millis {
                self.status = pending.status;
            } else {
                self.pending_status = Some(pending);
            }
        }
    }

    /// Settle the transition the clock already owes this plan and decide whether it may
    /// take a mutating call: `Ok(())` for a plan that is [`STATUS_ACTIVE`] once settled,
    /// otherwise the modelled refusal naming it, the status that refuses the call and
    /// `verb` — what the caller asked to do to it ([`MUTATION_DELETED`],
    /// [`MUTATION_UPDATED`]).
    ///
    /// The ONE place a mutating operation makes that decision, so the predicate and its
    /// sentence cannot drift between the operations that mutate a plan; the caller only
    /// lifts the error into its own operation's error enum. Settling first is what makes
    /// the decision agree with the status a reader sees at the same instant rather than a
    /// stale one.
    pub fn settle_for_mutation(
        &mut self,
        now_millis: i64,
        verb: &str,
    ) -> Result<(), ConflictException> {
        self.settle_status(now_millis);
        if self.status == STATUS_ACTIVE {
            Ok(())
        } else {
            Err(plan_mutation_conflict(&self.id, &self.status, verb))
        }
    }

    /// Put this plan into `transient` now, with `settled` the status the clock owes it
    /// [`TRANSIENT_STATUS_MILLIS`] later.
    pub fn begin_transition(&mut self, transient: &str, settled: &str, now_millis: i64) {
        self.status = transient.to_string();
        self.pending_status = Some(PendingStatus::due_after_transient_span(settled, now_millis));
    }

    /// Start this plan's teardown: it takes [`STATUS_DELETING`] now, and the clock owes
    /// its removal [`TRANSIENT_STATUS_MILLIS`] later.
    ///
    /// `DELETING` is terminal for the plan — nothing settles out of it — so any status
    /// the clock still owed it is dropped: the plan is on its way out, not on its way to
    /// `ACTIVE`.
    pub fn begin_deletion(&mut self, now_millis: i64) {
        self.status = STATUS_DELETING.to_string();
        self.pending_status = None;
        self.deletion_due_millis = Some(now_millis.saturating_add(TRANSIENT_STATUS_MILLIS));
    }

    /// Whether the clock has already finished this plan's teardown, so its id resolves
    /// to nothing and it is listed no more. False for a plan no delete has started, and
    /// for one whose teardown span has not passed yet.
    pub fn teardown_is_complete(&self, now_millis: i64) -> bool {
        self.deletion_due_millis
            .is_some_and(|due_millis| now_millis >= due_millis)
    }
}

// ---------------------------------------------------------------------------
// The character set a tag key or value may be built from.
//
// "A tag key and value can consist of Unicode letters, digits, white space, and
// any of the following symbols: _ . : / = + - @." (`Tag` in the model). Neither
// `TagKey` nor `TagValue` carries a `@pattern`, so the boundary validator cannot
// refuse a character outside that set: the rule is stated here instead, and every
// operation that takes a tag key or value from the request applies it before it
// writes anything.
// ---------------------------------------------------------------------------

/// The symbols a tag key or value may use beside Unicode letters, digits and white
/// space (`Tag` in the model names exactly these eight).
pub const TAG_SYMBOLS: [char; 8] = ['_', '.', ':', '/', '=', '+', '-', '@'];

/// The key prefix AWS reserves for itself, so a caller may not write a tag under it.
/// Matched against [`normalized_tag_key`], since a tag key is case-insensitive here
/// ("Keys are not case sensitive and must be unique." — `Tag$Key`).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether every character of `text` is one a tag key or value may be built from.
pub fn tag_text_is_permitted(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_alphanumeric() || c.is_whitespace() || TAG_SYMBOLS.contains(&c))
}

/// Whether `key` lies under the prefix AWS reserves for its own tags.
pub fn tag_key_is_reserved(key: &str) -> bool {
    normalized_tag_key(key).starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The requirement clause a key or value outside the documented character set failed.
fn tag_character_requirement() -> String {
    let symbols: Vec<String> = TAG_SYMBOLS.iter().map(char::to_string).collect();
    format!(
        "Member must consist of Unicode letters, digits, white space and the symbols {}",
        symbols.join(" ")
    )
}

/// The requirement clause a key under the reserved prefix failed.
fn tag_key_prefix_requirement() -> String {
    format!("Member must not begin with the reserved prefix {RESERVED_TAG_KEY_PREFIX}")
}

/// The dotted path of a member of the `position`-th element of the request list
/// `list`, spelled the way the boundary validator spells one (`tags.1.member.key`).
/// `member` is omitted for a list whose element IS the value (`tagKeys.1.member`).
fn element_path(list: &str, position: usize, member: Option<&str>) -> String {
    let mut segments = vec![list.to_string(), path::element(position)];
    if let Some(member) = member {
        segments.push(path::member(member));
    }
    path::join(&segments)
}

/// The modelled `ValidationException` the tags of a write are refused with, or `None`
/// when every key and value is one the service admits.
///
/// Every offending key and value is reported, in request order, so a caller is told
/// about all of them at once — the standard AWS report, bounded by the modelled length
/// of the member it is carried in ([`validation_report`] states the leading clauses that
/// fit). A request this refuses is refused WHOLE: the caller applies it before writing
/// any tag.
pub fn tag_set_rejection(list_member: &str, tags: &[Tag]) -> Option<ValidationException> {
    let mut clauses = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        let position = index + 1;
        if !tag_text_is_permitted(&tag.key) {
            clauses.push(clause(
                &element_path(list_member, position, Some("Key")),
                &tag_character_requirement(),
            ));
        } else if tag_key_is_reserved(&tag.key) {
            // The reserved prefix is only worth stating for a key the service would
            // otherwise have taken: a key outside the character set already failed.
            clauses.push(clause(
                &element_path(list_member, position, Some("Key")),
                &tag_key_prefix_requirement(),
            ));
        }
        if !tag_text_is_permitted(&tag.value) {
            clauses.push(clause(
                &element_path(list_member, position, Some("Value")),
                &tag_character_requirement(),
            ));
        }
    }
    (!clauses.is_empty()).then(|| ValidationException {
        message: Some(validation_report(&clauses)),
    })
}

/// [`tag_set_rejection`] for a list of bare tag keys: the same character set, since
/// these are `TagKey` values too. The reserved prefix is not stated here — it bounds
/// the keys a caller may WRITE, and a key it refuses can never be held.
pub fn tag_key_rejection(list_member: &str, keys: &[String]) -> Option<ValidationException> {
    let clauses: Vec<String> = keys
        .iter()
        .enumerate()
        .filter(|(_, key)| !tag_text_is_permitted(key))
        .map(|(index, _)| {
            clause(
                &element_path(list_member, index + 1, None),
                &tag_character_requirement(),
            )
        })
        .collect();
    (!clauses.is_empty()).then(|| ValidationException {
        message: Some(validation_report(&clauses)),
    })
}

/// The form of a tag key two spellings of the same key share: its lowercasing.
/// Case-insensitively equal keys are the same key ("Keys are not case sensitive and
/// must be unique." — `Tag$Key` in the model); the lowercasing is Unicode-aware
/// because a key "can consist of Unicode letters, digits, white space, and any of the
/// following symbols: _ . : / = + - @" (`Tag`).
pub fn normalized_tag_key(key: &str) -> String {
    key.to_lowercase()
}

/// The number of tags one rescore execution plan may carry.
///
/// `TagResource` writes tags across calls, so without a ceiling a caller could grow
/// one plan's tag set without bound — the modelled `TagList` `@length` caps one
/// REQUEST at 200 tags, not the accumulated set. This ceiling is the same 200, so a
/// plan holds no more tags than one request may carry; with
/// [`MAX_RESCORE_EXECUTION_PLANS`] plans, each tag bounded by `TagKey` 1..128 and
/// `TagValue` 0..256, the whole store is bounded. A write that would take a plan past
/// it is refused whole with the modelled `ValidationException` (the only
/// constraint-refusal error the tagging operations declare — they model no
/// `ServiceQuotaExceededException`), worded by [`constraint_report`] like every other
/// constraint this service states in a handler.
pub const MAX_TAGS_PER_PLAN: usize = 200;

/// The number of rescore execution plans this emulator holds.
///
/// The service quotas these plans per account (the modelled
/// `ServiceQuotaExceededException`, HTTP 402, documents itself as "You have exceeded
/// the set limits for your Amazon Kendra Intelligent Ranking service. Please see
/// Quotas for more information"), so a create past the quota is refused with that
/// error rather than stored. The model does not carry the published number, so the
/// count here is the emulator's own; enforcing it is what keeps this store from
/// growing without bound in a long-lived emulator, since each plan's size is bounded
/// in turn — by the modelled `@length` constraints on the members a single call
/// writes, and by [`MAX_TAGS_PER_PLAN`] for the tag set, which `TagResource` grows
/// across calls. It is measured over the WHOLE store
/// ([`plan_count`](ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState::plan_count)),
/// not over one region, so the number of regions a caller signs for cannot multiply
/// what this emulator holds.
pub const MAX_RESCORE_EXECUTION_PLANS: usize = 100;

/// The largest page a client may ask `ListRescoreExecutionPlans` for.
///
/// The model publishes it on the operation's `MaxResults`
/// (`MaxResultsIntegerForListRescoreExecutionPlansRequest`, `@range(min: 1, max: 50)`),
/// so it is the widest page the paginated listing can ever hand back — a request
/// naming more is already refused with the modelled `ValidationException` before any
/// handler runs.
pub const MAX_LISTED_PLANS_PER_PAGE: usize = 50;

/// The page size [`page_size`] serves a listing that names no `MaxResults`.
///
/// `ListRescoreExecutionPlans` is `@paginated` (`inputToken`/`outputToken` `NextToken`,
/// `pageSize` `MaxResults`), so a response is a PAGE whether or not the caller asked for
/// a size: the remainder is reached through the `NextToken` the page carries. The model
/// does not publish the service's own default, so this one is the emulator's — the
/// widest page a client may request ([`MAX_LISTED_PLANS_PER_PAGE`]), which is the most a
/// response may hold, and never the whole store ([`MAX_RESCORE_EXECUTION_PLANS`] is
/// larger than one page).
pub const DEFAULT_LISTED_PLANS_PER_PAGE: usize = MAX_LISTED_PLANS_PER_PAGE;

/// How many plans one `ListRescoreExecutionPlans` request's page holds: the
/// `MaxResults` it named, or [`DEFAULT_LISTED_PLANS_PER_PAGE`] when it named none.
///
/// The ONE place that decision is made, so a listing is bounded by a page size that is
/// never zero and never wider than [`MAX_LISTED_PLANS_PER_PAGE`], whatever the request
/// carried. `MaxResults`'s modelled range refuses a value outside 1..=50 before a
/// handler runs, so the clamp is only what keeps that bound true in code.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.map_or(DEFAULT_LISTED_PLANS_PER_PAGE, |requested| {
        usize::try_from(requested)
            .unwrap_or(DEFAULT_LISTED_PLANS_PER_PAGE)
            .clamp(1, MAX_LISTED_PLANS_PER_PAGE)
    })
}

/// The number of extra capacity units one rescore execution plan may be provisioned
/// with.
///
/// `CapacityUnitsConfiguration$RescoreCapacityUnits` publishes the ceiling in its own
/// documentation — "A single extra capacity unit for a rescore execution plan provides
/// 0.01 rescore requests per second. You can add up to 1000 extra capacity units." —
/// while the `@range` on `RescoreCapacityUnit` can only carry the lower bound (`min: 0`,
/// no maximum). So the ceiling is a service-side rule the handlers enforce: a create or
/// an update asking for more extra capacity than this is refused with the modelled
/// `ServiceQuotaExceededException` the operation declares, through
/// [`capacity_units_over_quota`], and the plan's stored capacity is left as it was.
pub const MAX_RESCORE_CAPACITY_UNITS: i32 = 1000;

/// The modelled quota refusal for a capacity configuration asking for more than
/// [`MAX_RESCORE_CAPACITY_UNITS`] extra units, or `None` for one within the ceiling.
///
/// The ONE place that decision and its sentence are made, so
/// `CreateRescoreExecutionPlan` and `UpdateRescoreExecutionPlan` — every operation that
/// takes a `CapacityUnitsConfiguration` — refuse the same values identically. The
/// sentence names the ceiling that was exceeded and never echoes the amount asked for,
/// like every other refusal this service states.
pub fn capacity_units_over_quota(
    requested: &CapacityUnitsConfiguration,
) -> Option<ServiceQuotaExceededException> {
    if requested.rescore_capacity_units <= MAX_RESCORE_CAPACITY_UNITS {
        return None;
    }
    Some(ServiceQuotaExceededException {
        message: Some(format!(
            "The amount of extra capacity for a rescore execution plan has exceeded the \
             quota of {MAX_RESCORE_CAPACITY_UNITS} extra capacity units."
        )),
    })
}

/// The number of documents one `Rescore` request may carry.
///
/// `Documents` is modelled with a minimum of 1 and no maximum, so the batch size — and
/// with it the scoring work, which is the batch size times the query's distinct term
/// count — is entirely caller-chosen. This ceiling is the emulator's own (the model
/// publishes none): a longer batch is refused with the modelled `ValidationException`
/// before any scoring runs.
pub const MAX_DOCUMENTS_PER_RESCORE: usize = 100;

/// The rescore execution plans of ONE region, with the indexes that resolve them.
///
/// One of these per region the store holds plans for: a rescore execution plan belongs
/// to the region its create was signed for, so this is the only scope in which its id,
/// its ARN and the `ClientToken` that minted it resolve.
#[derive(Debug, Default)]
pub struct RegionalPlans {
    /// This region's live plans, keyed by their 36-character id. A `BTreeMap` so
    /// `ListRescoreExecutionPlans` has one stable total order to page through.
    pub plans: BTreeMap<String, RescoreExecutionPlan>,
    /// Plan ARN -> plan id, so an ARN-addressed call (the tagging operations)
    /// resolves a `ResourceARN` without scanning every plan.
    pub arn_index: HashMap<String, String>,
    /// `ClientToken` -> plan id: backs `CreateRescoreExecutionPlan` idempotency. One
    /// entry per plan the token created, dropped with that plan, so this map is bounded
    /// like `plans` and `arn_index`.
    pub client_tokens: HashMap<String, String>,
}

impl RegionalPlans {
    /// Drop the plan that id names here, answering it when there was one.
    ///
    /// Its ARN index entry and the idempotency tokens that created it go with it — or a
    /// repeated create would answer with the id of a plan that no longer exists. The ONE
    /// place a plan leaves the store, so no index can outlive the plan it resolves.
    pub fn remove(&mut self, id: &str) -> Option<RescoreExecutionPlan> {
        let removed = self.plans.remove(id)?;
        self.arn_index.remove(&removed.arn);
        self.client_tokens.retain(|_, plan_id| plan_id != id);
        Some(removed)
    }
}

/// Per-service state for com-amazonaws-kendraranking-awskendrarerankingfrontendservice. A handler reaches it through [`service_state`].
///
/// Plans are keyed by REGION first, because a rescore execution plan is a regional
/// resource: this service has a per-region endpoint and mints an ARN that names the
/// region the creating request was signed for, so a plan is visible, addressable and
/// deletable only through that region. The account is not part of the key — this
/// emulator serves the single account [`::engine::emulator_arn`] mints every ARN for —
/// so every request's plans already belong to it.
#[derive(Debug, Default)]
pub struct ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState {
    /// Every live plan, keyed by the region it was created in and then by its id — the
    /// single source of truth. A region holding no plans keeps no entry.
    pub regions: BTreeMap<String, RegionalPlans>,
    /// How many identifiers this store has minted; see [`next_id`](Self::next_id). It
    /// spans every region, so no two plans this store holds ever share an id — the
    /// per-region uniqueness a plan id has in the real service, and no weaker.
    pub ids_minted: u64,
}

impl ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState {
    /// The plans of one region, or `None` when this store holds none there — which is
    /// what a listing in a region nothing was ever created in reports as an empty page.
    ///
    /// A READER: it never creates the region's bucket, so no read changes what a later
    /// caller sees.
    pub fn plans_in(&self, region: &str) -> Option<&RegionalPlans> {
        self.regions.get(region)
    }

    /// [`plans_in`](Self::plans_in), for a call that writes the plans it resolves.
    ///
    /// A WRITER's view of an EXISTING region: like the reader, it never creates the
    /// region's bucket, so a write addressed to a region this store holds nothing in
    /// resolves to nothing instead of leaving an empty bucket behind. The one place the
    /// write path takes a region's bucket, so it cannot reach a different set of plans
    /// than the read path does.
    fn plans_in_mut(&mut self, region: &str) -> Option<&mut RegionalPlans> {
        self.regions.get_mut(region)
    }

    /// The plan that id names in `region`, whatever its status.
    pub fn plan(&self, region: &str, id: &str) -> Option<&RescoreExecutionPlan> {
        self.plans_in(region)?.plans.get(id)
    }

    /// [`plan`](Self::plan), for a call that writes the plan it resolves.
    pub fn plan_mut(&mut self, region: &str, id: &str) -> Option<&mut RescoreExecutionPlan> {
        self.plans_in_mut(region)?.plans.get_mut(id)
    }

    /// The id of the plan an ARN names in `region`, or `None` when that region holds no
    /// plan under that ARN — the ONE place a `ResourceARN` becomes a plan id, so the
    /// reading and the writing lookup below resolve an ARN by the same steps and any
    /// rule added to this mapping reaches both.
    ///
    /// Resolved through the region's `arn_index`, so no caller scans `plans`; an ARN
    /// naming another region resolves to nothing here, because the plan it names does
    /// not live in this region.
    fn id_for_arn(&self, region: &str, resource_arn: &str) -> Option<&str> {
        Some(self.plans_in(region)?.arn_index.get(resource_arn)?.as_str())
    }

    /// The plan an ARN-addressed call names in `region`, or `None` when that ARN
    /// resolves to no plan there.
    pub fn plan_by_arn(&self, region: &str, resource_arn: &str) -> Option<&RescoreExecutionPlan> {
        self.plan(region, self.id_for_arn(region, resource_arn)?)
    }

    /// [`plan_by_arn`](Self::plan_by_arn), for a call that writes the plan it resolves:
    /// the same ARN -> id step, then the write path's lookup of that id.
    pub fn plan_by_arn_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut RescoreExecutionPlan> {
        let id = self.id_for_arn(region, resource_arn)?.to_string();
        self.plan_mut(region, &id)
    }

    /// The plan an ARN-addressed call names in `region`, or the modelled refusal that
    /// ARN resolves to nothing — the ONE place an ARN-addressed operation turns a
    /// `ResourceARN` into either a plan or its refusal, so every such operation resolves
    /// the ARN the same way and refuses it with the same sentence. The caller only lifts
    /// the error into its own operation's error enum.
    pub fn plan_for_arn(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Result<&RescoreExecutionPlan, ResourceUnavailableException> {
        self.plan_by_arn(region, resource_arn)
            .ok_or_else(|| resource_unavailable(resource_arn))
    }

    /// [`plan_for_arn`](Self::plan_for_arn), for a call that writes the plan it resolves.
    pub fn plan_for_arn_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Result<&mut RescoreExecutionPlan, ResourceUnavailableException> {
        self.plan_by_arn_mut(region, resource_arn)
            .ok_or_else(|| resource_unavailable(resource_arn))
    }

    /// The plan a repeated `ClientToken` already created in `region`, if any: a token is
    /// scoped to the region it was used in, so the same token in another region mints
    /// that region's own plan.
    pub fn plan_for_client_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&RescoreExecutionPlan> {
        let regional = self.plans_in(region)?;
        let id = regional.client_tokens.get(client_token)?;
        regional.plans.get(id)
    }

    /// Store a freshly minted plan in the region it was created in, under the
    /// `client_token` that minted it when the request carried one.
    pub fn insert_plan(
        &mut self,
        region: &str,
        plan: RescoreExecutionPlan,
        client_token: Option<String>,
    ) {
        let regional = self.regions.entry(region.to_string()).or_default();
        regional.arn_index.insert(plan.arn.clone(), plan.id.clone());
        if let Some(token) = client_token {
            regional.client_tokens.insert(token, plan.id.clone());
        }
        regional.plans.insert(plan.id.clone(), plan);
    }

    /// Drop every plan whose teardown the clock has already finished, so an id a
    /// `DeleteRescoreExecutionPlan` call put into [`STATUS_DELETING`] stops resolving
    /// once its span has passed — the plan is gone from every reader, its ARN names
    /// nothing, the `ClientToken` that minted it mints a new plan again, and the slot it
    /// held counts against [`MAX_RESCORE_EXECUTION_PLANS`] no more. A region left
    /// holding no plans keeps no bucket.
    ///
    /// Every operation calls this before it resolves anything: the removal was owed at
    /// this instant whether or not anybody called, so materialising it changes nothing a
    /// caller could otherwise have seen, and it is what keeps the store bounded however
    /// many plans are created and deleted.
    pub fn complete_due_teardowns(&mut self, now_millis: i64) {
        self.regions.retain(|_, regional| {
            let torn_down: Vec<String> = regional
                .plans
                .values()
                .filter(|plan| plan.teardown_is_complete(now_millis))
                .map(|plan| plan.id.clone())
                .collect();
            for id in torn_down {
                regional.remove(&id);
            }
            !regional.plans.is_empty()
        });
    }

    /// How many plans this store holds across every region — what
    /// [`MAX_RESCORE_EXECUTION_PLANS`] is measured against, so the whole store stays
    /// bounded however many regions a caller signs for.
    pub fn plan_count(&self) -> usize {
        self.regions
            .values()
            .map(|regional| regional.plans.len())
            .sum()
    }

    /// The next identifier this service hands out, in the 36-character `8-4-4-4-12`
    /// hexadecimal shape `RescoreExecutionPlanId` models (`length 36..36`, pattern
    /// `^[a-zA-Z0-9][a-zA-Z0-9-]*$`), also within `RescoreId`'s `1..36`.
    ///
    /// It carries the minting instant and a per-store sequence number, so no two ids a
    /// store mints are ever equal; an id is an opaque handle, so nothing about a
    /// resource can be read out of it. Written by hand rather than taken from a UUID
    /// crate because adding a dependency means editing `Cargo.lock`, which every
    /// concurrently landing fix also rewrites — a change this branch cannot land.
    pub fn next_id(&mut self, now_millis: i64) -> String {
        self.ids_minted = self.ids_minted.wrapping_add(1);
        let instant = now_millis as u64;
        let sequence = self.ids_minted;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            ((instant >> 16) & 0xffff_ffff) as u32,
            (instant & 0xffff) as u16,
            ((sequence >> 32) & 0xffff) as u16,
            ((sequence >> 16) & 0xffff) as u16,
            ((sequence & 0xffff) << 32) | (instant & 0xffff_ffff),
        )
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKendrarankingAwskendrarerankingfrontendserviceState>)
}

/// Now, as Unix epoch milliseconds.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// A stored epoch-millisecond instant as the modelled timestamp type.
pub fn timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The region this request is addressed to: the region field of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries none.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The ARN of a rescore execution plan in `region`:
/// `arn:aws:kendra-ranking:<region>:<account>:rescore-execution-plan/<id>`.
pub fn plan_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("rescore-execution-plan/{id}"),
    )
}
