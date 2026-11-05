Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cfn-lambda-env-ssm-dynamic-reference-and-parameter-type-resolve-and-update
Issue-Key: cloudformation__lambda__ssm.cfn-lambda-env-ssm-dynamic-reference-and-parameter-type-resolve-and-update|run#26|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Commit: bd1d039abc40ef8a9373a2b76b9b7f1b159abc82

# cloudformation__lambda__ssm: a stack update that changes an AWS::Lambda::Function ends UPDATE_FAILED with ResourceConflict "An update is in progress for resource", so no CFN-managed function can be updated

> _Note: this issue was identified against the code base at commit `bd1d039abc40ef8a9373a2b76b9b7f1b159abc82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: a CloudFormation stack update that touches an `AWS::Lambda::Function` (here its `Description` and its `Environment.Variables`, both re-resolved from SSM) fails the resource and rolls the stack back.

AWS: Lambda refuses a configuration change only while another change on the same function is still in flight, with `ResourceConflictException` (409) worded "The operation cannot be performed at this time. An update is in progress for resource: <function ARN>" — fixed text, with the function ARN the only variable. CloudFormation's `AWS::Lambda::Function` handler therefore waits for the function to be ready (`LastUpdateStatus` = `Successful`) around each call, so an ordinary property update is applied and the stack reaches UPDATE_COMPLETE; the recorded check asserts exactly that, and then asserts the function serves the re-resolved `{{resolve:ssm:...}}` and `AWS::SSM::Parameter::Value<String>` values.

Emulator: the resource ends UPDATE_FAILED with `Resource handler returned message: "The operation cannot be performed at this time. An update is in progress for resource: <function ARN>" (RequestToken: <id>, HandlerErrorCode: ResourceConflict)` and the stack ends UPDATE_ROLLBACK_COMPLETE with stack-level reason `The following resource(s) failed to update: [Fn].`. The ARN and RequestToken are per-run values; the refusal itself is the defect, not its wording. Create works (CREATE_COMPLETE, and the first invoke returns the resolved values), so only the update path is affected. Hypothesis: the provisioner issues more than one Lambda update call for one resource change (or does not wait for the function's in-progress update to clear) so its own second call collides with its first.

Why: a client cannot change anything about a Lambda function it manages with CloudFormation — every such stack update fails and rolls back, and any SSM-sourced environment value is therefore frozen at its create-time resolution.

Fix: the `AWS::Lambda::Function` update arm of the CloudFormation provisioner — coalesce one resource change into a single `UpdateFunctionConfiguration`/`UpdateFunctionCode` sequence and perform the readiness wait (`GetFunctionConfiguration` until `LastUpdateStatus` is `Successful`) between calls; alternatively, the lambda state's in-progress-update flag is not being cleared when an update completes, which would make every second update of any function refuse.

Traces: cloudformation__lambda__ssm.cfn-lambda-env-ssm-dynamic-reference-and-parameter-type-resolve-and-update.

## Evidence
`AssertionError: stack update must complete; it ended 'UPDATE_ROLLBACK_COMPLETE': [('sim2real-xsvc-cfnenv-<id>', 'UPDATE_FAILED', 'The following resource(s) failed to update: [Fn]. '), ('Fn', 'UPDATE_FAILED', 'Resource handler returned message: "The operation cannot be performed at this time. An update is in progress for resource: arn:aws:lambda:us-west-2:<account>:function:<stack>-Fn-<suffix>" (RequestToken: <id>, HandlerErrorCode: ResourceConflict)')]`
Expected: stack status UPDATE_COMPLETE, then `Invoke` returning `{"dynamic": "dyn-v2", "param_type": "pt-v2"}`.

## Steps to Reproduce
Replay the behavioral suite, group cloudformation__lambda__ssm, check cfn-lambda-env-ssm-dynamic-reference-and-parameter-type-resolve-and-update: (1) put two SSM String parameters; (2) CreateStack with an `AWS::Lambda::Function` (python3.12, inline `Code.ZipFile`) whose `Environment.Variables` are `{{resolve:ssm:<p1>}}` and `{"Ref": <AWS::SSM::Parameter::Value<String> parameter>}`, and wait for CREATE_COMPLETE; (3) overwrite both SSM values; (4) `UpdateStack` with the same template but a changed `Description` — the Fn resource goes UPDATE_FAILED with HandlerErrorCode ResourceConflict and the stack rolls back.
