Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error
Issue-Key: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption|_run_export_until_content#12|Timeout|time.sleep(20.0)
Issue-Key: logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix|run#22|Timeout|time.sleep(min(30.0, _left(deadline))) # fresh task after the index has had real time
Issue-Key: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected|run#20|AssertionError|assert message == "Resource limit exceeded.", (
Issue-Key: logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant|_error#26|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: logs__s3|GetQueryResults|NotImplemented
Issue-Commit: ec4677bae9b70a2d31b2abdbe7af61c1994727a3

# logs__s3: CloudWatch Logs export tasks never run, the concurrent-export refusal message is invented, CreateDelivery is never authorized, and GetQueryResults is a scaffold stub (4 causes)

> _Note: this issue was identified against the code base at commit `ec4677bae9b70a2d31b2abdbe7af61c1994727a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExportTask

## Description
Four distinct net-new root causes in the CloudWatch-Logs-to-S3 surface, all grounded in failing behavioral checks whose check id leads with `logs__s3`. None of these checks is marked ALREADY FILED or OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 4: an accepted CreateExportTask never leaves PENDING and writes no objects to the destination bucket, so every export-task consumer hangs.
Failing checks: logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption, logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix

AWS: Smithy models CreateExportTask -> CreateExportTaskResponse{taskId} with DescribeExportTasks returning ExportTasks carrying an ExportTaskStatusCode; the status progresses PENDING -> PENDING_CANCEL/RUNNING -> COMPLETED (or FAILED/CANCELLED), and on COMPLETED the task's log events exist as gzip objects under <destinationPrefix>/ in the destination bucket, encrypted with THAT bucket's own default encryption (HeadObject reports aws:kms + the bucket CMK's ARN for an SSE-KMS bucket, and AES256 with no SSEKMSKeyId for an SSE-S3 bucket). The two checks poll DescribeExportTasks for a terminal status and then read the objects.
Emulator: both checks run to the harness cap — `Error: Check timed out after 60.0s` — spinning in the terminal-status poll. Corroborating evidence from the sibling check in Cause 2: the emulator's own refusal text reports the first task as still `PENDING` ("Task 00000000-0000-4000-8000-000000000009 is PENDING"), i.e. accepted tasks are recorded and then never advanced or executed.
Why: an export task that is accepted and then never settles is indistinguishable from one that is working, so a client waiting on COMPLETED waits forever; nothing is ever written to S3, so no downstream consumer of exported logs (Athena over the prefix, retention archival, an audit pipeline) can be exercised at all, and the "objects carry the destination bucket's encryption, not a constant" contract is unverifiable.
Fix: the CreateExportTask handler and the export-task state/driver in the cloudwatch-logs crate must actually execute the task — select the log group's events in [from, to), write gzip objects under destinationPrefix through the S3 data plane (so the destination bucket's default encryption applies), and advance the task to COMPLETED so DescribeExportTasks reports a terminal status. The open issue 20261023T113357Z-logs-createexporttask-stub.md and the FIXED 20261025T070938Z-logs-cloudwatch-logs-6-net-new-root-causes-kinesis.md (its Cause 3) cover only CreateExportTask's ResourceNotFoundException refusal path; neither carries these two replay keys and neither addresses task execution.
Traces: the two check ids above.

----------------------------------------------------------------
Cause 2 of 4: the concurrent-export refusal carries an emulator-authored message instead of AWS's LimitExceededException text.
Failing check: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected

AWS: Smithy models LimitExceededException (400) among CreateExportTask's errors. An account may have only one export task in a non-terminal state; a second CreateExportTask while the first is PENDING/RUNNING is refused 400 LimitExceededException with the FIXED text `Resource limit exceeded.` — the message has no variable parts at all; it names neither the task id nor its status.
Emulator: the error CODE is right (LimitExceededException) but the message is `An account can only have one active export task at a time. Task 00000000-0000-4000-8000-000000000009 is PENDING.` — hand-authored prose, and it interpolates a task id and status that AWS's message never contains.
Why: the message is part of the error contract. Operators and tools match on `Resource limit exceeded.` to recognise the one-active-export quota; emulator-authored prose breaks that match, and embedding a task id invites clients to parse a field AWS never supplies.
Fix: the quota refusal in the CreateExportTask handler of the cloudwatch-logs crate must raise LimitExceededException with AWS's fixed message `Resource limit exceeded.` and no interpolated state.
Traces: logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected.

----------------------------------------------------------------
Cause 3 of 4: CreateDelivery creates the delivery for a caller who cannot grant delivery.logs.amazonaws.com write access on the destination, instead of refusing it.
Failing check: logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant

AWS: Smithy models CreateDelivery with AccessDeniedException (400) in its error set. Wiring a delivery requires the CALLER to be able to put the resource policy that grants delivery.logs.amazonaws.com write access on the delivery destination, so authorization is evaluated against logs:PutResourcePolicy ON THE DESTINATION — an action the client never calls itself. A principal allowed every logs:* action EXCEPT logs:PutResourcePolicy is refused AccessDeniedException naming the delivery destination; no delivery object is created and the destination log group carries no resource policy. Lifting exactly that one deny makes the same session's identical call succeed. (The exact AWS message wording for this refusal is recorded behavior only in the authoring recording — the requirement graded here is the refusal itself, its AccessDeniedException class, and that it names the delivery destination.)
Emulator: the call SUCCEEDS and returns a delivery — `AssertionError: expected a rejection, but the call SUCCEEDED and returned {'delivery': {'id': '0000000000000000', 'arn': 'arn:aws:logs:...:delivery:0000000000000000', 'deliverySourceName': ..., 'deliveryDestinationArn': ..., 'deliveryDestinationType': 'CWL'}, ...}` with HTTPStatusCode 200. The emulator evidently authorizes (at most) logs:CreateDelivery itself and never evaluates logs:PutResourcePolicy against the destination.
Why: a client gets a delivery object that can never write anything, and no signal that its own permissions are the reason. The failure surfaces much later as silently missing log data instead of synchronously at wiring time, which is the whole point of AWS evaluating the grant up front.
Fix: the authorization step of the CreateDelivery handler in the cloudwatch-logs crate must evaluate logs:PutResourcePolicy against the delivery destination resource (not just logs:CreateDelivery against the source) and refuse with AccessDeniedException naming that destination, leaving neither a delivery nor a destination resource policy behind. Related open issue that does NOT carry this replay key: 20261025T014519Z-logs-cloudwatch-logs-vended-delivery-and-subscription-plumbing-is.md.
Traces: logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant.

----------------------------------------------------------------
Cause 4 of 4: GetQueryResults is an unimplemented scaffold stub, so no scheduled-query result can be read or written to S3.
Failing checks: logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events, logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error

AWS: Smithy models GetQueryResults(queryId @required) -> GetQueryResultsResponse{queryLanguage, results, statistics, status, encryptionKey, nextToken}, with errors InvalidParameterException (400), ResourceNotFoundException (400) and ServiceUnavailableException (500). `NotImplemented` is not in that set: a queryId issued by StartQuery is answered 200 with a status and, once Complete, the result rows.
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetQueryResults operation: operation GetQueryResults is scaffolded but not implemented in this emulator`. Both checks error out at that call — StartQuery now answers, so GetQueryResults is the surviving blocker.
Why: Logs Insights is write-only. A client can start a query and never read it, so neither the positive leg (the rows written to the S3 destination equal the seeded events) nor the negative leg (a destination PutObject denial is recorded as the query's destination error) can be observed, and NotImplemented gives SDK error handling no modeled branch to take.
Fix: the GetQueryResults handler in the cloudwatch-logs crate is still the generated scaffold stub — it must resolve the queryId against query state and serialise status/results/statistics, with ResourceNotFoundException for an unknown queryId. The open issue 20261025T032458Z-logs-s3-cloudwatch-logs-startquery-is-an-unimplemented.md names StartQuery, not GetQueryResults, and does not carry these two replay keys.
Traces: the two check ids above.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 197/749 passed), service logs__s3 0/6 (6 failed):

