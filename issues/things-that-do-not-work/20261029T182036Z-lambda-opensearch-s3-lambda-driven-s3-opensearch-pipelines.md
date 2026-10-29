Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive lambda__opensearch__s3.deploy-iam-role-trusts-s3-service-positive lambda__opensearch__s3.cli-s3-put-object-triggers-lambda-opensearch-index-verify lambda__opensearch__s3.s3-event-triggers-lambda-search-index-update-positive lambda__opensearch__s3.s3-notification-lambda-indexes-object-metadata-opensearch-positive lambda__opensearch__s3.s3-notification-triggers-lambda-opensearch-index-positive lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative
Issue-Key: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative|_run_shell#17|AssertionError|raise AssertionError(
Issue-Key: lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify|_run_shell#16|AssertionError|raise AssertionError(
Issue-Key: lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive|_deploy_cfn#7|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive|_deploy_cfn#7|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: lambda__opensearch__s3.deploy-iam-role-trusts-s3-service-positive|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.cli-s3-put-object-triggers-lambda-opensearch-index-verify|run#26|Timeout|time.sleep(3)
Issue-Key: lambda__opensearch__s3.s3-event-triggers-lambda-search-index-update-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.s3-notification-lambda-indexes-object-metadata-opensearch-positive|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.s3-notification-triggers-lambda-opensearch-index-positive|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative|_expect_rejected#5|AssertionError|raise AssertionError(f"expected {description} to be refused, but it SUCCEEDED")
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# lambda__opensearch__s3: Lambda-driven S3/OpenSearch pipelines are inert — Invoke answers NotImplemented, CFN stacks roll back, S3 notifications never fire, and PutBucketNotificationConfiguration skips the invoke-permission check (5 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue bundles 5 root causes spanning the lambda/opensearch/s3 triple; the leading token of every failing check id is lambda__opensearch__s3. Two open issues describe neighbouring symptoms for OTHER checks — 20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md and 20261025T121424Z-lambda-opensearch-s3-an-s3-objectcreated-notification-with.md — and none of the checks below was marked owned by them. Each cause must be closed before submitting.

Cause 1 of 5: Invoke refuses to run a deployment package, including when called through the AWS CLI.
AWS: Lambda Invoke on a function whose Runtime is python3.12 runs the handler in the deployment package and answers HTTP 200 with StatusCode=200 and the function's response payload in the body (FunctionError set when the handler raised). The AWS CLI `aws lambda invoke --function-name <fn> --payload file://<p> <out>` therefore exits 0 and writes that payload to <out>.
Emulator: the call is refused with the error code `NotImplemented` and the message `Invoke resolved <functionArn> at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator: there is no in-process runtime for it, and no response payload is returned that the function did not produce. InvocationType=DryRun validates an invocation without running the function.` — fixed text with the resolved function ARN, the qualifier and the runtime substituted. Through the CLI this surfaces as exit code 254 and `aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: ...`.
Why: `NotImplemented` is not in Invoke's modelled error set, so no SDK/CLI error handler matches it; every workflow whose effect is produced BY the function body (writing to OpenSearch, reading S3, reporting a permission denial) produces no effect and no payload, and a negative check that expects the function to be DENIED cannot tell denial from non-execution.
Fix: the Invoke handler needs an execution path for a deployment package (or, at minimum, must not answer an unmodelled NotImplemented for a normal RequestResponse invocation).
Traces: lambda__opensearch__s3.cli-lambda-role-policy-detach-blocks-opensearch-access-negative, lambda__opensearch__s3.cli-update-lambda-env-opensearch-endpoint-verify.

Cause 2 of 5: a CloudFormation stack combining Lambda, OpenSearch and S3 resources ends in ROLLBACK_COMPLETE.
AWS: CreateStack for a template whose resources are an AWS::S3::Bucket, an AWS::IAM::Role, an AWS::Lambda::Function and an AWS::OpenSearchService::Domain reaches CREATE_COMPLETE, and every declared resource exists afterwards.
Emulator: the stack transitions to ROLLBACK_COMPLETE, so none of the resources survives and the deployment-order behaviour the checks exist to verify never happens.
Why: a client cannot deploy this (very ordinary) stack at all; whatever single resource provisioner fails takes the whole stack with it, and the client is left with a rolled-back stack instead of a working pipeline.
Fix: the CloudFormation resource provisioners for the declared types (the AWS::OpenSearchService::Domain and AWS::Lambda::Function provisioners in particular) must succeed so the stack converges to CREATE_COMPLETE.
Traces: lambda__opensearch__s3.deploy-lambda-opensearch-s3-stack-positive, lambda__opensearch__s3.deploy-s3-notification-lambda-opensearch-pipeline-positive.

Cause 3 of 5: the role-trust deployment check never produces a response at all.
AWS: a stack that declares an IAM role whose trust policy names s3.amazonaws.com, together with the Lambda function and bucket that use it, converges and the role can be assumed/used by that service; every call in the flow terminates.
Emulator: the check exhausts the 60 s harness cap with no terminal answer, so neither the trust relationship nor its effect can be observed.
Why: a client gets neither success nor a refusal — a deployment appears to hang, which is indistinguishable from a very slow provision.
Fix: same deployment path as cause 2 — the provisioners must reach a terminal stack state rather than leaving the flow pending.
Traces: lambda__opensearch__s3.deploy-iam-role-trusts-s3-service-positive.

