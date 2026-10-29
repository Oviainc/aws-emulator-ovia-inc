Issue-Source: trace-sweep-divergence
Issue-Trace: swf-visibility-apis-unregistered-domain-returns-zero
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: swf-visibility-apis-unregistered-domain-returns-zero swf-domain-errors swf-duplicate-registration-errors swf-register-duplicate-domain-error swf-undeprecate-active-domain-error swf-duplicate-start-workflow-error swf-unknown-domain-and-type-errors swf-error-paths-unknown-and-deprecated swf-unknown-workflow-execution-errors swf-list-open-workflow-executions-mutually-exclusive-filters-error swf-closed-workflow-count-and-list-after-terminate swf-open-executions-visibility-counts
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|CountPendingActivityTasks|2xx->4xx|$.count
Issue-Key: swf|CountPendingActivityTasks|2xx->4xx|$.truncated
Issue-Key: swf|CountPendingActivityTasks|2xx->4xx|status
Issue-Key: swf|CountPendingDecisionTasks|2xx->4xx|$.count
Issue-Key: swf|CountPendingDecisionTasks|2xx->4xx|$.truncated
Issue-Key: swf|CountPendingDecisionTasks|2xx->4xx|status
Issue-Key: swf|RegisterDomain|4xx->4xx|$.message
Issue-Key: swf|UndeprecateDomain|4xx->4xx|$.message
Issue-Key: swf|StartWorkflowExecution|4xx->4xx|$.workflowExecution
Issue-Key: swf|DescribeWorkflowType|4xx->4xx|$.message
Issue-Key: swf|DescribeWorkflowExecution|4xx->4xx|$.message
Issue-Key: swf|SignalWorkflowExecution|4xx->4xx|$.message
Issue-Key: swf|ListOpenWorkflowExecutions|4xx->4xx|$.message
Issue-Key: swf|StartWorkflowExecution|2xx->2xx|$.runId
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# swf: error responses carry the wrong status, code payload and message text

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CountPendingActivityTasks

## Description
This issue bundles SEVEN distinct root causes in the Amazon SWF emulator's failure paths, all grounded in wire-fixture divergences of the `swf` service replayed from simparity-wire-aws-v1. A fixer must close ALL SEVEN before submitting; each is numbered and carries its own fixture/step. Causes 1 and 2 overlap in subject matter with the open behavioral issue 20261029T184844Z-swf-six-fidelity-divergences-cancel-request-never-schedules.md (its Cause 2 and Cause 6), but that issue does not carry these wire replay keys, so the wire steps below are filed here and must be re-replayed to confirm the fix.

=== Cause 1 of 7: CountPendingActivityTasks / CountPendingDecisionTasks reject a never-registered domain with HTTP 400 instead of answering 200 count=0 ===
AWS: Smithy: CountPendingActivityTasks and CountPendingDecisionTasks both return PendingTaskCount {count (required, default 0, min 0), truncated (default false)} and both declare UnknownResourceFault (client, HTTP 400); recorded behavior shows AWS does NOT raise it for a missing domain — a task list named in a domain that was never registered answers HTTP 200 with count=0 and truncated=false, the same answer as an unused task list in a registered domain.
Emulator: both operations answer HTTP 400 UnknownResourceFault with the message "Unknown domain: <domain>" (fixed text "Unknown domain: " plus the request's `domain` value), and the body has no count/truncated at all.
Why: a client probing queue depth for a task list or a domain it has not provisioned gets a hard client error where AWS returns a usable zero, so auto-scaling and queue-depth polling loops fail on input AWS accepts.
Fix: the domain lookup in the CountPendingActivityTasks handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/count_pending_activity_tasks.rs) and the identical lookup in count_pending_decision_tasks.rs must treat an absent domain as holding no pending tasks and return 200 with count=0 / truncated=false.
Traces: swf-visibility-apis-unregistered-domain-returns-zero#task#3 (CountPendingActivityTasks), swf-visibility-apis-unregistered-domain-returns-zero#task#4 (CountPendingDecisionTasks).

