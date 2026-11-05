Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete cloudformation.stack-rollback-events-golden-failed-and-cancelled cloudformation.stack-rollback-on-resource-failure cloudformation.cdk-function-default-props-grant-real-log-writes
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|_cdk#17|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_cdk#29|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write|_cdk#25|AssertionError|assert result.returncode == expect_rc, (
Issue-Key: cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier|run#29|AssertionError|assert reason == expected_reason, (
Issue-Key: cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config|run#24|AssertionError|assert handler_message == _EXPECTED_HANDLER_MESSAGE, (
Issue-Key: cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle|_invoke#40|AssertionError|assert "FunctionError" not in response, (
Issue-Key: cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete|run#21|AssertionError|assert "DELETE_SKIPPED" in _resource_states(cfn, stacks["reoc-ok"], "P"), (
Issue-Key: cloudformation.stack-rollback-events-golden-failed-and-cancelled|run#6|AssertionError|assert _of(rows, name) == [
Issue-Key: cloudformation.stack-rollback-on-resource-failure|run#20|AssertionError|create_in_progress_ts < wait_failed_ts < rollback_in_progress_ts <= rollback_complete_ts
Issue-Key: cloudformation.cdk-function-default-props-grant-real-log-writes|_poll_until#34|Timeout|time.sleep(interval_s)
Issue-Commit: 269019b5b42949e66be9fce8117dfdac4ac9abf5

# cloudformation: 8 net-new root causes in the provisioning and rollback path — over-long AWS::Lambda::Permission StatementId fails every CDK REST-API deploy, Rule assertion text re-worded, reserved Lambda env key not refused, no boto3 in the Lambda runtime, RetainExceptOnCreate deletes on stack delete, rollback events and their timestamps diverge

> _Note: this issue was identified against the code base at commit `269019b5b42949e66be9fce8117dfdac4ac9abf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight net-new behavioral root causes of service `cloudformation`, grounded in nine failing behavioral checks. A fixer must close all eight before submitting.

Cause 1 of 8: CloudFormation generates an AWS::Lambda::Permission physical id / StatementId that exceeds Lambda's StatementId limit instead of truncating it, so every stack containing a CDK-synthesised AWS::Lambda::Permission fails to create.
AWS: Smithy (lambda) AddPermission.StatementId -> com.amazonaws.lambda#StatementId, @length min 1 max 100, @pattern ^([a-zA-Z0-9-_]+)$. CloudFormation itself mints the StatementId for an AWS::Lambda::Permission whose template omits one, composing it from the stack name, the logical id and a random suffix, and keeps the result inside that 100-character limit; a CDK `RestApi`/`LambdaRestApi` stack (whose `ApiPermission.Test.<...>` logical ids are long by construction) therefore reaches CREATE_COMPLETE on AWS. The refusal AWS would emit if the value really were too long is Lambda's standard validation message — fixed text `1 validation error detected: Value '` + the offending value + `' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100`, the only variable part being the value echoed from the request — surfaced by the resource handler as HandlerErrorCode `InvalidRequest`.
Emulator: the generated id is not truncated: `sim2real-cdktrio-<id>-ApiproxyANYApiPermissionTest<...>-650E61C4D4E5` (well over 100 characters) is passed to Lambda, which refuses it with that validation message; the AWS::Lambda::Permission resource goes CREATE_FAILED with HandlerErrorCode InvalidRequest and `cdk deploy` exits 1 with `DeploymentError: Resource updates failed`.
Why: no CDK app that fronts a Lambda with API Gateway can be deployed at all — the permission is emitted automatically by the L2 construct, so the client cannot avoid it by changing its template. The failure is attributed to the user's template when the id is the emulator's own.
Fix: the physical-id generation for AWS::Lambda::Permission in the cloudformation resource-provisioning layer (the generated-name/physical-id builder), which must bound the generated StatementId to Lambda's modeled 100-character limit before calling AddPermission.
Traces: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write. The two cdk-restapi checks carry replay keys on the open issue 20261029T214321Z-cloudformation-five-behavioral-checks-exhaust-the-harness-s.md, which describes them as never converging inside the harness budget; they now converge and fail on this StatementId refusal instead, so that issue does not cover this cause.

Cause 2 of 8: the rollback reason built from a template Rule's assertion does not reproduce the Rule's AssertDescription verbatim.
AWS: recorded behavior only (the Smithy model carries StackStatusReason as a free string). AWS composes the reason as the fixed text `Parameter validation failed: assertion error: ` + the Rule's AssertDescription copied character-for-character + ` Rollback requested by user.`; when the AssertDescription itself ends in a period, the two periods both survive. The recorded reason is `Parameter validation failed: assertion error: CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' with a recent version of the CDK CLI.. Rollback requested by user.` — the only variable part is the AssertDescription taken from the submitted template.
Emulator: returns `Parameter validation failed: assertion error: CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' with a recent version of the CDK CLI. Rollback requested by user.` — the trailing period of the AssertDescription is swallowed, so the assertion text a client reads back is not the text it declared.
Why: tooling (the CDK CLI among others) matches on its own assertion text to tell a bootstrap-version failure from any other parameter rejection; a reason that silently rewrites that text defeats the match.
Fix: the reason composition for Rule assertion failures in the cloudformation template/parameter validation path — concatenate the AssertDescription unmodified, do not normalise or trim its punctuation.
Traces: cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier.

