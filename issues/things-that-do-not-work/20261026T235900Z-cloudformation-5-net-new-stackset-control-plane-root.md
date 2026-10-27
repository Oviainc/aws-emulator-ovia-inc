Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.detect-stackset-drift-while-in-progress-rejected cloudformation.concurrent-stack-instance-update-rejected cloudformation.stack-set-operation-single-flight-sequencing cloudformation.stackset-concurrent-operation-rejected cloudformation.invalid-operation-error-org-and-stackset-rejected cloudformation.invalid-parameter-combination-stackset-ops-rejected cloudformation.stackset-deployment-targets-self-managed-rejected cloudformation.update-stack-set-invalid-region-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.detect-stackset-drift-while-in-progress-rejected|run#22|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: cloudformation.concurrent-stack-instance-update-rejected|run#19|AssertionError|assert code_update == "OperationInProgressException", (
Issue-Key: cloudformation.stack-set-operation-single-flight-sequencing|run#11|AssertionError|assert code == "OperationInProgressException", code
Issue-Key: cloudformation.stackset-concurrent-operation-rejected|run#16|AssertionError|assert code in (
Issue-Key: cloudformation.invalid-operation-error-org-and-stackset-rejected|run#16|AssertionError|assert error.get("Message") == "Cannot specify both Accounts and DeploymentTargets", (
Issue-Key: cloudformation.invalid-parameter-combination-stackset-ops-rejected|run#10|AssertionError|assert error.get("Message") == "Cannot specify both Accounts and DeploymentTargets", (
Issue-Key: cloudformation.stackset-deployment-targets-self-managed-rejected|run#9|AssertionError|assert error == expected, (
Issue-Key: cloudformation.update-stack-set-invalid-region-rejected|_error#10|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 281440c6fd2fb349634dcc135fbec4e15edad452

# cloudformation: 5 net-new StackSet control-plane root causes — OperationInProgressException carries the Smithy doc sentence, the single-flight lock is unreachable behind NotImplemented, two mutual-exclusion/permission-model refusal texts are reworded, and an unknown region is accepted

> _Note: this issue was identified against the code base at commit `281440c6fd2fb349634dcc135fbec4e15edad452`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStackInstances

## Description
Five distinct net-new root causes in the CloudFormation StackSet control plane, all grounded in failing behavioral checks of service `cloudformation`. A fixer must close all five before submitting.

