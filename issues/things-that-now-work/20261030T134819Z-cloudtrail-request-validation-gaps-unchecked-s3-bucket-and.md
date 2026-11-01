Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-trail-creation-validation-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-trail-creation-validation-errors cloudtrail-trail-error-paths cloudtrail-trail-lifecycle-error-paths cloudtrail-insights-metric-data-read cloudtrail-lookup-events-and-public-keys cloudtrail-event-configuration-error-paths
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|CreateTrail|4xx->2xx|$.Message
Issue-Key: cloudtrail|CreateTrail|4xx->2xx|$.__type
Issue-Key: cloudtrail|CreateTrail|4xx->2xx|status
Issue-Key: cloudtrail|ListInsightsMetricData|4xx->2xx|$.Message
Issue-Key: cloudtrail|ListInsightsMetricData|4xx->2xx|$.__type
Issue-Key: cloudtrail|ListInsightsMetricData|4xx->2xx|status
Issue-Key: cloudtrail|LookupEvents|4xx->4xx|$.Message
Issue-Key: cloudtrail|PutEventConfiguration|4xx->4xx|$.__type
Issue-Key: cloudtrail|PutEventConfiguration|4xx->4xx|$.message
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# CloudTrail request validation gaps: unchecked S3 bucket and insights window accept calls AWS refuses, and two rejection messages diverge

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrail

## Description
Four CloudTrail request-validation checks diverge from the recorded AWS responses. Two validations are absent, so a call AWS refuses is answered HTTP 200; two are present but produce the wrong error code or the wrong message text. In each case the status, the error code and the message are the client's entire contract for the refusal, so all four are wire defects.

There are 4 distinct causes in this issue; all 4 must be closed.

Cause 1 of 4: CreateTrail accepts an S3 bucket that does not exist.
AWS: Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101) models CreateTrail.S3BucketName as a required String and binds error S3BucketDoesNotExistException (errorKind client, awsQueryError code S3BucketDoesNotExist). AWS resolves the destination bucket before creating the trail and refuses when it is absent. Recorded response: HTTP 400 with __type 'S3BucketDoesNotExistException' and Message 'S3 bucket <bucket name> does not exist!' — fixed text 'S3 bucket ' and ' does not exist!' around one variable part, the S3BucketName taken verbatim from the request. (Smithy gives this error httpError 404; the recorded wire status is 400, and the recording governs.)
Emulator: HTTP 200 and the trail is created, with no __type and no Message in the body. The destination is never resolved against S3 state.
Why: a client gets a trail that can never deliver a log file while being told the trail is healthy, and its S3BucketDoesNotExist remediation path is unreachable. This is the single commonest real CreateTrail failure, so the emulator diverges precisely where callers most need fidelity.
Fix: CreateTrail's validation must resolve S3BucketName against the emulator's S3 state and raise S3BucketDoesNotExistException with the message above before persisting the trail.
Traces: cloudtrail-trail-creation-validation-errors#task#2; same cluster also covers cloudtrail-trail-error-paths#task#10 and cloudtrail-trail-lifecycle-error-paths#task#1.

Cause 2 of 4: ListInsightsMetricData accepts a StartTime outside the supported 90-day window.
AWS: Smithy models ListInsightsMetricData.StartTime/EndTime as timestamp Date and binds error InvalidParameterException (client, awsQueryError code InvalidParameter). Insights metric data is only queryable over a bounded retention window, and a StartTime further in the past than that window is refused rather than silently clamped. Recorded response: HTTP 400 with __type 'InvalidParameterException' and Message 'The start time cannot be greater than 90 days in the past.' — ENTIRELY fixed text: it echoes neither the supplied StartTime, nor the member name, nor the computed age of the request.
Emulator: HTTP 200 with a normal metric-data response body; no __type and no Message. The StartTime window is never range-checked.
Why: a caller that asks for an out-of-window range is handed a result set (in practice an empty or truncated one) and reads it as 'no insight activity in that period' rather than 'this question cannot be asked'. Dashboards and backfill jobs silently produce wrong history instead of failing loudly, and the InvalidParameter branch is untestable.
Fix: the ListInsightsMetricData handler's parameter validation must compare StartTime against the retention window and raise InvalidParameterException with that fixed message before any data is gathered.
Traces: cloudtrail-insights-metric-data-read#task#1

Cause 3 of 4: LookupEvents' InvalidTimeRangeException message is a generated comparison string where AWS's is fixed text.
AWS: Smithy binds LookupEvents error InvalidTimeRangeException (client, awsQueryError code InvalidTimeRange); StartTime/EndTime target timestamp Date. Recorded response for StartTime after EndTime: HTTP 400 with Message 'The start time precedes the end time.' — all FIXED text, containing NO request-derived values: no timestamps, no member names, no units.
Emulator: status 400 and the error code match, but Message is 'StartTime (<start> ms since the epoch) is after EndTime (<end> ms since the epoch): the start of a lookup window must not be after its end', interpolating the two request timestamps as epoch milliseconds. The specific epoch numbers in the recording are that request's values filling the emulator's template; the defect is the template — it is differently worded AND differently SHAPED (variable where AWS is fixed).
Why: clients, SDK helpers and operator runbooks match on the message text. AWS's sentence never appears, so assertions and log-based alerting keyed on it never fire, and anything parsing the AWS shape (a message with no interpolated values) mis-parses.
Fix: the StartTime/EndTime ordering check in the LookupEvents handler must raise CloudTrail's fixed message instead of formatting a comparison message.
Traces: cloudtrail-lookup-events-and-public-keys#task#3; same cluster also covers cloudtrail-lookup-events-and-public-keys#task#4.
Note: the open behavioral issue 20261029T173328Z-cloudtrail-lookupevents-rejection-messages-use-a-generic-constraint.md describes this same message template as its Cause 3, but it carries only behavioral check ids — no open issue owns these wire replay keys (the replay report lists no owner for this cluster), so the wire binding is filed here.