Cause 3 of 8: a Lambda configuration update that sets a reserved environment-variable key is not refused; a spurious concurrent-update conflict is reported instead.
AWS: recorded behavior only for the message text (the Smithy model's com.amazonaws.lambda#EnvironmentVariables constrains only the key pattern, not the reserved names). UpdateFunctionConfiguration refuses a request whose Environment.Variables contains an AWS-reserved key with the fixed text `Lambda was unable to configure your environment variables because the environment variables you have provided contains reserved keys that are currently not supported for modification. Reserved keys used in this request: ` followed by the reserved key names taken from the request (here `AWS_REGION`); CloudFormation wraps it as the resource's UPDATE_FAILED reason `Resource handler returned message: "<that message>"`.
Emulator: the reserved key is accepted by the validation path and the resource instead fails with `Resource handler returned message: "The operation cannot be performed at this time. An update is in progress for resource: arn:aws:lambda:<region>:<account>:function:<name>" (HandlerErrorCode: ResourceConflict)` — a concurrency error for a single in-flight update.
Why: a client gets the wrong error class (a retryable ResourceConflict instead of a terminal InvalidRequest) and a reason that names no cause, so it retries forever instead of fixing the template; and an update that AWS rejects outright is treated as merely contended.
Fix: the environment-variable validation in the Lambda UpdateFunctionConfiguration handler (reserved-key rejection), and the in-flight-update guard in the cloudformation resource handler that is raising ResourceConflict for a single update.
Traces: cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config.

Cause 4 of 8: the Lambda Python runtime serves no boto3, so any function that imports the AWS SDK fails at module load.
AWS: recorded behavior only — the managed `python3.x` runtimes ship the AWS SDK for Python, so `import boto3` at module scope succeeds and the handler runs.
Emulator: Invoke answers with FunctionError `Unhandled` and payload `{"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","stackTrace":[]}`.
Why: the overwhelmingly common Lambda idiom — a function that calls another AWS service — cannot run, so no CloudFormation scenario whose function touches AWS can be exercised.
Fix: the Lambda Python runtime/bundle used by the invoke path, which must expose boto3 (and botocore) on the import path. The open issue 20261105T111951Z-apigateway-cloudformation-lambda-the-lambda-python-runtime-has.md reports this class under the apigateway__cloudformation__lambda heading; this check is listed under `cloudformation` and is not covered by it.
Traces: cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle.

Cause 5 of 8: DeletionPolicy/UpdateReplacePolicy `RetainExceptOnCreate` is ignored when a successfully-created stack is deleted.
AWS: recorded behavior only (DeletionPolicy is a template attribute, not a modeled member). `RetainExceptOnCreate` retains the resource on a stack DELETE exactly as `Retain` does — the resource's event stream ends with `DELETE_SKIPPED` and the physical resource survives; it is only on the rollback of a FAILED CREATE that the resource is deleted.
Emulator: on the delete of a CREATE_COMPLETE stack the retained resource's events are `CREATE_IN_PROGRESS, CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_IN_PROGRESS, DELETE_COMPLETE` — no DELETE_SKIPPED is ever emitted and the resource is destroyed.
Why: the policy exists precisely to keep data alive past a stack delete; a client that declares it loses the resource it asked to keep, irrecoverably.
Fix: the deletion-policy evaluation in the cloudformation stack-delete path (and the DELETE_SKIPPED event it must emit instead of deleting).
Traces: cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete.

Cause 6 of 8: the stack-level event sequence for a create that fails and rolls back is wrong — an extra CREATE_FAILED event, a ROLLBACK_IN_PROGRESS reason copied from the resource instead of the aggregate, and a reason on ROLLBACK_COMPLETE.
AWS: recorded behavior only (StackEvent.ResourceStatusReason is a free string). For a create whose resource fails, the events whose LogicalResourceId is the stack are exactly three: `CREATE_IN_PROGRESS` reason `User Initiated`; `ROLLBACK_IN_PROGRESS` reason with the fixed text `The following resource(s) failed to create: [` + the failed logical ids, comma-separated, taken from the stack + `]. Rollback requested by user.`; and `ROLLBACK_COMPLETE` with NO reason (DescribeStacks likewise carries no StackStatusReason).
Emulator: emits four stack-level events — `CREATE_IN_PROGRESS/User Initiated`, a stack-level `CREATE_FAILED` carrying the resource's own reason `WaitCondition timed out. Received 0 conditions when expecting 1`, `ROLLBACK_IN_PROGRESS` with that same resource reason plus `. Rollback requested by user.`, and `ROLLBACK_COMPLETE` carrying the resource reason as well.
Why: the aggregate `The following resource(s) failed to create: [...]` line is the one event that names which resources failed; a client that reads it to triage a rollback gets a single resource's text instead, cannot enumerate the failures, and sees a terminal status that falsely carries a reason.
Fix: the stack-level event emission in the cloudformation rollback engine (the aggregate failed-resource reason, suppression of a stack-level CREATE_FAILED, and the empty reason on ROLLBACK_COMPLETE).
Traces: cloudformation.stack-rollback-events-golden-failed-and-cancelled.

Cause 7 of 8: stack events emitted at a failure-to-rollback transition share one timestamp instead of being strictly ordered in time.
AWS: recorded behavior only — StackEvent.Timestamp advances across the sequence, so a resource's CREATE_FAILED is strictly earlier than the stack's ROLLBACK_IN_PROGRESS that it causes.
Emulator: the WaitCondition's CREATE_FAILED and the stack's ROLLBACK_IN_PROGRESS carry the identical timestamp (recorded run: both at the same microsecond), so a consumer ordering events by Timestamp cannot tell cause from effect.
Why: DescribeStackEvents is the only ordering a client has for a rollback post-mortem; identical timestamps make the causal order unrecoverable and break any consumer that sorts by time.
Fix: the event-clock/sequencing in the cloudformation stack-event recorder — each appended event must take a strictly greater timestamp than the previous one.
Traces: cloudformation.stack-rollback-on-resource-failure.

Cause 8 of 8: the CDK default-function-props scenario never reaches a verdict — the deploy-then-read-logs path does not settle inside the suite's per-check budget.
AWS: recorded behavior only — a `cdk deploy` of a single default-props Lambda reaches CREATE_COMPLETE and the function's first invoke lands log events in its implicit log group well inside the budget.
Emulator: the check is cut off with `Error: Check timed out after 60.0s` and no response to compare; the 60 s figure is the suite's budget, not a requirement — the requirement is that the polled surfaces (DescribeStacks StackStatus, then the log group's events) reach a terminal value.
Why: a client waiting on `stack_create_complete` or on log delivery hangs instead of being told what happened.
Fix: whichever polled surface does not settle in the cloudformation stack-provisioning path and the Lambda log-delivery path it waits on.
Traces: cloudformation.cdk-function-default-props-grant-real-log-writes.

