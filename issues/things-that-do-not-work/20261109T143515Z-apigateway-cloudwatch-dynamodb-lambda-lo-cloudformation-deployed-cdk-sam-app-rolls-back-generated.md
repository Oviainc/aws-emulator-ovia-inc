Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree|_wait_stack#4|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam|_wait_stack#4|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# CloudFormation-deployed CDK/SAM app rolls back: generated Lambda Permission StatementId exceeds 100 chars, and a numeric Lambda property is refused as a string (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes keep a whole order-service template from deploying.

Cause 1 of 2: the physical id CloudFormation generates for AWS::Lambda::Permission is longer than Lambda's StatementId limit, so AddPermission refuses it and the stack rolls back.
AWS: Smithy (lambda) AddPermission.StatementId -> com.amazonaws.lambda#StatementId, @length min 1 max 100, @pattern ^([a-zA-Z0-9-_]+)$. Real AWS deploys this CDK app to CREATE_COMPLETE, so the statement id CloudFormation generates for an AWS::Lambda::Permission always fits inside that 100-character limit (CFN truncates/hashes the <stack>-<logical-id>-<suffix> form to fit); it never hands Lambda a value that its own model forbids.
Emulator: CloudFormation generates the statement id by concatenating the full stack name, the full CDK logical id and a 12-char suffix, producing a 110+ character value, and Lambda then refuses the create with 400 InvalidParameterValueException whose message is '1 validation error detected: Value '<generated statement id>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100'. The stack ends ROLLBACK_COMPLETE with resource status reason 'Resource handler returned message: "<that message> (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Fixed text here is the validation sentence and the constraint clause; the statement id, request id and request token are values from the request and the emulator's state.
Why: no CDK or SAM application that attaches an API Gateway method permission to a Lambda function can be deployed at all — every such stack rolls back, so nothing downstream of the deploy can be exercised.
Fix: the AWS::Lambda::Permission resource handler's physical-id generation in the CloudFormation crate (it must shorten the generated statement id to the modelled 100-character maximum before calling AddPermission).
Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree.

Cause 2 of 2: a Lambda resource in a SAM-rendered template is refused with 'expected number, got string'.
AWS: recorded behavior only — the recording shows the same SAM template reaching CREATE_COMPLETE in real AWS. CloudFormation template values are JSON strings, and the AWS::Lambda::Function handler coerces the numeric properties (MemorySize, Timeout) from their string form before calling CreateFunction; it does not refuse them.
Emulator: the create of the IntakeFunction resource fails and the stack ends ROLLBACK_COMPLETE with reason 'Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The fixed text is 'expected number, got string'; the ids are per-run values.
Why: a SAM-packaged function cannot be deployed; the emulator's own deserializer rejects a template shape AWS accepts, so the application never exists.
Fix: the AWS::Lambda::Function resource handler's property deserialization in the CloudFormation crate (numeric properties must accept the string form a template carries).
Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam.

## Evidence
Cause 1 — expected: stack reaches CREATE_COMPLETE. Actual: 'AssertionError: stack <name> ended in ROLLBACK_COMPLETE: ApiordersPOSTApiPermissionTestOrderServiceStackApiDF2F37BDPOSTordersC0D5751F: Resource handler returned message: "1 validation error detected: Value '<110-char statement id>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'.
Cause 2 — expected: stack reaches CREATE_COMPLETE. Actual: 'AssertionError: stack <name> ended in ROLLBACK_COMPLETE: IntakeFunction: Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'.

## Steps to Reproduce
Replay apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree and .xflavor-order-service-sam: each synthesizes its template, calls CreateStack and waits for CREATE_COMPLETE; both end ROLLBACK_COMPLETE on the resource reasons quoted above.
