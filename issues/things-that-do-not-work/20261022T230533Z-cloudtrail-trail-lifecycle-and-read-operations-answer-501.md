Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-add-list-remove-tags-trail
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-add-list-remove-tags-trail cloudtrail-default-event-selectors-on-new-trail cloudtrail-insights-data-and-selectors-on-never-enabled-trail cloudtrail-list-insights-data-time-range-rejected-on-live-trail cloudtrail-create-multi-region-trail-echoes-response cloudtrail-create-trail-defaults-and-arn-format cloudtrail-describe-trails-unknown-name-returns-empty cloudtrail-trail-creation-validation-errors cloudtrail-lake-read-surface-and-error-contracts
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|status
Issue-Key: cloudtrail|DeleteTrail|2xx->5xx|status
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.IncludeGlobalServiceEvents
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.IsMultiRegionTrail
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.IsOrganizationTrail
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.LogFileValidationEnabled
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.Name
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.RecursiveLogging
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.S3BucketName
Issue-Key: cloudtrail|CreateTrail|2xx->5xx|$.TrailARN
Issue-Key: cloudtrail|GetTrail|2xx->5xx|$.Trail
Issue-Key: cloudtrail|GetTrail|2xx->5xx|status
Issue-Key: cloudtrail|DescribeTrails|2xx->5xx|$.trailList
Issue-Key: cloudtrail|DescribeTrails|2xx->5xx|status
Issue-Key: cloudtrail|ListTrails|2xx->5xx|$.Trails
Issue-Key: cloudtrail|ListTrails|2xx->5xx|status
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# CloudTrail trail lifecycle and read operations answer 501 NotImplemented instead of their 200 responses

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrail

## Description
SIX causes (Cause 1 of 6 .. Cause 6 of 6) sharing one fault: the happy path of every CloudTrail trail-management operation is refused with HTTP 501 and the unmodeled error code `NotImplemented` instead of returning its 200 response body, even though these operations are NOT scaffold stubs (the replay report reports no stub_path for any of them, and services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_trail.rs is a complete handler). Nothing a trail-based scenario needs can be set up. Related open issue: 'Every CloudTrail operation answers HTTP 501 NotImplemented from the dispatcher, including the ones whose handlers are fully implemented' (20261022T220446Z-cloudtrail-every-cloudtrail-operation-answers-http-501-notimplemented-from.md) describes the same suspected dispatch root cause from the BEHAVIORAL corpus, but it carries no wire replay keys, so none of the six fixture steps below are gated by it; they must be re-replayed from this issue.

Cause 1 of 6 — CreateTrail (fixture cloudtrail-add-list-remove-tags-trail, step setup#3).
AWS: Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json): CreateTrail input Name (required), S3BucketName (required); output CreateTrailResponse; 501/NotImplemented is not a modeled error of the operation. Recorded response: HTTP 200.
Emulator: HTTP 501, body `__type` = `NotImplemented`.
Why: a client cannot create a trail at all, so every trail scenario dies at setup.
Fix: the operation binding/dispatch of the cloudtrail service crate (the generated service/op table), not the CreateTrail handler, which exists.
Traces: cloudtrail-add-list-remove-tags-trail#setup#3 (+3 more fixtures in the cluster).