## Evidence
Cause 1 — cloudformation.cdk-deployed-l2-trio-serves-http-and-persists [mismatch]:
  AssertionError: `cdk deploy sim2real-cdktrio-<id> --require-approval never` exited 1, expected 0; output tail: Resource handler returned message: "1 validation error detected: Value 'sim2real-cdktrio-<id>-ApiproxyANYApiPermissionTestsim2realcdktrio<id>Api095D9779ANYproxyE1346085-650E61C4D4E5' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100" (HandlerErrorCode: InvalidRequest), for AWS::Lambda::Permission ApiproxyANYApiPermissionTest...
  cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves [mismatch] and cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write [mismatch] fail identically on their own generated StatementIds.
  Expected: CREATE_COMPLETE (AWS truncates the generated StatementId to Lambda's modeled @length max 100).

Cause 2 — cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier [mismatch]:
  rollback reason was "Parameter validation failed: assertion error: CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' with a recent version of the CDK CLI. Rollback requested by user."
  expected the Rule's own assertion message "Parameter validation failed: assertion error: CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' with a recent version of the CDK CLI.. Rollback requested by user."

Cause 3 — cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config [mismatch]:
  got  'Resource handler returned message: "The operation cannot be performed at this time. An update is in progress for resource: arn:aws:lambda:us-west-2:000000000000:function:<fn>" (HandlerErrorCode: ResourceConflict)'
  want 'Resource handler returned message: "Lambda was unable to configure your environment variables because the environment variables you have provided contains reserved keys that are currently not supported for modification. Reserved keys used in this request: AWS_REGION'

Cause 4 — cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle [mismatch]:
  invoking '<fn>' raised 'Unhandled': {"errorMessage":"Unable to import module 'index': No module named 'boto3'","errorType":"Runtime.ImportModuleError","stackTrace":[]}

Cause 5 — cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete [mismatch]:
  AssertionError: a retained resource is DELETE_SKIPPED; P event statuses were ['DELETE_COMPLETE', 'DELETE_IN_PROGRESS', 'CREATE_COMPLETE', 'CREATE_IN_PROGRESS', 'CREATE_IN_PROGRESS']

Cause 6 — cloudformation.stack-rollback-events-golden-failed-and-cancelled [mismatch]:
  stack-level events were [('CREATE_IN_PROGRESS','User Initiated'), ('CREATE_FAILED','WaitCondition timed out. Received 0 conditions when expecting 1'), ('ROLLBACK_IN_PROGRESS','WaitCondition timed out. Received 0 conditions when expecting 1. Rollback requested by user.'), ('ROLLBACK_COMPLETE','WaitCondition timed out. Received 0 conditions when expecting 1')]
  expected [('CREATE_IN_PROGRESS','User Initiated'), ('ROLLBACK_IN_PROGRESS','The following resource(s) failed to create: [Fast, Slow]. Rollback requested by user.'), ('ROLLBACK_COMPLETE', None)]

Cause 7 — cloudformation.stack-rollback-on-resource-failure [mismatch]:
  expected CREATE_IN_PROGRESS < WaitCondition CREATE_FAILED < ROLLBACK_IN_PROGRESS <= ROLLBACK_COMPLETE; the CREATE_FAILED and ROLLBACK_IN_PROGRESS timestamps were byte-identical (…:40.561429+00:00 for both).

Cause 8 — cloudformation.cdk-function-default-props-grant-real-log-writes [mismatch]:
  Error: Check timed out after 60.0s (no response observed).

Smithy consulted: lambda AddPermission.StatementId (com.amazonaws.lambda#StatementId, @length 1..100, @pattern ^([a-zA-Z0-9-_]+)$); lambda Environment/EnvironmentVariables (key @pattern ^[a-zA-Z]([a-zA-Z0-9_])+$ only — reserved-key rejection is recorded behavior, not modeled); cloudformation StackEvent.ResourceStatusReason and Stack.StackStatusReason are free strings, so the reason texts above are recorded behavior.

## Steps to Reproduce
1. Cause 1: `cdk deploy` any stack built from a CDK `LambdaRestApi`/`RestApi` L2 (long construct paths). Observe the AWS::Lambda::Permission resource go CREATE_FAILED with the 'statementId' length validation message and the deploy exit 1. Equivalently: CreateStack with an AWS::Lambda::Permission whose logical id plus stack name exceed 100 characters and no explicit StatementId.
2. Cause 2: deploy a template carrying a `Rules` block whose Assert fails and whose AssertDescription ends in a period; read DescribeStackEvents/DescribeStacks StackStatusReason and compare the assertion text to the declared AssertDescription.
3. Cause 3: UpdateStack an AWS::Lambda::Function so its Environment.Variables contains `AWS_REGION`; read the resource's UPDATE_FAILED ResourceStatusReason.
4. Cause 4: invoke any Lambda whose handler module does `import boto3`.
5. Cause 5: create a stack with a resource carrying `DeletionPolicy: RetainExceptOnCreate`, let it reach CREATE_COMPLETE, DeleteStack, then read that resource's events — expect DELETE_SKIPPED and the physical resource still present.
6. Cause 6/7: create a stack with two AWS::CloudFormation::WaitCondition resources sharing one handle, one with a 1 s Timeout and one with a long one, so the create fails and rolls back; read DescribeStackEvents and compare the stack-level rows, their reasons and their timestamps.
7. Cause 8: `cdk deploy` a single default-props Lambda function and wait for its first invoke's log events.
8. Re-replay: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists, cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves, cloudformation.cdk-restapi-cloudwatch-role-flag-governs-account-global-write, cloudformation.cfn-bootstrap-version-rule-blocks-stale-qualifier, cloudformation.deploy-cdk-update-rejected-by-aws-rolls-back-keeping-prior-config, cloudformation.cdk-synth-refuses-construct-cycle-but-emits-a-cfn-cycle, cloudformation.retain-except-on-create-deletes-on-failed-create-retains-on-delete, cloudformation.stack-rollback-events-golden-failed-and-cancelled, cloudformation.stack-rollback-on-resource-failure, cloudformation.cdk-function-default-props-grant-real-log-writes.
