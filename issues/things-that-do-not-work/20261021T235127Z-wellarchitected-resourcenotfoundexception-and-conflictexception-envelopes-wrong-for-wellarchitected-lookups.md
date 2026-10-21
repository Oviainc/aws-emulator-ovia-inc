Issue-Source: trace-sweep-divergence
Issue-Trace: wellarchitected-resource-notfound-envelopes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wellarchitected-resource-notfound-envelopes wellarchitected-workload-lifecycle wellarchitected-workload-name-conflict
Issue-Shape-Id: com.amazonaws.wellarchitected#WellArchitectedApiServiceLambda
Issue-Key: wellarchitected|GetWorkload|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetWorkload|4xx->4xx|$.ResourceId
Issue-Key: wellarchitected|GetWorkload|4xx->4xx|$.ResourceType
Issue-Key: wellarchitected|GetWorkload|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetWorkload|4xx->4xx|status
Issue-Key: wellarchitected|GetMilestone|4xx->4xx|$.Message
Issue-Key: wellarchitected|GetMilestone|4xx->4xx|$.ResourceId
Issue-Key: wellarchitected|GetMilestone|4xx->4xx|$.ResourceType
Issue-Key: wellarchitected|GetMilestone|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wellarchitected|GetMilestone|4xx->4xx|status
Issue-Key: wellarchitected|DeleteWorkload|4xx->4xx|$.Message
Issue-Related: 20261021T235127Z-wellarchitected-validationexception-envelope-and-input-validation-diverge-across-wellarchitected.md
Issue-Commit: 5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9

# ResourceNotFoundException and ConflictException envelopes wrong for wellarchitected lookups