Cause 2 of 6 — DeleteTrail (fixture cloudtrail-add-list-remove-tags-trail, step teardown#13).
AWS: Smithy: DeleteTrail input Name (required), output DeleteTrailResponse (empty); modeled errors are TrailNotFoundException / InvalidTrailNameException / CloudTrailARNInvalidException only. Recorded response: HTTP 200.
Emulator: HTTP 501 NotImplemented.
Why: trails can never be torn down, so state leaks across a client's scenarios and a delete-then-recreate sequence is impossible.
Fix: same dispatch/binding layer; DeleteTrail handler and state removal.
Traces: cloudtrail-add-list-remove-tags-trail#teardown#13 (+5 more fixtures).

Cause 3 of 6 — CreateTrail response echo (fixture cloudtrail-create-multi-region-trail-echoes-response, step task#3).
AWS: Smithy: CreateTrailResponse carries Name, TrailARN, S3BucketName, IncludeGlobalServiceEvents, IsMultiRegionTrail, IsOrganizationTrail, LogFileValidationEnabled, RecursiveLogging. Recorded response: HTTP 200 echoing the request's name/bucket plus the trail ARN in the form arn:aws:cloudtrail:<region>:<account>:trail/<name> and the boolean defaults.
Emulator: HTTP 501 NotImplemented; the whole body is absent (Name, TrailARN, S3BucketName, IsMultiRegionTrail, IncludeGlobalServiceEvents, IsOrganizationTrail, LogFileValidationEnabled, RecursiveLogging all missing).
Why: a client that reads TrailARN off the create response to tag or address the trail has nothing to read.
Fix: same dispatch/binding layer, then the CreateTrail serializer/response assembly.
Traces: cloudtrail-create-multi-region-trail-echoes-response#task#3 (+1 more fixture).

Cause 4 of 6 — GetTrail on an existing trail (fixture cloudtrail-create-multi-region-trail-echoes-response, step task#4).
AWS: Smithy: GetTrail output member Trail (shape Trail). Recorded response: HTTP 200 with a Trail structure carrying HomeRegion, HasCustomEventSelectors, HasInsightSelectors, IncludeGlobalServiceEvents, IsMultiRegionTrail, IsOrganizationTrail, LogFileValidationEnabled, Name, RecursiveLogging, S3BucketName, TrailARN.
Emulator: HTTP 501 NotImplemented, $.Trail missing.
Why: a client cannot read back a trail's configuration, so read-after-write verification and any configuration-dependent branch is broken.
Fix: same dispatch/binding layer; GetTrail handler reads the trail registry in crate::state.
Traces: cloudtrail-create-multi-region-trail-echoes-response#task#4 (+2 more fixtures).

Cause 5 of 6 — DescribeTrails with an unknown trail name (fixture cloudtrail-describe-trails-unknown-name-returns-empty, step task#4).
AWS: Smithy: DescribeTrails output member trailList (list TrailList); InvalidTrailNameException/CloudTrailARNInvalidException are the only name-related modeled errors. Recorded response: HTTP 200 with trailList = [] — an unknown name is a successful empty list, not an error.
Emulator: HTTP 501 NotImplemented, $.trailList missing.
Why: a client probing for a trail's existence gets a hard 501 where AWS returns an empty list, so existence checks read as service failure.
Fix: same dispatch/binding layer; DescribeTrails handler/serializer.
Traces: cloudtrail-describe-trails-unknown-name-returns-empty#task#4 (+1 more fixture).

Cause 6 of 6 — ListTrails on an empty account (fixture cloudtrail-lake-read-surface-and-error-contracts, step task#1).
AWS: Smithy: ListTrails output members Trails (list) + NextToken, paginated (inputToken NextToken / outputToken NextToken / items Trails); only OperationNotPermittedException and UnsupportedOperationException are modeled. Recorded response: HTTP 200 with Trails = [].
Emulator: HTTP 501 NotImplemented, $.Trails missing.
Why: enumeration/pagination over trails is unusable, and a client cannot distinguish 'no trails' from 'service broken'.
Fix: same dispatch/binding layer; ListTrails handler/serializer.
Traces: cloudtrail-lake-read-surface-and-error-contracts#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):
- CreateTrail — expected 200, got 501 (cluster cloudtrail-add-list-remove-tags-trail#setup#3; also cloudtrail-default-event-selectors-on-new-trail#setup#3, cloudtrail-insights-data-and-selectors-on-never-enabled-trail#setup#3, cloudtrail-list-insights-data-time-range-rejected-on-live-trail#setup#3).
- DeleteTrail — expected 200, got 501 (cluster cloudtrail-add-list-remove-tags-trail#teardown#13; 6 steps affected).
- CreateTrail — expected 200, got 501 with every response member missing: $.Name, $.TrailARN, $.S3BucketName, $.IsMultiRegionTrail, $.IncludeGlobalServiceEvents, $.IsOrganizationTrail, $.LogFileValidationEnabled, $.RecursiveLogging (cluster cloudtrail-create-multi-region-trail-echoes-response#task#3). Example recorded values: Name 'sim2real-ct-mrtrail-1fa64101', TrailARN 'arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-ct-mrtrail-1fa64101' — the generated name/account are examples; the requirement is the status and the presence/shape of the members.
- GetTrail — expected 200, got 501, $.Trail missing (cluster cloudtrail-create-multi-region-trail-echoes-response#task#4; 3 steps).
- DescribeTrails — expected 200, got 501, $.trailList missing (expected []) (cluster cloudtrail-describe-trails-unknown-name-returns-empty#task#4; 2 steps).
- ListTrails — expected 200, got 501, $.Trails missing (expected []) (cluster cloudtrail-lake-read-surface-and-error-contracts#task#1).
All six clusters report stub_path = "" and owned_by = [], i.e. no scaffold stub blocks them and no open issue carries their replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK CloudTrail client at it (region us-west-2, awsJson1_1).
2. CreateTrail with a valid Name and an S3BucketName: expect HTTP 200 and a CreateTrailResponse carrying Name, S3BucketName and TrailARN arn:aws:cloudtrail:<region>:<account>:trail/<name> plus the boolean members. Observe HTTP 501 with __type NotImplemented and no body members.
3. GetTrail with that name: expect HTTP 200 with the Trail structure. Observe HTTP 501 NotImplemented.
4. DescribeTrails with trailNameList = ['<a name that does not exist>']: expect HTTP 200 with trailList = []. Observe HTTP 501.
5. ListTrails with no trails present: expect HTTP 200 with Trails = []. Observe HTTP 501.
6. DeleteTrail with the created name: expect HTTP 200 and an empty body. Observe HTTP 501.
7. Replay the fixture steps in Issue-Trace/Issue-Step and Issue-Trace-Ids to confirm all six causes.