=== Cause 2 of 7: DomainAlreadyExistsFault's message must be the domain name alone, for both RegisterDomain and UndeprecateDomain ===
AWS: Smithy: RegisterDomain and UndeprecateDomain both declare DomainAlreadyExistsFault (client, HTTP 400) — "Returned if the domain already exists. You may get this fault if you are registering a domain that is already registered or if you are undeprecating a domain that is currently registered." Recorded behavior: the fault's `message` is exactly the requested domain name and nothing else — the whole message is the variable part (the request's `name`), with no fixed text around it.
Emulator: right status (400) and right code, but the message is "the domain <name> already exists" — the name wrapped in invented fixed text — from both operations.
Why: callers that read this fault's message as the resource identifier (the AWS convention for DomainAlreadyExistsFault) parse a sentence instead of a name and mis-identify the domain.
Fix: the DomainAlreadyExistsFault constructions in the RegisterDomain handler and the UndeprecateDomain handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/register_domain.rs, undeprecate_domain.rs) must set the message to the requested domain name alone.
Traces: swf-domain-errors#task#3 (RegisterDomain; cluster also covers swf-duplicate-registration-errors#task#4 and swf-register-duplicate-domain-error#task#2), swf-domain-errors#task#5 (UndeprecateDomain; cluster also covers swf-undeprecate-active-domain-error#task#2).

=== Cause 3 of 7: WorkflowExecutionAlreadyStartedFault omits the workflowExecution identifying the run that is already open ===
AWS: Smithy: StartWorkflowExecution -> WorkflowExecutionAlreadyStartedFault (client, HTTP 400), "Returned by StartWorkflowExecution when an open execution with the same workflowId is already running in the specified domain." Recorded behavior: the 400 body carries, besides the code, a `workflowExecution` member holding the already-running execution's identity — {workflowId: the request's workflowId, runId: the open run's runId taken from the emulator's own state}.
Emulator: the 400 body has no `workflowExecution` member at all.
Why: a client that collides on a workflowId cannot learn which run is holding it, so it cannot describe, signal or terminate the incumbent execution without a separate visibility query — the fault exists precisely to supply that identity.
Fix: the duplicate-start rejection path of the StartWorkflowExecution handler (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/start_workflow_execution.rs) must populate the fault's workflowExecution from the open run it found, and the error serializer must emit the member.
Traces: swf-duplicate-start-workflow-error#task#4.

