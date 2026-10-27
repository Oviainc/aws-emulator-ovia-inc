Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched
Issue-Key: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Commit: d9d92e1bb4d26644359d4701433c452d79a4d930

# config: the remediation-disarmed check hangs to the 60 s cap — a running configuration recorder produces no configuration item for a newly created S3 bucket (1 cause)

> _Note: this issue was identified against the code base at commit `d9d92e1bb4d26644359d4701433c452d79a4d930`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
Cause 1 of 1: a started AWS Config configuration recorder reports recording=true but never produces a configuration item for a resource created in another service, so every read that polls for one never converges and the whole remediation flow is unreachable.

AWS: recorded behavior only for the end-to-end timing (the Smithy model describes the shapes, not the recording pipeline). A configuration recorder created with PutConfigurationRecorder (recordingGroup.allSupported=true) and started with StartConfigurationRecorder discovers supported resources in the account/region and emits a configuration item for each; GetResourceConfigHistory(resourceType="AWS::S3::Bucket", resourceId=<bucket>) then returns at least one configurationItems entry for a bucket created after the recorder started, and ListDiscoveredResources lists that bucket. The replayed scenario depends on that: it creates a bucket, waits for its configuration item, then puts the S3_BUCKET_VERSIONING_ENABLED managed rule, reads the NON_COMPLIANT evaluation, and only then exercises the thing under test — that DeleteRemediationConfigurations really disarms remediation, so StartRemediationExecution answers exactly NoSuchRemediationConfigurationException and the bucket's versioning is provably untouched, while re-putting the identical remediation configuration makes the identical StartRemediationExecution reach SUCCEEDED and turn versioning on.

Emulator: the check never reaches any of that. It hangs and is killed at the harness cap — `Error: Check timed out after 60.0s`. The only unbounded wait reachable before the 60 s cap is the configuration-item poll for the bucket it just created (budgeted 500 s in the check), every earlier step being a bounded sleep or a single call; Hypothesis: the stall is that poll, i.e. GetResourceConfigHistory stays empty for the bucket forever. Two open issues record the same emulator behavior from other checks and corroborate it: 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md ("the configuration recorder never records resources from other services, so no configuration item is ever produced") and 20261025T202833Z-config-the-auto-remediation-end-to-end-flow.md ("a running recorder never discovers an S3 bucket, so ListDiscoveredResources stays empty"). Neither carries this check's replay key, so this check is not covered by them; if the fix for those lands, re-replaying this check is what confirms it reaches the remediation legs.

Why: a client cannot use Config for anything downstream of discovery. Rule evaluation, compliance reads and remediation all key off configuration items, so with none produced every such client waits forever instead of getting either a verdict or an error — and in particular the negative contract this check grades (a deleted remediation configuration really disarms remediation, with StartRemediationExecution raising NoSuchRemediationConfigurationException and the target resource left untouched) can never be observed.

Fix: the configuration-recorder state/discovery component — the recorder must enumerate and subscribe to supported resource types in its recordingGroup and write configuration items into the store that GetResourceConfigHistory, ListDiscoveredResources and the rule evaluator read, rather than reporting recording=true over an empty store.

Traces: config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched

## Evidence
Replay output for config__s3__ssm.config-remediation-execution-without-configuration-rejected-and-resource-untouched: `Error: Check timed out after 60.0s` — no exception and no AWS error anywhere, which is the signature of a poll that never converges rather than of a refused call.
The check's own sequence (from the replayed source): `_start_recording(...)` -> `_create_bucket(...)` -> `_await_history_items(ctx, "AWS::S3::Bucket", bucket, timeout_s=500, description=f"a configuration item for bucket {bucket!r}")` -> `_put_managed_rule("S3_BUCKET_VERSIONING_ENABLED", ...)` -> `_poll_until(... an evaluation result for bucket ..., timeout_s=400)` -> the StartRemediationExecution legs. The 500 s and 400 s polls both exceed the 60 s cap, so an empty configuration-item store terminates the run before the graded assertions.
Expected: GetResourceConfigHistory(ResourceType='AWS::S3::Bucket', ResourceId=<the bucket just created>) returns a non-empty configurationItems list within the recorder's discovery delay, after which the check's graded legs are reachable: StartRemediationExecution with no remediation configuration refused with code `NoSuchRemediationConfigurationException` (the bucket's versioning still absent afterwards), and the same call reaching SUCCEEDED after the identical configuration is re-put. The bucket name is a per-run value; the empty history is the defect.

## Steps to Reproduce
1. PutConfigurationRecorder with recordingGroup {allSupported: true, includeGlobalResourceTypes: false} and a roleARN trusting config.amazonaws.com, then StartConfigurationRecorder; confirm DescribeConfigurationRecorderStatus reports recording=true.
2. CreateBucket a new S3 bucket in the same region.
3. Poll GetResourceConfigHistory(ResourceType='AWS::S3::Bucket', ResourceId=<that bucket>) (and ListDiscoveredResources(resourceType='AWS::S3::Bucket')).
AWS returns a configuration item for the bucket; the emulator returns an empty list indefinitely, so any client polling for it — including this check, which then never reaches its StartRemediationExecution legs — hangs until it is killed (`Check timed out after 60.0s`).
