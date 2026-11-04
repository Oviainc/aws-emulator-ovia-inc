Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__logs.log-cwl-plumbing-kinesis-control-message-precedes-data-message-with-exact-lines kinesis__logs.deep-cwlogs-semantics-two-subscription-filters-envelopes-carry-filter-name-and-event-ids kinesis__logs.deep-cwlogs-semantics-deleted-subscription-filter-delivery-blocked
Issue-Key: kinesis__logs.log-cwl-plumbing-kinesis-control-message-precedes-data-message-with-exact-lines|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: kinesis__logs.deep-cwlogs-semantics-two-subscription-filters-envelopes-carry-filter-name-and-event-ids|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: kinesis__logs.deep-cwlogs-semantics-deleted-subscription-filter-delivery-blocked|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: b37186c89a6243bd753c08865f727ca2e1551d74

# kinesis__logs: a CloudWatch Logs subscription filter delivers nothing to its Kinesis destination — neither the CONTROL_MESSAGE health check nor any DATA_MESSAGE record ever appears, so all three Kinesis delivery checks hang

> _Note: this issue was identified against the code base at commit `b37186c89a6243bd753c08865f727ca2e1551d74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One net-new root cause in the CloudWatch-Logs-to-Kinesis subscription-delivery surface, grounded in the three failing behavioral checks whose check id leads with `kinesis__logs`. None of them is marked ALREADY FILED or OWNED in the replay report. Cause count: 1.

----------------------------------------------------------------
Cause 1 of 1: PutSubscriptionFilter to a Kinesis stream is accepted, and the destination stream then receives no record at all — no CONTROL_MESSAGE health-check record on creation and no DATA_MESSAGE record for subsequently written events.
Failing checks: kinesis__logs.log-cwl-plumbing-kinesis-control-message-precedes-data-message-with-exact-lines, kinesis__logs.deep-cwlogs-semantics-two-subscription-filters-envelopes-carry-filter-name-and-event-ids, kinesis__logs.deep-cwlogs-semantics-deleted-subscription-filter-delivery-blocked

AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) PutSubscriptionFilter takes logGroupName, filterName, filterPattern and destinationArn as @required plus roleArn and applyOnTransformedLogs, and returns Unit — an empty 200 — so the model says nothing about the delivered payload and the delivery itself is recorded behavior only. The recordings show that, on a Kinesis destination, CloudWatch Logs:
  (a) immediately writes ONE health-check record to the stream, before any log event exists: gzip (magic 0x1f 0x8b) wrapping a JSON envelope with messageType "CONTROL_MESSAGE", owner the literal "CloudwatchLogs" (the service, not the account), logGroup and logStream both the empty string, subscriptionFilters [] and exactly one logEvent whose message begins with the fixed text "CWL CONTROL MESSAGE: Checking health of destination";
  (b) then, for each batch of matching events, writes gzip DATA_MESSAGE envelopes whose owner is the caller's account id, logGroup and logStream are the source group and stream, subscriptionFilters names exactly the one filter that matched, and logEvents carry the written messages with their own millisecond timestamps and ids equal to the eventId FilterLogEvents reports for the same stored events — with the control record ordered before the first data record on the shard's own sequence numbers;
  (c) honours the filter set as it stands: an event matching two filters is delivered once under each filter's name, events written before a filter existed are never delivered, and after DeleteSubscriptionFilter that filter's deliveries stop (eventually) while a sibling filter on the same group keeps delivering to the same stream, with no retroactive delivery of events written while the filter was absent.
Emulator: every one of the three checks hangs in its "read the stream and look for the record" poll and is killed at the harness cap — `Error: Check timed out after 60.0s`. The Kinesis data plane answers now (kinesis GetShardIterator/GetRecords are implemented in this checkout — services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_records.rs no longer carries the SCAFFOLDED marker), so the reads succeed and simply return nothing: no CONTROL_MESSAGE after PutSubscriptionFilter, and no DATA_MESSAGE after PutLogEvents. Not one of the three checks can get past its FIRST positive leg, so even the baseline "both filters deliver" control never completes.
Why: a client cannot consume a CloudWatch Logs subscription at all. The subscription is accepted with a 200 and then exists as a wire-level fiction: nothing arrives on the destination, so any consumer (a Lambda/Kinesis analytics consumer, a log-shipping pipeline, a cross-account logical destination) blocks forever rather than seeing data or an error. Because the health-check record is also missing, a client has no way to confirm at wiring time that the destination is even reachable — the one signal AWS gives synchronously after PutSubscriptionFilter. Every downstream contract is consequently ungraded: envelope identity (owner/logGroup/logStream/subscriptionFilters), the id join back to FilterLogEvents' eventId, per-filter fan-out of a shared event, control-before-data ordering, and the stop-on-delete / no-retroactive-delivery semantics.
Fix: the subscription-filter delivery path in the cloudwatch-logs crate must actually produce records on a Kinesis destination: on PutSubscriptionFilter (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs) resolve destinationArn to the Kinesis stream and put the CONTROL_MESSAGE health-check record through the Kinesis shard state; on the PutLogEvents ingest path (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_log_events.rs) evaluate each stored event against every live filter on the group and put one gzip DATA_MESSAGE envelope per matching filter, stamping the stored event's own id and timestamp and the matching filter's name; and on DeleteSubscriptionFilter stop only that filter's deliveries. Delivery must be driven from the live filter set so a deleted filter stops and a recreated one resumes without replaying events written while it was absent.
Related issues, none of which carries any of these three replay keys: the open 20261026T072730Z-logs-cloudwatch-logs-subscription-filter-delivery-produces-nothing.md and 20261025T215029Z-logs-cloudwatch-logs-putdestinationpolicy-invents-the-missing-destination-refusal.md (same symptom class under the `logs` token, the latter written while kinesis GetRecords was still a stub), the open 20261025T100129Z-logs-cloudwatch-logs-subscription-destination-plumbing-a-logical-destination.md (logical-destination/roleArn admission and the per-group filter quota), and the open 20261026T063455Z-kms-logs-a-subscription-filter-on-a-cmk.md (the same absence for a CMK-encrypted group, under the kms__logs token). The CONTROL_MESSAGE record, the id join and the delete/recreate semantics graded here appear in none of them.
Traces: the three check ids above.

## Evidence
Replay report (suite simparity-behavioral-ga-prod-v1, 134/432 passed); service kinesis__logs 0/4, with these three failures net-new (the fourth, kinesis__logs.log-cwl-plumbing-sixth-subscription-filter-rejected-while-five-deliver, is OWNED by 20261025T100129Z-logs-cloudwatch-logs-subscription-destination-plumbing-a-logical-destination.md and is excluded here):

  [mismatch] kinesis__logs.log-cwl-plumbing-kinesis-control-message-precedes-data-message-with-exact-lines — Error: Check timed out after 60.0s
    Setup: kinesis.CreateStream (polled ACTIVE) -> iam role trusted by logs.amazonaws.com with kinesis:PutRecord -> logs.CreateLogGroup + CreateLogStream -> logs.PutSubscriptionFilter(filterPattern="", destinationArn=<stream arn>, roleArn=<role>).
    The check then polls its shard reader (kinesis.GetShardIterator TRIM_HORIZON + GetRecords) for the FIRST record, budgeting 120 s, with the description "a CONTROL_MESSAGE record appears on <stream> after PutSubscriptionFilter".
    Expected: one gzip CONTROL_MESSAGE record with owner "CloudwatchLogs", logGroup "" and logStream "", subscriptionFilters [], and one logEvent whose message starts with "CWL CONTROL MESSAGE: Checking health of destination"; then, after PutLogEvents of three exact lines, gzip DATA_MESSAGE records whose owner is the STS account, logGroup/logStream are the source names, subscriptionFilters is exactly [<filterName>], each logEvent's timestamp equals the written millisecond timestamp and each id equals the eventId FilterLogEvents reports for that event — with the control record's shard sequence number lower than the first data record's.
    Actual:   the poll never sees a single record; the check is killed at the 60 s harness cap.

  [mismatch] kinesis__logs.deep-cwlogs-semantics-two-subscription-filters-envelopes-carry-filter-name-and-event-ids — Error: Check timed out after 60.0s
    Setup: one log group, two filters ("alpha", "beta") on it, both targeting ONE Kinesis stream; two events written BEFORE either filter existed; then events matching alpha only, beta only, and both.
    Expected: gzip DATA_MESSAGE envelopes per filter, each naming exactly its own filter in subscriptionFilters and carrying exactly that filter's matches; the both-matching event delivered under BOTH names; delivered id == FilterLogEvents' eventId for the same stored event; pre-subscription and non-matching events never delivered.
    Actual:   nothing is delivered; the check hangs to the cap.

  [mismatch] kinesis__logs.deep-cwlogs-semantics-deleted-subscription-filter-delivery-blocked — Error: Check timed out after 60.0s
    Setup: one group with a "kept" and a "dropped" filter, both to the same stream; each round writes one event per filter in the SAME PutLogEvents batch and only counts once the SURVIVING filter's event has arrived (a differential barrier, never a sleep).
    Expected: the round-0 positive control delivers both events, DeleteSubscriptionFilter(dropped) then stops the dropped filter's deliveries within three rounds while the kept filter keeps delivering, re-creating the filter resumes delivery, and events written while it was absent are never delivered retroactively.
    Actual:   the round-0 positive control never completes — the surviving filter's own event never arrives — so the check is killed at the cap before the deletion leg is reached.

Corroboration that the blocker is the logs side and not the Kinesis read path: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/get_records.rs documents itself as "Implemented: answers the data records the iterator's position names in one shard of one stream" (the SCAFFOLDED/NotImplemented marker cited by the earlier issues is gone), and no check in this run reports NotImplemented for kinesis GetRecords or GetShardIterator. The reads answer; the stream is empty.

## Steps to Reproduce
Boot the emulator from this checkout and replay the three `kinesis__logs` checks listed in trace_ids.

  1. kinesis.CreateStream(StreamName=<s>, ShardCount=1); poll kinesis.DescribeStream until ACTIVE; kinesis.ListShards to get the shard id.
  2. iam.CreateRole(<r>) with a trust policy for logs.amazonaws.com and an inline policy allowing kinesis:PutRecord/PutRecords on the stream ARN.
  3. logs.CreateLogGroup(<g>) + logs.CreateLogStream(<g>, <ls>).
  4. logs.PutSubscriptionFilter(logGroupName=<g>, filterName=<f>, filterPattern="", destinationArn=<stream arn>, roleArn=<role arn>) -> accepted, empty 200.
  5. kinesis.GetShardIterator(ShardIteratorType=TRIM_HORIZON) + kinesis.GetRecords, draining the shard.
     -> expect ONE record: gzip, messageType "CONTROL_MESSAGE", owner "CloudwatchLogs", logGroup "", logStream "", subscriptionFilters [], one logEvent whose message begins "CWL CONTROL MESSAGE: Checking health of destination".
     -> observe no records at all.
  6. logs.PutLogEvents(<g>, <ls>, three events with explicit millisecond timestamps); drain the shard again.
     -> expect gzip DATA_MESSAGE records with owner = sts.GetCallerIdentity Account, logGroup=<g>, logStream=<ls>, subscriptionFilters=[<f>], one logEvent per written line with the written timestamp and with id equal to the eventId logs.FilterLogEvents reports for that event; the control record's sequence number must be lower than the first data record's.
     -> observe no records at all.
  7. Two-filter fan-out: put a second filter <f2> with a different pattern on the SAME group to the SAME stream, write one event matching only <f>, one matching only <f2>, and one matching both.
     -> expect each event delivered under exactly the filters it matched, the shared event delivered twice (once per filter name); events written before the filters existed never delivered.
     -> observe nothing delivered.
  8. Delete/recreate: with a "kept" and a "dropped" filter on one group, write one event per filter in a single PutLogEvents batch per round; then logs.DeleteSubscriptionFilter(<dropped>) and keep rounding.
     -> expect the pre-delete round to deliver both, the dropped filter's deliveries to stop within a few rounds while the kept filter keeps delivering, re-creating the dropped filter to resume delivery, and no retroactive delivery of events written while it was absent.
     -> observe the pre-delete control round never delivering anything, so the whole sequence is unreachable.
