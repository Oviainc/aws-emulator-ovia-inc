Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3
Issue-Key: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3|_invoke#37|KeyError|return resp["ResponseMetadata"]["RequestId"]
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudtrail__lambda__s3: S3 data events are not recorded for a trail with an S3 data-event selector

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1 — a trail configured with an S3 object-level advanced event selector records no data events for the object operations the scenario performs.
AWS: PutEventSelectors / PutAdvancedEventSelectors with a resource type of AWS::S3::Object on a bucket makes the trail capture GetObject/PutObject/DeleteObject as data events; those events are delivered to the trail's destination and are visible with the eventName, the S3 resource ARN and the invoking identity.
Emulator: recorded behavior only — the check fails in its body where it asserts the data events were captured, so the selector records nothing. Open issues 20261029T173328Z-cloudtrail-records-and-delivers-no-audit-record-for.md and 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md describe management-event gaps for other checks and do not carry this replay key.
Why: an auditor cannot see object-level access at all, so an account that depends on data-event trails for compliance has no record of who read or wrote an object.
Fix: the CloudTrail data-event capture path — the S3 data-plane hook that emits object-level events for a trail whose selectors match.
Traces: cloudtrail__lambda__s3.obs-lambda-3-data-events-s3.

## Evidence
Error: File "check_cloudtrail__lambda__s3_obs_lambda_3_data_events_s3.py", line 310, in run — the assertion that the trail captured the S3 object-level events does not hold. Expected: the trail's events include the object operations with eventName GetObject/PutObject and the object's ARN as a resource.

## Steps to Reproduce
CreateTrail with an S3 destination; PutEventSelectors with DataResources of type AWS::S3::Object for the bucket; StartLogging; PutObject/GetObject on that bucket from a Lambda; then look the data events up on the trail.
