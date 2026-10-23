Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-validation-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-validation-errors
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|CreateWorkflow|4xx->4xx|$.FieldList[*].Message
Issue-Key: mwaa-serverless|CreateWorkflow|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|ListWorkflowRuns|4xx->4xx|$.FieldList[*]
Issue-Key: mwaa-serverless|ListWorkflowRuns|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.FieldList[*]
Issue-Key: mwaa-serverless|TagResource|4xx->4xx|$.Message
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.FieldList[*]
Issue-Key: mwaa-serverless|UntagResource|4xx->4xx|$.Message
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# ValidationException for invalid ARNs reports generic Smithy constraint text instead of MWAA Serverless ARN validation

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
This bundle has 4 causes, all one fault: when an ARN-shaped input member fails its pattern, the emulator answers with a *generic* Smithy constraint-violation rendering (top-level "N validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <regex>" and a FieldList entry whose Name is the modeled member name) instead of MWAA Serverless's own ARN validation result, which names the field 'Arn' and carries a short, ARN-specific message. The HTTP status (400) and the error shape (ValidationException, with Reason) are already correct in every case; what diverges is the structured FieldList content and the Message. All 4 causes must be closed before submitting.

Cause 1 of 4: CreateWorkflow, RoleArn rejected (fixture mwaa-serverless-validation-errors, step task#1).
AWS: Smithy: CreateWorkflow.RoleArn targets RoleARN, @required, @length 1..2048, @pattern ^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:iam::[0-9]{12}:role(/...)*?/...$; modeled error ValidationException (errorKind client, httpError 400) whose Message is @required and whose FieldList members Name/Message are @required (shape ValidationExceptionField). The recorded AWS response is 400 ValidationException with Message "Invalid Role ARN: not-a-valid-role-arn" and FieldList[0].Message identical to it; the message rule is derived from the request — the fixed prefix "Invalid Role ARN: " followed by the rejected value as supplied. FieldList[0].Name ('RoleArn') already matches.
Emulator: same 400, but Message = "1 validation error detected: Value 'not-a-valid-role-arn' at 'roleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws...$" and FieldList[0].Message = the same constraint sentence without the "1 validation error detected: " prefix.
Why: a client that surfaces or matches on the ARN validation message gets the raw member-level constraint dump, including the full internal regex, instead of the service's "Invalid Role ARN: <value>" wording; error-classification logic keyed on that wording never fires.
Fix: the mwaa-serverless CreateWorkflow request validation path — ARN members must be validated by the service's ARN validator, which produces the ARN-specific Message, rather than falling through to the shared generic pattern-constraint renderer.
Traces: mwaa-serverless-validation-errors#task#1.

Cause 2 of 4: ListWorkflowRuns, WorkflowArn rejected (fixture mwaa-serverless-validation-errors, step task#3).
AWS: Smithy: ListWorkflowRuns.WorkflowArn targets WorkflowArn, @required, @length 1..2048, @pattern ^arn:aws...:airflow-serverless:<region>:<account>:workflow/<name>-<10 chars>$; modeled error ValidationException (400). Recorded AWS response: 400 ValidationException, Message "Validation failed. Invalid arn", FieldList[0] = {"Name": "Arn", "Message": "Invalid arn"} — i.e. the field is reported under the generic name 'Arn', not the member name, and the top-level Message is the fixed prefix "Validation failed. " plus that field message.
Emulator: same 400, but FieldList[0] = {"Name": "WorkflowArn", "Message": "Value 'not-an-arn' at 'workflowArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws...$"} and Message = "1 validation error detected: " + that sentence.
Why: FieldList[].Name is a structured, @required response field that clients key on to attribute a validation failure to an input field; reporting 'WorkflowArn' where AWS reports 'Arn' makes that attribution miss, and the Message no longer follows AWS's "Validation failed. <field message>" composition.
Fix: the mwaa-serverless ListWorkflowRuns validation/ValidationException serializer — emit Name 'Arn' with the ARN-specific field message and compose the top-level Message as "Validation failed. <field message>".
Traces: mwaa-serverless-validation-errors#task#3.

Cause 3 of 4: TagResource, ResourceArn rejected (fixture mwaa-serverless-validation-errors, step task#4).
AWS: Smithy: TagResource.ResourceArn targets TaggableResourceArn, @required, @length 1..1011, @pattern ^arn:aws...:airflow-serverless:...:workflow/...$; modeled error ValidationException (400). Recorded AWS response: 400 ValidationException, Message "Validation failed. Invalid arn", FieldList[0] = {"Name": "Arn", "Message": "Invalid arn"}.
Emulator: same 400, but FieldList[0] = {"Name": "ResourceArn", "Message": "Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws...$"} and Message = "1 validation error detected: " + that sentence.
Why: same as cause 2 — the structured field name a client reads to locate the offending input is wrong, so tag-call error handling cannot distinguish an ARN problem from any other field problem.
Fix: the mwaa-serverless TagResource validation path / ValidationException construction.
Traces: mwaa-serverless-validation-errors#task#4.

Cause 4 of 4: UntagResource, ResourceArn rejected (fixture mwaa-serverless-validation-errors, step task#5).
AWS: Smithy: UntagResource.ResourceArn targets TaggableResourceArn, @required, @length 1..1011, same airflow-serverless workflow-ARN @pattern; modeled error ValidationException (400). Recorded AWS response: 400 ValidationException, Message "Validation failed. Invalid arn", FieldList[0] = {"Name": "Arn", "Message": "Invalid arn"}.
Emulator: same 400, but FieldList[0] = {"Name": "ResourceArn", "Message": "Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws...$"} and Message = "1 validation error detected: " + that sentence.
Why: same as causes 2 and 3; an untag client keying on FieldList[].Name == 'Arn' sees no such entry.
Fix: the mwaa-serverless UntagResource validation path / ValidationException construction — ideally one shared ARN-validation helper used by all four operations above.
Traces: mwaa-serverless-validation-errors#task#5.

Note: no open issue covers any of these (no mwaa-serverless issues are filed). The quoted messages are examples of the documented/recorded composition rules ("Invalid Role ARN: <value>"; "Validation failed. <field message>"); the hard requirements are status 400, error ValidationException, and the FieldList entry's Name.

## Evidence
Replay of fixture mwaa-serverless-validation-errors (trace set simparity-wire-aws-v1), 4 mismatch clusters, all expected 400 / actual 400:

- task#1 CreateWorkflow: $.Message expected 'Invalid Role ARN: not-a-valid-role-arn', got "1 validation error detected: Value 'not-a-valid-role-arn' at 'roleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_\-]{1,512})*?/[a-zA-Z0-9+=,.@_\-]{1,64}$"; $.FieldList[0].Message expected 'Invalid Role ARN: not-a-valid-role-arn', got the same constraint sentence.
- task#3 ListWorkflowRuns: $.FieldList[0] expected {'Name': 'Arn', 'Message': 'Invalid arn'}, got {'Name': 'WorkflowArn', 'Message': "Value 'not-an-arn' at 'workflowArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws...:airflow-serverless:...$"}; $.Message expected 'Validation failed. Invalid arn'.
- task#4 TagResource: $.FieldList[0] expected {'Name': 'Arn', 'Message': 'Invalid arn'}, got {'Name': 'ResourceArn', 'Message': "Value 'not-an-arn' at 'resourceArn' failed to satisfy constraint: ..."}; $.Message expected 'Validation failed. Invalid arn'.
- task#5 UntagResource: identical to task#4 (FieldList[0].Name 'ResourceArn' vs 'Arn'; $.Message 'Validation failed. Invalid arn' vs "1 validation error detected: ...").

Model evidence (services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model): ValidationException {Message @required, Reason @required, FieldList: ValidationExceptionFields}, ValidationExceptionField {Name @required, Message @required}; ValidationException is a modeled error of all four operations with httpError 400. Status and Reason already match; only FieldList content and Message diverge.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture mwaa-serverless-validation-errors, step task#1: POST awsJson1_0 CreateWorkflow with RoleArn="not-a-valid-role-arn" (plus the fixture's required Name/DefinitionS3Location). Observe 400 ValidationException whose Message is "1 validation error detected: Value ... failed to satisfy constraint: Member must satisfy regular expression pattern: ..." instead of "Invalid Role ARN: not-a-valid-role-arn", and whose FieldList[0].Message carries the same constraint text.
3. Same fixture, step task#3: call ListWorkflowRuns with WorkflowArn="not-an-arn". Observe FieldList[0].Name == "WorkflowArn" instead of "Arn" and Message not of the form "Validation failed. <field message>".
4. Same fixture, steps task#4 and task#5: call TagResource and UntagResource with ResourceArn="not-an-arn". Observe FieldList[0].Name == "ResourceArn" instead of "Arn" and the same generic top-level Message.
5. Expected in all four: status 400, ValidationException, FieldList[0].Name "Arn" (generic ARN field name) for the ARN members, and Message composed as "Validation failed. <field message>" (CreateWorkflow's RoleArn: "Invalid Role ARN: <supplied value>").
