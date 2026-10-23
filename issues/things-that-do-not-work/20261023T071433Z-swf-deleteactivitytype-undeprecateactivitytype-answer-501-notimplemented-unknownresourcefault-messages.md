Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.delete-non-deprecated-activity-type-rejected swf.undeprecate-active-activity-type-rejected swf.describe-activity-type-cross-domain-rejected swf.request-cancel-unknown-execution-error swf.request-cancel-unknown-execution-rejected swf.operation-on-deprecated-domain-still-succeeds
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.delete-non-deprecated-activity-type-rejected|run#7|AssertionError|assert code == "TypeNotDeprecatedFault", (
Issue-Key: swf.undeprecate-active-activity-type-rejected|run#7|AssertionError|assert code == "TypeAlreadyExistsFault", f"expected TypeAlreadyExistsFault, got {code!r}"
Issue-Key: swf.describe-activity-type-cross-domain-rejected|run#14|AssertionError|assert message == f"Unknown type: ActivityType=[name={activity_name}, version={version}]", message
Issue-Key: swf.request-cancel-unknown-execution-error|run#24|AssertionError|assert describe_message == expected, f"expected message {expected!r}, got {describe_message!r}"
Issue-Key: swf.request-cancel-unknown-execution-rejected|run#23|AssertionError|assert describe_message == expected, f"expected message {expected!r}, got {describe_message!r}"
Issue-Key: swf.operation-on-deprecated-domain-still-succeeds|run#17|AssertionError|assert type_info["typeInfo"]["status"] == "REGISTERED", (
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# SWF: DeleteActivityType/UndeprecateActivityType answer 501 NotImplemented; UnknownResourceFault messages append a domain clause; DeprecateDomain cascade-deprecates workflow types (4 causes)

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteActivityType

## Description
Four distinct net-new root causes found replaying the behavioral suite against the SWF emulator (service token `swf`, bootstrapped). All four live in the SWF crate services/service-com-amazonaws-swf-simpleworkflowservice. Each cause is numbered below and carries its own failing check ids; ALL FOUR must be closed before this issue is submitted.

None of these is covered by an existing open issue. The open issue 20261022T233359Z-swf-executionstarttoclosetimeout-never-fires-server-side-undeprecate-tagging.md covers the *Domain* and *WorkflowType* undeprecate stubs, the GetWorkflowExecutionHistory message suffix, the TagResource limit, ListTagsForResource, SignalWorkflowExecution, the poller cap and the execution timeout — it does NOT name DeleteActivityType, UndeprecateActivityType, DescribeWorkflowExecution/DescribeActivityType message formatting, or DeprecateDomain's effect on type status. The scaffold-stub issues already filed in this repo cover PollForDecisionTask, SignalWorkflowExecution, CountPendingDecisionTasks, CountPendingActivityTasks, DeleteWorkflowType, DeprecateActivityType, UndeprecateDomain, UndeprecateWorkflowType and ValidateTemplate — again not the two activity-type operations below.

=== Cause 1 of 4: DeleteActivityType answers 501 NotImplemented instead of refusing a never-deprecated type with TypeNotDeprecatedFault ===
AWS: Smithy: DeleteActivityType(domain: DomainName @length 1-256 required, activityType: ActivityType required) -> Unit, and models exactly OperationNotPermittedFault, TypeNotDeprecatedFault and UnknownResourceFault, each HTTP 400. TypeNotDeprecatedFault doc: "Returned when the resource type has not been deprecated." The operation doc states "Prior to deletion, activity types must first be deprecated." So DeleteActivityType on a type that is still REGISTERED is a 400 TypeNotDeprecatedFault, the type survives (DescribeActivityType still reports typeInfo.status=REGISTERED), and the identical delete after DeprecateActivityType succeeds. Recorded message example: "The type is currently registered and cannot be deleted in its current state" (example only; the status and the error code are the requirement).
Emulator: the call answers error code 'NotImplemented' (HTTP 501 scaffold stub — services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/delete_activity_type.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() returns `Scaffolded::not_implemented("DeleteActivityType")`), so neither the state guard nor the legitimate deprecate-then-delete path is reachable.
Why: a client that catches TypeNotDeprecatedFault to drive the deprecate->delete lifecycle instead sees an unmodelled 501 it cannot classify; retry logic treats a 501 as a server fault and spins, and activity types can never be removed from a domain at all.
Fix: implement the DeleteActivityType handler and its status guard in the SWF crate's ops module (see Traces for the file), over the activity-type registry's REGISTERED/DEPRECATED status held in the crate's state module.
Traces: swf.delete-non-deprecated-activity-type-rejected.

