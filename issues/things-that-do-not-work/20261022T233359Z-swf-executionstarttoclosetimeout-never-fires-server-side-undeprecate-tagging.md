Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.execution-timeout-enforced-server-side swf.workflow-execution-timeout-terminates-open-execution swf.undeprecate-active-domain-rejected swf.undeprecate-active-workflow-type-rejected swf.tag-resource-exceeds-limit-rejected swf.list-tags-unknown-resource-rejected swf.signal-after-close-rejected swf.poll-for-decision-task-limit-exceeded-error swf.get-workflow-history-unknown-execution-error swf.duplicate-open-workflow-start-rejected
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.execution-timeout-enforced-server-side|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: swf.workflow-execution-timeout-terminates-open-execution|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: swf.undeprecate-active-domain-rejected|run#6|AssertionError|assert code == "DomainAlreadyExistsFault", (
Issue-Key: swf.undeprecate-active-workflow-type-rejected|run#7|AssertionError|assert code == "TypeAlreadyExistsFault", (
Issue-Key: swf.tag-resource-exceeds-limit-rejected|run#8|AssertionError|assert code == "TooManyTagsFault", f"expected TooManyTagsFault for 51 tags, got {code!r}"
Issue-Key: swf.list-tags-unknown-resource-rejected|run#10|AssertionError|assert code == "UnknownResourceFault", (
Issue-Key: swf.signal-after-close-rejected|run#12|AssertionError|assert code == "UnknownResourceFault", f"expected UnknownResourceFault signaling a closed execution, got {code!r}"
Issue-Key: swf.poll-for-decision-task-limit-exceeded-error|run#16|Timeout|found_limit.wait(timeout=max(0.0, deadline - time.monotonic()))
Issue-Key: swf.get-workflow-history-unknown-execution-error|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: swf.duplicate-open-workflow-start-rejected|run#11|AssertionError|assert message == "", f"expected an empty fault message, got {message!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# SWF: executionStartToCloseTimeout never fires server-side; Undeprecate*/tagging/signal rejection semantics missing; poller cap absent; two fault messages diverge (9 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartWorkflowExecution

## Description
Nine distinct root causes found replaying the behavioral suite against the SWF emulator (service token `swf`, bootstrapped). All nine belong to the SWF crate services/service-com-amazonaws-swf-simpleworkflowservice. Each is numbered and carries its own failing check ids; all nine must be closed before this issue is submitted.

=== Cause 1 of 9: executionStartToCloseTimeout is never enforced server-side ===
AWS: Smithy: StartWorkflowExecution.executionStartToCloseTimeout (DurationInSecondsOptional, @length 0-8) is the execution's start-to-close budget. When it elapses with no client action, SWF itself force-closes the execution: it appends exactly one WorkflowExecutionTimedOut history event whose workflowExecutionTimedOutEventAttributes.timeoutType is START_TO_CLOSE, DescribeWorkflowExecution then reports executionInfo.executionStatus=CLOSED with closeStatus=TIMED_OUT, and the execution becomes visible through ListClosedWorkflowExecutions with closeStatus=TIMED_OUT. Recorded behavior confirms this happens with no decider ever polling or responding.
Emulator: an execution started with executionStartToCloseTimeout="8" and then left completely unattended never closes. Both checks poll DescribeWorkflowExecution / GetWorkflowExecutionHistory for up to 90s and neither ever observes executionStatus=CLOSED nor a WorkflowExecutionTimedOut event; the checks are killed by the harness at 60s ("Check timed out after 60.0s"). No timer exists, so the execution stays OPEN indefinitely.
Why: a client that relies on SWF to reap abandoned executions sees them accumulate as OPEN forever. Deciders and reapers that wait for WorkflowExecutionTimedOut hang; the registered-execution/open-execution quota is never released; closed-execution visibility (ListClosedWorkflowExecutions with closeStatus TIMED_OUT) never reports the execution at all.
Fix: the execution state machine / timer wheel in the SWF crate's state module — a per-execution start-to-close deadline armed at StartWorkflowExecution that, on expiry, appends WorkflowExecutionTimedOut(timeoutType=START_TO_CLOSE) and transitions the execution to CLOSED/TIMED_OUT so DescribeWorkflowExecution and ListClosedWorkflowExecutions reflect it.
Traces: swf.execution-timeout-enforced-server-side, swf.workflow-execution-timeout-terminates-open-execution.