Cause 4 of 5: an S3 ObjectCreated notification with a Lambda destination never invokes the function.
AWS: once PutBucketNotificationConfiguration has registered a LambdaFunctionConfiguration for s3:ObjectCreated:* and the function's resource policy allows s3.amazonaws.com to invoke it, a PutObject into the bucket asynchronously invokes the function with the S3 event record, and the function's effect (here, a document indexed into OpenSearch, or a marker object written back to S3) becomes observable shortly after.
Emulator: the PutObject produces no invocation and therefore no effect; the checks poll for the effect and exhaust the 60 s cap with no answer.
Why: the entire event-driven path — the dominant way S3 and Lambda are wired together — is inert. A client sees a registered notification configuration that never delivers anything, with no error to diagnose.
Fix: the S3 PutObject path must deliver matching ObjectCreated events to the registered notification destinations and invoke the Lambda target (which also requires cause 1's execution path for any effect to appear).
Traces: lambda__opensearch__s3.cli-s3-put-object-triggers-lambda-opensearch-index-verify, lambda__opensearch__s3.s3-event-triggers-lambda-search-index-update-positive, lambda__opensearch__s3.s3-notification-lambda-indexes-object-metadata-opensearch-positive, lambda__opensearch__s3.s3-notification-triggers-lambda-opensearch-index-positive.

Cause 5 of 5: PutBucketNotificationConfiguration does not verify that S3 may invoke the destination Lambda function.
AWS: at PutBucketNotificationConfiguration time S3 checks that it can invoke the named destination. Without a resource-based policy statement on the function allowing s3.amazonaws.com to lambda:InvokeFunction (for that source bucket/account), the call is REFUSED with HTTP 400 and the error code `InvalidArgument`, and the notification configuration does NOT persist; after AddPermission grants the invoke, the identical call succeeds.
Emulator: the call SUCCEEDS with no permission check at all, and the configuration persists.
Why: the misconfiguration AWS reports synchronously at configuration time becomes silent — a client believes its trigger is wired up, and the undeliverable configuration is only discovered by the absence of invocations. A negative test for the missing grant cannot be written at all.
Fix: S3's PutBucketNotificationConfiguration handler must resolve each LambdaFunctionConfiguration destination and evaluate the function's resource-based policy for the s3.amazonaws.com principal before persisting, refusing with 400 InvalidArgument when the grant is absent.
Traces: lambda__opensearch__s3.s3-notification-missing-lambda-permission-negative.

## Evidence
Cause 1 — expected the CLI invoke to exit 0 with the handler's payload. Actual: `AssertionError: Command failed (rc=254): cmd: ['aws','--output','json','lambda','invoke','--function-name','sim2real-fn-78f2c66d2c9a','--cli-binary-format','raw-in-base64-out','--payload','file://...','out.json']` / `aws: [ERROR]: An error occurred (NotImplemented) when calling the Invoke operation: Invoke resolved arn:aws:lambda:us-west-2:000000000000:function:sim2real-fn-78f2c66d2c9a at version $LATEST, but running its python3.12 deployment package is not implemented in this emulator...`.
Cause 2 — expected StackStatus CREATE_COMPLETE. Actual: `AssertionError: Stack sim2real-xsvc-stk-b9e6d23dc26d ended in ROLLBACK_COMPLETE` and `Stack sim2real-xsvc-stk-8cc2e6ada5c7 ended in ROLLBACK_COMPLETE`.
Cause 3 — expected a terminal result. Actual: `Error: Check timed out after 60.0s`.
Cause 4 — expected the indexed document / marker object to appear after PutObject. Actual: `Error: Check timed out after 60.0s` for all four checks; no invocation and no effect.
Cause 5 — expected code `InvalidArgument` (HTTP 400) and no persisted configuration. Actual: `AssertionError: expected PutBucketNotificationConfiguration for a Lambda destination with no s3.amazonaws.com invoke permission to be refused, but it SUCCEEDED`.

## Steps to Reproduce
1. Cause 1: CreateFunction (Runtime=python3.12) with a zip handler, then `aws lambda invoke --function-name <fn> --cli-binary-format raw-in-base64-out --payload file://payload.json out.json` — the CLI exits 254 with (NotImplemented).
2. Cause 2: CreateStack with a template declaring AWS::S3::Bucket + AWS::IAM::Role + AWS::Lambda::Function + AWS::OpenSearchService::Domain and poll DescribeStacks — the stack reaches ROLLBACK_COMPLETE.
3. Cause 3: deploy the same shape of stack whose IAM role trust policy names s3.amazonaws.com and wait for the flow to terminate — it never does inside 60 s.
4. Cause 4: CreateBucket; CreateFunction; AddPermission(Principal='s3.amazonaws.com', Action='lambda:InvokeFunction', SourceArn=<bucket arn>); PutBucketNotificationConfiguration with a LambdaFunctionConfiguration for s3:ObjectCreated:*; PutObject into the bucket; then poll for the function's effect — nothing arrives.
5. Cause 5: same as 4 but SKIP the AddPermission call — PutBucketNotificationConfiguration returns success instead of 400 InvalidArgument, and GetBucketNotificationConfiguration shows the configuration persisted.
