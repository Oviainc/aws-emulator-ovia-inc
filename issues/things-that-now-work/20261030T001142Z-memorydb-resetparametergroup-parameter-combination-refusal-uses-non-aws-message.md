Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-parameter-group-lifecycle
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-parameter-group-lifecycle memorydb-parameter-update-and-reset-rules
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|ResetParameterGroup|4xx->4xx|$.message
Issue-Commit: 1d9465ca659a2c5547587e4da8977c83baa0074e

# ResetParameterGroup parameter-combination refusal uses non-AWS message wording

> _Note: this issue was identified against the code base at commit `1d9465ca659a2c5547587e4da8977c83baa0074e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResetParameterGroup

## Description
Replay of the memorydb wire corpus produced one net-new divergence cluster (5 mismatched steps across 2 fixtures). 1 CAUSE, which must be closed before this issue is resolved.

Cause 1 of 1: ResetParameterGroup - the refusal that fires when AllParameters is requested together with an explicit ParameterNames list answers the right status and error class but the wrong message text. Fixture memorydb-parameter-group-lifecycle step task#13 (the cluster also covers memorydb-parameter-update-and-reset-rules#task#9, #task#10, #task#11, #task#12).

AWS: Smithy: ResetParameterGroup takes ParameterGroupName (required), AllParameters (boolean, default false) and ParameterNames (ParameterNameList), and declares InvalidParameterCombinationException (awsQueryError code InvalidParameterCombination, httpError 400, documented "The specified parameter combination is not valid."). Supplying both the reset-everything flag and a list of individual parameters is exactly that invalid combination, and AWS answers HTTP 400 with $.message exactly "May not specify both ResetAllParameters and a list of parameters to reset." That sentence is entirely FIXED text - there is no variable part: no group name, no parameter names, no count is interpolated, and AWS words the flag with its legacy public name "ResetAllParameters" rather than the wire member name AllParameters.

Emulator: HTTP 400 (status matches) with $.message "ParameterNames must not be specified when AllParameters is true." - emulator-invented wording that names the two wire members and inverts the sentence form. Every step in the cluster diverges on $.message alone; the status and the response envelope are correct.

Why: the message is part of the error-class contract. A client or deployment test that branches on, logs, or asserts MemoryDB's documented parameter-combination text never matches, so a caller that sent the illegal combination is shown text AWS never emits and cannot distinguish this refusal from other InvalidParameterCombination refusals by message. It also tells the caller to drop ParameterNames when AWS's wording tells it the two inputs are mutually exclusive.

Fix: ResetParameterGroup's input validation (the mutual-exclusion check between AllParameters and ParameterNames) in the operation's validation layer - replace the invented sentence with AWS's fixed text; no state or serializer change is needed.

Traces: memorydb-parameter-group-lifecycle#task#13; memorydb-parameter-update-and-reset-rules#task#9, #task#10, #task#11, #task#12.

Related but NOT covering this: open issue 20261029T174443Z-memorydb-error-messages-and-parameter-catalog-diverge-from.md bundles eight other memorydb message/catalog causes, and it carries no replay key for ResetParameterGroup, so this cause is unowned by it.

## Evidence
replay_wire_corpus (trace set simparity-wire-aws-v1): 233 steps, 156 passed, 77 mismatched, score 67.0%, emulator_up=true, bootstrap_signal=false. 9 divergence clusters, all service=memorydb; 8 carry owned_by pointing at 20261029T174443Z-memorydb-error-messages-and-parameter-catalog-diverge-from.md, and this one carries neither stub_path nor owned_by.

Cluster divergence_key: service=memorydb|ResetParameterGroup|status=400->400|keys=$.message
Representative: fixture memorydb-parameter-group-lifecycle, step task#13; affected_count 5 (memorydb-parameter-group-lifecycle#task#13, memorydb-parameter-update-and-reset-rules#task#9, #task#10, #task#11, #task#12).
expected_status 400, actual_status 400.
detail: $.message: value_mismatch (expected 'May not specify both ResetAllParameters and a list of parameters to reset.', got 'ParameterNames must not be specified when AllParameters is true.')

Expected error code/status: InvalidParameterCombination / HTTP 400 (Smithy: ResetParameterGroup errors include InvalidParameterCombinationException, awsQueryError code InvalidParameterCombination, httpResponseCode 400). Expected message fixed text, in full: "May not specify both ResetAllParameters and a list of parameters to reset." - no part of it is taken from the request or from emulator state.

Smithy evidence cited: ResetParameterGroupRequest members ParameterGroupName (String, required), AllParameters (Boolean, default false), ParameterNames (ParameterNameList); errors InvalidParameterCombinationException, InvalidParameterGroupStateFault, InvalidParameterValueException, ParameterGroupNotFoundFault, ServiceLinkedRoleNotFoundFault. The error's only member is `message` (AwsQueryErrorMessage) and carries no trait constraining its text, so the recorded sentence is the requirement.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus); the cluster reproduces deterministically on all 5 steps.
3. Cause 1: replay memorydb-parameter-group-lifecycle step task#13 - call ResetParameterGroup on an existing (non-default) parameter group with AllParameters=true AND a non-empty ParameterNames list.
4. Read the response: status is 400 as expected, but $.message is 'ParameterNames must not be specified when AllParameters is true.' instead of AWS's 'May not specify both ResetAllParameters and a list of parameters to reset.'
5. Confirm the same wording on memorydb-parameter-update-and-reset-rules steps task#9, task#10, task#11 and task#12, which exercise the same refusal with different group/parameter inputs - the message must be identical fixed text in every case.