=== Cause 2 of 9: UndeprecateDomain answers 501 NotImplemented instead of rejecting an already-REGISTERED domain ===
AWS: Smithy: UndeprecateDomain(name: DomainName, required, @length 1-256) models errors DomainAlreadyExistsFault, OperationNotPermittedFault, UnknownResourceFault, each HTTP 400. DomainAlreadyExistsFault is documented as "Returned if the domain already exists. You may get this fault ... if you undeprecate a domain that is currently registered." So UndeprecateDomain on a REGISTERED domain is a 400 DomainAlreadyExistsFault, the domain stays REGISTERED, and the same call on a DEPRECATED domain succeeds and returns it to REGISTERED. Recorded behavior: the fault's message is the domain name itself (example only).
Emulator: the call returns error code 'NotImplemented' (501 scaffold stub), so neither the rejection nor the legitimate deprecate->undeprecate transition is reachable.
Why: a client cannot distinguish "already registered" from any other failure, and cannot restore a deprecated domain at all — SWF has no DeleteDomain, so undeprecation is the only way back.
Fix: implement the UndeprecateDomain handler and its status-guard validation in the SWF crate's ops module, over the domain registry's REGISTERED/DEPRECATED status. An "implement the stub" scaffold issue may exist for neighbouring SWF operations; this cause additionally states the required status-guard error code.
Traces: swf.undeprecate-active-domain-rejected.

=== Cause 3 of 9: UndeprecateWorkflowType answers 501 NotImplemented instead of rejecting an already-REGISTERED type ===
AWS: Smithy: UndeprecateWorkflowType(domain, workflowType, both required) models TypeAlreadyExistsFault, OperationNotPermittedFault, UnknownResourceFault, each HTTP 400. TypeAlreadyExistsFault is documented as "Returned if the type already exists in the specified domain ... or if you undeprecate a type that is currently registered." So the call on a REGISTERED workflow type is a 400 TypeAlreadyExistsFault, DescribeWorkflowType still reports typeInfo.status=REGISTERED, and the identical call after DeprecateWorkflowType succeeds and returns the type to REGISTERED. Recorded behavior: the fault message is built from the request as "WorkflowType=[name=<name>, version=<version>]" (that construction rule is the requirement, not any one recorded string).
Emulator: the call returns error code 'NotImplemented' (501 scaffold stub).
Why: the workflow-type deprecation lifecycle is one-way in the emulator, and the "already registered" rejection a client branches on is unobservable.
Fix: implement the UndeprecateWorkflowType handler and its status-guard validation in the SWF crate's ops module, over the workflow-type registry status.
Traces: swf.undeprecate-active-workflow-type-rejected.

=== Cause 4 of 9: TagResource does not enforce the 50-tag-per-resource limit ===
AWS: Smithy: TagResource(resourceArn, tags: ResourceTagList, both required) models TooManyTagsFault with an explicit @httpError 400 (not defaulted), documented "You've exceeded the number of tags allowed for a domain." Tagging a domain with more tags than the 50-per-resource limit (the check submits 51) is rejected with a 400 TooManyTagsFault and NO tags are applied — the rejected call must leave ListTagsForResource empty — while a small tag set is accepted and persists. Recorded message example: "Too many tags provided."
Emulator: TagResource returns error code 'NotImplemented' (501), so neither the over-limit rejection nor the accepted-and-persisted control path exists.
Why: a client that catches TooManyTagsFault to trim its tag set instead sees an unmodelled 501; worse, an implementation that later accepts the call without the limit would silently truncate tags, which is the overly-permissive failure this check guards.
Fix: the TagResource handler plus its tag-count validation and tag state in the SWF crate. A separate scaffold issue covers merely implementing the TagResource stub; that issue does not state the 50-tag limit or its TooManyTagsFault code, which is what this cause requires.
Traces: swf.tag-resource-exceeds-limit-rejected.