=== Cause 4 of 7: UnknownResourceFault names the DOMAIN where AWS names the unresolved leaf resource (workflow type / workflow execution) ===
AWS: Smithy: DescribeWorkflowType and DescribeWorkflowExecution both declare UnknownResourceFault (client, HTTP 400), "Returned when the named resource cannot be found with in the scope of this operation." Recorded behavior: AWS reports the leaf resource the request asked for, not the enclosing domain. DescribeWorkflowType answers `Unknown type: WorkflowType=[name=<name>, version=<version>]` — fixed text "Unknown type: WorkflowType=[name=", ", version=", "]" with name/version taken from the request's workflowType — and DescribeWorkflowExecution answers `Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]` — fixed text "Unknown execution: WorkflowExecution=[workflowId=", ", runId=", "]" with workflowId/runId taken from the request's execution.
Emulator: both operations answer with the message "Unknown domain: <domain>" (fixed text "Unknown domain: " plus the request's `domain`), i.e. its domain-existence guard reports first and the request's actual target is never named. Status 400 and the code match.
Why: a client cannot tell which resource was missing — it is told the domain is unknown even when the request's problem is an unregistered workflow type or a non-existent execution — so error handling that branches on "provision the type" vs "start the run" vs "create the domain" branches wrongly.
Fix: the resource-resolution error paths of the DescribeWorkflowType and DescribeWorkflowExecution handlers must render the unresolved target with AWS's own template (`Unknown type: WorkflowType=[name=..., version=...]` / `Unknown execution: WorkflowExecution=[workflowId=..., runId=...]`) instead of reporting the domain lookup.
Traces: swf-unknown-domain-and-type-errors#task#1 (DescribeWorkflowType), swf-error-paths-unknown-and-deprecated#task#4 (DescribeWorkflowExecution).

=== Cause 5 of 7: SignalWorkflowExecution's unknown-execution message uses the wrong template when no runId was supplied ===
AWS: Smithy: SignalWorkflowExecution -> UnknownResourceFault (client, HTTP 400). Recorded behavior: when the request names a workflowId with NO runId and no open execution has that workflowId, the message is `Unknown execution, workflowId = <workflowId>` — fixed text "Unknown execution, workflowId = " plus the request's workflowId. AWS uses the bracketed WorkflowExecution=[...] form only when a runId was actually addressed.
Emulator: answers `Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=]` — the bracketed template with an empty runId, which AWS never emits.
Why: clients matching the documented message to distinguish "no such workflowId" from "that specific run is gone" see a template AWS does not produce, and the trailing `runId=` renders a value that was never part of the request.
Fix: the unknown-execution rejection path of the SignalWorkflowExecution handler must select the message template by whether the request supplied a runId, emitting "Unknown execution, workflowId = <workflowId>" when it did not.
Traces: swf-unknown-workflow-execution-errors#task#3.

=== Cause 6 of 7: ListOpenWorkflowExecutions' mutually-exclusive-filter rejection uses emulator-invented prose ===
AWS: Smithy: ListOpenWorkflowExecutions takes executionFilter (WorkflowExecutionFilter), typeFilter (WorkflowTypeFilter) and tagFilter (TagFilter), of which at most one may be specified. Recorded behavior: specifying two of them is refused with HTTP 400 and the message `Cannot specify more than one exclusive filters in the same query: [WorkflowExecutionFilter, TagFilter]` — fixed text "Cannot specify more than one exclusive filters in the same query: [", the SHAPE names of the filters actually supplied (WorkflowExecutionFilter / WorkflowTypeFilter / TagFilter) comma-separated, then "]".
Emulator: same status but the message is "executionFilter, typeFilter, tagFilter are mutually exclusive; specify at most one of them in a request, got executionFilter and tagFilter" — invented prose naming the member names rather than AWS's bracketed shape-name list.
Why: a client validating against AWS's message (or surfacing it verbatim) sees text AWS never emits, and loses the bracketed machine-parsable list of the conflicting filter shapes.
Fix: the mutually-exclusive-filter validation in the ListOpenWorkflowExecutions handler (and the matching validation for ListClosedWorkflowExecutions) must render AWS's template with the supplied filters' shape names.
Traces: swf-list-open-workflow-executions-mutually-exclusive-filters-error#task#2.

=== Cause 7 of 7: StartWorkflowExecution's generated runId does not satisfy the recorded WorkflowRunId shape ===
AWS: Smithy: StartWorkflowExecution -> Run.runId (com.amazonaws.swf#WorkflowRunId, string, @length 1..64). The corpus asserts this value by SHAPE only (the recorded value itself is redacted as not-asserted, since a runId is generated per run and is never a requirement), and the emulator's runId fails that shape assertion.
Emulator: returns a 64-character lowercase-hex digest as runId, which the shape assertion rejects on the StartWorkflowExecution responses listed below while other steps' runIds pass — so the generator produces a runId outside the shape AWS's runIds occupy for these starts.
Why: clients and tooling that validate a runId's shape before persisting or re-sending it (or that round-trip it through a typed field) reject a value the emulator issues.
Fix: the runId generator used by the StartWorkflowExecution handler must produce identifiers in the shape AWS's WorkflowRunId occupies (within @length 1..64) consistently for every start. Note this is the weakest-grounded cause in this bundle: the assertion is on the id's shape, never on a recorded id value.
Traces: swf-closed-workflow-count-and-list-after-terminate#task#3 (cluster also covers swf-open-executions-visibility-counts#task#3 and #task#4).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 409 steps, 303 passed, 90 mismatched, 4 errored, 12 blocked, score 74.1%, emulator_up=true, bootstrap_signal=false. All 35 divergence clusters belong to service `swf`; none carries a stub_path or an owned_by.

Cause 1 — swf-visibility-apis-unregistered-domain-returns-zero#task#3, CountPendingActivityTasks: status: expected 200, got 400; $.count: missing (expected 0, got None); $.truncated: missing (expected False, got None). #task#4 is the identical divergence for CountPendingDecisionTasks. (The emulator's body is UnknownResourceFault "Unknown domain: <domain>".)

Cause 2 — swf-domain-errors#task#3, RegisterDomain, 400 vs 400: $.message: value_mismatch (expected '<domain name>', got 'the domain <domain name> already exists'). swf-domain-errors#task#5, UndeprecateDomain, 400 vs 400: the same mismatch — expected the bare domain name, got 'the domain <domain name> already exists'.

Cause 3 — swf-duplicate-start-workflow-error#task#4, StartWorkflowExecution, 400 vs 400: $.workflowExecution: missing (expected an object {runId: <the open run's runId>, workflowId: <the request's workflowId>}, got None).

Cause 4 — swf-unknown-domain-and-type-errors#task#1, DescribeWorkflowType, 400 vs 400: $.message: value_mismatch (expected 'Unknown type: WorkflowType=[name=<name>, version=1.0]', got 'Unknown domain: <domain>'). swf-error-paths-unknown-and-deprecated#task#4, DescribeWorkflowExecution, 400 vs 400: $.message: value_mismatch (expected 'Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]', got 'Unknown domain: <domain>').

Cause 5 — swf-unknown-workflow-execution-errors#task#3, SignalWorkflowExecution, 400 vs 400: $.message: value_mismatch (expected 'Unknown execution, workflowId = <workflowId>', got 'Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=]').

Cause 6 — swf-list-open-workflow-executions-mutually-exclusive-filters-error#task#2, ListOpenWorkflowExecutions, 400 vs 400: $.message: value_mismatch (expected 'Cannot specify more than one exclusive filters in the same query: [WorkflowExecutionFilter, TagFilter]', got 'executionFilter, typeFilter, tagFilter are mutually exclusive; specify at most one of them in a request, got executionFilter and tagFilter').

Cause 7 — swf-closed-workflow-count-and-list-after-terminate#task#3, StartWorkflowExecution, 200 vs 200: $.runId: value_mismatch (expected '<shaped-not-asserted>', got a 64-character lowercase-hex string) — a shape assertion, not a value assertion; same cluster for swf-open-executions-visibility-counts#task#3 and #task#4.

## Steps to Reproduce
Cause 1 (CountPending* on an unknown domain):
1. Pick a domain name that was never registered and any task-list name.
2. CountPendingActivityTasks(domain=<unregistered>, taskList={name:<any>}) -> AWS: 200 {count:0, truncated:false}; emulator: 400 UnknownResourceFault "Unknown domain: <domain>".
3. Repeat with CountPendingDecisionTasks -> same divergence.
Replay: swf-visibility-apis-unregistered-domain-returns-zero#task#3, swf-visibility-apis-unregistered-domain-returns-zero#task#4.

Cause 2 (DomainAlreadyExistsFault message):
1. RegisterDomain(name=D, workflowExecutionRetentionPeriodInDays=1).
2. RegisterDomain(name=D) again -> expect 400 DomainAlreadyExistsFault whose message is exactly "D"; emulator returns "the domain D already exists".
3. UndeprecateDomain(name=D) on the still-registered domain -> same expected message "D"; emulator returns the same invented sentence.
Replay: swf-domain-errors#task#3, swf-domain-errors#task#5.

Cause 3 (WorkflowExecutionAlreadyStartedFault payload):
1. RegisterDomain; RegisterWorkflowType; StartWorkflowExecution(workflowId=W) -> note the returned runId.
2. StartWorkflowExecution(workflowId=W) again while the first run is open -> expect 400 WorkflowExecutionAlreadyStartedFault whose body carries workflowExecution={workflowId:W, runId:<the open run's runId>}; the emulator's body omits workflowExecution.
Replay: swf-duplicate-start-workflow-error#task#4.

Cause 4 (unknown leaf resource vs unknown domain):
1. DescribeWorkflowType(domain=<any>, workflowType={name:N, version:V}) where the type cannot be resolved -> expect 400 UnknownResourceFault "Unknown type: WorkflowType=[name=N, version=V]"; emulator returns "Unknown domain: <domain>".
2. DescribeWorkflowExecution(domain=<any>, execution={workflowId:X, runId:Y}) where the execution cannot be resolved -> expect "Unknown execution: WorkflowExecution=[workflowId=X, runId=Y]"; emulator returns "Unknown domain: <domain>".
Replay: swf-unknown-domain-and-type-errors#task#1, swf-error-paths-unknown-and-deprecated#task#4.

Cause 5 (SignalWorkflowExecution without runId):
1. RegisterDomain; RegisterWorkflowType.
2. SignalWorkflowExecution(domain, workflowId=<never started>, signalName=s) with NO runId -> expect 400 UnknownResourceFault "Unknown execution, workflowId = <workflowId>"; emulator returns "Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=]".
Replay: swf-unknown-workflow-execution-errors#task#3.

Cause 6 (mutually exclusive filters):
1. RegisterDomain.
2. ListOpenWorkflowExecutions(domain, startTimeFilter=..., executionFilter={workflowId:W}, tagFilter={tag:T}) -> expect 400 with message "Cannot specify more than one exclusive filters in the same query: [WorkflowExecutionFilter, TagFilter]"; emulator returns its own prose naming the input member names.
Replay: swf-list-open-workflow-executions-mutually-exclusive-filters-error#task#2.

Cause 7 (runId shape):
1. RegisterDomain; RegisterWorkflowType; StartWorkflowExecution.
2. Validate the returned runId against the recorded WorkflowRunId shape (@length 1..64) -> the emulator's 64-char lowercase-hex runId fails the corpus's shape assertion on these starts.
Replay: swf-closed-workflow-count-and-list-after-terminate#task#3.
