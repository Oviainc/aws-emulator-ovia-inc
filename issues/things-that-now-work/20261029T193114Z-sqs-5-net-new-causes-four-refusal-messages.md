Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.add-permission-over-limit-rejected sqs.immutable-queue-arn-mutation-rejected sqs.set-queue-attributes-readonly-attribute-rejected sqs.op-fifo-attributes-on-standard-queue-rejected sqs.redrive-allow-policy-document-validation-rejected sqs.start-message-move-task-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.add-permission-over-limit-rejected|run#14|AssertionError|assert message == "21 Statements were found, maximum allowed is 20. ", (
Issue-Key: sqs.immutable-queue-arn-mutation-rejected|run#7|AssertionError|assert error.get("Message") == _READ_ONLY_ATTRIBUTE_MESSAGE, (
Issue-Key: sqs.set-queue-attributes-readonly-attribute-rejected|run#7|AssertionError|assert message == "Unknown Attribute ApproximateNumberOfMessages.", (
Issue-Key: sqs.op-fifo-attributes-on-standard-queue-rejected|run#6|AssertionError|assert err_cbd.get("Message") == MSG_CBD, f"got {err_cbd!r}"
Issue-Key: sqs.redrive-allow-policy-document-validation-rejected|_err#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.start-message-move-task-type-mismatch-rejected|_error#17|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# sqs: 5 net-new causes — four refusal messages are emulator-authored, and RedriveAllowPolicy / StartMessageMoveTask validation is missing entirely

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
Five net-new root causes in the SQS refusal surface. All status codes and error codes are modelled: Smithy sqs SetQueueAttributes declares InvalidAttributeName (HTTP 400) and InvalidAttributeValue (HTTP 400); AddPermission declares OverLimit (awsQueryError OverLimit, HTTP 403); StartMessageMoveTask declares ResourceNotFoundException and UnsupportedOperation.

Cause 1 of 5: the AddPermission over-limit refusal message is emulator-authored.
AWS: Error.Code `OverLimit` with the message "21 Statements were found, maximum allowed is 20. " — the fixed shape is "<n> Statements were found, maximum allowed is <limit>. " (note the trailing space), where <n> is the statement count the request would produce and <limit> is the policy statement limit.
Emulator: the code is right but the message is "An Amazon SQS policy can have a maximum of 20 statements, and the policy of queue <queue-name> already has 20." — a different sentence that also interpolates the queue name, which AWS's message never carries.
Why: a client that classifies this refusal by message text, or surfaces it to an operator, sees a sentence AWS never emits and cannot extract the attempted-count/limit pair AWS reports.
Fix: the sqs AddPermission handler's policy-statement-limit refusal message.
Traces: sqs.add-permission-over-limit-rejected

Cause 2 of 5: the read-only-attribute refusal message is emulator-authored (two attributes, one template).
AWS: SetQueueAttributes on an attribute that is reported by GetQueueAttributes but cannot be set is refused with Error.Code `InvalidAttributeName` and the message "Unknown Attribute <AttributeName>." — the fixed text is "Unknown Attribute " + the attribute name from the request + ".".
Emulator: returns the right code with "Attribute <AttributeName> is reported by GetQueueAttributes and cannot be set." for both QueueArn and ApproximateNumberOfMessages.
Why: AWS deliberately reports these as unknown attributes; a different sentence breaks message-based classification and tells a client the attribute exists-but-is-read-only, which is not what AWS communicates.
Fix: the sqs SetQueueAttributes attribute-name validation's refusal message template.
Traces: sqs.immutable-queue-arn-mutation-rejected, sqs.set-queue-attributes-readonly-attribute-rejected

Cause 3 of 5: the FIFO-only-attribute-on-a-standard-queue refusal message is emulator-authored.
AWS: refuses with Error.Code `InvalidAttributeName` and the messages "Unknown Attribute ContentBasedDeduplication." and "Unknown Attribute FifoQueue." — same "Unknown Attribute <name>." template as cause 2, with the attribute name from the request.
Emulator: returns the right code with "Attribute ContentBasedDeduplication applies only to FIFO queues.".
Why: same classification break as cause 2, on a distinct validation path (FIFO-only attributes rather than read-only ones), so it must be fixed separately.
Fix: the sqs SetQueueAttributes FIFO-attribute validation's refusal message template.
Traces: sqs.op-fifo-attributes-on-standard-queue-rejected

