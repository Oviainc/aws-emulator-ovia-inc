Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
Issue-Key: events__lambda__sns__sqs.lambda-sdk-publish-to-sns|_poll_until#11|Timeout|last = probe()
Issue-Commit: bd416c0ab091bf65c958786969d0a3369dfa0e54

# events__lambda__sns__sqs: the granted in-function boto3 sns:Publish -> SNS -> SQS happy path still reaches no verdict (check cut off at 60 s)

> _Note: this issue was identified against the code base at commit `bd416c0ab091bf65c958786969d0a3369dfa0e54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: a python3.12 Lambda whose execution role carries an explicit `sns:Publish` grant on a topic calls `boto3.client('sns').publish(TopicArn=..., Message='from-lambda-sdk')` from its handler, and the topic has an `sqs`-protocol subscription whose queue policy admits `sns.amazonaws.com`. Neither the invoke nor the fan-out ever converges: the check is cut off with `Check timed out after 60.0s` and no assertion is ever reached. Failing check: events__lambda__sns__sqs.lambda-sdk-publish-to-sns.

AWS: Smithy (com.amazonaws.sns#Publish) takes `TopicArn` plus the required `Message` and returns `PublishResponse.MessageId`; its modeled refusals are AuthorizationErrorException (403, awsQueryError code `AuthorizationError`), NotFoundException (404, `NotFound`), InvalidParameterException (400, `InvalidParameter`) and the KMS family — none of which applies to a caller whose identity policy allows `sns:Publish` on that topic, so the call answers 200 with a MessageId. Lambda `Invoke` with `InvocationType=RequestResponse` answers `StatusCode=200` with NO `FunctionError` member (and no `x-amz-function-error` header) when the handler returns normally; `FunctionError: Unhandled` is documented only for an error raised while running the function. The managed Python runtimes bundle the AWS SDK for Python, so `import boto3` resolves for a deployment package containing only the handler module, and the runtime injects the execution role's credentials so a client built inside the handler is signed as that role. Recorded behavior for the fan-out: the `sqs`-protocol subscriber receives the notification and the envelope's `Message` member equals the published Message verbatim.

Emulator: the check reaches no verdict within its 60 s budget; the replay output carries no traceback frame at all, only `Check timed out after 60.0s`. The check's own bounded re-invoke loop (60 s budget, 5 s interval, described as "Lambda's sns:Publish succeeding once its role grant propagates") exists only to absorb the IAM grant-propagation window, and its 45 s `_wait_for_messages` poll on the subscribed queue never sees a message — so one of the two legs never completes: either the in-function SDK `sns.publish` keeps failing (so `Invoke` keeps reporting a FunctionError and the loop never exits) or the publish is accepted and the notification never reaches the subscribed queue. Because the runner cuts the check off, the emulator produces neither the expected 200/MessageId nor any modeled refusal status, error code or message for this path.

Why: a client relying on AWS's behavior sees the single most common serverless shape — a function that publishes to a topic that fans out to a queue — silently produce nothing. The downstream consumer is starved with no error to act on, and because the invoke never settles, a caller cannot distinguish "the SDK call never ran", "SNS refused the publish" and "SNS accepted it but never delivered".

Fix: the Lambda execution environment's in-function AWS SDK / execution-role credential plumbing and its dispatch to the emulator's SNS front door, the SNS Publish handler's authorization of `sns:Publish` against the calling role, and the Publish handler's `sqs`-protocol subscription delivery path together with the SQS queue-policy evaluation that admits `sns.amazonaws.com`.

Related but NOT reported as covering this check: open issues 20261107T162047Z-events-lambda-sns-sqs-an-in-function-boto3.md and 20261105T081727Z-events-lambda-sns-sqs-a-lambda-handler-cannot.md both describe in-function boto3 defects under this same suite heading, and open issues 20261106T133436Z-lambda-sns-sqs-in-function-aws-sdk-calls.md and 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md describe them under the `lambda__sns__sqs` heading. The suite replay does not mark this check as OWNED by any of them — the check's current replay key (a bare 60 s cut-off with no frame) matches none of their recorded keys — so it is filed here. Whoever fixes it should check those issues first: the fix may be shared, and this check is the one that must be re-replayed to confirm it.

Traces: events__lambda__sns__sqs.lambda-sdk-publish-to-sns.

## Evidence
Expected (AWS):
  - `Invoke(FunctionName=<fn>, InvocationType='RequestResponse', Payload='{}')` -> HTTP 200, `StatusCode=200`, no `FunctionError`, payload `{"ok": true}`, because the execution role's inline policy allows `sns:Publish` on the topic ARN.
  - The handler's `sns.publish(TopicArn=<topic>, Message='from-lambda-sdk')` -> 200 with a `MessageId` (Smithy: PublishResponse.MessageId).
  - `ReceiveMessage` on the subscribed queue then returns exactly 1 message whose JSON body has `Message == "from-lambda-sdk"`.

Actual (emulator): no verdict at all — the runner reports `Check timed out after 60.0s` with no traceback frame. The bounded re-invoke loop never sees an invoke without a FunctionError, and the 45 s queue poll never sees a message, so none of the check's assertions (`"error" not in invoke_result`, `len(msgs) == 1`, `body["Message"] == "from-lambda-sdk"`) is ever reached and no status/error-code/message is produced for the path.

(The per-run function, topic and queue names, ARNs and queue URLs are environment values, not part of the requirement. The requirement is: a granted in-function `sns:Publish` answers 200 with a MessageId, the invoke answers 200 with no FunctionError, and the published Message arrives verbatim as the `Message` member of the envelope delivered to the `sqs`-protocol subscriber.)

## Steps to Reproduce
1. sns: CreateTopic.
2. sqs: CreateQueue; GetQueueAttributes(AttributeNames=[QueueArn]).
3. sqs: SetQueueAttributes with a Policy allowing Principal Service `sns.amazonaws.com` to `sqs:SendMessage` on the queue ARN, conditioned ArnEquals aws:SourceArn = the topic ARN.
4. sns: Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<queue ARN>).
5. iam: CreateRole trusting `lambda.amazonaws.com`; PutRolePolicy with an inline policy allowing logs:CreateLogGroup/CreateLogStream/PutLogEvents plus `sns:Publish` on the topic ARN.
6. lambda: CreateFunction (Runtime=python3.12, Handler=lambda_function.lambda_handler) with a single-module package whose code is `import boto3` / `sns = boto3.client("sns")` and whose handler calls `sns.publish(TopicArn=<topic arn>, Message="from-lambda-sdk")` then returns `{"ok": True}`; wait for State=Active.
7. lambda: Invoke(InvocationType='RequestResponse', Payload='{}') in a loop (5 s interval) until it answers with no FunctionError.
8. sqs: ReceiveMessage(MaxNumberOfMessages=1, WaitTimeSeconds=5, MessageAttributeNames=['All']) in a loop for 45 s.
9. Observed: steps 7-8 never converge; the check is cut off at 60 s with `Check timed out after 60.0s`. Expected: step 7 succeeds on the first or second attempt and step 8 returns one message whose body's `Message` is `from-lambda-sdk`.

Replay: events__lambda__sns__sqs.lambda-sdk-publish-to-sns