=== Cause 5 of 9: ListTagsForResource does not reject an ARN for a nonexistent domain ===
AWS: Smithy: ListTagsForResource(resourceArn, required, @length 1-1600) models UnknownResourceFault (HTTP 400), documented "Returned when the named resource cannot be found with in the scope of this operation (region or domain)." A well-formed domain ARN whose domain was never registered is rejected with 400 UnknownResourceFault; the message is built from the request as "Unknown domain: <domainName>" (same form DescribeDomain uses for the same nonexistent domain). The real domain's own ARN is listable and returns an empty tag list.
Emulator: the call returns error code 'NotImplemented' (501).
Why: callers cannot distinguish "this resource does not exist" from a transport/implementation failure, and tag-discovery code that treats a non-400 as retryable will spin.
Fix: the ListTagsForResource handler, its ARN->domain resolution and not-found classification in the SWF crate. A separate scaffold issue covers implementing the ListTagsForResource stub; it does not state this not-found classification or message rule.
Traces: swf.list-tags-unknown-resource-rejected.

=== Cause 6 of 9: SignalWorkflowExecution does not reject a signal to an already-CLOSED execution ===
AWS: Smithy: SignalWorkflowExecution(domain, workflowId, signalName required; runId, input optional) models only OperationNotPermittedFault and UnknownResourceFault, both HTTP 400. A signal aimed at an execution that has already closed (here, terminated via TerminateWorkflowExecution and confirmed CLOSED) is rejected with 400 UnknownResourceFault — a closed execution is out of scope for this operation — and NO WorkflowExecutionSignaled event is appended to that execution's history. The message is built from the request as "Unknown execution: WorkflowExecution=[workflowId=<id>, runId=<runId>]". Signalling a still-OPEN execution succeeds and records WorkflowExecutionSignaled with the given signalName and input.
Emulator: the call returns error code 'NotImplemented' (501), so the closed-execution rejection, the absence-of-effect guarantee, and the open-execution positive path are all unreachable.
Why: a decider that signals a race-losing execution expects UnknownResourceFault and treats it as "already finished"; an emulator that instead errors non-deterministically — or, once implemented without the state guard, accepts the signal and appends an event to a closed history — corrupts the client's view of the execution lifecycle.
Fix: the SignalWorkflowExecution handler and its open/closed state guard in the SWF crate. A separate scaffold issue covers implementing the SignalWorkflowExecution stub; it does not state the closed-execution UnknownResourceFault guard.
Traces: swf.signal-after-close-rejected.

=== Cause 7 of 9: PollForDecisionTask does not enforce the per-task-list poller cap ===
AWS: Smithy: PollForDecisionTask models LimitExceededFault (HTTP 400), documented "Returned by any operation if a system imposed limitation has been reached," and separately OperationNotPermittedFault, which is reserved for permission failures. The Amazon SWF Developer Guide ("Quotas on Task Executions") documents a maximum of 1,000 simultaneous pollers per task list; opening more concurrent long polls than that against a single task list makes the excess PollForDecisionTask calls fail promptly with LimitExceededFault specifically — never OperationNotPermittedFault.
Emulator: with 1,001 concurrent long polls open against one task list, no call ever returns LimitExceededFault; the check's wait for that code never completes and the check is killed at 60s. There is no per-task-list poller accounting at all. (PollForDecisionTask is additionally an unimplemented scaffold stub, which has its own filed issue; that issue covers standing the operation up but not this quota.)
Why: a client fleet that scales deciders relies on LimitExceededFault to back off; without it the emulator admits unbounded pollers, so the back-off path is never exercised and the over-quota behaviour clients must handle in production is invisible.
Fix: a per-(domain, task-list) concurrent-poller counter in the SWF crate's PollForDecisionTask handler/long-poll registry, rejecting admissions beyond the cap with LimitExceededFault (HTTP 400), not OperationNotPermittedFault.
Traces: swf.poll-for-decision-task-limit-exceeded-error.