> _Note: this issue was identified against the code base at commit `5939f31bbed4b1d4ad1ad30abce57388bc2dc3a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkload

## Description
The emulator's non-validation error envelopes for wellarchitected diverge: the 404 bodies it does produce carry a message that does not follow AWS's form, and when the requested id fails the modelled WorkloadId pattern it answers 400 ValidationException instead of a 404 ResourceNotFoundException, so the ResourceId/ResourceType members a client needs are never emitted. Smithy: ResourceNotFoundException is @error("client") @httpError(404) with REQUIRED members Message, ResourceId and ResourceType; ConflictException is @error("client") @httpError(409) with the same three required members. GetWorkload, DeleteWorkload, GetMilestone and CreateWorkload all bind both errors. 5 causes, all in this service.

Cause 1 of 5 — GetWorkload on an unknown workload id: not-found message is not built AWS's way (representative cause).
AWS: a well-formed but unknown WorkloadId yields 404 ResourceNotFoundException whose Message is built from the request and the account — it names the workload id that was looked up and the account it was looked up in (recorded example: '[ResourceNotFound] No workload with ID 00000000000000000000000000000000 was found in account 000000000000.').
Emulator: 404 (correct) but Message is 'No workload exists with id <id>' — it never names the account, so the message is not produced by AWS's rule.
Why: the message rule here is request-derived (id + owning account), and operators and tooling read the account out of it to distinguish 'wrong account / not shared with me' from 'deleted'; the emulator's form drops that signal.
Fix: the shared ResourceNotFoundException constructor used by the workload lookup handlers.
Traces: wellarchitected-resource-notfound-envelopes#task#1, and the same cluster member wellarchitected-workload-lifecycle#task#6.

Cause 2 of 5 — GetWorkload on a malformed workload id answers 400 instead of 404.
AWS: recorded behavior shows a WorkloadId that does not satisfy @pattern("^[0-9a-f]{32}$") / @length(32,32) is still answered as 404 ResourceNotFoundException with ResourceId set to the id as sent, ResourceType 'WORKLOAD' and a Message naming id and account — the lookup path, not the validation path, owns this case.
Emulator: 400 with x-amzn-errortype 'ValidationException' and no ResourceId, ResourceType or Message at all.
Why: the status itself is wrong (400 vs 404) and the error type differs, so a client's not-found branch (catching ResourceNotFoundException to treat the workload as absent) raises a validation error instead and the operation aborts.
Fix: GetWorkload handler — resolve unknown/unparseable path ids to ResourceNotFoundException rather than rejecting them in input validation.
Traces: wellarchitected-resource-notfound-envelopes#task#2.

Cause 3 of 5 — GetMilestone on a missing milestone answers 400 instead of 404.
AWS: recorded behavior shows 404 ResourceNotFoundException with ResourceType 'MILESTONE', ResourceId formed as '<workloadId>.<milestoneNumber>' and a Message naming the milestone number, the workload id and the account (example: '[ResourceNotFound] No milestone with number 0 in a workload with ID 00000000000000000000000000000000 was found in account 000000000000.'). Smithy GetMilestone.MilestoneNumber is @range(min 1, max 100), yet the recorded service answers the lookup error.
Emulator: 400 with x-amzn-errortype 'ValidationException' and no ResourceId/ResourceType/Message.
Why: same as cause 2 — wrong status and wrong error shape for a missing milestone, and the composite ResourceId that identifies which milestone was missing is never emitted.
Fix: GetMilestone handler — milestone resolution must raise ResourceNotFoundException with the composite ResourceId.
Traces: wellarchitected-resource-notfound-envelopes#task#3.

Cause 4 of 5 — DeleteWorkload on an unknown workload id: same malformed not-found message.
AWS: 404 ResourceNotFoundException with the id/account Message rule described in cause 1.
Emulator: 404 with 'No workload exists with id <id>'.
Fix: same shared ResourceNotFoundException constructor as cause 1, reached from DeleteWorkload.
Traces: wellarchitected-resource-notfound-envelopes#task#4.

Cause 5 of 5 — CreateWorkload duplicate name: ConflictException message is not built AWS's way.
AWS: creating a second workload with an existing name yields 409 ConflictException whose Message is built from the request's workload name (recorded example: '[Conflict] A workload with name sim2real-probe-wa-conflict-2f8d6a05 already exists.').
Emulator: 409 (correct) with 'A workload named "<name>" already exists in this account and Region' — a different construction of the same request-derived message.
Why: the message rule is known (it is composed from the submitted WorkloadName), so clients and fixtures that match on AWS's form do not recognise the conflict; the status and error type are already correct.
Fix: CreateWorkload's duplicate-name check / ConflictException constructor.
Traces: wellarchitected-workload-name-conflict#task#2, and the same cluster member wellarchitected-workload-name-conflict#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, no state-reset failures).

Cause 1 wellarchitected-resource-notfound-envelopes#task#1 GetWorkload (cluster also covers wellarchitected-workload-lifecycle#task#6): expected 404 / actual 404 — $.Message: value_mismatch (expected '[ResourceNotFound] No workload with ID 00000000000000000000000000000000 was found in account 000000000000.', got 'No workload exists with id 00000000000000000000000000000000').
Cause 2 wellarchitected-resource-notfound-envelopes#task#2 GetWorkload: status expected 404, got 400 — $.ResourceId: missing (expected 'not-a-workload-id'); $.ResourceType: missing (expected 'WORKLOAD'); $.Message: missing (expected '[ResourceNotFound] No workload with ID not-a-workload-id was found in account 000000000000.'); $headers.x-amzn-errortype: got 'ValidationException'.
Cause 3 wellarchitected-resource-notfound-envelopes#task#3 GetMilestone: status expected 404, got 400 — $.ResourceId: missing (expected '00000000000000000000000000000000.0'); $.ResourceType: missing (expected 'MILESTONE'); $.Message: missing (expected '[ResourceNotFound] No milestone with number 0 in a workload with ID 00000000000000000000000000000000 was found in account 000000000000.'); $headers.x-amzn-errortype: got 'ValidationException'.
Cause 4 wellarchitected-resource-notfound-envelopes#task#4 DeleteWorkload: 404/404 — $.Message: value_mismatch (expected '[ResourceNotFound] No workload with ID 00000000000000000000000000000000 was found in account 000000000000.', got 'No workload exists with id 00000000000000000000000000000000').
Cause 5 wellarchitected-workload-name-conflict#task#2 CreateWorkload (cluster also covers #task#3): 409/409 — $.Message: value_mismatch (expected '[Conflict] A workload with name sim2real-probe-wa-conflict-2f8d6a05 already exists.', got 'A workload named "sim2real-probe-wa-conflict-2f8d6a05" already exists in this account and Region').

The statuses (404, 409), the error types and the required ResourceId/ResourceType members are requirements; the quoted strings are examples of messages built from the request (looked-up id, milestone number, workload name) plus the owning account.

## Steps to Reproduce
1. Boot the emulator from the checkout (wellarchitected is bootstrapped) and replay trace set simparity-wire-aws-v1.
2. Representative cause: fixture wellarchitected-resource-notfound-envelopes, step task#1 — GET /workloads/00000000000000000000000000000000 for an id that does not exist; the emulator returns 404 with a message that does not name the account.
3. Replay the other steps in trace_ids: wellarchitected-resource-notfound-envelopes#task#2 (GET /workloads/not-a-workload-id -> 400 instead of 404), #task#3 (GET /workloads/{id}/milestones/0 -> 400 instead of 404), #task#4 (DELETE /workloads/{unknown}), and wellarchitected-workload-name-conflict#task#2 (POST /workloads twice with the same WorkloadName -> 409).
4. All five causes must pass before this issue is closed.