Cause 1 — export tasks never settle (2 checks, verdict [mismatch]):
  [mismatch] logs__s3.export-task-objects-carry-the-destination-buckets-own-encryption — Error: Check timed out after 60.0s
  [mismatch] logs__s3.log-cwl-plumbing-export-objects-hold-exactly-the-streams-events-under-prefix — Error: Check timed out after 60.0s
  Both hang polling DescribeExportTasks for a status in {COMPLETED, CANCELLED, FAILED}.
  Expected: the task reaches COMPLETED and at least one .gz object exists under <destinationPrefix>/; HeadObject on each reports the DESTINATION bucket's own encryption (aws:kms + that bucket's CMK ARN for the SSE-KMS bucket; AES256 and no SSEKMSKeyId for the AES-256 twin).
  Actual:   the task stays non-terminal and no object is ever written; corroborated by the emulator's own text in the sibling check: "Task 00000000-0000-4000-8000-000000000009 is PENDING".

Cause 2 — invented LimitExceededException message (verdict [mismatch]):
  [mismatch] logs__s3.log-cwl-plumbing-second-concurrent-export-task-rejected
    check line 101: assert message == "Resource limit exceeded."
    AssertionError: expected the concurrent-export refusal message 'Resource limit exceeded.', got 'An account can only have one active export task at a time. Task 00000000-0000-4000-8000-000000000009 is PENDING.'
  The preceding assertion on the error CODE passed (LimitExceededException), so only the message diverges.
  Expected: 400 LimitExceededException, message exactly `Resource limit exceeded.` (entirely fixed text).
  Actual:   400 LimitExceededException with emulator-authored prose that interpolates a task id and status.