=== Cause 8 of 9: UnknownResourceFault message for GetWorkflowExecutionHistory appends a domain clause AWS does not include ===
AWS: Smithy: GetWorkflowExecutionHistory(domain, execution required) models UnknownResourceFault (HTTP 400). For a (workflowId, runId) pair that identifies no execution in the domain, AWS returns 400 UnknownResourceFault whose message is built purely from the requested execution: "Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]". The message is a known, request-derived rule (same form used by SignalWorkflowExecution, cause 6), so the rule — not any recorded string — is the requirement.
Emulator: the error code is correct (UnknownResourceFault) but the message carries an extra trailing clause: "Unknown execution: WorkflowExecution=[workflowId=..., runId=...] in domain <domainName>".
Why: a client that matches the documented message form (or compares it against the form other SWF operations return for the same condition) mis-parses it; the emulator also leaks the domain into a message AWS scopes to the execution only, so message-equality assertions in client test suites fail against the emulator and pass against AWS.
Fix: the UnknownResourceFault message formatter used by the GetWorkflowExecutionHistory handler in the SWF crate — emit only the execution clause, consistently with the other execution-scoped operations.
Traces: swf.get-workflow-history-unknown-execution-error.

=== Cause 9 of 9: WorkflowExecutionAlreadyStartedFault carries a message where AWS returns an empty one ===
AWS: Smithy: StartWorkflowExecution models WorkflowExecutionAlreadyStartedFault (HTTP 400), documented "Returned by StartWorkflowExecution when an open execution with the same workflowId is already running in the specified domain." Recorded behavior only: AWS returns this particular fault with an EMPTY message — the fault code alone carries the meaning, and the duplicate start leaves exactly one OPEN execution behind, with the same workflowId reusable once that execution closes.
Emulator: the fault code is correct, but the message is populated with the colliding execution's identity, e.g. "WorkflowExecution=[workflowId=<id>, runId=<runId>]".
Why: lowest-impact of the nine — the status and code are right. It still diverges from the recorded envelope and leaks the existing run's runId to a caller AWS does not give it, which a client can come to depend on.
Fix: the WorkflowExecutionAlreadyStartedFault construction in the StartWorkflowExecution handler's duplicate-workflowId guard in the SWF crate — leave the message empty.
Traces: swf.duplicate-open-workflow-start-rejected.

## Evidence
Behavioral suite, 2/68 passed. All evidence below is the emulator's replayed response for a check in this suite.

Cause 1 — swf.execution-timeout-enforced-server-side and swf.workflow-execution-timeout-terminates-open-execution: expected a single WorkflowExecutionTimedOut event (timeoutType=START_TO_CLOSE) and DescribeWorkflowExecution executionStatus=CLOSED / closeStatus=TIMED_OUT within 90s of an 8s executionStartToCloseTimeout; actual: execution never closes, both checks killed with "Check timed out after 60.0s".

Cause 2 — swf.undeprecate-active-domain-rejected: AssertionError: expected DomainAlreadyExistsFault undeprecating a REGISTERED domain, got 'NotImplemented'. Expected HTTP 400 / DomainAlreadyExistsFault; actual 501 / NotImplemented.

Cause 3 — swf.undeprecate-active-workflow-type-rejected: AssertionError: expected TypeAlreadyExistsFault undeprecating a REGISTERED (not deprecated) workflow type, got 'NotImplemented'. Expected HTTP 400 / TypeAlreadyExistsFault; actual 501 / NotImplemented.

Cause 4 — swf.tag-resource-exceeds-limit-rejected: AssertionError: expected TooManyTagsFault for 51 tags, got 'NotImplemented'. Expected HTTP 400 / TooManyTagsFault (message example "Too many tags provided.") with no tags applied; actual 501 / NotImplemented.

Cause 5 — swf.list-tags-unknown-resource-rejected: AssertionError: ListTagsForResource on nonexistent domain ARN 'arn:aws:swf:<region>:<account>:/domain/<unregistered-domain>' returned error code 'NotImplemented', expected UnknownResourceFault. Expected HTTP 400 / UnknownResourceFault, message of the form "Unknown domain: <domainName>"; actual 501 / NotImplemented.

Cause 6 — swf.signal-after-close-rejected: AssertionError: expected UnknownResourceFault signaling a closed execution, got 'NotImplemented'. Expected HTTP 400 / UnknownResourceFault, message of the form "Unknown execution: WorkflowExecution=[workflowId=..., runId=...]", and no WorkflowExecutionSignaled event on the closed execution; actual 501 / NotImplemented.

