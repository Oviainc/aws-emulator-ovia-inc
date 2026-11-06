Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle
Issue-Key: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle|_pre_invoke#7|Timeout|if ddb.get_item(TableName=est["table"], Key={"image_name": {"S": probe}},
Issue-Commit: aa8a353620cb5955c93b9fc60be9de25d17574bd

# dynamodb__lambda__rekognition__s3: the Rekognition S3-trigger scenario never reaches a verdict — the run hangs in a poll until the harness cap instead of settling (stack terminal status / S3 ObjectCreated -> Lambda delivery)

> _Note: this issue was identified against the code base at commit `aa8a353620cb5955c93b9fc60be9de25d17574bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the end-to-end Rekognition/S3-trigger scenario makes no observable progress — every poll it issues keeps returning a non-terminal answer, so the check is killed by the harness cap (60 s) with no status, no error code and no verdict from the emulator.

AWS: the scenario's CloudFormation stack (AWS::DynamoDB::Table + AWS::IAM::Role + AWS::Lambda::Function nodejs20.x + AWS::Lambda::Permission + AWS::S3::Bucket with three suffix-filtered LambdaConfigurations) settles into a TERMINAL StackStatus — Smithy: cloudformation StackStatus enum includes CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED; a create always leaves *_IN_PROGRESS for one of those, and DescribeStacks reports it. On the success path (CREATE_COMPLETE) a PutObject of a key matching a stored LambdaFunctionConfigurations suffix rule invokes the target function, which writes the DynamoDB row the next phase reads (recorded behavior only for the arming latency; AWS documents notification arming as 'usually about five minutes' with no stated bound). Either way the caller gets a decidable answer: a terminal stack status, or a delivered event.

Emulator: the check produces only `Error: Check timed out after 60.0s`. Nothing was refused and nothing succeeded: the emulator answered every poll with a value that is neither the terminal state nor the delivered effect, so the scenario stalls. The two possible stall points are both the emulator's and both are poll loops whose budgets (600 s for the stack terminal state, 600 s for the notification-arming probe) far exceed the harness cap: (a) DescribeStacks never reports a terminal StackStatus for the created stack, or (b) the stack completes and the three installed s3:ObjectCreated:* -> Lambda configurations never deliver, so the probe row never appears in the labels table. Hypothesis: (b) is the active stall — the two sibling checks in this same service heading (same-key-overwrite-invokes-the-notification-target-a-second-time, suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered) time out the same way at 60 s on exactly that delivery, which shows S3 object-write notification dispatch is inert in this build.

Related open issue: 20261103T031416Z-dynamodb-lambda-rekognition-s3-7-net-new-causes.md names this check under its Cause 1 (stack reached ROLLBACK_COMPLETE) and under its Cause 6 (notifications never delivered), but it does NOT carry this check's current replay key — the failure mode has changed from a fast ROLLBACK_COMPLETE assertion to a hang, so the replay report lists it as unowned. It is filed here so the hang is re-replayable; a fixer closing the related issue should re-run this check id as well.

Why: a client deploying this reference architecture gets neither a working pipeline nor a failure it can act on. The deploy call returns, the uploads return 200, and then nothing ever happens: no stack verdict and/or no invocation, so the client cannot distinguish 'still arming', 'filtered out' and 'permanently broken'. Every downstream behaviour in the scenario (the stale conditional-put row, the swallowed ConditionalCheckFailedException in the handler's log group, the diagnosis sequence, the restore and the stack destroy) is unreachable, and an automated deployer waits forever instead of erroring.

Fix: two components, whichever is stalling — cloudformation's stack create driver and status projection (it must advance every created stack to a terminal StackStatus and report it through DescribeStacks), and s3's object-write notification dispatcher (match the stored LambdaFunctionConfigurations suffix rules on PutObject and invoke the Lambda destination so the handler runs). File and line citations belong with the handlers for those two paths.

Traces: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle

## Evidence
Expected: the deploy phase reads a terminal StackStatus from DescribeStacks (CREATE_COMPLETE on the success path), and the pre_invoke phase sees the probe upload's row appear in the labels table because the s3:ObjectCreated:* suffix-filtered configuration invoked the handler; the check then proceeds through solve / post_invoke / cleanup / destroy to a verdict.
Actual: the check yields no emulator status or error code at all — the only output is `Error: Check timed out after 60.0s`. No terminal stack status and/or no notification delivery was ever observed; the scenario is stuck inside a poll loop (stack-terminal poll, 600 s budget; notification-arming probe loop, 600 s budget), both of which outlast the 60 s harness cap.
Corroboration that the delivery leg is inert: the sibling checks dynamodb__lambda__rekognition__s3.same-key-overwrite-invokes-the-notification-target-a-second-time and dynamodb__lambda__rekognition__s3.suffix-filter-is-case-sensitive-so-an-uppercase-key-is-not-delivered also report `Check timed out after 60.0s` waiting for an S3-triggered Lambda invocation.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the check id in trace_ids.
2. CreateStack with the scenario template (DynamoDB table with ProvisionedThroughput 5/5, IAM role, nodejs20.x function from Code.ZipFile, AWS::Lambda::Permission for s3.amazonaws.com scoped to this account and bucket ARN, and an S3 bucket whose NotificationConfiguration declares three s3:ObjectCreated:* LambdaConfigurations filtered on suffix .jpg/.jpeg/.png).
3. Poll DescribeStacks and record the StackStatus sequence: assert it reaches one of CREATE_COMPLETE / CREATE_FAILED / ROLLBACK_COMPLETE / ROLLBACK_FAILED rather than remaining *_IN_PROGRESS.
4. If the stack completes, PutObject a key ending in .jpg into the stack's bucket and poll GetItem (ConsistentRead) on the labels table for that key's row; the row must appear because the notification invoked the handler.
5. Observe that one of steps 3/4 never converges, so the scenario is killed by the harness cap with no emulator-side status or error.