Cause 1 of 5 — OperationInProgressException's Message is the model's documentation sentence instead of AWS's wording.
AWS: Smithy (com.amazonaws.cloudformation) models OperationInProgressException on CreateStackInstances, UpdateStackInstances, UpdateStackSet and DetectStackSetDrift: errorKind client, httpError 409, awsQueryError code `OperationInProgressException`, member Message @length 1..255. Recorded behavior: while one stack-set operation is RUNNING, the second call is refused 409 OperationInProgressException whose Message is the fixed text `Another Operation on StackSet <stack-set-arn> is in progress: <operation-id>` — the fixed words are "Another Operation on StackSet" and "is in progress:"; the variable parts are the ARN of the stack set named by the request and the OperationId of the operation that still holds the lock (both taken from the request and the emulator's own state).
Emulator: refuses with the right code but the Message `Another operation is currently in progress for this StackSet. Only one operation can be performed for a stack set at a given time.` — verbatim the `@documentation` sentence of the OperationInProgressException shape — naming neither the stack set nor the blocking operation.
Why: callers and CLIs parse this Message to learn WHICH operation is blocking them (to poll DescribeStackSetOperation on that id) and which stack set is locked when a tool drives several; both facts are absent, so retry/diagnostic logic that works against AWS reports nothing useful.
Fix: the refusal builder behind the StackSet single-flight guard in the cloudformation crate's stack-set operation handlers (the guard must render the stack-set ARN and the in-flight OperationId into the message rather than echoing the shape doc).
Traces: cloudformation.detect-stackset-drift-while-in-progress-rejected

Cause 2 of 5 — the single-flight lock is unreachable for stack-set and stack-instance mutations: they answer NotImplemented.
AWS: UpdateStackSet and UpdateStackInstances both model OperationInProgressException (409) and StaleRequestException (409); a mutation issued while a prior operation on the same stack set is RUNNING is rejected, never queued (recorded behavior shows the rejection arrives synchronously, and the identical call is accepted once the prior operation settles).
Emulator: the probe calls are refused with code `NotImplemented` ("operation ... is scaffolded but not implemented in this emulator"), so no concurrency verdict exists at all: UpdateStackInstances answers NotImplemented where OperationInProgressException is required, and the UpdateStackSet probe likewise. Being refused unconditionally, these operations can neither acquire nor observe the lock.
Why: a client that serializes stack-set rollouts by catching OperationInProgressException sees an unknown error class and cannot distinguish "busy, retry" from "unsupported"; nothing can update a stack set or its instances at all.
Fix: the UpdateStackSet and UpdateStackInstances handlers (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack_set.rs, .../update_stack_instances.rs) plus the shared stack-set lock they must consult. The suite already has a scaffold-stub issue filed naming UpdateStackSet/update_stack_set.rs; these checks were NOT attributed to it (they catch the ClientError and grade its code), so they are recorded here as the concurrency obligations that must be re-replayed once the stubs land — that stub issue does not carry their replay keys.
Traces: cloudformation.concurrent-stack-instance-update-rejected, cloudformation.stack-set-operation-single-flight-sequencing, cloudformation.stackset-concurrent-operation-rejected

Cause 3 of 5 — the Accounts/DeploymentTargets mutual-exclusion refusal is reworded.
AWS: CreateStackInstances models both `Accounts` and `DeploymentTargets` as optional, mutually exclusive targeting modes; specifying both is a request-shape problem refused before any stack-set state logic runs, with ValidationError (Sender) and the fixed Message `Cannot specify both Accounts and DeploymentTargets` (entirely fixed text, no variable parts).
Emulator: same code and type, but the Message is `You cannot specify both Accounts and DeploymentTargets` — a reworded sentence with a leading "You ".
Why: refusal-message equality is what fidelity suites and error-mapping layers key on; a reworded sentence breaks exact-match handling of a documented refusal.
Fix: the input validation for CreateStackInstances (and the sibling stack-instance ops that share the targeting-mode check) in the cloudformation crate.
Traces: cloudformation.invalid-operation-error-org-and-stackset-rejected, cloudformation.invalid-parameter-combination-stackset-ops-rejected

Cause 4 of 5 — DeploymentTargets against a SELF_MANAGED stack set is not recognised as a permission-model violation; it is treated as "no targeting mode given".
AWS: DeploymentTargets/OrganizationalUnitIds are valid only for SERVICE_MANAGED stack sets. Recorded behavior: CreateStackInstances with DeploymentTargets against a SELF_MANAGED stack set is refused ValidationError with the fixed Message `StackSets with SELF_MANAGED permission model can only have accounts as target` (fixed text; the permission-model name is the one the stack set carries).
Emulator: refuses with ValidationError but Message `You must specify at least one value for either Accounts or DeploymentTargets` — the DeploymentTargets the request DID carry was not seen at all, so the diagnosis is the opposite one.
Why: the caller is told to supply a targeting mode it already supplied, instead of being told its permission model forbids OU targeting; org-vs-self-managed configuration bugs become unintelligible.
Fix: CreateStackInstances validation must resolve the stack set's PermissionModel before judging the targeting members.
Traces: cloudformation.stackset-deployment-targets-self-managed-rejected

Cause 5 of 5 — CreateStackInstances accepts a Regions list containing a string that is not an AWS region.
AWS: recorded behavior only — a Regions list mixing one valid region with a non-existent region string is refused wholesale, and no instance is deployed to the valid region either; the identical call with only the valid region succeeds end to end.
Emulator: answers HTTP 200 with an `OperationId` (the recorded response shows a 200 and a minted OperationId), i.e. it accepts the unknown region and starts an operation.
Why: this is overly permissive — a typo in a region name silently produces a half-targeted rollout instead of a refusal, and the caller believes it deployed to a region that does not exist.
Fix: CreateStackInstances input validation (region-list membership check against the partition's regions) in the cloudformation crate's stack-instance handler.
Traces: cloudformation.update-stack-set-invalid-region-rejected

## Evidence
Cause 1 — cloudformation.detect-stackset-drift-while-in-progress-rejected:
  AssertionError: expected 'Another Operation on StackSet arn:aws:cloudformation:us-west-2:000000000000:stackset/<name>:<id> is in progress: op-create-<id>', got 'Another operation is currently in progress for this StackSet. Only one operation can be performed for a stack set at a given time.'
  (the "got" string is the Smithy OperationInProgressException @documentation sentence)

Cause 2 —
  cloudformation.concurrent-stack-instance-update-rejected: AssertionError: UpdateStackInstances: expected OperationInProgressException, got 'NotImplemented'
  cloudformation.stack-set-operation-single-flight-sequencing: assert code == "OperationInProgressException" -> AssertionError: NotImplemented (probe is cf.update_stack_set(... UsePreviousTemplate=True ...) issued while a CreateStackInstances operation is RUNNING)
  cloudformation.stackset-concurrent-operation-rejected: AssertionError: expected a concurrent-operation rejection, got NotImplemented (accepted codes were OperationInProgressException / StaleRequestException / ValidationError)

Cause 3 — both checks:
  AssertionError: expected message 'Cannot specify both Accounts and DeploymentTargets', got {'Type': 'Sender', 'Code': 'ValidationError', 'Message': 'You cannot specify both Accounts and DeploymentTargets'}

Cause 4 — cloudformation.stackset-deployment-targets-self-managed-rejected:
  AssertionError: expected ('ValidationError', 'StackSets with SELF_MANAGED permission model can only have accounts as target') using DeploymentTargets against a SELF_MANAGED StackSet, got ('ValidationError', 'You must specify at least one value for either Accounts or DeploymentTargets')

Cause 5 — cloudformation.update-stack-set-invalid-region-rejected:
  AssertionError: expected rejection, but the call SUCCEEDED and returned {'OperationId': '...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Smithy consulted: OperationInProgressException (client, httpError 409, awsQueryError OperationInProgressException, Message @length 1..255); CreateStackInstances/UpdateStackInstances/UpdateStackSet error sets all include OperationInProgressException and StaleRequestException; CreateStackInstances input has optional Accounts (AccountList) and DeploymentTargets with required Regions.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite checks listed in trace_ids.
2. Cause 1: CreateStackSet (SELF_MANAGED, admin+exec roles), CreateStackInstances to start an operation, then immediately DetectStackSetDrift on the same stack set; inspect Error.Message of the 409 OperationInProgressException — it must be "Another Operation on StackSet <stackset-arn> is in progress: <operation-id>".
3. Cause 2: with that same operation still RUNNING, call UpdateStackSet(StackSetName=..., UsePreviousTemplate=true) and UpdateStackInstances(Accounts=[...], Regions=[...]); both must answer 409 OperationInProgressException, not NotImplemented, and must succeed once the first operation settles.
4. Cause 3: CreateStackInstances with BOTH Accounts=[account] and DeploymentTargets={'OrganizationalUnitIds': [...]} -> ValidationError 'Cannot specify both Accounts and DeploymentTargets'.
5. Cause 4: CreateStackInstances with only DeploymentTargets against a SELF_MANAGED stack set -> ValidationError 'StackSets with SELF_MANAGED permission model can only have accounts as target'.
6. Cause 5: CreateStackInstances with Regions=['us-west-2','not-a-region'] -> must be refused wholesale; the emulator returns 200 + OperationId.