Cause 7 — swf.poll-for-decision-task-limit-exceeded-error: expected at least one of 1,001 concurrent PollForDecisionTask long polls on one task list to be rejected with LimitExceededFault (and none with OperationNotPermittedFault); actual: LimitExceededFault never observed, check killed with "Check timed out after 60.0s".

Cause 8 — swf.get-workflow-history-unknown-execution-error: expected message 'Unknown execution: WorkflowExecution=[workflowId=<id>, runId=<runId>]', got 'Unknown execution: WorkflowExecution=[workflowId=<id>, runId=<runId>] in domain <domainName>'. Code UnknownResourceFault matched; the trailing " in domain <domainName>" clause is the divergence.

Cause 9 — swf.duplicate-open-workflow-start-rejected: AssertionError: expected an empty fault message, got 'WorkflowExecution=[workflowId=<id>, runId=<runId>]'. Code WorkflowExecutionAlreadyStartedFault matched.

## Steps to Reproduce
Boot the emulator and replay the SWF behavioral checks listed in trace_ids.

Cause 1: RegisterDomain; RegisterWorkflowType; StartWorkflowExecution with executionStartToCloseTimeout="8"; never poll or respond to any task; poll GetWorkflowExecutionHistory / DescribeWorkflowExecution for 90s -> no WorkflowExecutionTimedOut event and executionStatus stays OPEN (AWS: exactly one WorkflowExecutionTimedOut with timeoutType=START_TO_CLOSE, CLOSED/TIMED_OUT, and the execution listed by ListClosedWorkflowExecutions with closeStatus=TIMED_OUT).

Cause 2: RegisterDomain(name=D, workflowExecutionRetentionPeriodInDays="1"); UndeprecateDomain(name=D) -> 501 NotImplemented (AWS: 400 DomainAlreadyExistsFault, D still REGISTERED). Control: DeprecateDomain(D2), wait for DEPRECATED, UndeprecateDomain(D2) -> should succeed and read back REGISTERED.

Cause 3: RegisterDomain; RegisterWorkflowType(name=T, version="1.0"); UndeprecateWorkflowType(domain, {T,1.0}) -> 501 NotImplemented (AWS: 400 TypeAlreadyExistsFault, status still REGISTERED). Control: DeprecateWorkflowType then the identical UndeprecateWorkflowType -> should succeed.

Cause 4: RegisterDomain; DescribeDomain to get domainInfo.arn; TagResource(resourceArn=arn, tags=51 distinct tags) -> 501 NotImplemented (AWS: 400 TooManyTagsFault and ListTagsForResource still empty). Control: TagResource with 2 tags -> accepted and readable.

Cause 5: RegisterDomain; take its ARN and swap the domain segment for a name never registered; ListTagsForResource(resourceArn=fakeArn) -> 501 NotImplemented (AWS: 400 UnknownResourceFault, "Unknown domain: <name>"). Corroborate with DescribeDomain on the same unregistered name.

Cause 6: RegisterDomain; RegisterWorkflowType; StartWorkflowExecution; TerminateWorkflowExecution; poll DescribeWorkflowExecution until CLOSED; SignalWorkflowExecution on that closed execution -> 501 NotImplemented (AWS: 400 UnknownResourceFault, and no WorkflowExecutionSignaled event in its history). Control: signal a still-OPEN execution -> succeeds and records WorkflowExecutionSignaled with signalName/input.

Cause 7: RegisterDomain; open 1,001 concurrent PollForDecisionTask long polls against one task list from a client pool sized to hold them all -> collect the error codes seen; LimitExceededFault never appears (AWS: the excess calls fail promptly with LimitExceededFault, never OperationNotPermittedFault).

Cause 8: RegisterDomain; RegisterWorkflowType; StartWorkflowExecution to obtain a real runId; GetWorkflowExecutionHistory with that runId paired with a workflowId that was never started -> UnknownResourceFault whose message has the extra " in domain <domainName>" suffix.

Cause 9: RegisterDomain; RegisterWorkflowType; StartWorkflowExecution(workflowId=W); StartWorkflowExecution again with the same W while the first is OPEN -> WorkflowExecutionAlreadyStartedFault with a non-empty message (AWS: empty message).
