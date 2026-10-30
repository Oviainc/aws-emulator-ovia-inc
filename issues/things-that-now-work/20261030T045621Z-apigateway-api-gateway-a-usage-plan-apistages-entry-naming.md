Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.op-contract-createUsagePlan apigateway.update-usage-plan-invalid-apistage-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.op-contract-createUsagePlan|run#14|AssertionError|assert err.get("Message") == f"Invalid API identifier specified {ctx.account}:zzzznotarealapiid", (
Issue-Key: apigateway.update-usage-plan-invalid-apistage-rejected|run#19|AssertionError|assert (err.get("Code"), err.get("Message")) == ("NotFoundException", expected), (
Issue-Commit: 77b7d3dfd1a46160280007e44fe4e2e402c212f4

# API Gateway: a usage plan apiStages entry naming a non-existent restApiId is refused with an emulator-authored "API Stage not found: <apiId>:<stage>" message instead of AWS's "Invalid API identifier specified <account>:<apiId>" (1 cause)

> _Note: this issue was identified against the code base at commit `77b7d3dfd1a46160280007e44fe4e2e402c212f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUsagePlan

## Description
Cause 1 of 1: the NotFoundException message both usage-plan handlers emit when an `apiStages` entry cannot be resolved names the apiId:stage PAIR in emulator-authored prose, where AWS resolves the REST API id first and names it alone, prefixed with the caller's account id. The refusal is shared by CreateUsagePlan (apiStages in the create input) and UpdateUsagePlan (an `add /apiStages` patch), because both call the same message helper, so one fix closes both. Failing checks: apigateway.op-contract-createUsagePlan, apigateway.update-usage-plan-invalid-apistage-rejected.

AWS: Smithy (com.amazonaws.apigateway) — CreateUsagePlan POST /usageplans (success 201) and UpdateUsagePlan PATCH /usageplans/{usagePlanId} (success 200) both model NotFoundException with httpError 404, documented as "The requested resource is not found. Make sure that the request URI is correct." The model does not carry message text, so for the text this is recorded behavior only: the recording shows real AWS resolving the entry's `apiId` BEFORE its `stage` and refusing an unknown REST API id with HTTP 404, Code `NotFoundException` and Message whose FIXED text is `Invalid API identifier specified ` followed by the VARIABLE part `<accountId>:<restApiId>` — the account id of the caller and the `apiId` taken verbatim from the request's apiStages entry. The requested stage name does not appear in that message at all, and no usage plan is created / no apiStages mutation is applied.

Emulator: both handlers resolve the (apiId, stage) pair as a single unit and, on any failure, return NotFoundException carrying `crate::state::no_such_api_stage(api_id, stage_name)`. The replay shows Code `NotFoundException` (correct) with Message `API Stage not found: <apiId>:<stage>` — observed as 'API Stage not found: zzzznotarealapiid:nonexistent-stage' for the create path and 'API Stage not found: nope7ce091:missing-stage' for the update path. Relative to AWS: the fixed text differs entirely, the caller's account id is missing from the variable part, and a stage name AWS never mentions is appended. The HTTP status and error code are right; only the message diverges.

Why: a client that keys off the refusal text — CLI/SDK error handling, CDK/CFN provisioning logic, or test suites asserting the API Gateway error contract — cannot tell an unknown REST API id from an unknown stage of a known API, and sees no account scoping, so it misreports or mis-retries a usage-plan association that real AWS refuses with a specific, account-qualified identifier diagnosis.

Fix: the shared refusal message used by the apiStages resolution in CreateUsagePlan's handler and in UpdateUsagePlan's patch validation (the `no_such_api_stage` helper in this service's state module). The resolution should check the `apiId` against the account's stored REST APIs first and refuse with AWS's account-qualified "Invalid API identifier specified" message naming only that id; the stage-level refusal remains for an apiId that does resolve. Note: the open issues 20261029T193654Z-apigateway-api-gateway-refusal-contracts-part-1-10-root.md, -part-2-10-root.md and -part-3-8-root.md collect other API Gateway refusal-message causes but neither carries these two checks.

Traces: apigateway.op-contract-createUsagePlan and apigateway.update-usage-plan-invalid-apistage-rejected; emitting sites services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_usage_plan.rs lines 301-316 (message at line 311) and services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_usage_plan.rs lines 196-208 (message at line 203), reached from apply_api_stage_patch -> require_stage at line 384.

## Evidence
Check apigateway.op-contract-createUsagePlan (mismatch):
  CreateUsagePlan with apiStages=[{apiId: <unknown-api-id>, stage: <unknown-stage>}]
  expected: Code 'NotFoundException', Message 'Invalid API identifier specified <account>:<unknown-api-id>'
  actual:   Code 'NotFoundException', Message 'API Stage not found: zzzznotarealapiid:nonexistent-stage'
  AssertionError: expected the exact NotFound message, got 'API Stage not found: zzzznotarealapiid:nonexistent-stage'

Check apigateway.update-usage-plan-invalid-apistage-rejected (mismatch):
  UpdateUsagePlan patchOperations=[{op: add, path: /apiStages, value: '<unknown-api-id>:missing-stage'}]
  expected: ('NotFoundException', 'Invalid API identifier specified <account>:<unknown-api-id>')
  actual:   {'Message': 'API Stage not found: nope7ce091:missing-stage', 'Code': 'NotFoundException'}

Message anatomy (AWS): fixed text 'Invalid API identifier specified ' + variable '<accountId>:<restApiId>', the account id from the caller's credentials/account and the restApiId copied from the request's apiStages entry. The stage name is NOT part of the message. Emulator message anatomy: fixed text 'API Stage not found: ' + '<apiId>:<stage>' — wrong fixed text, missing account id, extra stage value.

Both paths are correct on status (404) and code (NotFoundException), and both correctly leave state unmutated; only the Message diverges. Shared emitter: crate::state::no_such_api_stage, called at create_usage_plan.rs:311 and update_usage_plan.rs:203.

## Steps to Reproduce
1. Create a usage plan referencing an API that does not exist:
   aws apigateway create-usage-plan --name sim2real-plan --api-stages apiId=zzzznotarealapiid,stage=nonexistent-stage
   Observe HTTP 404 NotFoundException with Message 'API Stage not found: zzzznotarealapiid:nonexistent-stage'; AWS answers 'Invalid API identifier specified <account>:zzzznotarealapiid'.
2. Create a usage plan with no apiStages, then patch in a reference to a non-existent API:
   aws apigateway create-usage-plan --name sim2real-plan2
   aws apigateway update-usage-plan --usage-plan-id <planId> --patch-operations op=add,path=/apiStages,value=zzzznotarealapiid:missing-stage
   Observe the same 'API Stage not found: ...' message instead of 'Invalid API identifier specified <account>:zzzznotarealapiid'.
3. Re-replay the behavioral checks apigateway.op-contract-createUsagePlan and apigateway.update-usage-plan-invalid-apistage-rejected to confirm the fix.
