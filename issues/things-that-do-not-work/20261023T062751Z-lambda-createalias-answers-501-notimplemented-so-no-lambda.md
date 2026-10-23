Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.concurrent-deployment-same-group-rejected-while-active codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm
Issue-Key: codedeploy|CreateAlias|NotImplemented
Issue-Key: cloudwatch__codedeploy__lambda|CreateAlias|NotImplemented
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# Lambda CreateAlias answers 501 NotImplemented, so no Lambda-platform CodeDeploy deployment (alias traffic shift, alarm stop/rollback) can run

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
One root cause. Service attribution: the failing operation is Lambda's CreateAlias, so this is filed under `lambda` (bootstrapped). Two of the grounding check ids lead with `codedeploy` and one leads with the composite token `cloudwatch__codedeploy__lambda`; all three fail on the same Lambda call before any CodeDeploy assertion runs.

Cause 1 of 1: Lambda CreateAlias is an unimplemented scaffold stub.
AWS: Smithy Lambda CreateAlias (POST /2015-03-31/functions/{FunctionName}/aliases) creates a named alias pointing at a published function version and returns the alias configuration (AliasArn, Name, FunctionVersion, RevisionId); it models ResourceNotFoundException, ResourceConflictException, InvalidParameterValueException, TooManyRequestsException and ServiceException — HTTP 501 / 'NotImplemented' is not among them. An alias is also the required target of every Lambda-compute-platform CodeDeploy deployment (CreateDeploymentGroup's deploymentStyle/lambda revision shifts an alias's weights between two versions).
Emulator: HTTP 501 with error code 'NotImplemented' — "operation CreateAlias is scaffolded but not implemented in this emulator".
Why: no client can create a Lambda alias, so versioned invocation targets and every Lambda-platform CodeDeploy flow are unreachable: the suite's Lambda deployment checks never get past setup, and a caller that depends on the modeled errors (e.g. ResourceConflictException for an existing alias) sees an unmodeled code instead.
Fix: implement the CreateAlias handler in the lambda service crate's ops (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_alias.rs) against the function/version state, and remove that file's scaffold marker.
Traces: codedeploy.concurrent-deployment-same-group-rejected-while-active, codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced, cloudwatch__codedeploy__lambda.lambda-deployment-stopped-and-rolled-back-by-alarm.

## Evidence
Replayed behavioral corpus simparity-behavioral-aws-v1 against the emulator built from this checkout (bootstrap_signal false — the emulator was serving). All three checks abort on the same call:

  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAlias operation: operation CreateAlias is scaffolded but not implemented in this emulator

Expected: HTTP 201/200 with the alias configuration (Name, FunctionVersion, AliasArn) for a published version of a freshly created function. Actual: HTTP 501, Code 'NotImplemented'.

The suite marks other Lambda stubs as already filed (UpdateFunctionCode) but does NOT mark CreateAlias, and no open issue names CreateAlias: the open Lambda issues are "Lambda Invoke refuses every real invocation with NotImplemented...", "Implement lambda PublishLayerVersion: unimplemented scaffold stub" and the wire/lambda-core issues, none of which covers this operation.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the behavioral suite (run_behavioral_suite).
2. CreateFunction(FunctionName=F, Runtime=python3.x, Handler=..., Code=<zip>, Publish=True) and PublishVersion(FunctionName=F) to get version '1'.
3. CreateAlias(FunctionName=F, Name='live', FunctionVersion='1') -> expect the alias configuration; observe HTTP 501 with Code 'NotImplemented'.
4. Any Lambda-platform CodeDeploy deployment (CreateDeploymentGroup with CodeDeployDefault.LambdaAllAtOnce + a lambda revision naming that alias) then cannot be set up at all.