=== Cause 2 of 4: UndeprecateActivityType answers 501 NotImplemented instead of rejecting an already-REGISTERED activity type with TypeAlreadyExistsFault ===
AWS: Smithy: UndeprecateActivityType(domain required @length 1-256, activityType required) -> Unit, models OperationNotPermittedFault, TypeAlreadyExistsFault and UnknownResourceFault, each HTTP 400. TypeAlreadyExistsFault doc: "Returned if the type already exists in the specified domain. You may get this fault if you are registering a type that is either already registered or deprecated, or if you undeprecate a type that is currently registered." So undeprecating a currently-REGISTERED activity type is a 400 TypeAlreadyExistsFault, the status stays REGISTERED, and the same call after DeprecateActivityType succeeds and returns the type to REGISTERED. Recorded message is built from the request as "ActivityType=[name=<name>, version=<version>]" — that construction rule is the requirement, not any one recorded string.
Emulator: the call answers error code 'NotImplemented' (HTTP 501 scaffold stub), so both the rejection and the deprecate->undeprecate round trip are unreachable; the activity-type deprecation lifecycle is one-way.
Why: a client branching on TypeAlreadyExistsFault to tell "already active" from "restored" cannot do so, and a deprecated activity type can never be brought back into service.
Fix: implement the UndeprecateActivityType handler and its status-guard validation in the SWF crate's ops module, over the same activity-type registry status as cause 1.
Traces: swf.undeprecate-active-activity-type-rejected.

=== Cause 3 of 4: UnknownResourceFault messages append a " in domain <domainName>" clause AWS does not emit (DescribeWorkflowExecution, DescribeActivityType) ===
AWS: Smithy: DescribeWorkflowExecution(domain, execution required) and DescribeActivityType(domain, activityType required) each model UnknownResourceFault (HTTP 400), doc: "Returned when the named resource cannot be found with in the scope of this operation (region or domain)." For a (workflowId, runId) that names no execution, the message is built purely from the requested execution: "Unknown execution: WorkflowExecution=[workflowId=<workflowId>, runId=<runId>]". For an activity type looked up through a domain it was not registered in, it is built purely from the requested type: "Unknown type: ActivityType=[name=<name>, version=<version>]". These are known, request-derived message rules (the same form the service uses for the identical condition on other operations), so the rule — not a recorded string — is the requirement; the domain is never part of the clause.
Emulator: the error code and status are correct (400 UnknownResourceFault) but both formatters append a trailing clause, e.g. "Unknown execution: WorkflowExecution=[workflowId=..., runId=bogus-run-id] in domain <domainName>" and "Unknown type: ActivityType=[name=..., version=1.0] in domain <domainName>".
Why: clients and client test suites that match the documented request-derived message form mis-parse it, and the emulator leaks the domain into a message AWS scopes to the execution/type only — so assertions that pass against AWS fail against the emulator. This is the same formatter defect already filed for GetWorkflowExecutionHistory (cause 8 of 20261022T233359Z-swf-executionstarttoclosetimeout-never-fires-server-side-undeprecate-tagging.md); that issue names only GetWorkflowExecutionHistory, so these two operations are still open and should be fixed with the shared formatter.
Fix: the UnknownResourceFault message formatter used by the DescribeWorkflowExecution and DescribeActivityType handlers in the SWF crate — emit only the execution/type clause, consistently across every execution- and type-scoped operation.
Traces: swf.request-cancel-unknown-execution-error, swf.request-cancel-unknown-execution-rejected (both reach the DescribeWorkflowExecution assertion after the RequestCancelWorkflowExecution code and message already matched), swf.describe-activity-type-cross-domain-rejected.

=== Cause 4 of 4: DeprecateDomain cascade-deprecates the domain's workflow types ===
AWS: Smithy: DeprecateDomain(name: DomainName required @length 1-256) -> Unit, models DomainDeprecatedFault, OperationNotPermittedFault, UnknownResourceFault. The model gives DeprecateDomain no effect on registered type status, and recorded behavior (verified against real AWS by this check) is explicit: after DeprecateDomain lands — confirmed via DescribeDomain reporting status=DEPRECATED — a workflow type registered BEFORE the deprecation still reports typeInfo.status=REGISTERED from DescribeWorkflowType; there is no cascade-deprecation of types, no rejection of new RegisterWorkflowType into the deprecated domain, and no rejection of new StartWorkflowExecution within the observed window.
Emulator: DescribeWorkflowType for the pre-existing type reports status 'DEPRECATED' immediately after the domain is deprecated — the domain's deprecation is propagated onto its workflow types.
Why: type status is a value clients branch on. A workflow type that flips to DEPRECATED on its own makes an emulator-backed client believe the type was retired, so it will not schedule against it and will attempt to undeprecate a type AWS still reports as REGISTERED; the deprecated-domain lifecycle is modelled wrongly in the emulator's state, which also makes any later per-type status check unreliable.
Fix: the DeprecateDomain handler and the domain/type status model in the SWF crate's state module — deprecating a domain must set only the domain's own status and must leave every activity-type and workflow-type status in that domain untouched (DescribeWorkflowType/DescribeActivityType read the type's own status, not the domain's).
Traces: swf.operation-on-deprecated-domain-still-succeeds.

