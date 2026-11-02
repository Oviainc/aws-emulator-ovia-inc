Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected logs.put-log-events-message-exceeds-size-limit-rejected logs.put-log-events-oversized-batch-rejected logs.integration-lifecycle-transitions logs.list-integrations-reflects-creation-order-stable logs.kms-grant-scoped-to-another-log-group-rejected logs.log-group-name-pattern-lookup-omits-retention-and-storage-fields logs.e2e-export-task-produces-s3-object-with-log-content
Issue-Key: logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected|_error#9|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: logs.put-log-events-message-exceeds-size-limit-rejected|run#12|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: logs.put-log-events-oversized-batch-rejected|run#16|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: logs.integration-lifecycle-transitions|run#22|AssertionError|assert get_message == f"Integration with name {integration_name} does not exist.", (
Issue-Key: logs.list-integrations-reflects-creation-order-stable|run#27|AssertionError|assert get_message == f"Integration with name {name1} does not exist.", (
Issue-Key: logs.kms-grant-scoped-to-another-log-group-rejected|_error#21|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded and returned {result!r}")
Issue-Key: logs.log-group-name-pattern-lookup-omits-retention-and-storage-fields|run#14|AssertionError|assert set(slim) == _PATTERN_FIELDS, (
Issue-Key: logs.e2e-export-task-produces-s3-object-with-log-content|_download_prefix_text#16|Timeout|time.sleep(3.0)
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CloudWatch Logs: PutLogEvents validation and messages, KMS-scope and integration refusals, pattern-lookup projection and export tasks all diverge (7 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
Seven net-new root causes in CloudWatch Logs. Each must be closed before submitting.

Cause 1 of 7 — PutLogEvents accepts a batch whose events span more than 24 hours.
AWS: Smithy (com.amazonaws.cloudwatchlogs) PutLogEvents, required logGroupName/logStreamName/logEvents (list, length 1..10000); models InvalidParameterException with httpError 400. Model text only; the batch contract AWS enforces includes that the span between a batch's oldest and newest event timestamp must not exceed 24 hours — a batch that violates it is refused whole with InvalidParameterException, and no event of it is stored.
Emulator: answers HTTP 200, returns a nextSequenceToken, and reports the offending event via `rejectedLogEventsInfo: {tooOldLogEventEndIndex: 1}` — i.e. it treats the violation as a per-event rejection instead of refusing the request.
Why: the two outcomes are not interchangeable. A caller that gets 200 records the put as successful and moves its checkpoint forward; AWS's refusal tells it to split the batch. Events are silently dropped with the caller believing they were accepted.
Fix: the batch-span check in PutLogEvents' validation layer — refuse the request rather than reporting it in rejectedLogEventsInfo.
Traces: logs.deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected

Cause 2 of 7 — the event-too-large refusal uses the emulator's own sentence and the wrong byte accounting.
AWS: PutLogEvents, InvalidParameterException 400; recorded behavior only for the text. A single event whose message exceeds the maximum is refused with `Log event too large: <size> bytes exceeds limit of 1048576` — fixed text `Log event too large: `, ` bytes exceeds limit of 1048576`; the variable part is the event's size AS AWS COUNTS IT, which is the UTF-8 message length plus the 26-byte per-event overhead (a 1048577-byte message is reported as 1048603).
Emulator: answers `logEvents.0.message is <size> bytes; a log event can be no larger than 1048576 bytes` — a different sentence, naming the input member path, and reporting the raw message length (1048577) without the 26-byte overhead.
Why: a caller that reads the reported size to decide how much to trim trims the wrong amount, because the emulator's number excludes the overhead AWS charges; and anything matching AWS's sentence fails.
Fix: the per-event size validation message and its byte accounting in PutLogEvents' validation layer.
Traces: logs.put-log-events-message-exceeds-size-limit-rejected

Cause 3 of 7 — the batch-too-large refusal uses the emulator's own sentence.
AWS: PutLogEvents, InvalidParameterException 400; recorded behavior only for the text. A batch whose total size exceeds the maximum is refused with `Upload too large: <size> bytes exceeds limit of 1048576` — fixed text `Upload too large: ` and ` bytes exceeds limit of 1048576`, with the computed batch size as the only variable part.
Emulator: answers `logEvents is <size> bytes; the maximum batch size is 1048576 bytes, counted as the sum of all event messages in UTF-8 plus 26 bytes for each log event` — a different sentence that also spells out the accounting rule AWS does not include.
Why: message-only divergence, but it is the paired sentence to cause 2: a client that distinguishes a single oversized event (`Log event too large`) from an oversized batch (`Upload too large`) cannot, since neither sentence is emitted.
Fix: the batch size validation message in PutLogEvents' validation layer, alongside cause 2.
Traces: logs.put-log-events-oversized-batch-rejected

Cause 4 of 7 — integration not-found refusals use one generic sentence instead of naming the integration.
AWS: Smithy GetIntegration (required integrationName) and DeleteIntegration model ResourceNotFoundException with httpError 400. Model text only; the recorded refusal for an integration name that does not resolve is `Integration with name <integrationName> does not exist.` — fixed text `Integration with name ` and ` does not exist.`, with the requested name copied from the request.
Emulator: answers ResourceNotFoundException (code correct) with `The specified integration does not exist.` — a generic sentence that drops the name, on both the read after a delete and the read of a never-created integration.
Why: when a caller manages several integrations (or retries a delete), the name in the refusal is what identifies which one is gone; a generic sentence makes the refusal unattributable and breaks anything matching AWS's text. Note: open issue 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md carries this sentence for logs.delete-integration-unknown-name-rejected, but not for the two checks here.
Fix: the ResourceNotFoundException message for the integration read/delete paths in the logs crate's validation layer.
Traces: logs.integration-lifecycle-transitions, logs.list-integrations-reflects-creation-order-stable

Cause 5 of 7 — CreateLogGroup accepts a CMK whose key policy scopes it to a different log group.
AWS: Smithy CreateLogGroup, members logGroupName (required) and kmsKeyId; models InvalidParameterException/LimitExceeded/OperationAborted/ResourceAlreadyExists. Recorded behavior only for this gate: when the CMK's key policy allows logs.amazonaws.com only under `kms:EncryptionContext:aws:logs:arn` equal to ANOTHER log group's ARN, CreateLogGroup for a group not named by that condition is refused with AccessDeniedException and the message `The specified KMS key does not exist or is not allowed to be used with Arn '<log group arn>'` — fixed text `The specified KMS key does not exist or is not allowed to be used with Arn '` and the closing `'`, with the ARN of the group being created as the only variable part — and no group is created. The group the policy does name is created and serves its events.
Emulator: answers HTTP 200 and creates the group, so an encrypted log group exists whose CMK can never be used for it.
Why: the encryption-context condition is the standard way a CMK is scoped to one log group; skipping it creates a group that appears encrypted but whose key grant does not cover it, and a caller relying on the refusal to validate its key policy gets no signal.
Fix: evaluate the CMK's key policy (including the aws:logs:arn encryption-context condition) for the target group in CreateLogGroup's and AssociateKmsKey's handlers in the logs crate.
Traces: logs.kms-grant-scoped-to-another-log-group-rejected

Cause 6 of 7 — DescribeLogGroups by logGroupNamePattern returns a narrower projection than AWS.
AWS: Smithy DescribeLogGroups, member logGroupNamePattern (length 0..512, pattern ^[\.\-_/#A-Za-z0-9]*$), output logGroups: LogGroups. Recorded/documented behavior (API_DescribeLogGroups): a pattern lookup returns a REDUCED LogGroup projection — exactly arn, creationTime, logGroupArn, logGroupClass, logGroupName and metricFilterCount — omitting retention, storage and encryption fields that the prefix lookup of the same group carries, and agreeing with the prefix lookup on arn and creationTime.
Emulator: returns only arn, creationTime and logGroupName, so logGroupArn, logGroupClass and metricFilterCount are missing.
Why: logGroupArn and logGroupClass are what a caller uses to address and classify the groups a pattern search found (STANDARD versus INFREQUENT_ACCESS changes what it may do next); their absence forces a second per-group describe and breaks clients that read them from the pattern result.
Fix: the reduced LogGroup projection used by DescribeLogGroups when logGroupNamePattern is supplied, in the logs crate's serializer.
Traces: logs.log-group-name-pattern-lookup-omits-retention-and-storage-fields

Cause 7 of 7 — CreateExportTask never produces an S3 object, so the export is unobservable.
AWS: recorded behavior only. An export task over a log group with events moves to COMPLETED and writes the exported log content as objects under the requested destination prefix, readable with S3.
Emulator: the check's bounded poll for an object carrying the log content never succeeds and the harness ends it with `Check timed out after 60.0s`.
Why: the exported objects are the entire output of an export task; without them a client that exports logs to S3 gets nothing and no error, and a task status cannot be trusted as evidence that data was written. Note: open issues 20261025T181311Z-logs-s3-cloudwatch-logs-export-tasks-never-run.md and 20261026T102156Z-logs-cloudwatch-logs-export-tasks-complete-but-write-no.md record this family and carry none of this check's keys.
Fix: the export-task worker in the logs crate — write the exported events to the destination bucket/prefix before the task is reported COMPLETED.
Traces: logs.e2e-export-task-produces-s3-object-with-log-content

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — deep-cwlogs-semantics-put-log-events-surviving-span-over-24h-rejected:
  AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'nextSequenceToken': '...', 'rejectedLogEventsInfo': {'tooOldLogEventEndIndex': 1}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 2 — put-log-events-message-exceeds-size-limit-rejected:
  expected 'Log event too large: 1048603 bytes exceeds limit of 1048576'
  actual   'logEvents.0.message is 1048577 bytes; a log event can be no larger than 1048576 bytes'

Cause 3 — put-log-events-oversized-batch-rejected:
  expected 'Upload too large: 1048577 bytes exceeds limit of 1048576'
  actual   'logEvents is 1048577 bytes; the maximum batch size is 1048576 bytes, counted as the sum of all event messages in UTF-8 plus 26 bytes for each log event'

Cause 4 — integration-lifecycle-transitions:
  AssertionError: unexpected GetIntegration message after DeleteIntegration: 'The specified integration does not exist.'
  list-integrations-reflects-creation-order-stable:
  AssertionError: unexpected GetIntegration message for the just-deleted integration: 'The specified integration does not exist.'
  Expected: 'Integration with name <integrationName> does not exist.'

Cause 5 — kms-grant-scoped-to-another-log-group-rejected:
  AssertionError: expected a ClientError, call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected code 'AccessDeniedException', message "The specified KMS key does not exist or is not allowed to be used with Arn 'arn:aws:logs:<region>:<account>:log-group:<name>'", and the group absent afterwards.

Cause 6 — log-group-name-pattern-lookup-omits-retention-and-storage-fields:
  AssertionError: a pattern lookup returned the fields ['arn', 'creationTime', 'logGroupName']; only ['arn', 'creationTime', 'logGroupArn', 'logGroupClass', 'logGroupName', 'metricFilterCount'] are included when logGroupNamePattern is given

Cause 7 — e2e-export-task-produces-s3-object-with-log-content:
  Error: Check timed out after 60.0s (polling the destination prefix for an object holding the exported log content)

Byte counts, group/integration names and ARNs above are this run's values filling the message templates; the limits (1048576) and the fixed sentences are the requirement.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: create group g and stream s, then aws logs put-log-events --log-group-name g --log-stream-name s --log-events timestamp=<now>,message=a timestamp=<now minus 30 hours>,message=b -> 200 with rejectedLogEventsInfo; AWS refuses with InvalidParameterException.

Cause 2: put a single event whose message is 1048577 bytes -> 'logEvents.0.message is 1048577 bytes; ...'; AWS answers 'Log event too large: 1048603 bytes exceeds limit of 1048576'.

Cause 3: put several events whose total exceeds 1048576 bytes -> 'logEvents is ... bytes; the maximum batch size ...'; AWS answers 'Upload too large: <size> bytes exceeds limit of 1048576'.

Cause 4: aws logs get-integration --integration-name nosuch (and after a delete-integration) -> 'The specified integration does not exist.'; AWS answers 'Integration with name nosuch does not exist.'

Cause 5: create a CMK whose policy allows logs.amazonaws.com only under ArnEquals kms:EncryptionContext:aws:logs:arn = arn:aws:logs:<region>:<account>:log-group:A; create group A with that kmsKeyId (succeeds), then aws logs create-log-group --log-group-name B --kms-key-id <key arn> -> 200; AWS answers AccessDeniedException with the 'not allowed to be used with Arn' sentence and creates nothing.

Cause 6: create group g, put-retention-policy, then aws logs describe-log-groups --log-group-name-pattern <substring of g> -> entry has only arn/creationTime/logGroupName; AWS also returns logGroupArn, logGroupClass and metricFilterCount.

Cause 7: put events into g, aws s3api create-bucket --bucket b, aws logs create-export-task --log-group-name g --from <t0> --to <t1> --destination b --destination-prefix p, then list objects under p -> nothing is ever written.

Then re-replay the eight check ids in trace_ids.
