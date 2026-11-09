// SPDX-License-Identifier: CC0-1.0

//! The change-set RECORD: the plan one `CreateChangeSet` wrote, the transition its creation
//! and its execution are headed for, and what a read, a delete and an execution make of the
//! statuses it carries.
//!
//! Split out of `state.rs` so one file holds the change set's own state and the decisions
//! that read it; the store that owns these records, and the stacks they plan changes for,
//! stay in the module root.

use super::*;

/// The change-set statuses a change set's creation is headed for, and the instant they
/// are due.
///
/// `CreateChangeSet` answers while CloudFormation is still computing the change set ("After
/// the CreateChangeSet call successfully completes, CloudFormation starts creating the
/// change set"), so the terminal `Status`/`ExecutionStatus` pair is recorded as due a
/// moment later and materialised by [`StoredChangeSet::settle`].
#[derive(Debug, Clone)]
pub struct PendingChangeSetStatus {
    /// The `ChangeSetStatus` that becomes due, if this transition moves it.
    pub status: Option<String>,
    /// The `StatusReason` that comes with that status (the reason a creation that would
    /// change nothing failed, say).
    pub status_reason: Option<String>,
    /// The `ExecutionStatus` that becomes due, if this transition moves it.
    pub execution_status: Option<String>,
    pub due_at: SystemTime,
    /// Whether this transition becomes due WITH the stack operation the change set's
    /// execution started, rather than on a clock of its own: executing a change set applies
    /// its plan to the stack, so the execution is finished when that operation is — however
    /// long the stack is held (a wait condition nobody has signalled, a resource its owning
    /// service has not reported ready). `false` for the change set's own creation, which no
    /// stack operation carries out.
    pub awaits_the_stack: bool,
}

/// What the stack a change set belongs to is doing, as [`StoredChangeSet::settle`] needs it:
/// a change set's EXECUTION is the stack operation it started, so both whether that
/// operation is still running and what it ended in are read off the stack.
#[derive(Debug, Clone, Copy)]
pub struct StackProgress {
    /// Whether an operation is still being carried out on the stack.
    pub operation_in_flight: bool,
    /// Whether the operation it last concluded carried its change through
    /// ([`operation_carried_through`]).
    pub carried_through: bool,
}

/// One change set, as this emulator persists it.
///
/// The change set owns the template and the configuration the request submitted;
/// `ExecuteChangeSet` is what copies them onto the stack. Nothing here is derived on a
/// read: the change list is computed once, when the change set is created.
#[derive(Debug, Clone)]
pub struct StoredChangeSet {
    pub change_set_id: String,
    pub change_set_name: String,
    /// The stack this change set belongs to, by id: a change set outlives its stack's
    /// name changes and is never re-keyed.
    pub stack_id: String,
    pub stack_name: String,
    /// `ChangeSetType`: `CREATE` for a change set that creates its stack, `UPDATE` for one
    /// that updates an existing stack.
    pub change_set_type: String,
    pub description: Option<String>,
    /// The template the change set would put on the stack, as the request submitted it:
    /// the `Original` stage `GetTemplate` serves for this change set.
    pub template_body: String,
    /// The PROCESSED stage of that body — the document this change set would actually
    /// deploy — computed ONCE when the change set was created, like
    /// [`StoredStack::processed_template`]. It is what the change list was planned from,
    /// what `GetTemplate(TemplateStage=Processed)` serves for this change set and what
    /// `ExecuteChangeSet` puts on the stack, so the plan a caller reviews and the
    /// deployment that is carried out are the same document. `Processed::Unavailable` for a
    /// transform this service cannot apply, which is why such a change set settles to
    /// `FAILED`.
    pub processed_template: template::Processed,
    pub parameters: Vec<StoredParameter>,
    pub capabilities: Vec<String>,
    pub tags: Vec<StoredTag>,
    pub notification_arns: Vec<String>,
    pub role_arn: Option<String>,
    pub rollback_configuration: Option<StoredRollbackConfiguration>,
    pub deployment_config: Option<StoredDeploymentConfig>,
    pub deployment_mode: Option<String>,
    pub resource_types: Vec<String>,
    pub on_stack_failure: Option<String>,
    pub include_nested_stacks: bool,
    pub import_existing_resources: bool,
    pub client_token: Option<String>,
    pub creation_time: SystemTime,
    /// The model's `ChangeSetStatus`.
    pub status: String,
    pub status_reason: Option<String>,
    /// The model's `ExecutionStatus`.
    pub execution_status: String,
    /// `DescribeChangeSetOutput$StackDriftStatus`: "the drift status of the stack WHEN THE
    /// CHANGE SET WAS CREATED" — so it is captured off the stack here, at creation, and
    /// never re-read later.
    pub stack_drift_status: String,
    pub pending: Option<PendingChangeSetStatus>,
    /// Whether `DeleteChangeSet` has DISCARDED this change set's plan after its execution
    /// had already been carried out.
    ///
    /// Deleting a change set discards a plan. A change set that has been EXECUTED is no
    /// longer only a plan: the stack took the change, so the record of that execution
    /// survives the delete even though the plan it described does not. A discarded change
    /// set is addressable by nothing but the refusal that tells a caller asking for it to be
    /// executed again that it has already run
    /// ([`ComAmazonawsCloudformationCloudformationState::resolve_executed_change_set`]);
    /// every read answers the modelled not-found for it, and it is listed for its stack no
    /// more.
    pub plan_discarded: bool,
    pub changes: Vec<StoredResourceChange>,
    /// The Hook invocations planned for this change set, in the order they were planned,
    /// so `DescribeChangeSetHooks` pages over them deterministically (see
    /// [`change_set_hooks`]). Bounded at [`change_set_hooks::MAX_CHANGE_SET_HOOKS`],
    /// oldest reclaimed first.
    pub hooks: Vec<change_set_hooks::StoredChangeSetHook>,
}

