Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.cancel-request-triggers-server-emitted-event swf.e2e-workflow-cancellation-flow swf.count-pending-tasks-unknown-domain-returns-zero swf.count-pending-tasks-unknown-tasklist-error swf.respond-decision-task-completed-after-timeout-rejected swf.stale-decision-task-token-reused-rejected swf.tag-resource-exceeds-limit-rejected swf.tag-resource-too-many-tags-rejected swf.undeprecate-active-domain-rejected
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.cancel-request-triggers-server-emitted-event|_poll_decision_until_task#11|Timeout|resp = swf.poll_for_decision_task(domain=domain, taskList={"name": task_list})
Issue-Key: swf.e2e-workflow-cancellation-flow|_poll_for_decision_task#19|Timeout|resp = swf.poll_for_decision_task(domain=domain, taskList=task_list)
Issue-Key: swf|CountPendingActivityTasks|UnknownResourceFault
Issue-Key: swf.respond-decision-task-completed-after-timeout-rejected|run#30|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: swf.stale-decision-task-token-reused-rejected|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: swf.tag-resource-exceeds-limit-rejected|run#9|AssertionError|assert message == "Too many tags provided.", f"unexpected message {message!r}"
Issue-Key: swf.tag-resource-too-many-tags-rejected|run#13|AssertionError|assert message == "Too many tags provided.", f"unexpected message {message!r}"
Issue-Key: swf.undeprecate-active-domain-rejected|run#7|AssertionError|assert message == domain, f"expected message {domain!r}, got {message!r}"
Issue-Commit: 1f4ddd4c47b0507bb1e50344cb9c3bb105203334

# swf: six fidelity divergences — cancel request never schedules the decider's decision task; CountPending* rejects an unknown domain; and four wrong error messages (RespondDecisionTaskCompleted x2, TagResource, UndeprecateDomain)

