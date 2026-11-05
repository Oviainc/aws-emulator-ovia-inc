Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-update-positive apigateway__cloudformation__lambda.cfn-custom-resource-signals-success-updates-stack-status-positive apigateway__cloudformation__lambda.cfn-nested-stack-lambda-provider-reuse-positive apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-delete-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-update-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-signals-success-updates-stack-status-positive|run#9|KeyError|marker_matches = [o["OutputValue"] for o in outputs if o["OutputKey"] == "MarkerOutput"]
Issue-Key: apigateway__cloudformation__lambda.cfn-nested-stack-lambda-provider-reuse-positive|_deploy_cfn#8|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE"
Issue-Key: apigateway__cloudformation__lambda.cfn-stack-rollback-on-custom-resource-failure|run#8|AssertionError|raise AssertionError(f"expected ROLLBACK_COMPLETE, got {status}. {detail}")
Issue-Key: apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected|run#4|AssertionError|assert described["Status"] == "FAILED", (
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# apigateway__cloudformation__lambda: 3 net-new causes — DescribeStacks Outputs omit OutputValue, a custom-resource FAILED/denied provider never rolls the stack back, and an invalid SAM document reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three distinct root causes, all reached through CloudFormation stacks that carry Lambda-backed custom resources. None of these failing checks is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 3: DescribeStacks returns stack Outputs entries without OutputValue.
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model) — DescribeStacks -> Stack.Outputs: list<Output>, and com.amazonaws.cloudformation#Output has members OutputKey, OutputValue, Description, ExportName. AWS populates OutputValue with the resolved value of every template Output; a client reads Outputs[i]["OutputValue"] unconditionally.
Emulator: each Output entry is serialized with OutputKey but no OutputValue key at all, so a client indexing the member raises KeyError: 'OutputValue' immediately after the stack reaches CREATE_COMPLETE.
Why: every consumer of a stack's declared outputs breaks — a client cannot learn the function name, ARN, table name or endpoint the template published, so no stack output can be used to address the resources the stack created. The failure is a hard client-side crash, not a degraded value.
Fix: the CloudFormation DescribeStacks response serializer / the stack-output resolution in the stack state that builds Stack.Outputs. An open issue describes the same symptom in other scenarios (20261104T232822Z-cloudformation-lambda-s3-sqs-2-net-new-root.md and 20261102T090835Z-dynamodb-lambda-logs-describestacks-outputs-entries-omit-outputvalue-so-a-stack.md); it does not carry these checks' replay keys.
Traces: apigateway__cloudformation__lambda.cfn-custom-resource-invokes-lambda-create-positive, .cfn-custom-resource-invokes-lambda-delete-positive, .cfn-custom-resource-invokes-lambda-update-positive, .cfn-custom-resource-signals-success-updates-stack-status-positive, .cfn-nested-stack-lambda-provider-reuse-positive.

Cause 2 of 3: a custom resource that fails is treated as a success — the stack ends CREATE_COMPLETE instead of rolling back.
AWS: recorded behavior only (CloudFormation custom-resource protocol). When the provider Lambda cannot be invoked (the ServiceToken function has no resource policy admitting cloudformation.amazonaws.com) or when the provider PUTs a response document whose Status is "FAILED", CloudFormation marks the custom resource CREATE_FAILED and drives the stack to ROLLBACK_IN_PROGRESS and then ROLLBACK_COMPLETE.
Emulator: the stack and every resource in it reach CREATE_COMPLETE. The replay shows the full event stream as "<stack>=CREATE_COMPLETE: | MyResource=CREATE_COMPLETE: | MyResource=CREATE_IN_PROGRESS:Resource creation Initiated | ProviderFunction=CREATE_COMPLETE: ..." — the custom resource is completed without the provider ever being consulted, so neither the missing invoke permission nor a FAILED response body changes the outcome.
Why: a client that models provisioning failure on stack status sees a green deploy for a stack whose custom resource never ran. Negative deployment tests pass vacuously and rollback behaviour is unobservable.
Fix: the CloudFormation custom-resource provider invocation path and the stack state machine's CREATE_FAILED -> ROLLBACK transition.
Traces: apigateway__cloudformation__lambda.cfn-custom-resource-lambda-missing-servicetoken-permission-rejected, .cfn-stack-rollback-on-custom-resource-failure.

Cause 3 of 3: an invalid SAM (AWS::Serverless-2016-10-31) document is accepted by CreateChangeSet and the change set reaches CREATE_COMPLETE.
AWS: recorded behavior only. A change set whose template fails the SAM transform (here: an AWS::Serverless::Function with a malformed Policies entry) is described with Status FAILED and a StatusReason naming the transform failure; it is never CREATE_COMPLETE.
Emulator: DescribeChangeSet reports Status CREATE_COMPLETE for the malformed document, so the error taxonomy for invalid SAM documents is never exercised.
Why: a client relying on change-set Status to gate `aws cloudformation execute-change-set` executes a document AWS would have refused, and never sees the transform's diagnosis.
Fix: the SAM/transform validation in the CreateChangeSet handler and the change-set status it stores.
Traces: apigateway__cloudformation__lambda.sam-invalid-document-error-taxonomy-rejected.

## Evidence
Cause 1 — expected: Stack.Outputs[i] carries OutputValue (Smithy com.amazonaws.cloudformation#Output). Actual: `KeyError: 'OutputValue'` raised from the check immediately after the stack settles (replay, check cfn-custom-resource-invokes-lambda-create-positive line 23, and 4 sibling checks).

Cause 2 — expected: stack status ROLLBACK_COMPLETE. Actual: `AssertionError: expected ROLLBACK_COMPLETE, got CREATE_COMPLETE. <stack>=CREATE_COMPLETE: | MyResource=CREATE_COMPLETE: | MyResource=CREATE_IN_PROGRESS:Resource creation Initiated | MyResource=CREATE_IN_PROGRESS: | ProviderFunction=CREATE_COMPLETE: | ProviderFunction=CREATE_IN_PROGRESS:Resource creation Initiated | AlwaysTopic=CREATE_COMPLETE:` (stack name is a per-run value; the status sequence is the requirement).

Cause 3 — expected: DescribeChangeSet Status == "FAILED". Actual: `AssertionError: leg bad_policy_name: an invalid SAM document must fail the change set, not reach 'CREATE_COMPLETE'`.

## Steps to Reproduce
1. Create a stack whose template declares a Lambda function (the custom-resource provider), an AWS::Lambda::Permission for cloudformation.amazonaws.com, a Custom::X resource with ServiceToken pointing at that function, and an Outputs section exporting the function ARN.
2. Wait for CREATE_COMPLETE and call DescribeStacks; read Stacks[0].Outputs[0]["OutputValue"] — the member is absent (cause 1).
3. Repeat with the AWS::Lambda::Permission removed (or with a provider that PUTs a response document whose Status is "FAILED"); poll DescribeStacks for the terminal status — it is CREATE_COMPLETE, not ROLLBACK_COMPLETE (cause 2).
4. Call CreateChangeSet with Capabilities CAPABILITY_AUTO_EXPAND on a SAM template whose AWS::Serverless::Function carries an invalid Policies entry, then poll DescribeChangeSet — Status is CREATE_COMPLETE, not FAILED (cause 3).
