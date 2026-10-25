Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.op-contract-createRequestValidator
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.op-contract-createRequestValidator|run#15|AssertionError|assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err.get('Code')!r}"
Issue-Commit: 6ef21a1d109a450086c7f18e8e030ea8cff3e1e7

# CreateRequestValidator answers 404 NotFoundException for an unknown restApiId where AWS refuses with 400 BadRequestException

> _Note: this issue was identified against the code base at commit `6ef21a1d109a450086c7f18e8e030ea8cff3e1e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRequestValidator

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: CreateRequestValidator misclassifies an unknown REST API identifier as a not-found lookup instead of a request-validation failure.

AWS: API Gateway validates CreateRequestValidator's path/body together and reports a same-shaped but non-existent `restApiId` as a bad request, not as a missing sub-resource. The blessed replay against real AWS (the check is marked BLESSED and asserts the recorded refusal) records HTTP 400 `BadRequestException` with the fixed message `Invalid REST API identifier specified` — fixed text AWS emits for an invalid REST API identifier, not text built from the request (the identifier is not echoed). Smithy: `CreateRequestValidator` (POST /restapis/{restApiId}/requestvalidators, success 201) models BOTH `BadRequestException` (httpError 400) and `NotFoundException` (httpError 404), so the choice between them is the operation's own classification, and AWS documents BadRequestException as "The submitted request is not valid, for example, the input is incomplete or incorrect" versus NotFoundException as "The requested resource is not found". Recorded behavior confirms AWS picks the 400 for this input.

Emulator: the handler answers 404 `NotFoundException` instead. The replayed check fails with `AssertionError: expected BadRequestException, got 'NotFoundException'`, so the emulator never reaches the message assertion either. The refusal itself is otherwise correct in effect (no validator is created), only its classification, status and message diverge.

Why: a client that distinguishes "you passed me a bad API id" (400, non-retryable input error, surfaced to the caller as a validation problem) from "the API is gone" (404, which SDK/IaC layers commonly treat as a benign already-deleted/eventually-consistent condition and swallow or retry) takes the wrong branch. Provisioning code that creates a validator against a stale or mistyped REST API id sees a 404 and silently treats the API as not-yet-visible, retrying or continuing, instead of failing fast on an invalid identifier; error-handling tests keyed on `BadRequestException` never fire.

Fix: the CreateRequestValidator handler's REST-API lookup/validation branch — the unknown-`restApiId` arm must raise the modeled `BadRequestException` (400) carrying API Gateway's fixed invalid-identifier text rather than the shared no-such-rest-api `NotFoundException` (404). Note this classification is operation-specific: plain GETs of sub-resources do answer NotFoundException, so the fix belongs in this operation's validation, not in the shared state helper used by read paths.

Traces: apigateway.op-contract-createRequestValidator (mismatch). Relevant code: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_request_validator.rs lines 201-205 (the `rest_apis.get_mut(...).ok_or_else(|| ... NotFoundException ...)` arm, using `crate::state::no_such_rest_api`).

## Evidence
Failing check: [mismatch] apigateway.op-contract-createRequestValidator

Expected (real AWS, blessed recording): CreateRequestValidator with a same-shaped but non-existent restApiId -> HTTP 400, Error.Code = "BadRequestException", Error.Message = "Invalid REST API identifier specified" (fixed text, identifier not echoed); no validator created on any API.

Actual (emulator): HTTP 404, Error.Code = "NotFoundException".
Replay output:
  File "check_apigateway_op_contract_createRequestValidator.py", line 62, in run
    assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err.get('Code')!r}"
  AssertionError: expected BadRequestException, got 'NotFoundException'

Model evidence (smithy_query, operation=CreateRequestValidator): errors include BadRequestException (httpError 400) and NotFoundException (httpError 404); http = POST /restapis/{restApiId}/requestvalidators, code 201.

Emulator source: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_request_validator.rs:201-205 returns CreateRequestValidatorErrorModeled::NotFoundException for a restApiId absent from store.rest_apis.

## Steps to Reproduce
1. CreateRestApi to obtain a valid restApiId (so the earlier positive legs of the check pass).
2. CreateRequestValidator(restApiId=<valid id>, name="strict-validator", validateRequestBody=true, validateRequestParameters=false) -> succeeds, flags echoed.
3. Build a non-existent identifier of the same shape as a real REST API id (same charset/length, different value).
4. Call CreateRequestValidator(restApiId=<non-existent id>, name="orphan-validator", validateRequestBody=true).
5. Observe the refusal: emulator answers HTTP 404 with Error.Code "NotFoundException"; real AWS answers HTTP 400 with Error.Code "BadRequestException" and the fixed message "Invalid REST API identifier specified".
6. GetRequestValidators on the real API to confirm no validator was added (both sides agree here).
