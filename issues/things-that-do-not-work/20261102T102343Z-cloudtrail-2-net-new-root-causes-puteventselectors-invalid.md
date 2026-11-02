Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set cloudtrail.trail-delivers-a-gzipped-log-file-to-the-documented-s3-key-layout
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set|run#22|AssertionError|assert message == expected_messages[label], (
Issue-Key: cloudtrail.trail-delivers-a-gzipped-log-file-to-the-documented-s3-key-layout|run#28|Timeout|time.sleep(_DELIVERY_POLL_S)
Issue-Commit: 699db40da250628f4cc12f3dc175d789bc27ad1e

# cloudtrail: 2 net-new root causes — PutEventSelectors' invalid-DataResources.Type refusal message is re-worded, and a logging trail still delivers no log file to its S3 bucket

> _Note: this issue was identified against the code base at commit `699db40da250628f4cc12f3dc175d789bc27ad1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventSelectors

## Description
TWO distinct causes in this issue; both must be closed before submitting.

Cause 1 of 2 — PutEventSelectors rejects an unknown basic DataResources.Type with its own invented wording instead of AWS's fixed per-value message.
AWS: Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json) binds InvalidEventSelectorsException (errorKind client, httpError 400, awsQueryError code InvalidEventSelectors) to PutEventSelectors, and the DataResource shape documents exactly three basic types (AWS::DynamoDB::Table, AWS::Lambda::Function, AWS::S3::Object; anything else 'must use AdvancedEventSelectors'). The refusal AWS returns for a value outside that set is recorded behavior: HTTP 400, code InvalidEventSelectorsException, message `Value <Type> for DataResources.Type is invalid.` — fixed text 'Value ' + ' for DataResources.Type is invalid.' with the single variable part being the DataResource.Type value copied verbatim from the request. (The sibling count refusal on the same path, 'Specify a valid number of selectors (1 to 5) for your trail', is already correct in this emulator, so only the type message diverges.)
Emulator: the error code and 400 status are right, but the message is replaced by a self-authored sentence that enumerates the allowed types and appends advice: `DataResource Type must be one of AWS::DynamoDB::Table, AWS::Lambda::Function, AWS::S3::Object, not "AWS::Nope::Nope"; every other resource type needs AdvancedEventSelectors`. It also quotes the offending value with Rust's Debug escaping (`{resource_type:?}` → surrounding double quotes) where AWS renders it bare. Because the check asserts the message exactly, the run stops at that assertion and the remaining rejection paths on the same call (unknown ReadWriteType) are never reached.
Why: the error message is part of the contract. Clients, CDK/Terraform providers and operators match on CloudTrail's fixed refusal text (and on the 'Value X for <Member> is invalid.' family shared across CloudTrail validation) to tell an invalid-type rejection from a quota rejection; a re-worded message makes that classification fail and makes emulator-recorded failures non-comparable with AWS ones.
Fix: the refusal-text construction in the PutEventSelectors handler's basic-selector validation — the branch over crate::state::BASIC_DATA_RESOURCE_TYPES must emit AWS's template with the request's value substituted, unescaped, instead of an enumeration plus advice.
Traces: cloudtrail.rejected-put-event-selectors-preserves-a-non-default-selector-set (the diverging message is produced at services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_selectors.rs:446-451, inside normalize_event_selectors).

Cause 2 of 2 — a trail that reports IsLogging=true never writes a log file into its S3 bucket, so the documented key layout and gzip framing can never be observed.
AWS: recorded behavior only (measured by the check against real AWS): after StartLogging on a trail whose bucket carries the CloudTrail delivery policy, CloudTrail writes gzipped objects under AWSLogs/<account-id>/CloudTrail/<region>/YYYY/MM/DD/, each object name carrying the YYYYMMDDTHHmmZ stamp the check's regex expects, and each object being a gzip stream whose JSON 'Records' array holds the account's recorded events; the same events are also readable through LookupEvents.
Emulator: no object ever appears under the trail's bucket. The check drives CreateTrail/StartLogging, re-issues a marker call every 60s and polls the bucket until the harness's 60s cap ends it ('Check timed out after 60.0s'); the check carries the suite's DATAPLANE_UNIMPLEMENTED marker for exactly this absence.
Why: the S3 prefix, not LookupEvents, is what real audit pipelines consume (Athena/Glue over the log prefix, digest validation, SIEM ingestion). A client reading its trail's bucket sees a permanently empty prefix and cannot distinguish 'nothing happened' from 'delivery is not implemented'; log-file validation cannot be exercised at all.
Fix: trail log-file delivery — the trail's logging state must materialize the account's recorded events as gzipped S3 objects under the documented key layout (and digest objects when LogFileValidationEnabled is set) in the configured bucket/prefix.
Traces: cloudtrail.trail-delivers-a-gzipped-log-file-to-the-documented-s3-key-layout. NOTE: the open issue 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md describes this same delivery gap as its Cause 2 and even lists this trace id, but the replay report did NOT mark this check as owned by it, so the check is still failing and unclaimed and is re-filed here; if that issue is being worked, close this cause with it and re-replay the trace below.

## Evidence
Cause 1: verdict 'mismatch' — AssertionError: expected the unknown DataResources type rejection message, got 'DataResource Type must be one of AWS::DynamoDB::Table, AWS::Lambda::Function, AWS::S3::Object, not "AWS::Nope::Nope"; every other resource type needs AdvancedEventSelectors'. The check asserted code == 'InvalidEventSelectorsException' first and that passed, so only the message text diverges; expected `Value AWS::Nope::Nope for DataResources.Type is invalid.` for the payload [{'ReadWriteType':'All','IncludeManagementEvents':True,'DataResources':[{'Type':'AWS::Nope::Nope','Values':['arn:aws:s3:::<bucket>/']}]}] (AWS::Nope::Nope is the request's value filling the template's one slot, not part of the fixed text). Corroborating code: services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_selectors.rs:446.

Cause 2: verdict 'mismatch', detail 'Check timed out after 60.0s'. The check's own budget is _DELIVERY_BUDGET_S = 900.0 with _DELIVERY_POLL_S = 2.0 and a fresh marker call every _MARKER_REPEAT_S = 60.0, and it declares DATAPLANE_MARKER = 'DATAPLANE_UNIMPLEMENTED' for the case where no log file is delivered — i.e. it was still polling an empty bucket when the cap ended it, after GetTrailStatus had reported the trail logging.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: create an S3 bucket with the CloudTrail delivery policy, CreateTrail(Name=t, S3BucketName=b), then PutEventSelectors(TrailName=t, EventSelectors=[{ReadWriteType:'All', IncludeManagementEvents:true, DataResources:[{Type:'AWS::Nope::Nope', Values:['arn:aws:s3:::b/']}]}]). Expect HTTP 400 InvalidEventSelectorsException with message 'Value AWS::Nope::Nope for DataResources.Type is invalid.'; observe the enumerate-and-advise message above. Repeat with ReadWriteType:'Sideways' and expect 'Value Sideways for ReadWriteType is invalid.'
3. Cause 2: with that trail, StartLogging(Name=t), confirm GetTrailStatus reports IsLogging=true, make a few CloudTrail calls, then ListObjectsV2 on the bucket. Expect gzipped objects under AWSLogs/<account>/CloudTrail/<region>/YYYY/MM/DD/ whose names carry the _YYYYMMDDTHHmmZ_ stamp and whose gunzipped JSON holds Records; observe an empty bucket indefinitely.
