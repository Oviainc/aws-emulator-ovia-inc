Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix
Issue-Key: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix|_watch_absent#26|Timeout|return found
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# lambda__s3: an armed S3 ObjectCreated notification never invokes the function, so the literal case-sensitive suffix filter cannot be observed

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration (ObjectCreated -> Lambda, suffix FilterRule)

## Description
One net-new root cause in the lambda__s3 suite. It is NOT covered by the open lambda__s3 issues (20261103T235649Z-lambda-s3-putbucketnotificationconfiguration-refuses-a-filterrule-name-that.md is about FilterRule NAME validation, 20261108T193502Z / 20261109T210043Z / 20261103T031416Z are about S3-sourced layers and GetLayerVersion, 20261105T111951Z is about the Python runtime); this check carries none of their replay keys.

Cause 1 of 1: once an S3 bucket notification to a Lambda function is armed, an object uploaded under the filtered prefix never triggers the function, so neither the conversion effect nor the case-sensitivity of the suffix filter is observable.
AWS: recorded behavior only for the delivery; the filter semantics are documented. With a notification configuration whose LambdaFunctionConfiguration lists s3:ObjectCreated:* and a Filter of prefix "raw/" and suffix ".csv", a PutObject of raw/<name>.csv invokes the function with no further API call, and the function's effect (here, a converted object written back to the bucket) appears. S3's suffix FilterRule is a literal, case-SENSITIVE match, so a PutObject of raw/<name>.CSV is not delivered at all — even though the handler's own .lower().endswith('.csv') would have accepted it — which is exactly what distinguishes S3-side filtering from handler-side filtering.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. The positive control — waiting for the converted object that the armed notification should have produced — never arrives, so the suppression leg (which only opens after the positive control has arrived, so that absence is never confused with lateness) is never entered.
Why: a client's event-driven S3 pipeline does nothing. And because the positive control never fires, the negative is vacuous: a dispatcher that ignored the suffix filter entirely, and one that implemented it case-sensitively, are indistinguishable — the defect hides the very contract the check exists to pin.
Fix: the S3 notification dispatcher's Lambda destination — an ObjectCreated event matching the configuration's prefix/suffix FilterRules must invoke the configured function asynchronously, with the suffix compared literally and case-sensitively.
Traces: lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix.

## Evidence
Check lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "An object uploaded under raw/ once the notification is armed converts with no further API call, and a case-mismatched suffix (.CSV vs filter suffix=.csv) is suppressed by S3's literal, case-sensitive filter even though the handler's own .lower().endswith('.csv') would accept it." The check notes: "The absence window opens only AFTER the positive control has arrived, so the pipeline is known live and known to have drained an event" (_ABSENCE_WINDOW_S = 60.0, _PROPAGATION_ROUND_S = 15.0).

Expected: after PutObject of raw/<name>.csv, the converted object appears with no further API call; after PutObject of raw/<name>.CSV, nothing appears for the absence window. Actual: the positive control never arrives, so the check is killed with nothing compared. Bucket names, function names and key suffixes are per-run values.

## Steps to Reproduce
1. Create a bucket and a Lambda function (with an execution role) whose handler converts an uploaded CSV and writes the result back under a different prefix.
2. AddPermission for s3.amazonaws.com on the function, then PutBucketNotificationConfiguration with a LambdaFunctionConfiguration on ["s3:ObjectCreated:*"] and Filter prefix "raw/", suffix ".csv".
3. PutObject raw/<name>.csv and poll for the converted object: AWS produces it with no further API call; the emulator never does.
4. PutObject raw/<name>.CSV and confirm nothing is produced (S3's suffix filter is literal and case-sensitive).
5. Re-replay lambda__s3.armed-notification-converts-csv-and-suppresses-uppercase-suffix to confirm the fix.