Cause 4 of 5: RedriveAllowPolicy documents are not validated at all.
AWS: refuses a malformed or over-limit RedriveAllowPolicy with Error.Code `InvalidParameterValue` and the message "Value <policy-json> for parameter RedriveAllowPolicy is invalid. Reason: <reason>" (the policy text is taken from the request), and an unknown redrivePermission value with Error.Code `InvalidAttributeValue` and "Invalid value for the parameter RedriveAllowPolicy. Reason: Amazon SQS can't ...". The previously stored policy is left unchanged.
Emulator: SetQueueAttributes answers HTTP 200 with an empty JSON body (`content-type: application/x-amz-json-1.0`, `content-length: 2`) for the document the check expected to be refused — there is no RedriveAllowPolicy document validation.
Why: an unparseable or over-limit allow-policy is stored, so redrive authorization silently becomes whatever the malformed document degrades to; the 10-ARN limit and the permission/ARN coupling AWS enforces are exactly what IaC modules depend on being caught.
Fix: the sqs SetQueueAttributes RedriveAllowPolicy validation (document shape, redrivePermission enum, sourceQueueArns arity), with the stored value left untouched on refusal.
Traces: sqs.redrive-allow-policy-document-validation-rejected

Cause 5 of 5: StartMessageMoveTask does not check that source and destination are the same queue type.
AWS: refuses with Error.Code `InvalidParameterValue` and the exact message "The source queue and destination queue must be of the same queue type." — entirely fixed text — and creates no task.
Emulator: answers HTTP 200 with a `TaskHandle`, so a FIFO source redirected at a standard destination (or the reverse) starts a task AWS would never accept.
Why: a move task across incompatible queue types is started and reported healthy; the messages it moves cannot carry the FIFO metadata the destination requires, so a redrive operation that must be refused up front instead corrupts or loses ordering guarantees.
Fix: the sqs StartMessageMoveTask handler's source/destination queue-type validation, before the task is created.
Traces: sqs.start-message-move-task-type-mismatch-rejected

## Evidence
Cause 1 — expected `OverLimit` / '21 Statements were found, maximum allowed is 20. '; got message 'An Amazon SQS policy can have a maximum of 20 statements, and the policy of queue sim2real-behav-e4013289cbe9 already has 20.'
Cause 2 — expected 'Unknown Attribute QueueArn.', got 'Attribute QueueArn is reported by GetQueueAttributes and cannot be set.'; expected 'Unknown Attribute ApproximateNumberOfMessages.', got 'Attribute ApproximateNumberOfMessages is reported by GetQueueAttributes and cannot be set.'
Cause 3 — expected 'Unknown Attribute ContentBasedDeduplication.' (and 'Unknown Attribute FifoQueue.'), got {'Message': 'Attribute ContentBasedDeduplication applies only to FIFO queues.', 'Code': 'InvalidAttributeName'}
Cause 4 — `AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}`
Cause 5 — `AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'TaskHandle': 'f5bb4d34-c839-49c2-8b3f-2e61c115c3d2', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`

## Steps to Reproduce
Cause 1: AddPermission 20 distinct labels on one queue, then add a 21st — code is OverLimit but the message text differs.
Cause 2: SetQueueAttributes with Attributes={'QueueArn': ...} and separately {'ApproximateNumberOfMessages': '1234'} — InvalidAttributeName with the wrong message.
Cause 3: on a standard queue, SetQueueAttributes with {'ContentBasedDeduplication': 'true'} and then {'FifoQueue': 'true'} — InvalidAttributeName with the wrong message.
Cause 4: SetQueueAttributes with a malformed/over-limit RedriveAllowPolicy document — HTTP 200, no refusal.
Cause 5: create a FIFO DLQ and a standard destination queue, then StartMessageMoveTask(SourceArn=<fifo>, DestinationArn=<standard>) — HTTP 200 with a TaskHandle.
