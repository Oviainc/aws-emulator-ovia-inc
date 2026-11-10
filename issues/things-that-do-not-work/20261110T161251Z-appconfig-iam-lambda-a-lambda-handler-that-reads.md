Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler
Issue-Key: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler|_invoke_until#27|Timeout|time.sleep(5.0)
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# appconfig__iam__lambda: a Lambda handler that reads AppConfig never returns an answer, so neither the permitted role's read nor the under-privileged role's AccessDeniedException is ever observed (check burns the whole deadline)

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the in-Lambda AppConfig read path produces no verdict at all. A function deployed with an execution role that grants appconfig read actions, invoked synchronously, never answers with the deployed configuration, and its sibling whose role grants no AppConfig action never answers with a denial either, so the authorization contract reaches no verdict before the harness deadline (60s) expires.
AWS: a synchronous Invoke (RequestResponse) runs the deployment package and returns StatusCode 200 with the handler's payload; a handler that calls AppConfigData StartConfigurationSession/GetLatestConfiguration under an execution role holding the appconfig read actions receives the deployed document, and the same handler under a role granting no AppConfig action receives an IAM denial the SDK surfaces as AccessDeniedException (HTTP 403, fixed text 'User: <principal arn> is not authorized to perform: <action> on resource: <resource arn> because no identity-based policy allows the <action> action', with the principal, action and resource taken from the request), which the handler catches and reports as its fallback answer. Nothing here is a long-running operation: the check's strategy is a 0-minute, 100%-growth, 0-bake deployment strategy, so the deployed configuration is available immediately.
Emulator: the check makes no observable progress — it is polling Invoke in a bounded loop and times out at the harness ceiling of 60s without ever seeing either the AppConfig-sourced answer or the denied answer. The emulator's Lambda data plane is known not to execute a deployment package (see open issues 20261103T165236Z-lambda-invoke-does-not-execute-the-deployment-package.md and 20261029T182228Z-events-lambda-sqs-6-of-the-7-causes-remain-open.md, neither of which carries this check's replay key), and AppConfig control-plane calls made with a Lambda execution role are not IAM-authorized elsewhere either (20261107T132205Z-apigateway-cognito-idp-lambda-2-net-new-causes.md), so the authorization distinction cannot be made.
Why: a client that gates a feature flag read inside a Lambda on its execution role's AppConfig permissions cannot test either outcome: the permitted function never receives the deployed document, and the under-privileged one is never denied, so an over-permissive role is indistinguishable from a correct one and a handler's fallback path is never exercised.
Fix: the Lambda Invoke (RequestResponse) execution path, so a deployed package actually runs and returns its payload, and the IAM authorization of AppConfigData/AppConfig reads made with a Lambda execution role, so a role with no appconfig actions is refused AccessDeniedException.
Traces: appconfig__iam__lambda.execution-role-without-appconfig-actions-denied-in-handler.

## Evidence
Expected: lam.invoke(FunctionName=<permitted fn>) returns StatusCode 200 with no FunctionError and a payload reporting source='appconfig' and the deployed document {"useNewBackend": true}; lam.invoke(FunctionName=<denied fn>) returns StatusCode 200 with a payload reporting source='default', code='AccessDeniedException' and the handler's fallback {"useNewBackend": false}.
Actual: 'Check timed out after 60.0s' — the bounded invoke loop never observed either answer. The check's own deployment uses DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, so no wall-clock-bound AppConfig rollout is being waited on.

## Steps to Reproduce
1. Create two IAM roles with the Lambda trust policy: one whose inline policy grants appconfig:StartConfigurationSession/appconfig:GetLatestConfiguration (plus GetConfiguration), one granting no appconfig action at all.
2. Create the same AppConfig-reading function twice, once per role, and deploy a hosted configuration version with CreateDeploymentStrategy(DeploymentDurationInMinutes=0, GrowthFactor=100, GrowthType=LINEAR, FinalBakeTimeInMinutes=0, ReplicateTo=NONE) + StartDeployment.
3. Call Invoke (RequestResponse) on the permitted function: expect StatusCode 200 and a payload sourced from AppConfig carrying the deployed document.
4. Call Invoke on the under-privileged function: expect StatusCode 200 and a payload reporting the handler caught AccessDeniedException and fell back to its default.
5. Observe neither answer is ever produced and the scenario consumes the whole deadline.
