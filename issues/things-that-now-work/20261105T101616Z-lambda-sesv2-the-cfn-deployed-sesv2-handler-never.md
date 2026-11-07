Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution|_poll_until#5|Timeout|last = probe()
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive|_poll_until#9|Timeout|last = probe()
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected|_poll_until#9|Timeout|last = probe()
Issue-Key: lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative|_poll_until#10|Timeout|last = probe()
Issue-Key: lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive|_poll_until#8|Timeout|last = probe()
Issue-Commit: 272a9e194c556b77cae169106dcdf0edb27f0f40

# lambda__sesv2: the CFN-deployed SESv2 handler never produces a clean Invoke, so five deploy/stack-update checks burn their whole budget and never reach a verdict

> _Note: this issue was identified against the code base at commit `272a9e194c556b77cae169106dcdf0edb27f0f40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a CloudFormation-deployed python Lambda whose handler imports the AWS SDK and calls one SESv2 API never returns the handler's own response from Invoke. Every one of these five checks deploys its stack successfully and then polls lambda Invoke until the response has StatusCode 200 and no 'Unhandled' FunctionError; that condition is never reached, the retry loop spins until the harness wall-clock cap fires, and the check reports 'Check timed out after 60.0s' without ever evaluating its assertions.

AWS: Smithy lambda Invoke — POST /2015-03-31/functions/{FunctionName}/invocations, success code 200, output members Payload (httpPayload, the handler's own return value) and FunctionError (httpHeader X-Amz-Function-Error). The modeled contract is that FunctionError is populated ONLY when the handler itself raised; a healthy invocation of an Active function answers 200 with no X-Amz-Function-Error and the handler's JSON in Payload. The content of a managed runtime is not in the model — recorded behavior only: the python3.x managed runtime has the AWS SDK for Python on the handler's module search path, so a ZipFile-only handler that does `import boto3` / `boto3.client("sesv2")` imports and runs. Each of these recordings relies on exactly that: the handler returns a small JSON object such as {"ok": true, "name": "<configuration set name>"} (arn-resolution), {"ok": true, "messageId": "<id>"} (role-trust), or {"ok": false, "errorCode": "MessageRejected"|"NotFoundException"|"BadRequestException"|"AccessDeniedException"} for the rejection variants.

Emulator: Invoke answers 200 but with X-Amz-Function-Error: Unhandled and a runtime-authored payload instead of the handler's result, so the handler body never executes and the polling predicate (StatusCode==200 and FunctionError not starting with 'Unhandled') is never satisfied. The five checks therefore produce no outcome at all: the harness kills them at its 60.0 s cap. The replay detail for all five is identical: 'Error: Check timed out after 60.0s'.

Why: a client cannot observe anything that depends on a function's own AWS calls. The whole class of behavior these checks cover disappears: a configuration-set Ref delivered as an environment variable and read back with GetConfigurationSet (so Ref/Fn::GetAtt resolution is unverifiable from inside the function), an execution role trusting lambda.amazonaws.com with ses:SendEmail actually reaching the SESv2 SendEmail business logic, SES's own MessageRejected for an unverified sender being distinguishable from an IAM AccessDeniedException, a never-created identity producing NotFoundException/BadRequestException while a sibling function on the real identity succeeds, and a stack update that adds ses:SendEmail turning AccessDeniedException into MessageRejected. In addition, a client that follows the documented retry pattern (invoke until FunctionError clears) never converges and hangs indefinitely rather than failing fast.

Fix: the lambda Invoke data-plane runtime — the python execution environment the function-runtime path builds for a ZipFile deployment package, including functions created by the CloudFormation AWS::Lambda::Function provisioner. The runtime must provide the AWS SDK for Python on the handler's module search path, resolved against the emulator's own endpoints and credentials, as the managed python3.x runtimes do, so that only a genuine handler exception surfaces as FunctionError=Unhandled and Payload otherwise carries the handler's own JSON.

Related, and NOT covering these checks: open 20261105T080633Z-lambda-sesv2-the-emulator-s-python-lambda-runtime.md describes the same runtime gap for 57 other lambda__sesv2 checks, but the suite does not report these five as owned by it — their replay keys are the harness-level 'Check timed out after 60.0s' verdict rather than the per-frame assertion keys that issue carries, so they are filed here and must be re-replayed on their own. Also related: open 20261105T022225Z-lambda-sesv2-lambda-invoke-refuses-every-real-invocation.md (the older 501 refusal); Invoke now answers 200, so this is the next layer, not that refusal.

Traces: the five failing checks listed in trace_ids, all CFN-deployed lambda+sesv2 scenarios: cfn-deploy-lambda-sesv2-env-configset-arn-resolution, cfn-deploy-lambda-sesv2-role-trust-positive, cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected, cfn-deploy-lambda-sesv2-wrong-arn-negative, cfn-stack-update-lambda-sesv2-permission-add-positive.

## Evidence
Expected (AWS): after the stack reaches CREATE_COMPLETE, lambda Invoke answers HTTP 200 with no X-Amz-Function-Error header and Payload = the handler's own JSON, e.g. {"ok": true, "name": "<the configuration set name the stack deployed>"}; the rejection variants answer 200 with the handler's caught-error JSON naming the SES error code (MessageRejected for an unverified sender, NotFoundException/BadRequestException for a never-created identity, AccessDeniedException before the policy is added). A caller polling invoke for a non-Unhandled FunctionError converges on the first attempt.

Actual (replay): the invoke-retry loop never sees a response whose FunctionError is absent, so each of the five checks is terminated by the harness with the identical detail:
- lambda__sesv2.cfn-deploy-lambda-sesv2-env-configset-arn-resolution — 'Error: Check timed out after 60.0s'
- lambda__sesv2.cfn-deploy-lambda-sesv2-role-trust-positive — 'Error: Check timed out after 60.0s'
- lambda__sesv2.cfn-deploy-lambda-sesv2-role-unverified-sender-message-rejected — 'Error: Check timed out after 60.0s'
- lambda__sesv2.cfn-deploy-lambda-sesv2-wrong-arn-negative — 'Error: Check timed out after 60.0s'
- lambda__sesv2.cfn-stack-update-lambda-sesv2-permission-add-positive — 'Error: Check timed out after 60.0s'
In every case the stack-creation poll (300 s budget) is not what expires: the checks stall inside the _invoke_json/_invoke_once poll, whose own budget is 60.0 s, i.e. on lam.invoke(FunctionName=<fn>, Payload=...). No assertion in any of the five is ever evaluated, so none of the response shapes, SES error codes or Ref-resolved environment values they cover is exercised. Function names, stack names, account ids, request ids and the 60.0 s budget itself are per-run/harness values; the requirement being violated is that Invoke of a deployed python function returns that function's own response with no FunctionError.

## Steps to Reproduce
1. CreateStack with a template declaring: an AWS::SES::ConfigurationSet (or AWS::SES::EmailIdentity), an AWS::IAM::Role trusting lambda.amazonaws.com with an inline policy granting the SESv2 action under test, and an AWS::Lambda::Function with Runtime python3.12, Handler index.handler, Role = Fn::GetAtt of that role, Environment carrying {"Ref": <the SES resource>}, and Code.ZipFile whose handler does `import boto3` then one sesv2 call (get_configuration_set / send_email), returning {"ok": true, ...} on success and {"ok": false, "errorCode": e.response["Error"]["Code"]} on ClientError. Use Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM.
2. Poll DescribeStacks until CREATE_COMPLETE (it is reached).
3. Call lambda Invoke on the deployed function, retrying every 3 s until the response has StatusCode 200 and no FunctionError beginning with 'Unhandled'.
4. Observe that the predicate is never satisfied: every response carries X-Amz-Function-Error: Unhandled and a runtime-authored payload instead of the handler's JSON, so the loop runs to its 60 s budget and the scenario never reaches a verdict. Repeat with a stack update that adds ses:SendEmail to the role (cfn-stack-update-lambda-sesv2-permission-add-positive) and with a function pointed at a never-created identity (cfn-deploy-lambda-sesv2-wrong-arn-negative): same stall.
