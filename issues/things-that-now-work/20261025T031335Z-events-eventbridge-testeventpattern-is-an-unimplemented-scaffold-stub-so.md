Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive
Issue-Key: athena__glue__s3|TestEventPattern|NotImplemented
Issue-Commit: 176bb95e3c08e52684921328b2da3d677bd84f34

# EventBridge TestEventPattern is an unimplemented scaffold stub, so a Glue-job-name event pattern cannot be validated before the rule is relied on (1 cause)

> _Note: this issue was identified against the code base at commit `176bb95e3c08e52684921328b2da3d677bd84f34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestEventPattern

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1 — TestEventPattern answers NotImplemented.

AWS: Smithy com.amazonaws.eventbridge#TestEventPattern takes a required EventPattern (length 0..4096) and a required Event JSON string and returns HTTP 200 with the boolean member Result (default false); its only modeled errors are InvalidEventPatternException (client, 400) and InternalException (server, 500). An event that matches the pattern returns Result == true; one that does not returns Result == false.

Emulator: the call is refused with error code NotImplemented and the message 'operation TestEventPattern is scaffolded but not implemented in this emulator' — a code the operation does not model, so neither Result value is ever observable.

Why: TestEventPattern is the pre-flight a client uses to prove a rule's pattern matches the events it cares about (here, filtering 'Glue Job State Change' events down to one jobName) before depending on delivery. With the operation stubbed, the pattern can only be validated by provoking a real event, and any caller treating NotImplemented as a transient/server fault retries a call that can never succeed.

Fix: implement the handler services/service-com-amazonaws-eventbridge-awsevents/src/ops/test_event_pattern.rs (evaluate the pattern against the supplied event and return Result; refuse a malformed pattern with InvalidEventPatternException/400). The open issue 20261025T002357Z-events-eventbridge-rule-state-connection-archive-api-destination-and.md names TestEventPattern among its stubbed operations but does not carry this check's replay key, so this check id is recorded here and can be closed together with it.

Traces: athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive.

Service-attribution note: the leading token of the check id is the composite 'athena__glue__s3' (the check declares SERVICES = ['athena','glue','s3'] and does not list EventBridge at all), which is not a single service token. The failing call is EventBridge's TestEventPattern, so this issue is filed under service 'events' (bootstrapped).

## Evidence
Expected (AWS): events.test_event_pattern(EventPattern=json.dumps({'source': ['aws.glue'], 'detail-type': ['Glue Job State Change'], 'detail': {'jobName': [<job>]}}), Event=<a JSON event for that job>) -> HTTP 200 {'Result': True}; the same call with a non-matching jobName -> {'Result': False}.

Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestEventPattern operation: operation TestEventPattern is scaffolded but not implemented in this emulator.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. events.test_event_pattern(EventPattern='{"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":["my-job"]}}', Event='{"id":"1","detail-type":"Glue Job State Change","source":"aws.glue","account":"123456789012","time":"2024-01-01T00:00:00Z","region":"us-east-1","resources":[],"detail":{"jobName":"my-job","state":"SUCCEEDED","jobRunId":"jr_1"}}') -> observe NotImplemented; expect {'Result': True}.
3. Repeat with detail.jobName changed to another name -> expect {'Result': False}.
4. Re-replay athena__glue__s3.eventbridge-rule-filters-glue-job-name-positive.
