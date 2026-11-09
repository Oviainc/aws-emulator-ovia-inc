Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue cloudformation__sqs.cdk-refactor-renames-a-construct-keeping-the-queues-physical-id-and-messages cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template cloudformation__sqs.cfn-resource-import-wrong-identifier-key-rejected-before-change-set-exists cloudformation__sqs.cfn-sqs-queue-create-visibility-timeout-over-limit-rejected cloudformation__sqs.cfn-sqs-queuepolicy-invalid-action-rejected
Issue-Key: cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue|_cdk#17|Timeout|result = subprocess.run(cmd, cwd=work, env=ctx.subprocess_env(), capture_output=True,
Issue-Key: cloudformation__sqs.cdk-refactor-renames-a-construct-keeping-the-queues-physical-id-and-messages|run#30|AssertionError|assert after["LogicalResourceId"].startswith(_NEW_ID), (
Issue-Key: cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudformation__sqs.cfn-resource-import-wrong-identifier-key-rejected-before-change-set-exists|expect_error#7|AssertionError|raise AssertionError(f"expected {code} but the call succeeded: {result!r}")
Issue-Key: cloudformation__sqs.cfn-sqs-queue-create-visibility-timeout-over-limit-rejected|_resource_failure#9|AssertionError|raise AssertionError(f"could not parse handler reason: {reason!r}")
Issue-Key: cloudformation__sqs.cfn-sqs-queuepolicy-invalid-action-rejected|run#10|AssertionError|assert m is not None, f"not an SQS service error: {message!r}"
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# 5 net-new causes: resource import does not reconcile or validate, cdk refactor/migrate do not take effect, and SQS handler failure reasons omit the service-error suffix

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Five distinct root causes in the cloudformation__sqs suite. Each is listed with its own failing check ids; all five must be closed.

Cause 1 of 5 — `cdk migrate`/deploy of a migrated stack never completes. AWS: deploying the stack a `cdk migrate --from-stack` app synthesizes updates the existing queue in place (same physical id, no replacement) and the deploy returns. Emulator: the check never gets that far — 'Error: Check timed out after 180.0s' with no terminal stack status, so the migrate-then-deploy path hangs. Why: an adoption/migration workflow that AWS completes in seconds never returns, and the client cannot tell a hung update from a slow one. Fix: the UpdateStack path taken by a cdk-migrate-generated template (the stack-update state machine must reach a terminal status). Traces: cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue.

Cause 2 of 5 — a stack refactor does not rename the resource's logical id. AWS: after `cdk refactor` moves a construct, the queue keeps its physical id and messages but its logical id in the stack becomes the NEW construct path's id (here one starting 'QueueRenamed'). Emulator: the logical id is still 'Queue4A7E3555' — the refactor reported success but the stack's resource mapping was not rewritten. Why: a client's stack drifts from its app: subsequent deploys see the old logical id and will delete-and-recreate the resource the refactor was meant to preserve. Fix: the stack-refactor execution path's logical-id remapping in the CloudFormation state. Traces: cloudformation__sqs.cdk-refactor-renames-a-construct-keeping-the-queues-physical-id-and-messages.

Cause 3 of 5 — resource import does not reconcile the live resource to the template. AWS: importing an existing queue whose live VisibilityTimeout (30) differs from the template's (90) makes the import apply the template value, so GetQueueAttributes reads 90 afterwards. Emulator: the attribute stays '30' for the whole 120s window — the import records ownership without applying the template. Why: after an import the client's template and the real resource disagree, silently, which is the one thing import exists to prevent. Fix: the resource-import execution path for AWS::SQS::Queue (apply the template properties to the adopted resource). Traces: cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template.

Cause 4 of 5 — an import change set with a wrong ResourceIdentifier key is accepted. AWS: CreateChangeSet(ChangeSetType='IMPORT') whose ResourcesToImport uses an identifier key that is not the resource type's primary identifier is refused with ValidationError (HTTP 400) and no change set is created. Emulator: the call succeeds and returns a ChangeSetId/StackId with HTTP 200. Why: a malformed import is accepted and only fails (or silently adopts the wrong thing) at execution, where the blast radius is a live resource. Fix: ResourcesToImport validation in the CreateChangeSet handler. Traces: cloudformation__sqs.cfn-resource-import-wrong-identifier-key-rejected-before-change-set-exists.

Cause 5 of 5 — AWS::SQS::* resource-handler failure reasons omit the service-error suffix. AWS: a resource handler that failed because the downstream service refused the call reports ResourceStatusReason of the form 'Resource handler returned message: "<service message> (Service: Sqs, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the fixed parts are 'Resource handler returned message:', '(Service: Sqs, Status Code: 400, Request ID: ...)', 'RequestToken:' and 'HandlerErrorCode:'; the variable parts are the service message, the status code, and the ids. Emulator: the suffix is absent — e.g. 'Resource handler returned message: "Invalid value for the parameter VisibilityTimeout." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' and 'Value sqs:NotARealAction for parameter ActionName is invalid. Reason: Please refer to the appropriate WSDL for a list of valid actions.' with no (Service: Sqs, Status Code: 400, Request ID: ...) part, so a client parsing the documented reason shape finds no match. Why: tooling and humans read the service/status/request-id out of this reason to attribute a stack failure; without it the failure cannot be traced to the SQS call that caused it. Fix: the resource-handler error formatter used by the AWS::SQS::Queue / AWS::SQS::QueuePolicy handlers. Traces: cloudformation__sqs.cfn-sqs-queue-create-visibility-timeout-over-limit-rejected, cloudformation__sqs.cfn-sqs-queuepolicy-invalid-action-rejected.

## Evidence
Cause 1: Error: Check timed out after 180.0s (cdk-migrate-from-stack-deploy...)
Cause 2: AssertionError: after `cdk refactor`, the queue's logical id is 'Queue4A7E3555', expected it to start with 'QueueRenamed'
Cause 3: AssertionError: VisibilityTimeout on https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-xsvc-imp-recon-... to read '90' not met within 120.0s. Last result: '30'
Cause 4: AssertionError: expected ValidationError but the call succeeded: {'Id': 'arn:aws:cloudformation:...:changeSet/...', 'StackId': '...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Cause 5: AssertionError: could not parse handler reason: 'Resource handler returned message: "Invalid value for the parameter VisibilityTimeout." (RequestToken: ..., HandlerErrorCode: InvalidRequest)' — expected ('Invalid value for the parameter VisibilityTimeout.', 'Sqs', '400', 'InvalidRequest') parsed from a '(Service: Sqs, Status Code: 400, Request ID: <id>)' suffix; and AssertionError: not an SQS service error: 'Value sqs:NotARealAction for parameter ActionName is invalid. Reason: Please refer to the appropriate WSDL for a list of valid actions.'

## Steps to Reproduce
Cause 1: `cdk migrate --from-stack` an existing queue stack, then `cdk deploy` the generated app.
Cause 2: `cdk refactor` renaming a queue construct; then DescribeStackResources and read the queue's LogicalResourceId.
Cause 3: create a queue with VisibilityTimeout 30, import it with a template saying 90, then GetQueueAttributes.
Cause 4: CreateChangeSet(ChangeSetType='IMPORT') with a ResourcesToImport entry keyed by a non-primary identifier.
Cause 5: CreateStack with AWS::SQS::Queue VisibilityTimeout above the SQS maximum (and separately an AWS::SQS::QueuePolicy with Action 'sqs:NotARealAction'); read the CREATE_FAILED ResourceStatusReason.