> _Note: this issue was identified against the code base at commit `1f4ddd4c47b0507bb1e50344cb9c3bb105203334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestCancelWorkflowExecution

## Description
This issue bundles SIX distinct root causes in the Amazon SWF emulator, all grounded in failing behavioral checks of the `swf` service. A fixer must close ALL SIX before submitting; each is numbered and carries its own failing check ids.

=== Cause 1 of 6: RequestCancelWorkflowExecution records the event but never makes the decider's reaction pollable (no DecisionTaskScheduled), so PollForDecisionTask never returns the cancel decision task ===
AWS: Smithy: RequestCancelWorkflowExecution (input domain/workflowId/runId, output Unit, errors UnknownResourceFault, OperationNotPermittedFault). The model documents: "Records a WorkflowExecutionCancelRequested event in the currently running workflow execution ... It is up to the decider to take appropriate actions when it receives an execution history with this event." The decider can only receive that history through a decision task: on real AWS the cancel request appends WorkflowExecutionCancelRequested AND causes a new decision task to become available, so the very next PollForDecisionTask for the execution's task list returns a task whose `events` include WorkflowExecutionCancelRequested; the decider then answers it with a CancelWorkflowExecution decision and the run closes CLOSED/CANCELED with WorkflowExecutionCanceled as the last history event. Recorded behavior confirms a decision task is delivered after the cancel request with no client-side history mutation call.
Emulator: the request latches `cancel_requested` and appends the WorkflowExecutionCancelRequested event, but schedules NO decision task. The handler says so in its own module doc: "in this slice every open run still has the decision task its start scheduled (nothing consumes one — PollForDecisionTask/RespondDecisionTaskCompleted are still 501), so no further decision task is ever owed here" — that premise no longer holds, because PollForDecisionTask/RespondDecisionTaskCompleted ARE implemented and consume the start's decision task. With the initial decision task already drained by RespondDecisionTaskCompleted, the post-cancel PollForDecisionTask returns an empty response forever and both checks hang until the harness kills them ("Check timed out after 60.0s"). DescribeWorkflowExecution does report cancelRequested=true and executionStatus=OPEN, so the divergence is exactly the missing decision task, not the latch.
Why: a decider that follows the AWS cancellation protocol — drain a decision task, request cancel, poll for the next decision task — blocks indefinitely against the emulator. Cooperative cancellation (the AWS-recommended alternative to TerminateWorkflowExecution) can never complete, so no execution can ever reach closeStatus=CANCELED through a decider.
Fix: the RequestCancelWorkflowExecution handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/request_cancel_workflow_execution.rs) must, when it latches the cancel request on an open run with no decision task outstanding, also schedule a decision task for the run's task list through the same state path StartWorkflowExecution/RespondDecisionTaskCompleted use (crate::state), honouring SWF's one-outstanding-decision-task-per-execution rule; the stale module doc claiming nothing is owed should go with it.
Traces: swf.cancel-request-triggers-server-emitted-event, swf.e2e-workflow-cancellation-flow.

=== Cause 2 of 6: CountPendingActivityTasks / CountPendingDecisionTasks reject a never-registered domain with UnknownResourceFault instead of answering count=0 ===
AWS: recorded behavior only (the model does declare UnknownResourceFault for both operations, but AWS does not raise it here). The recording shows both Count operations do NOT validate domain existence: a task list named in a domain that was never registered answers 200 with `{"count": 0, "truncated": false}`, identical to the answer for an unused task list in a registered domain (which the emulator gets right — that part of the check passed).
Emulator: `CountPendingActivityTasks` answers 400 `UnknownResourceFault` with message "Unknown domain: <domain>" (fixed text "Unknown domain: " plus the request's `domain` value) whenever the name is not in the domain store. Same code path in the decision-task counterpart.
Why: a client probing queue depth for a task list it has not provisioned yet, or probing across a domain list, gets a hard client error where AWS returns a usable zero; polling/auto-scaling loops built on these counters break on the emulator for an input AWS accepts.
Fix: the domain lookup in the CountPendingActivityTasks handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/count_pending_activity_tasks.rs, the `guard.domains.get(...).ok_or_else(...)` UnknownResourceFault at ~lines 185-189) and the identical lookup in count_pending_decision_tasks.rs must treat an absent domain as holding no pending tasks and return count=0/truncated=false.
Traces: swf.count-pending-tasks-unknown-domain-returns-zero, swf.count-pending-tasks-unknown-tasklist-error.

=== Cause 3 of 6: RespondDecisionTaskCompleted with a token whose decision task TIMED OUT returns the wrong UnknownResourceFault message ===
AWS: Smithy: RespondDecisionTaskCompleted -> UnknownResourceFault (client, HTTP 400), "Returned when the named resource cannot be found with in the scope of this operation". Recorded behavior: once a decision task's taskStartToCloseTimeout elapses and the task is reassigned, the stale token is refused with code `UnknownResourceFault` and message `Unknown decision task, scheduledEventId = <id>` — fixed text "Unknown decision task, scheduledEventId = ", variable part being the `scheduledEventId` of the DecisionTaskStarted event the stale token addressed (taken from the emulator's own history state, not a literal).
Emulator: right code and status, but the message is the single generic string "Unknown decision task: the taskToken does not address a decision task that is still outstanding" — none of AWS's fixed text and no scheduledEventId.
Why: deciders and SDK-level retry/diagnostic tooling match on this message to tell "my task was reassigned, re-poll" from other UnknownResourceFault causes, and the scheduledEventId is the only pointer back to the history event that was lost; both are unavailable.
Fix: the stale-token rejection path of the RespondDecisionTaskCompleted handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/respond_decision_task_completed.rs) must render the timed-out/superseded-task case with AWS's template, filling scheduledEventId from the addressed DecisionTaskStarted event.
Traces: swf.respond-decision-task-completed-after-timeout-rejected.

=== Cause 4 of 6: RespondDecisionTaskCompleted replaying a token against an already-CLOSED execution must report the execution, not the decision task ===
AWS: Smithy: RespondDecisionTaskCompleted -> UnknownResourceFault (client, HTTP 400). Recorded behavior: reusing a token that was already consumed and whose run has since closed is refused with `UnknownResourceFault` and message `Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]` — fixed text "Unknown execution: WorkflowExecution=[workflowId=", ", runId=", "]", with workflowId and runId taken from the execution the token addressed. Note this is a DIFFERENT template from Cause 3: AWS reports the missing EXECUTION when the run is gone and the missing DECISION TASK when the run is open but the task is stale.
Emulator: returns the same generic "Unknown decision task: the taskToken does not address a decision task that is still outstanding" for both situations, collapsing two distinct AWS diagnoses into one. (The effect-absence part of the check is satisfied: no extra history events, status stays CLOSED/COMPLETED.)
Why: a client cannot distinguish "re-poll, the task moved" from "this run no longer exists, stop", and loses the workflowId/runId identification AWS supplies.
Fix: same handler as Cause 3 — classify the stale-token refusal by whether the addressed run is still open, and emit the `Unknown execution: WorkflowExecution=[workflowId=..., runId=...]` template when it is closed/absent.
Traces: swf.stale-decision-task-token-reused-rejected.

=== Cause 5 of 6: TagResource's TooManyTagsFault message is emulator-invented prose instead of AWS's fixed text ===
AWS: Smithy: TagResource -> TooManyTagsFault (client, HTTP 400), "You've exceeded the number of tags allowed for a domain" (SWF allows 50 tags per resource). Recorded behavior: both over-limit shapes — a single request carrying 51 tags, and a request whose tags would push an already-tagged domain from 2 to 51 — are refused with code `TooManyTagsFault` and the message `Too many tags provided.` This message is entirely FIXED text: it has no variable parts, no count and no resource name.
Emulator: right code and status, wrong message, and two different invented strings: the pre-store batch bound answers "The request carries 51 tags, above the maximum of 50 per resource" (services/.../src/ops/tag_resource.rs ~lines 185-194) and the cumulative merge bound answers "The domain would carry 51 tags, above the maximum of 50 per resource". (Effect absence and the positive control both behave correctly — only the message diverges.)
Why: clients and test suites that assert on the documented message text, or surface it to users verbatim, see a message AWS never emits; and the emulator's two variants mean the text also changes with which bound tripped, which AWS's does not.
Fix: both TooManyTagsFault constructions in the TagResource handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/tag_resource.rs — the batch-size check and the post-merge total check) must carry the single fixed message "Too many tags provided."
Traces: swf.tag-resource-exceeds-limit-rejected, swf.tag-resource-too-many-tags-rejected.

=== Cause 6 of 6: UndeprecateDomain on a REGISTERED domain returns DomainAlreadyExistsFault with prose instead of the domain name ===
AWS: Smithy: UndeprecateDomain -> DomainAlreadyExistsFault (client, HTTP 400), "Returned if the domain already exists. You may get this fault ... if you undeprecate a domain that is currently registered." Recorded behavior: the message body is exactly the requested domain name and nothing else — i.e. the whole message is the variable part (the request's `name`), with no fixed text around it.
Emulator: right code and status, but the message is "the domain <name> already exists" (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/undeprecate_domain.rs ~lines 203-210), wrapping the name in invented fixed text. The state effect is correct (the domain stays REGISTERED) and the deprecate-then-undeprecate positive control passes.
Why: callers that read the fault's message as the resource identifier (the AWS convention for this fault, shared with RegisterDomain) parse a sentence instead of a name and mis-identify the domain.
Fix: the DomainAlreadyExistsFault construction in the UndeprecateDomain handler must set the message to the requested domain name alone; align RegisterDomain's rendering of the same fault with it.
Traces: swf.undeprecate-active-domain-rejected.

Note: swf.activity-heartbeat-timeout-triggers-timeout-event and the other PollForActivityTask-blocked checks, swf.describe-activity-type-cross-domain-rejected (ListActivityTypes), and lambda__swf.schedule-lambda-function-without-role-rejected (owned by 20261025T021137Z-lambda-swf-a-swf-schedulelambdafunction-decision-with-a.md) are excluded from this issue — they are already filed/owned.

## Evidence
Suite: 37/66 passed. Service swf: 37/65, 28 failed (18 of them blocked by the already-filed unimplemented PollForActivityTask, 1 by unimplemented ListActivityTypes).

Cause 1 — swf.cancel-request-triggers-server-emitted-event: "Error: Check timed out after 60.0s". The check drains the initial decision task with RespondDecisionTaskCompleted(decisions=[]), calls RequestCancelWorkflowExecution, successfully polls DescribeWorkflowExecution until cancelRequested is True (so the latch works), then blocks in PollForDecisionTask waiting for the decision task whose `events` must contain WorkflowExecutionCancelRequested. Expected: a decision task is returned and its events (and the history) include WorkflowExecutionCancelRequested. Actual: PollForDecisionTask never yields a task; the check is killed at 60s.
  swf.e2e-workflow-cancellation-flow: same "Check timed out after 60.0s" at the same point — cancelRequested=True/executionStatus=OPEN are asserted successfully, then the poll for the cancel decision task never returns, so the decider can never issue CancelWorkflowExecution and the run never reaches CLOSED/CANCELED with a trailing WorkflowExecutionCanceled event.
  Emulator source confirms the gap: request_cancel_workflow_execution.rs appends the event and sets cancel_requested but schedules no decision task, on the stale premise (module doc, lines 17-21) that PollForDecisionTask/RespondDecisionTaskCompleted are "still 501".

Cause 2 — swf.count-pending-tasks-unknown-domain-returns-zero and swf.count-pending-tasks-unknown-tasklist-error both error identically:
  botocore.errorfactory.UnknownResourceFault: An error occurred (UnknownResourceFault) when calling the CountPendingActivityTasks operation: Unknown domain: <never-registered-domain>
  Expected: HTTP 200 with count=0 (the same checks' positive control — a registered domain with an unused task list — already returns count=0 and passes). Actual: HTTP 400 UnknownResourceFault, "Unknown domain: <domain>".

Cause 3 — swf.respond-decision-task-completed-after-timeout-rejected:
  AssertionError: expected message 'Unknown decision task, scheduledEventId = 2', got 'Unknown decision task: the taskToken does not address a decision task that is still outstanding'
  (code UnknownResourceFault matched; `2` is the scheduledEventId of the stale task's DecisionTaskStarted event in that run, i.e. a state-derived value, not a constant.)

Cause 4 — swf.stale-decision-task-token-reused-rejected:
  AssertionError: expected message 'Unknown execution: WorkflowExecution=[workflowId=sim2real-swf-wf-<id>, runId=<run id>]', got 'Unknown decision task: the taskToken does not address a decision task that is still outstanding'
  (code UnknownResourceFault matched; workflowId/runId are the addressed execution's own identifiers.)

Cause 5 — swf.tag-resource-exceeds-limit-rejected (51 tags in one request):
  AssertionError: unexpected message 'The request carries 51 tags, above the maximum of 50 per resource' — expected 'Too many tags provided.'
  swf.tag-resource-too-many-tags-rejected (2 existing tags + 49 more = 51):
  AssertionError: unexpected message 'The domain would carry 51 tags, above the maximum of 50 per resource' — expected 'Too many tags provided.'
  (code TooManyTagsFault matched in both.)

Cause 6 — swf.undeprecate-active-domain-rejected:
  AssertionError: expected message 'sim2real-behav-domain-<id>' (the requested domain name alone), got 'the domain sim2real-behav-domain-<id> already exists'
  (code DomainAlreadyExistsFault matched; domain status correctly stayed REGISTERED.)

## Steps to Reproduce
Cause 1 (RequestCancelWorkflowExecution schedules no decision task):
1. RegisterDomain; RegisterWorkflowType with a defaultTaskList T, defaultTaskStartToCloseTimeout=30, defaultExecutionStartToCloseTimeout=120, defaultChildPolicy=TERMINATE.
2. StartWorkflowExecution on task list T.
3. PollForDecisionTask(domain, T) -> task; RespondDecisionTaskCompleted(taskToken, decisions=[]) to drain it.
4. RequestCancelWorkflowExecution(domain, workflowId, runId).
5. DescribeWorkflowExecution -> cancelRequested=true, executionStatus=OPEN (passes).
6. PollForDecisionTask(domain, T) again. AWS returns a task whose `events` include WorkflowExecutionCancelRequested; the emulator returns no task, forever.
7. (Blocked follow-on) RespondDecisionTaskCompleted with a CancelWorkflowExecution decision should close the run CLOSED/CANCELED with WorkflowExecutionCanceled last in history.
Replay: swf.cancel-request-triggers-server-emitted-event, swf.e2e-workflow-cancellation-flow.

Cause 2 (CountPending* on an unknown domain):
1. Pick a domain name that was never registered, and any task-list name.
2. CountPendingActivityTasks(domain=<unregistered>, taskList={name:<any>}) -> AWS: 200 count=0; emulator: 400 UnknownResourceFault "Unknown domain: <domain>".
3. Repeat with CountPendingDecisionTasks -> same divergence.
4. Control (already correct): same calls against a REGISTERED domain with an unused task list return count=0.
Replay: swf.count-pending-tasks-unknown-domain-returns-zero, swf.count-pending-tasks-unknown-tasklist-error.

Cause 3 (stale token after decision-task timeout):
1. RegisterDomain; RegisterWorkflowType with defaultTaskStartToCloseTimeout=6.
2. StartWorkflowExecution(taskStartToCloseTimeout=6); PollForDecisionTask -> keep the token and never respond.
3. Poll GetWorkflowExecutionHistory until DecisionTaskTimedOut appears; PollForDecisionTask again to get a fresh, different token.
4. RespondDecisionTaskCompleted with the STALE token -> expect UnknownResourceFault "Unknown decision task, scheduledEventId = <scheduledEventId of the stale task's DecisionTaskStarted>"; emulator returns the generic "Unknown decision task: the taskToken does not address a decision task that is still outstanding".
Replay: swf.respond-decision-task-completed-after-timeout-rejected.

Cause 4 (token replay against a closed run):
1. RegisterDomain; RegisterWorkflowType; StartWorkflowExecution.
2. PollForDecisionTask -> token; RespondDecisionTaskCompleted(token, [CompleteWorkflowExecution]) -> run closes CLOSED/COMPLETED.
3. RespondDecisionTaskCompleted with the SAME token again -> expect UnknownResourceFault "Unknown execution: WorkflowExecution=[workflowId=<id>, runId=<id>]"; emulator returns the generic decision-task message.
Replay: swf.stale-decision-task-token-reused-rejected.

Cause 5 (TagResource over-limit message):
1. RegisterDomain; DescribeDomain -> domainInfo.arn.
2. TagResource(resourceArn=arn, tags=51 distinct tags) -> expect TooManyTagsFault with message exactly "Too many tags provided."; emulator says "The request carries 51 tags, above the maximum of 50 per resource".
3. RegisterDomain with 2 tags, then TagResource with 49 more (total 51) -> same expected message; emulator says "The domain would carry 51 tags, above the maximum of 50 per resource".
Replay: swf.tag-resource-exceeds-limit-rejected, swf.tag-resource-too-many-tags-rejected.

Cause 6 (UndeprecateDomain on a registered domain):
1. RegisterDomain(name=D, workflowExecutionRetentionPeriodInDays=1).
2. UndeprecateDomain(name=D) -> expect DomainAlreadyExistsFault whose message is exactly "D"; emulator returns "the domain D already exists".
3. DescribeDomain(D) -> status REGISTERED (already correct).
Replay: swf.undeprecate-active-domain-rejected.
