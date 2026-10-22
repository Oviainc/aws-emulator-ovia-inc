Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-validation-errors
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|CreateWorkflow|4xx->4xx|$.FieldList
Issue-Key: mwaa-serverless|CreateWorkflow|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|CreateWorkflow|4xx->4xx|$.Reason
Issue-Key: mwaa-serverless|ListWorkflowRuns|4xx->4xx|$.FieldList
Issue-Key: mwaa-serverless|ListWorkflowRuns|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|ListWorkflowRuns|4xx->4xx|$.Reason
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.FieldList
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.Reason
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.FieldList
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.Reason
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# MWAA Serverless validation failures return an empty 400 body with no ValidationException fields

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Four causes, all the same fault: the emulator correctly rejects malformed input with HTTP 400 but serialises NONE of the ValidationException payload — Message, Reason and FieldList all come back absent. Smithy (com.amazonaws.mwaaserverless, awsJson1_0): ValidationException has @httpError 400 with members Message (required, ErrorMessage, @length 1..2048) and Reason (required, enum ValidationExceptionReason with values unknownOperation|cannotParse|fieldValidationFailed|other), plus FieldList (list of ValidationExceptionField, each with required Name and required Message). Because Message and Reason are @required on the error shape, emitting them is a modelled requirement, not a recorded nicety; the recordings confirm Reason='fieldValidationFailed' and a FieldList entry naming the offending member. The per-field Message is built from the request (it names the rejected member and value), so the shape of the payload is the requirement; the exact recorded wording is an example.

Cause 1 of 4: CreateWorkflow with a RoleArn that is not a role ARN. AWS: HTTP 400 ValidationException with Reason 'fieldValidationFailed', Message describing the bad role ARN (recorded example 'Invalid Role ARN: not-a-valid-role-arn') and FieldList [{Name: 'RoleArn', Message: 'Invalid Role ARN: not-a-valid-role-arn'}]. Emulator: HTTP 400 with Message, Reason and FieldList all missing. Fixture mwaa-serverless-validation-errors, step task#1.

Cause 2 of 4: ListWorkflowRuns with a malformed workflow Arn. AWS: HTTP 400, Reason 'fieldValidationFailed', Message (recorded example 'Validation failed. Invalid arn') and FieldList [{Name: 'Arn', Message: 'Invalid arn'}]. Emulator: HTTP 400 with an empty error body. Fixture mwaa-serverless-validation-errors, step task#3.

Cause 3 of 4: TagResource with a malformed resource Arn. AWS: HTTP 400, Reason 'fieldValidationFailed', Message (recorded example 'Validation failed. Invalid arn') and FieldList [{Name: 'Arn', ...}]. Emulator: HTTP 400 with an empty error body. Fixture mwaa-serverless-validation-errors, step task#4.

Cause 4 of 4: UntagResource with a malformed resource Arn. AWS: HTTP 400, Reason 'fieldValidationFailed', Message (recorded example 'Validation failed. Invalid arn') and FieldList [{Name: 'Arn', ...}]. Emulator: HTTP 400 with an empty error body. Fixture mwaa-serverless-validation-errors, step task#5.

Why: an awsJson1_0 SDK deserialises the error body into ValidationException and exposes message()/reason()/fieldList(). With no body, every client sees a ValidationException whose message is null/empty and whose reason and fieldList are absent, so a caller cannot tell WHY the request was rejected or WHICH member was at fault. Code that branches on reason == fieldValidationFailed, surfaces the field name to a user, or logs the error message gets nothing usable, and required-member deserialisation can fault outright in strict SDKs.

Fix: the validation layer and the error serialiser for these operations — input validation must populate ValidationException with Reason (fieldValidationFailed for a bad member value), a top-level Message, and a FieldList entry naming the rejected member, and the awsJson1_0 error serialiser must write that structure into the 400 response body instead of an empty one.

Traces: mwaa-serverless-validation-errors#task#1 (CreateWorkflow, RoleArn), #task#3 (ListWorkflowRuns, Arn), #task#4 (TagResource, Arn), #task#5 (UntagResource, Arn).

## Evidence
Replay of fixture mwaa-serverless-validation-errors (4 divergence clusters, verdict=mismatch; status matched at 400 in all four, body did not):
- task#1 CreateWorkflow: $.Message missing (expected 'Invalid Role ARN: not-a-valid-role-arn', got None); $.Reason missing (expected 'fieldValidationFailed', got None); $.FieldList missing (expected [{'Name': 'RoleArn', 'Message': 'Invalid Role ARN: not-a-valid-role-arn'}], got None)
- task#3 ListWorkflowRuns: $.Message missing (expected 'Validation failed. Invalid arn'); $.Reason missing (expected 'fieldValidationFailed'); $.FieldList missing (expected [{'Name': 'Arn', 'Message': 'Invalid arn'}])
- task#4 TagResource: same three keys missing, same expected values
- task#5 UntagResource: same three keys missing, same expected values
Model reference: ValidationException @httpError 400, Message @required, Reason @required (enum: unknownOperation, cannotParse, fieldValidationFailed, other), FieldList -> ValidationExceptionField{Name @required, Message @required}. Emulator reachable (health 200), bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay wire fixture mwaa-serverless-validation-errors from trace set simparity-wire-aws-v1.
3. Step task#1 calls CreateWorkflow with RoleArn='not-a-valid-role-arn'; steps task#3/task#4/task#5 call ListWorkflowRuns/TagResource/UntagResource with a malformed ARN.
4. Observe HTTP 400 on all four but an error body with no Message, no Reason and no FieldList, where the recording carries all three.
