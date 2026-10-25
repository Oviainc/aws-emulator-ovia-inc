Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events logs.logsquery-parse-with-an-unclosed-regex-group-rejected logs.logsquery-redefining-a-parsed-field-rejected logs.logsquery-window-ending-before-group-creation-rejected logs.start-query-end-time-before-start-time-rejected logs.e2e-export-task-rejected-for-invalid-input logs.e2e-delivery-missing-source-rejected logs.resource-policy-revision-conflict-rejected logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|GetShardIterator|NotImplemented
Issue-Key: logs.logsquery-parse-with-an-unclosed-regex-group-rejected|run#15|AssertionError|assert code == "MalformedQueryException", (
Issue-Key: logs.logsquery-redefining-a-parsed-field-rejected|run#15|AssertionError|assert code == "MalformedQueryException", (
Issue-Key: logs.logsquery-window-ending-before-group-creation-rejected|run#13|AssertionError|assert code == "MalformedQueryException", (
Issue-Key: logs.start-query-end-time-before-start-time-rejected|run#12|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: logs.e2e-export-task-rejected-for-invalid-input|run#6|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: logs.e2e-delivery-missing-source-rejected|run#14|AssertionError|assert code == "ResourceNotFoundException", code
Issue-Key: logs.resource-policy-revision-conflict-rejected|run#19|AssertionError|assert code == "InvalidParameterException", f"got {code!r}"
Issue-Key: logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected|run#15|Timeout|time.sleep(5.0)
Issue-Key: logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group|run#20|Timeout|time.sleep(5.0)
Issue-Commit: f29a5831c5957291c6cb768de44e48e5e558bd01

# CloudWatch Logs: 6 net-new root causes — kinesis GetShardIterator stub blocks subscription-filter delivery, StartQuery/CreateExportTask/CreateDelivery answer NotImplemented on their modeled rejection paths, expectedRevisionId misclassification, and EMF extraction produces no metric data

> _Note: this issue was identified against the code base at commit `f29a5831c5957291c6cb768de44e48e5e558bd01`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Six distinct net-new root causes in the CloudWatch Logs surface, all grounded in failing behavioral checks whose check id leads with `logs`. None of these checks is marked ALREADY FILED or OWNED in the replay report.

----------------------------------------------------------------
Cause 1 of 6: kinesis GetShardIterator is an unimplemented scaffold stub, so no CloudWatch Logs subscription-filter delivery can be observed.
Failing checks: logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest, logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint, logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events, logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events