## Evidence
Behavioral suite: 8/68 passed. Every line below is the emulator's replayed response for a check in this suite.

Cause 1 — swf.delete-non-deprecated-activity-type-rejected [mismatch]: AssertionError: "deleting a never-deprecated activity type must be refused with TypeNotDeprecatedFault; got 'NotImplemented'". Expected HTTP 400 / TypeNotDeprecatedFault (message example "The type is currently registered and cannot be deleted in its current state") with the type surviving as REGISTERED; actual HTTP 501 / NotImplemented ("operation DeleteActivityType is scaffolded but not implemented in this emulator").

Cause 2 — swf.undeprecate-active-activity-type-rejected [mismatch]: AssertionError: "expected TypeAlreadyExistsFault, got 'NotImplemented'". Expected HTTP 400 / TypeAlreadyExistsFault with a message of the form "ActivityType=[name=<name>, version=<version>]" and status unchanged at REGISTERED; actual HTTP 501 / NotImplemented.

Cause 3 — swf.request-cancel-unknown-execution-error and swf.request-cancel-unknown-execution-rejected [mismatch]: AssertionError: expected message 'Unknown execution: WorkflowExecution=[workflowId=<id>, runId=bogus-run-id]', got 'Unknown execution: WorkflowExecution=[workflowId=<id>, runId=bogus-run-id] in domain <domainName>'. The code UnknownResourceFault matched; the trailing " in domain <domainName>" clause is the divergence.
swf.describe-activity-type-cross-domain-rejected [mismatch]: AssertionError: expected 'Unknown type: ActivityType=[name=<name>, version=1.0]', got 'Unknown type: ActivityType=[name=<name>, version=1.0] in domain <domainName>'. Code UnknownResourceFault matched.

Cause 4 — swf.operation-on-deprecated-domain-still-succeeds [mismatch]: AssertionError: "expected pre-existing workflow type {'name': '<wt>', 'version': '1.0'} to stay REGISTERED after the domain was deprecated, got 'DEPRECATED'". Expected DescribeWorkflowType typeInfo.status=REGISTERED after DeprecateDomain; actual DEPRECATED.

## Steps to Reproduce
Boot the emulator and replay the SWF behavioral checks listed in trace_ids.

Cause 1: RegisterDomain(name=D, workflowExecutionRetentionPeriodInDays="1"); RegisterActivityType(domain=D, name=A, version="1.0"); DeleteActivityType(domain=D, activityType={A,1.0}) -> 501 NotImplemented (AWS: 400 TypeNotDeprecatedFault, and DescribeActivityType still reports REGISTERED). Control: DeprecateActivityType(domain=D, activityType={A,1.0}) then the identical DeleteActivityType -> should succeed.

Cause 2: RegisterDomain(D); RegisterActivityType(domain=D, name=A, version="1.0"); UndeprecateActivityType(domain=D, activityType={A,1.0}) -> 501 NotImplemented (AWS: 400 TypeAlreadyExistsFault, message of the form "ActivityType=[name=A, version=1.0]", status still REGISTERED). Control: register a second type, DeprecateActivityType it, then UndeprecateActivityType it -> DescribeActivityType should read back REGISTERED.

Cause 3: (a) RegisterDomain(D); RegisterWorkflowType; StartWorkflowExecution to obtain a real execution; then DescribeWorkflowExecution(domain=D, execution={workflowId=<never-started-id>, runId="bogus-run-id"}) -> UnknownResourceFault whose message carries the extra " in domain D" suffix. (b) RegisterDomain(home) and RegisterDomain(other); RegisterActivityType into `home`; DescribeActivityType(domain=other, activityType={same name, version}) -> UnknownResourceFault whose message carries the extra " in domain other" suffix. In both cases AWS emits only the execution/type clause.

Cause 4: RegisterDomain(D, retention="1"); RegisterWorkflowType(domain=D, name=T, version="1.0", defaults set); StartWorkflowExecution against T (control: OPEN); DeprecateDomain(name=D); poll DescribeDomain until domainInfo.status=DEPRECATED; DescribeWorkflowType(domain=D, workflowType={T,1.0}) -> typeInfo.status is 'DEPRECATED' (AWS: still 'REGISTERED'). Also expected to keep working against the deprecated domain: RegisterWorkflowType of a brand-new type and StartWorkflowExecution of the pre-existing one.