impl StoredChangeSet {
    /// Record one planned Hook invocation on this change set. The oldest record is
    /// reclaimed past [`change_set_hooks::MAX_CHANGE_SET_HOOKS`], so the store cannot grow
    /// without limit.
    ///
    /// This is the ONLY way a record enters a change set's plan, and
    /// [`change_set_hooks`] is where what can and cannot plan one is written down.
    pub fn plan_hook(&mut self, hook: change_set_hooks::StoredChangeSetHook) {
        let over_the_bound = (self.hooks.len() + 1)
            .saturating_sub(change_set_hooks::MAX_CHANGE_SET_HOOKS);
        if over_the_bound > 0 {
            // One shift for however many records the bound reclaims, rather than one per
            // record.
            self.hooks.drain(..over_the_bound);
        }
        self.hooks.push(hook);
    }

    /// The `ChangeSetHooksStatus` this change set's Hook plan is in
    /// ([`change_set_hooks::hooks_status`]).
    ///
    /// The plan is computed as part of creating the change set, so its status follows that
    /// creation: `PLANNING` while the creation is still in flight, `PLANNED` once it has
    /// completed and the plan it computed is what a read reports, and `UNAVAILABLE` for a
    /// change set whose creation did not complete, which has no computed plan to report.
    pub fn hooks_status(&self) -> &'static str {
        match self.status.as_str() {
            change_set_status::CREATE_PENDING => change_set_hooks::hooks_status::PLANNING,
            change_set_status::CREATE_COMPLETE => change_set_hooks::hooks_status::PLANNED,
            _ => change_set_hooks::hooks_status::UNAVAILABLE,
        }
    }
    /// Materialise the change-set transition the clock has already made due, if any, and
    /// answer the instant it was due at — which is when the change set reached the status
    /// it now carries, and so when the operation that was creating it concluded
    /// ([`StoredStack::conclude_change_set_operation`]). `None` when nothing was due.
    ///
    /// Like [`StoredStack::settle`], it only ever applies a transition whose due instant
    /// has passed: it reports what was owed at `now` even if nobody had called.
    ///
    /// `stack` is what the stack this change set belongs to is doing ([`StackProgress`]): a
    /// transition owed WITH that stack's operation
    /// ([`PendingChangeSetStatus::awaits_the_stack`]) stays owed until the operation
    /// concludes — so an execution the stack is still applying keeps reporting
    /// `EXECUTE_IN_PROGRESS` for as long as the stack reports its own operation in flight —
    /// and reaches the terminal status that operation's OUTCOME decides.
    pub fn settle(&mut self, now: SystemTime, stack: StackProgress) -> Option<SystemTime> {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return None;
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.awaits_the_stack && stack.operation_in_flight)
        {
            return None;
        }
        let pending = self.pending.take().expect("checked just above");
        if let Some(status) = pending.status {
            self.status = status;
            self.status_reason = pending.status_reason;
        }
        if let Some(execution_status) = pending.execution_status {
            // An execution is the stack operation it started, so it ends in what that
            // operation ended in: one that could not carry the change through — a failure,
            // or the rollback of one — leaves the change set in the modelled
            // `EXECUTE_FAILED` rather than reporting an execution that completed.
            self.execution_status = if pending.awaits_the_stack && !stack.carried_through {
                execution_status::EXECUTE_FAILED.to_string()
            } else {
                execution_status
            };
        }
        Some(pending.due_at)
    }

    /// Whether `ExecuteChangeSet` can apply this change set: CloudFormation executes a
    /// change set only once its creation has completed and nothing has executed it yet
    /// (`InvalidChangeSetStatusException`: "the change set status might be
    /// CREATE_IN_PROGRESS").
    pub fn is_executable(&self) -> bool {
        self.status == change_set_status::CREATE_COMPLETE
            && self.execution_status == execution_status::AVAILABLE
    }

    /// Whether this change set's EXECUTION has begun — whether something has been applied
    /// to the stack on its behalf, however that application ended.
    ///
    /// A change set whose execution has begun is no longer a plan a caller may discard: it
    /// is the record of a change made to the stack, which is why it stays addressable by
    /// name and by ARN and why every call that would act on it again is refused on its
    /// status rather than answered not-found.
    pub fn execution_began(&self) -> bool {
        matches!(
            self.execution_status.as_str(),
            execution_status::EXECUTE_IN_PROGRESS
                | execution_status::EXECUTE_COMPLETE
                | execution_status::EXECUTE_FAILED
        )
    }

    /// Whether this change set's execution is IN FLIGHT: the stack operation it started is
    /// still being carried out, so its plan is being applied right now.
    pub fn execution_in_flight(&self) -> bool {
        self.execution_status == execution_status::EXECUTE_IN_PROGRESS
    }

    /// The `InvalidChangeSetStatusException` message `ExecuteChangeSet` answers for this
    /// change set, or `None` when it is executable ([`Self::is_executable`]).
    ///
    /// The refusal reports the member whose value REFUSES the call. A change set whose
    /// creation did not reach `CREATE_COMPLETE` has no plan to execute, so its
    /// `ChangeSetStatus` is the reason; one whose creation did complete is refused on its
    /// `ExecutionStatus` instead — the plan is fine, but something has already been applied
    /// on its behalf or the plan has been superseded, which the creation's own outcome does
    /// not say.
    pub fn not_executable_refusal(&self) -> Option<String> {
        if self.is_executable() {
            return None;
        }
        Some(if self.status == change_set_status::CREATE_COMPLETE {
            change_set_not_executable_in_execution_status(
                &self.change_set_id,
                &self.execution_status,
            )
        } else {
            change_set_not_executable_in_status(&self.change_set_id, &self.status)
        })
    }

    /// The body `GetTemplate(TemplateStage=Processed)` serves for this change set, or `None`
    /// when this service could not compute that stage from the submitted body.
    pub fn processed_template_body(&self) -> Option<&str> {
        self.processed_template.body(&self.template_body)
    }

    /// Whether the change set's own creation has settled.
    ///
    /// `GetTemplate`: "For change sets, the Original template is always available. After
    /// CloudFormation finishes creating the change set, the Processed template becomes
    /// available" — so the processed stage is serveable only once the creation this store
    /// answered `CREATE_PENDING` has reached its terminal status.
    pub fn creation_settled(&self) -> bool {
        self.status != change_set_status::CREATE_PENDING
    }

    /// The first attribute this stored change set states differently from a request that
    /// re-creates its name, spelled as the change set's OWN attribute, or `None` when the
    /// request submits the very same change set.
    ///
    /// What `CreateChangeSet` answers for a name the stack already carries depends on this
    /// comparison, not on the name alone: re-submitting the same plan under the same name is
    /// the same change set (which is what makes a retried create idempotent), while one
    /// whose attributes differ cannot be created, and the refusal names the attribute that
    /// differs — "cannot be created due to a mismatch with existing attribute `<attribute>`".
    ///
    /// The attributes are the change set's own, in the order this function compares them;
    /// its id, its stack, its creation time and its status are not submitted attributes and
    /// are not compared. An attribute is named as the SERVICE names it, which is the request
    /// member's name wherever one member carries one attribute — and `Template` for the
    /// template the change set would put on the stack, which three request members
    /// (`TemplateBody`, `TemplateURL`, `UsePreviousTemplate`) are three ways of stating, so
    /// naming any one of them would name a member the conflicting request need not have
    /// carried at all.
    pub fn mismatching_attribute(
        &self,
        submitted: &SubmittedChangeSet<'_>,
    ) -> Option<&'static str> {
        let comparisons: [(&'static str, bool); 14] = [
            ("ChangeSetType", self.change_set_type == submitted.change_set_type),
            (
                "Parameters",
                same_parameters(&self.parameters, submitted.parameters),
            ),
            ("Template", self.template_body == submitted.template_body),
            ("Capabilities", self.capabilities == submitted.capabilities),
            ("ResourceTypes", self.resource_types == submitted.resource_types),
            ("RoleARN", self.role_arn.as_deref() == submitted.role_arn),
            (
                "RollbackConfiguration",
                self.rollback_configuration.as_ref() == submitted.rollback_configuration,
            ),
            (
                "NotificationARNs",
                self.notification_arns == submitted.notification_arns,
            ),
            ("Tags", same_tags(&self.tags, submitted.tags)),
            ("Description", self.description.as_deref() == submitted.description),
            (
                "OnStackFailure",
                self.on_stack_failure.as_deref() == submitted.on_stack_failure,
            ),
            (
                "IncludeNestedStacks",
                self.include_nested_stacks == submitted.include_nested_stacks,
            ),
            (
                "ImportExistingResources",
                self.import_existing_resources == submitted.import_existing_resources,
            ),
            (
                "DeploymentMode",
                self.deployment_mode.as_deref() == submitted.deployment_mode,
            ),
        ];
        comparisons
            .into_iter()
            .find_map(|(attribute, agrees)| (!agrees).then_some(attribute))
    }
}