AWS: Smithy (kinesis, com.amazonaws.kinesis#GetShardIterator) models GetShardIterator as returning a ShardIterator for an existing shard, with ResourceNotFoundException / InvalidArgumentException / ProvisionedThroughputExceededException as its refusals. There is no NotImplemented in the model, and no modeled path on which a live stream's shard refuses to hand out an iterator. These `logs` checks use Kinesis purely as the subscription-filter destination: they read the delivered CloudWatch Logs record envelope (gzip, owner/logGroup/logStream/messageType/subscriptionFilters/logEvents) off the stream.
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator`. Every one of the four checks errors out at that call.
Why: a client cannot verify ANY CloudWatch Logs subscription-filter behavior — the record envelope, ForwardedLogEvents counting, DeliveryErrors on a dead destination, or transformer output on ingest — because the only destination the checks can read from refuses to produce an iterator. The whole log-delivery data plane is unobservable.
Fix: the GetShardIterator handler in the kinesis crate (services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_shard_iterator.rs) is still the scaffold stub. Note for the fixer: sibling kinesis stubs ListShards and PutRecords already have their own open issues (20261024T221823Z-kinesis-listshards-stub.md, 20261025T035202Z-kinesis-putrecords-stub.md) and the kinesis bootstrap issue 20261021T210901Z-bootstrap-service-com-amazonaws-kinesis-kinesis-20131202.md is still open; GetShardIterator is named by none of them. The four failing checks above are `logs` checks, which is why this cause is filed here.
Traces: the four check ids above.

----------------------------------------------------------------
Cause 2 of 6: StartQuery answers NotImplemented where AWS answers its modeled input-validation refusals.
Failing checks: logs.logsquery-parse-with-an-unclosed-regex-group-rejected, logs.logsquery-redefining-a-parsed-field-rejected, logs.logsquery-window-ending-before-group-creation-rejected, logs.start-query-end-time-before-start-time-rejected

AWS: Smithy: StartQuery errors are InvalidParameterException, LimitExceededException, MalformedQueryException, ResourceNotFoundException, ServiceUnavailableException — all 400 except the last. MalformedQueryException's own model doc says "The query string is not valid. Details about this error are displayed in a QueryCompileError object." So a queryString that does not compile (a `parse` regex with an unclosed capture group; a later `fields` command naming an already-parsed ephemeral field) is refused 400 MalformedQueryException, and a request whose modeled `endTime` precedes its modeled `startTime`, or whose window ends before the log group existed, is refused 400 (InvalidParameterException for the time-ordering violation, MalformedQueryException for the window case) — per the recorded responses these checks were authored against.
Emulator: every one of the four calls comes back with error code `NotImplemented` ("operation StartQuery is scaffolded but not implemented in this emulator"), so the checks read `got 'NotImplemented'` where they assert MalformedQueryException / InvalidParameterException.
Why: a client cannot distinguish "my query is malformed" from "the service is broken". NotImplemented is not in StartQuery's modeled error set, so a client's error handling (retry vs. fix-the-query) has no modeled branch to take, and Insights query validation is entirely unverifiable.
Fix: the StartQuery handler and its queryString/time-window validation in the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs). Related open issues that do NOT carry these four replay keys: 20261023T113357Z-logs-startquery-stub.md and 20261025T032458Z-logs-s3-cloudwatch-logs-startquery-is-an-unimplemented.md (the latter is scoped to the logs__s3 scheduled-query-to-S3 path).
Traces: the four check ids above.

----------------------------------------------------------------
Cause 3 of 6: CreateExportTask answers NotImplemented instead of ResourceNotFoundException for a log group that does not exist.
Failing check: logs.e2e-export-task-rejected-for-invalid-input

AWS: Smithy: CreateExportTask.logGroupName is @required (length 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$) and the operation's modeled errors include ResourceNotFoundException (400, "The specified resource does not exist."). A CreateExportTask naming a nonexistent log group is refused 400 ResourceNotFoundException.
Emulator: error code `NotImplemented` — "CreateExportTask against a nonexistent log group returned 'NotImplemented', expected ResourceNotFoundException".
Why: the operation's whole rejection contract is unreachable, so a client cannot tell a bad export request from an outage, and no export-task negative path can be relied on.
Fix: the CreateExportTask handler and its log-group resolution in the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_export_task.rs). Related open issue not carrying this replay key: 20261023T113357Z-logs-createexporttask-stub.md.
Traces: logs.e2e-export-task-rejected-for-invalid-input.

----------------------------------------------------------------
Cause 4 of 6: CreateDelivery answers NotImplemented instead of ResourceNotFoundException for a delivery source that does not exist.
Failing check: logs.e2e-delivery-missing-source-rejected

AWS: Smithy: CreateDelivery.deliverySourceName is @required (length 1..60, pattern ^[\w-]*$) and the operation's modeled errors include ResourceNotFoundException (400). A CreateDelivery naming a delivery source that was never put is refused 400 ResourceNotFoundException (recorded message, as an example: "Requested Delivery Source does not exist in this account."), and no delivery object appears in DescribeDeliveries.
Emulator: error code `NotImplemented`, asserted as `AssertionError: NotImplemented` where the check requires ResourceNotFoundException. The positive control that follows (put the real source, then the identical CreateDelivery succeeds, GetDelivery/DeleteDelivery round-trip) is therefore never reached either.
Why: the vended-delivery pre-condition is unenforceable and indistinguishable from an outage; a client wiring up a delivery pipeline gets no usable signal that its source name is wrong.
Fix: the CreateDelivery handler and its delivery-source resolution in the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_delivery.rs). Related open issues not carrying this replay key: 20261023T113357Z-logs-createdelivery-stub.md and 20261025T014519Z-cloudwatch-logs-createdelivery-stub.md.
Traces: logs.e2e-delivery-missing-source-rejected.

----------------------------------------------------------------
Cause 5 of 6: PutResourcePolicy classifies an expectedRevisionId the service never issued as a revision CONFLICT instead of an invalid parameter.
Failing check: logs.resource-policy-revision-conflict-rejected

AWS: Smithy: PutResourcePolicy.expectedRevisionId is @length(min 1) and the operation models BOTH InvalidParameterException (400, "A parameter is specified incorrectly.") and OperationAbortedException (400, "Multiple concurrent requests to update the same resource were in conflict."). These are two different conditions and AWS keeps them apart: a value that is not a revision id the service ever issued is an INVALID PARAMETER — refused 400 InvalidParameterException (recorded message, as an example: "Invalid ExpectedRevisionId"); a value that IS a real, previously-issued revision id but has since been superseded is a CONFLICT — refused 400 OperationAbortedException. The check exercises both arms in order: first `<current-revision>-stale` (never issued), then the genuinely superseded prior revision.
Emulator: the never-issued value is refused with error code `OperationAbortedException` (`AssertionError: got 'OperationAbortedException'` where InvalidParameterException is required), i.e. the handler collapses "unparseable / unknown revision id" into the optimistic-lock conflict branch.
Why: OperationAbortedException is the retryable class — it tells a client "a concurrent update is in flight, try again". A client given it for a permanently malformed expectedRevisionId will retry forever instead of correcting the request, and conversely cannot detect a real concurrent-update conflict because both now look alike.
Fix: the expectedRevisionId validation/classification in the PutResourcePolicy handler of the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_resource_policy.rs) — the well-formedness/known-revision check must precede and be distinct from the current-revision comparison.
Traces: logs.resource-policy-revision-conflict-rejected.

----------------------------------------------------------------
Cause 6 of 6: EMF log events produce no CloudWatch metric datapoint, so both METRIC_EXTRACTION_POLICY account-policy checks never terminate.
Failing checks: logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected, logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group

AWS: recorded behavior only — the Smithy model types PutAccountPolicy.policyDocument as an opaque string (AccountPolicyDocument) and says nothing about EMF extraction. What the recordings these checks were authored against show: PutLogEvents of an Embedded-Metric-Format line into a log group materializes a CloudWatch datapoint in the EMF document's namespace WITHOUT any PutMetricData, readable through GetMetricStatistics/GetMetricData within seconds; and a METRIC_EXTRACTION_POLICY account policy selecting that group is what suppresses it. Both checks poll for that datapoint after the policy call and assert the emitted value.
Emulator: `Error: Check timed out after 60.0s` for both. The checks get past the PutAccountPolicy leg and then spin in the datapoint poll — no EMF-derived datapoint ever appears in the namespace, so the loop runs to the harness cap and the emulator never answers the question either way.
Why: an emulator that silently extracts nothing is indistinguishable from one whose extraction policy is working, so a client cannot verify that EMF ingestion emits metrics at all, nor that an account policy scoped to one group suppresses only that group. Any consumer that waits on an EMF-derived metric hangs.
Fix: EMF parsing on the PutLogEvents ingest path and the metric emission it drives, plus the METRIC_EXTRACTION_POLICY scope application in the account-policy state, in the cloudwatch-logs crate (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_log_events.rs and the account-policy state module). Related open issue which covers the same symptom under a DIFFERENT service token (cloudwatch__logs) and does not carry these two `logs` replay keys: 20261025T020208Z-cloudwatch-logs-emf-log-events-and-metric-extraction-policy-produce.md.
Traces: the two check ids above.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 110/430 passed), service `logs` 101/243:

Cause 1 — kinesis GetShardIterator stub (4 checks, verdict [error]):
  [error] logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events
  [error] logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events
  [error] logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint
  [error] logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest
  all four: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetShardIterator operation: operation GetShardIterator is scaffolded but not implemented in this emulator
  Expected: a ShardIterator for the live destination stream's shard (no NotImplemented in GetShardIterator's modeled error set).

Cause 2 — StartQuery NotImplemented (4 checks, verdict [mismatch]):
  logs.logsquery-parse-with-an-unclosed-regex-group-rejected —
    AssertionError: a parse regex with an unclosed capture group must be rejected with MalformedQueryException, got 'NotImplemented'
  logs.logsquery-redefining-a-parsed-field-rejected —
    AssertionError: naming an already-parsed ephemeral field in a later 'fields' command must be rejected with MalformedQueryException, got 'NotImplemented'
  logs.logsquery-window-ending-before-group-creation-rejected —
    AssertionError: a query window ending before the log group's creation time must be rejected with MalformedQueryException, got 'NotImplemented'
  logs.start-query-end-time-before-start-time-rejected —
    AssertionError: expected InvalidParameterException, got NotImplemented
  Expected: 400 MalformedQueryException / 400 InvalidParameterException. Actual: error code NotImplemented.

Cause 3 — CreateExportTask NotImplemented (verdict [mismatch]):
  logs.e2e-export-task-rejected-for-invalid-input —
    AssertionError: CreateExportTask against a nonexistent log group returned 'NotImplemented', expected ResourceNotFoundException
  Expected: 400 ResourceNotFoundException. Actual: error code NotImplemented.

Cause 4 — CreateDelivery NotImplemented (verdict [mismatch]):
  logs.e2e-delivery-missing-source-rejected —
    check line 44: assert code == "ResourceNotFoundException", code → AssertionError: NotImplemented
  Expected: 400 ResourceNotFoundException (example message "Requested Delivery Source does not exist in this account.") and no new delivery in DescribeDeliveries. Actual: error code NotImplemented.

Cause 5 — expectedRevisionId misclassification (verdict [mismatch]):
  logs.resource-policy-revision-conflict-rejected —
    check line 87: assert code == "InvalidParameterException", f"got {code!r}" → AssertionError: got 'OperationAbortedException'
  Request: PutResourcePolicy(resourceArn=<log-group-arn>, expectedRevisionId="<current-revision>-stale", policyDocument=<v2>)
  Expected: 400 InvalidParameterException (example message "Invalid ExpectedRevisionId"), policy document unchanged.
  Actual:   400 OperationAbortedException — the conflict branch, which is the arm reserved for a genuinely superseded revision later in the same check.

Cause 6 — EMF extraction emits no metric data (2 checks, verdict [mismatch]):
  logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected — Error: Check timed out after 60.0s
  logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group — Error: Check timed out after 60.0s
  Both hang in the post-policy poll loop:
    while True: put_log_events(<EMF line>); sleep(5); maximum, count = _stats(cw, namespace); if count > 0: break
  Expected: a datapoint in the EMF document's namespace whose Maximum equals the emitted value (3), with no PutMetricData call.
  Actual:   count stays 0 until the harness's 60 s cap; no datapoint is ever produced.

## Steps to Reproduce
Boot the emulator from this checkout and replay the `logs` behavioral checks listed in trace_ids.

Cause 1 (kinesis GetShardIterator stub):
  1. logs.CreateLogGroup + CreateLogStream; create a Kinesis stream and a role the filter can assume.
  2. logs.PutSubscriptionFilter(logGroupName=<g>, filterName=<f>, filterPattern="ERROR", destinationArn=<stream-arn>, roleArn=<role-arn>).
  3. logs.PutLogEvents of matching and non-matching lines.
  4. kinesis.GetShardIterator(StreamName=<s>, ShardId=<shard>, ShardIteratorType="TRIM_HORIZON")
     → observe NotImplemented instead of a ShardIterator.

Cause 2 (StartQuery):
  1. logs.CreateLogGroup.
  2. logs.StartQuery(logGroupName=<g>, startTime=<t0>, endTime=<t1>, queryString="fields @message | parse @message /(?<x/")  — unclosed capture group.
     → expect 400 MalformedQueryException, observe error code NotImplemented.
  3. logs.StartQuery(..., startTime=<t1>, endTime=<t0>)  — endTime before startTime.
     → expect 400 InvalidParameterException, observe error code NotImplemented.

Cause 3 (CreateExportTask):
  1. logs.CreateExportTask(logGroupName="<a-name-no-group-has>", from=<t0>, to=<t1>, destination=<bucket>)
     → expect 400 ResourceNotFoundException, observe error code NotImplemented.

Cause 4 (CreateDelivery):
  1. logs.CreateLogGroup + logs.PutDeliveryDestination(name=<d>, deliveryDestinationConfiguration={destinationResourceArn:<log-group-arn>}).
  2. logs.CreateDelivery(deliverySourceName="<a-name-no-source-has>", deliveryDestinationArn=<d-arn>)
     → expect 400 ResourceNotFoundException and an unchanged DescribeDeliveries, observe error code NotImplemented.

Cause 5 (expectedRevisionId):
  1. logs.CreateLogGroup; logs.PutResourcePolicy(resourceArn=<log-group-arn>, policyDocument=<v1>).
  2. logs.DescribeResourcePolicies(resourceArn=<log-group-arn>, policyScope="RESOURCE") → read revisionId R.
  3. logs.PutResourcePolicy(resourceArn=<log-group-arn>, policyDocument=<v2>, expectedRevisionId=R + "-stale")
     → expect 400 InvalidParameterException, observe 400 OperationAbortedException.
  4. Contrast: repeat with the correct R (accepted, new revision issued), then re-send with the now-superseded R → that one IS OperationAbortedException, showing both conditions collapse onto one code.

Cause 6 (EMF extraction):
  1. logs.CreateLogGroup + CreateLogStream.
  2. logs.PutAccountPolicy(policyType="METRIC_EXTRACTION_POLICY", policyDocument='{"EmbeddedMetricFormat":{"Status":"Enabled"}}', selectionCriteria='LogGroupName IN ["<g>"]') → refused.
  3. logs.PutLogEvents of an EMF line declaring one metric with value 3 in namespace N.
  4. Poll cloudwatch.GetMetricStatistics / GetMetricData for namespace N
     → expect a datapoint with Maximum 3 within seconds; observe none, and the check runs to the 60 s harness cap.