Cause 3 — CreateDelivery never authorized (verdict [mismatch]):
  [mismatch] logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant
    check line 86: code, message = _error(lambda: restricted.create_delivery(deliverySourceName=source, deliveryDestinationArn=dest_arn))
    AssertionError: expected a rejection, but the call SUCCEEDED and returned {'delivery': {'id': '0000000000000000', 'arn': 'arn:aws:logs:us-west-2:000000000000:delivery:0000000000000000', 'deliverySourceName': 'sim2real-xsvc-sal-src-...', 'deliveryDestinationArn': 'arn:aws:logs:us-west-2:000000000000:delivery-destination:sim2real-xsvc-sal-dst-...', 'deliveryDestinationType': 'CWL'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Caller: a session allowed every logs:* action EXCEPT logs:PutResourcePolicy. PutDeliverySource (service "s3", logType S3_SERVER_ACCESS_LOGS, resourceArn arn:aws:s3:::<bucket>) and PutDeliveryDestination (destinationResourceArn = the log-group ARN) both succeeded beforehand.
  Expected: 400 AccessDeniedException naming the delivery destination; no delivery created and no resource policy on the destination log group.
  Actual:   HTTP 200 and a created delivery.

Cause 4 — GetQueryResults scaffold stub (2 checks, verdict [error]):
  [error] logs__s3.xsvc-logs-3-scheduled-query-s3-object-rows-equal-seeded-events
  [error] logs__s3.xsvc-logs-3-scheduled-query-s3-put-denied-records-destination-error
  both: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetQueryResults operation: operation GetQueryResults is scaffolded but not implemented in this emulator
  Expected: HTTP 200 with status and, once Complete, the result rows (errors are limited to InvalidParameterException / ResourceNotFoundException / ServiceUnavailableException).
  Actual:   error code NotImplemented, which is outside GetQueryResults' modeled error set.

## Steps to Reproduce
Boot the emulator from this checkout and replay the six `logs__s3` checks listed in trace_ids.

Cause 1 (export tasks never run):
  1. s3.CreateBucket(<dest>) and give it a bucket policy allowing logs.<region>.amazonaws.com GetBucketAcl/PutObject.
  2. logs.CreateLogGroup(<g>) + CreateLogStream(<s>) + PutLogEvents of one line at t0.
  3. logs.CreateExportTask(logGroupName=<g>, from=t0-1, to=now+1000, destination=<dest>, destinationPrefix=<p>) -> note taskId.
  4. Poll logs.DescribeExportTasks(taskId=<taskId>) -> expect a terminal COMPLETED; observe the status stays non-terminal until the 60 s harness cap.
  5. s3.ListObjectsV2(Bucket=<dest>, Prefix=<p>) -> expect at least one .gz object; observe none.
  6. Repeat with an SSE-KMS destination bucket and an AES256 twin and HeadObject each exported object -> expect each object to report ITS OWN bucket's encryption; unreachable today.

Cause 2 (concurrent-export message):
  1. As above, create a first export task and confirm via DescribeExportTasks that it is still non-terminal.
  2. logs.CreateExportTask(...) a second time for the same account, on a client configured with retries max_attempts=1.
     -> expect 400 LimitExceededException with message exactly "Resource limit exceeded."
     -> observe 400 LimitExceededException with "An account can only have one active export task at a time. Task <id> is PENDING."

Cause 3 (CreateDelivery authorization):
  1. s3.CreateBucket(<b>); logs.CreateLogGroup(<g>).
  2. logs.PutDeliverySource(name=<src>, resourceArn=arn:aws:s3:::<b>, logType="S3_SERVER_ACCESS_LOGS").
  3. logs.PutDeliveryDestination(name=<dst>, deliveryDestinationConfiguration={destinationResourceArn: <log-group-arn>}).
  4. Create an IAM role/session allowed logs:* with an explicit Deny on logs:PutResourcePolicy, and assume it.
  5. From that session: logs.CreateDelivery(deliverySourceName=<src>, deliveryDestinationArn=<dst-arn>)
     -> expect 400 AccessDeniedException naming the delivery destination, with no delivery created and no resource policy on <g>.
     -> observe HTTP 200 and a delivery object.
  6. Remove only that Deny and re-issue the identical call from the same session -> must now succeed.

Cause 4 (GetQueryResults):
  1. logs.CreateLogGroup(<g>) + CreateLogStream + PutLogEvents of the seeded events.
  2. logs.StartQuery(logGroupName=<g>, startTime=..., endTime=..., queryString="fields @message") -> note queryId.
  3. logs.GetQueryResults(queryId=<queryId>)
     -> expect HTTP 200 with a status and, once Complete, one row per seeded event.
     -> observe error code NotImplemented.
