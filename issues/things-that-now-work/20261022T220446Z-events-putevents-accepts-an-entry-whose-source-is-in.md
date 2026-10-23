Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-idp__events__sqs.xsvc-cognito-idp-1-put-events-forging-an-aws-cognito-idp-call-is-rejected
Issue-Key: cloudtrail__cognito-idp__events__sqs.xsvc-cognito-idp-1-put-events-forging-an-aws-cognito-idp-call-is-rejected|run#21|AssertionError|assert resp.get("FailedEntryCount") == 1, (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# PutEvents accepts an entry whose Source is in the reserved 'aws.' namespace instead of failing that entry

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: EventBridge PutEvents performs no reserved-namespace validation on an entry's `Source`, so a forged `aws.<service>` event is accepted and published.

AWS: recorded behavior (the check's asserted facts): the `aws.` Source prefix is reserved for AWS services. PutEvents accepts the CALL but rejects the offending ENTRY at entry level: `FailedEntryCount` counts it, that entry's result carries an `ErrorCode` (and `ErrorMessage`) and NO `EventId`, while the other entries in the same request are still accepted and each get an EventId. This is a namespace rule, not an IAM one — no policy can grant it. (Smithy: PutEvents returns PutEventsResponse{FailedEntryCount, Entries: [PutEventsResultEntry{EventId, ErrorCode, ErrorMessage}]}, i.e. the per-entry failure channel this rule uses is modeled; the status is 200 either way.)
Emulator: both entries are accepted — `FailedEntryCount` is 0 and both result entries carry only an EventId (`[{'EventId': '...0001'}, {'EventId': '...0002'}]`), so an entry with Source `aws.cognito-idp` is published as if it were a genuine AWS-service event.
Why: a client can forge events that appear to originate from an AWS service, and any rule matching `source: ["aws.<service>"]` will fire on caller-injected traffic — the emulator cannot be used to test that a pipeline is immune to forged AWS-sourced events, and code that handles the per-entry failure channel for this rejection is never exercised. It also masks the entry-level partial-failure contract: a caller who checks `FailedEntryCount` sees success where AWS reports one failed entry.
Fix: PutEvents request validation in the eventbridge service crate — reject each entry whose Source begins with `aws.` through the per-entry failure channel (ErrorCode set, EventId omitted, FailedEntryCount incremented) while still accepting the remaining entries.
Traces: cloudtrail__cognito-idp__events__sqs.xsvc-cognito-idp-1-put-events-forging-an-aws-cognito-idp-call-is-rejected.

## Evidence
`AssertionError: exactly the aws.cognito-idp entry fails: expected FailedEntryCount 1, actual 0 with entries [{'EventId': '18e0f89e-4634-e9e4-0000-000000000001'}, {'EventId': '18e0f89e-4634-e9e4-0000-000000000002'}]` (cloudtrail__cognito-idp__events__sqs.xsvc-cognito-idp-1-put-events-forging-an-aws-cognito-idp-call-is-rejected).

The check sends both entries in ONE PutEvents call — the reserved `aws.cognito-idp` source and a run-unique custom source with the same detail-type and detail — so the custom entry is the positive control proving the rule and sink were live when the forged entry was accepted.

## Steps to Reproduce
1. Create an event bus and a rule matching both a run-unique custom source and `aws.cognito-idp`, with an SQS queue target.
2. Call `put_events(Entries=[{'Source': 'aws.cognito-idp', 'DetailType': <dt>, 'Detail': <json>, 'EventBusName': <bus>}, {'Source': '<custom-source>', 'DetailType': <dt>, 'Detail': <json>, 'EventBusName': <bus>}])`.
   Expect: HTTP 200, `FailedEntryCount == 1`; the `aws.cognito-idp` entry's result carries an ErrorCode and no EventId, the custom entry's result carries an EventId.
   Observe: `FailedEntryCount == 0` and both entries carry an EventId.
3. Drain the target queue: only the custom-source event should have been delivered.
