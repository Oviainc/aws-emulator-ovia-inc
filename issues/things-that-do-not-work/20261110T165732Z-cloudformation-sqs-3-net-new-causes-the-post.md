Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template
Issue-Key: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one|_poll_until#29|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue|_cdk#17|TimeoutExpired|result = subprocess.run(cmd, cwd=work, env=ctx.subprocess_env(), capture_output=True,
Issue-Key: cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# cloudformation__sqs: 3 net-new causes — the post-import tag update never runs, `cdk migrate --from-stack` hangs, and the mismatched-VisibilityTimeout import never settles

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Three distinct net-new root causes in the cloudformation__sqs suite. Each is listed with its own failing check ids; all three must be closed. None of these three checks is marked OWNED or ALREADY FILED by the replay report. The open issue 20261109T185037Z-cloudformation-sqs-5-net-new-causes-resource-import-does-not.md describes neighbouring import/migrate defects and names two of these check ids, but its replay keys no longer match: the failures have MOVED (cause 2 now fails inside `cdk migrate` itself rather than in the later deploy, and cause 3 now never reaches its polling assertion at all), so they are reported as net-new here. Open issues 20261107T014900Z-cloudformation-sqs-the-cdk-import-adoption-flow-now.md and 20261108T061553Z-cloudformation-sqs-an-import-change-set-never-reaches.md cover the IMPORT change set not reaching a verdict; cause 1 below is the step AFTER that, which is not covered by them.

Cause 1 of 3 — after `cdk import` adopts an existing queue, the stack-level tag update never runs, so the imported resource stays at IMPORT_COMPLETE.
AWS: recorded behavior only (resource import is documented at docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/resource-import-existing-stack.html; no Smithy member governs the follow-on update). The recording shows that once an IMPORT change set is executed the stack reaches IMPORT_COMPLETE and CloudFormation then applies the stack-level tags to the adopted resource as an update of its own, so DescribeStackResource for that logical id moves past IMPORT_COMPLETE to `UPDATE_COMPLETE` while PhysicalResourceId stays the pre-existing queue URL and the queue's CreatedTimestamp is unchanged (it was adopted, not recreated).
Emulator: the resource is parked at `ResourceStatus: 'IMPORT_COMPLETE'` and stays there for the whole 180 s window — the tag-propagation update is never started. PhysicalResourceId is correct, so the adoption itself landed; only the follow-on update is missing.
Why: the client's stack-level tags never reach the resource it just adopted, so cost allocation, tag-based IAM conditions and tag-based resource discovery all silently miss the imported resource; and any tool that waits for the resource to leave IMPORT_COMPLETE before continuing hangs.
Fix: the CloudFormation resource-import execution path — after an IMPORT change set completes it must run the stack-level tag update against each adopted resource and publish the resulting per-resource UPDATE_IN_PROGRESS/UPDATE_COMPLETE statuses.
Traces: cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one.

Cause 2 of 3 — `cdk migrate --from-stack` never returns.
AWS: recorded behavior only. The recording shows `cdk migrate --from-stack --stack-name <stack> --language python` reading the deployed stack through the CloudFormation control plane and generating an app within its budget, so the command exits; the generated template then deploys back onto the same stack as an in-place update.
Emulator: the CLI subprocess is killed by its own deadline — `subprocess.TimeoutExpired: Command ['cdk', 'migrate', '--from-stack', '--stack-name', ..., '--language', 'python', ...] timed out after 300.0 seconds`. The migrate step never completes, so nothing downstream (the in-place update, the unchanged physical id, the applied property edit) is exercised. Hypothesis: one of the control-plane calls `cdk migrate --from-stack` makes to read the existing stack (GetTemplate / DescribeStacks / DescribeStackResources / the generated-template family) never answers or never reaches a terminal status, and the CLI retries indefinitely.
Why: a client cannot migrate an existing stack into CDK at all — the command hangs with no output and no way to tell a slow read from a dead one.
Fix: the CloudFormation control-plane read path `cdk migrate --from-stack` drives (the stack/template read operations must answer, and any generated-template status they poll must reach a terminal state).
Traces: cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue.

Cause 3 of 3 — an IMPORT of a queue whose live VisibilityTimeout differs from the template never settles.
AWS: recorded behavior only. The recording shows an IMPORT change set for an AWS::SQS::Queue carrying `DeletionPolicy: Retain` and a template VisibilityTimeout that differs from the live queue's: executing it reconciles the live resource to the template, so GetQueueAttributes reads the template value afterwards, and the stack reaches IMPORT_COMPLETE within the check's budget.
Emulator: the check reaches no assertion at all — `Error: Check timed out after 60.0s`. The import flow does not settle, so neither the stack status nor the reconciled attribute is ever observed. (This is a regression in observable shape from the previously filed form of this defect, where the import DID settle and merely left the attribute at the live value: the flow now hangs instead.)
Why: a client importing an existing queue gets a command that never finishes and a stack whose status it cannot read, so it cannot tell whether the resource was adopted, whether its template was applied, or whether it is safe to retry.
Fix: the IMPORT change-set execution path for AWS::SQS::Queue — it must reach a terminal stack status and apply the template's properties to the adopted queue.
Traces: cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template.

## Evidence
Cause 1 (check cloudformation__sqs.cdk-import-adopts-an-existing-queue-and-refuses-a-missing-one, verdict mismatch) — expected DescribeStackResource for logical id `Queue` to report `UPDATE_COMPLETE` with the pre-existing queue URL:
```
AssertionError: the imported queue's stack-level tag update on sim2real-cdk-imp-... not met within 180.0s. Last result: {'LogicalResourceId': 'Queue', 'PhysicalResourceId': 'https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-cdk-imp-...', 'ResourceType': 'AWS::SQS::Queue', 'ResourceStatus': 'IMPORT_COMPLETE', ...}
```

Cause 2 (check cloudformation__sqs.cdk-migrate-from-stack-deploy-updates-in-place-without-replacing-the-queue, verdict error):
```
subprocess.TimeoutExpired: Command '['cdk', 'migrate', '--from-stack', '--stack-name', 'sim2real-cdkmig-...', '--language', 'python', '--output-path', ..., '--ci', '--no-version-reporting', '--no-notices', '--no-color']' timed out after 300.0 seconds
```

Cause 3 (check cloudformation__sqs.cfn-resource-import-with-mismatched-visibility-timeout-reconciles-live-value-to-template, verdict mismatch) — live VisibilityTimeout 30, template 90:
```
Error: Check timed out after 60.0s
```

Stack names, queue URLs, stack ids and timestamps in the above are per-run values, not requirements.

## Steps to Reproduce
Cause 1: `cdk deploy` a stack with no queue; create a queue out of band; `cdk import <stack> --resource-mapping-inline '{"Queue":{"QueueUrl":"<url>"}}'`; then poll DescribeStackResource(LogicalResourceId='Queue') — it never leaves IMPORT_COMPLETE.
Cause 2: deploy a stack containing an SQS queue, then run `cdk migrate --from-stack --stack-name <stack> --language python --output-path <dir> --ci` — the command does not return.
Cause 3: create a queue with VisibilityTimeout 30; CreateChangeSet(ChangeSetType='IMPORT') with a template declaring that queue with DeletionPolicy Retain and VisibilityTimeout 90; execute it and poll DescribeStacks plus GetQueueAttributes — the flow never settles.
