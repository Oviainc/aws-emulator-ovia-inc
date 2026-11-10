Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.desired-state-pattern-violation-rejected
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.desired-state-pattern-violation-rejected|run#6|AssertionError|assert code == "ValidationException", (
Issue-Commit: 22d8facbc3b601c7c60aed2bc6871542e8162809

# cloudcontrol: a pattern-violating DesiredState value is refused by the downstream handler (InvalidRequestException, async) instead of the front-door schema ValidationException

> _Note: this issue was identified against the code base at commit `22d8facbc3b601c7c60aed2bc6871542e8162809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service: registry-schema (model) validation of DesiredState does not happen at Cloud Control's front door, so a value that violates a property's schema `pattern` is let through to the resource type's handler and refused there, in the wrong plane, with the wrong error code and the wrong message.

Cause 1 of 1: a DesiredState value violating the type schema's `pattern` keyword is not refused by front-door model validation.

AWS: recorded behavior only for the error code — ValidationException is NOT in the on-disk Smithy model (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json models CreateResource errors InvalidRequestException/NotUpdatableException/AlreadyExistsException/TypeNotFoundException/... all HTTP 400-502; DesiredState targets Properties, required, length 1-262144, sensitive). AWS validates DesiredState against the resource type's CloudFormation registry schema at the front door, BEFORE any handler runs and before a request record is minted: the call itself is refused with HTTP 400, error code ValidationException, and a model-validation diagnostic that names the offending property's JSON pointer and the schema keyword it violated. Message template — fixed text: "Model validation failed (#/" + <property pointer> + ": failed validation constraint for keyword [" + <violated keyword> + "])"; the variable parts are the property name taken from the submitted DesiredState and the keyword name from the type's schema (here the property is LogGroupName and the keyword is pattern, i.e. "Model validation failed (#/LogGroupName: failed validation constraint for keyword [pattern])"). InvalidRequestException (doc: "The resource handler has returned that invalid input from the user has generated a generic exception") is reserved for input the HANDLER rejects, and HandlerErrorCode=InvalidRequest likewise.

Emulator: the CreateResource call is ACCEPTED (HTTP 200 with a ProgressEvent and a RequestToken); the request is then driven to a terminal FAILED ProgressEvent whose ErrorCode maps to InvalidRequestException and whose StatusMessage is the downstream CloudWatch Logs validation sentence "LogGroupName must be 1 to 512 characters of letters, numbers and `. - _ / #`". So the refusal is asynchronous, classified as a handler verdict, and carries the owning service's own character-class text instead of the registry-schema model-validation diagnostic (no property pointer, no keyword name).

Why: a client cannot distinguish a caller-side schema violation — ValidationException, non-retryable, raised synchronously before any effect, which is what Cloud Control clients and IaC tooling catch and map back to the offending template property — from a downstream handler verdict that must be discovered by polling GetResourceRequestStatus. Retry/classification logic branches the wrong way, the caller is forced into an async poll for an input error AWS rejects at the door, and the diagnostic no longer names the property pointer or the violated keyword, which is the only thing that distinguishes a pattern violation from the sibling additionalProperties / type / enum violations.

Fix: the DesiredState registry-schema validation step in front of the CreateResource handler (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/create_resource.rs, and the shared request/effect materialisation in src/state.rs) must evaluate the type schema's pattern/enum/type/range keywords before a request record is minted and refuse with the front-door ValidationException carrying the model-validation message; handler-reported InvalidRequest must stay reserved for genuine handler verdicts.

Related open issue that does NOT cover this cause: 20261102T095426Z-cloudcontrol-9-net-new-causes-desiredstate-patchdocument-are.md (its Cause 4 is the ACCEPTANCE of enum/pattern/range violations reaching terminal SUCCESS; the request now does reach FAILED, so what remains is the refusal plane, code and message reported here). The fixed issue 20261102T095426Z-cloudcontrol-10-net-new-causes-cloud-control-refusal.md Cause 7 covers the same ValidationException-vs-InvalidRequestException confusion only for PatchDocument parse/apply on UpdateResource, not for DesiredState model validation on CreateResource.

Traces: cloudcontrol.desired-state-pattern-violation-rejected (suite simparity-behavioral-aws-v1).

## Evidence
Failing check cloudcontrol.desired-state-pattern-violation-rejected (verdict: mismatch), replayed from suite simparity-behavioral-aws-v1. Generated resource names, request tokens and timestamps in the quotes below are per-run fill-ins, not requirements.

Request: cc.create_resource(TypeName="AWS::Logs::LogGroup", DesiredState=json({"LogGroupName": "<base>:colon-not-allowed", "RetentionInDays": 1})) — the name carries a ':' which the AWS::Logs::LogGroup schema's pattern forbids.

Expected (AWS): the call is refused synchronously, HTTP 400, Error.Code "ValidationException", Error.Message "Model validation failed (#/LogGroupName: failed validation constraint for keyword [pattern])" — fixed text "Model validation failed (#/" ... ": failed validation constraint for keyword [" ... "])", with the property pointer (LogGroupName) taken from the request's DesiredState and the keyword (pattern) from the type schema.

Actual (emulator): CreateResource answered 200 with a ProgressEvent/RequestToken and the request terminated OperationStatus=FAILED with a handler error code normalising to InvalidRequestException and StatusMessage "LogGroupName must be 1 to 512 characters of letters, numbers and `. - _ / #`".

Assertion as reported:
  File "check_cloudcontrol_desired_state_pattern_violation_rejected.py", line 177, in run
    assert code == "ValidationException", (
  AssertionError: a pattern-violating DesiredState value must be refused with ValidationException, got InvalidRequestException: LogGroupName must be 1 to 512 characters of letters, numbers and `. - _ / #`

The check's later assertions (the GetResource-of-an-illegal-identifier InvalidRequestException with the "AWS::Logs::LogGroup Handler returned status FAILED" / "logGroupNamePrefix" / "(HandlerErrorCode: InvalidRequest" fragments, the NotFound control, the ListResources effect-absent check and the positive control create) were never reached because the run stopped at this assertion.

## Steps to Reproduce
Boot the emulator and point botocore at it; cc = client("cloudcontrol").

1. illegal = "<any legal log group name>:colon-not-allowed"  (a ':' violates the AWS::Logs::LogGroup schema pattern for LogGroupName).
2. cc.create_resource(TypeName="AWS::Logs::LogGroup", DesiredState=json.dumps({"LogGroupName": illegal, "RetentionInDays": 1})).
   Expected: botocore raises immediately with HTTP 400, Error.Code "ValidationException" and Error.Message "Model validation failed (#/LogGroupName: failed validation constraint for keyword [pattern])".
   Observed: HTTP 200 with a ProgressEvent + RequestToken; polling cc.get_resource_request_status(RequestToken=...) to a terminal state yields OperationStatus=FAILED with an ErrorCode normalising to InvalidRequestException and StatusMessage "LogGroupName must be 1 to 512 characters of letters, numbers and `. - _ / #`".
3. The same front-door model validation applies to the sibling keywords: a DesiredState whose value violates an enum, a declared JSON type, or a numeric range must also be refused synchronously with ValidationException and the matching "failed validation constraint for keyword [<keyword>]" pointer diagnostic rather than being forwarded to the type's handler.