Cause 4 of 4: PutEventConfiguration resolves the event data store before validating its required parameters, so a missing-parameter request is answered the wrong error code and no message at all.
AWS: Smithy models PutEventConfiguration with optional TrailName/EventDataStore plus MaxEventSize (enum) and ContextKeySelectors (list, length 0..2), and binds both EventDataStoreNotFoundException and InvalidParameterException. AWS validates the request's own parameters BEFORE resolving the target resource: a call that omits the configuration members is refused on the parameters, whatever the state of the target. Recorded response: HTTP 400 with __type 'ValidationException' and Message 'You must specify the maximum event size and context key selectors.' — fixed text naming the two required members, with no variable part. (ValidationException is not among this operation's modelled errors, so this code and message are recorded behavior only.)
Emulator: status 400 (correct) but __type 'EventDataStoreNotFoundException' and NO Message member in the body at all. It skipped parameter validation, went straight to resolving EventDataStore, and reported the resolution failure — and then serialized the error without the Message member every CloudTrail error body carries.
Why: two independent breakages for a client. The wrong code sends the caller down a resource-provisioning path (create or await the event data store) when the actual fault is in its own request and no amount of provisioning will fix it; and a body with no Message leaves SDK error objects with an empty message, so the one string that would have told an operator which members were missing is gone.
Fix: order the checks in the PutEventConfiguration handler so required-parameter validation runs before target resolution, and ensure the error serializer always populates Message.
Traces: cloudtrail-event-configuration-error-paths#task#2

Relation to existing open issues: 20261023T135552Z-cloudtrail-createtrail-updatetrail-skip-every-referenced-resource-and.md describes the unchecked CreateTrail S3 bucket as its Cause 1, and 20261029T173328Z-cloudtrail-lookupevents-rejection-messages-use-a-generic-constraint.md describes the LookupEvents time-range message as its Cause 3 — but both are behavioral-corpus issues whose keys are check ids, and the replay report lists NO owner for any of the four clusters here, so none of these wire steps is gated by an existing issue. Causes 2 and 4 are not described by any open issue.

## Evidence
Wire replay of simparity-wire-aws-v1 (emulator up; /__admin/health 200). Four mismatch clusters, all service=cloudtrail. Expected values are the recorded AWS responses; actual values are the emulator's.

1) CreateTrail — cloudtrail-trail-creation-validation-errors#task#2 (cluster also covers cloudtrail-trail-error-paths#task#10, cloudtrail-trail-lifecycle-error-paths#task#1):
   status: expected 400, got 200
   $.__type: missing (expected 'S3BucketDoesNotExistException')
   $.Message: missing (expected 'S3 bucket <bucket> does not exist!' — recorded with the probe bucket name in the variable slot)

2) ListInsightsMetricData — cloudtrail-insights-metric-data-read#task#1:
   status: expected 400, got 200
   $.__type: missing (expected 'InvalidParameterException')
   $.Message: missing (expected 'The start time cannot be greater than 90 days in the past.')

3) LookupEvents — cloudtrail-lookup-events-and-public-keys#task#3 (cluster also covers #task#4):
   status 400 matched; $.__type matched.
   $.Message: value_mismatch — expected 'The start time precedes the end time.', got 'StartTime (1717286400000 ms since the epoch) is after EndTime (1717200000000 ms since the epoch): the start of a lookup window must not be after its end'. The two epoch values are this request's timestamps filling the emulator's template and are not part of the requirement; the requirement is the fixed sentence.

4) PutEventConfiguration — cloudtrail-event-configuration-error-paths#task#2:
   status 400 matched.
   $.__type: value_mismatch — expected 'ValidationException', got 'EventDataStoreNotFoundException'
   $.Message: missing — expected 'You must specify the maximum event size and context key selectors.', got None (no Message member emitted)

The bucket name, region and account id in the recorded strings are per-environment values filling template slots; the requirements are the HTTP 400 status, the error codes S3BucketDoesNotExistException / InvalidParameterException / InvalidTimeRangeException / ValidationException, and the fixed message text identified per cause above.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus simparity-wire-aws-v1.
2. Replay these steps: cloudtrail-trail-creation-validation-errors#task#2, cloudtrail-insights-metric-data-read#task#1, cloudtrail-lookup-events-and-public-keys#task#3, cloudtrail-event-configuration-error-paths#task#2.
3. Manually, with a cloudtrail client:
   a. CreateTrail(Name=<new>, S3BucketName=<a bucket never created>) -> expect 400 S3BucketDoesNotExistException with Message 'S3 bucket <that bucket> does not exist!'; observe 200 and a created trail.
   b. ListInsightsMetricData(EventSource='s3.amazonaws.com', EventName='PutObject', InsightType='ApiCallRateInsight', StartTime=now-120days, EndTime=now) -> expect 400 InvalidParameterException with Message 'The start time cannot be greater than 90 days in the past.'; observe 200 with a metric-data body.
   c. LookupEvents(StartTime=now, EndTime=now-1day) -> expect 400 InvalidTimeRangeException with Message 'The start time precedes the end time.'; observe the epoch-millisecond comparison message.
   d. PutEventConfiguration(EventDataStore=<an event data store that does not exist>) with MaxEventSize and ContextKeySelectors both omitted -> expect 400 ValidationException with Message 'You must specify the maximum event size and context key selectors.'; observe 400 EventDataStoreNotFoundException with no Message member.
   e. Repeat (d) against an event data store that DOES exist, still omitting both members, to confirm the parameter check is what should fire.
