Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-config-other-capability-types
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-config-other-capability-types
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|CreateConfig|4xx->4xx|$.message
Issue-Commit: 1f7e5adccf9e411825240a72188af4d50262df19

# CreateConfig s3RecordingConfig bucket refusal uses the wrong InvalidParameterException message

> _Note: this issue was identified against the code base at commit `1f7e5adccf9e411825240a72188af4d50262df19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfig

## Description
Cause 1 of 1: CreateConfig with a `configData.s3RecordingConfig` whose `bucketArn` cannot be resolved is refused with the right status and error code but the wrong message text.

AWS: `CreateConfig` models `InvalidParameterException` (Smithy: com.amazonaws.groundstation#InvalidParameterException, errorKind client, httpError 431, members `message` + `parameterName`); `S3RecordingConfig.bucketArn` (target `BucketArn`) is required and documented only as "ARN of the bucket to record to", so the model states the status and the code but not the wording. Recorded behavior: when Ground Station cannot resolve the recording bucket it answers HTTP 431 `InvalidParameterException` with the fixed text "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket." That text is entirely FIXED — it interpolates no bucket name, ARN, account or region — and it names the permission/lookup Ground Station actually performs on the bucket (a bucket-location lookup, which is also how the real service reports a bucket it cannot see at all). Hypothesis: the real service resolves the bucket with `s3:GetBucketLocation` (it must know the bucket's region before it can record to it), so an absent, foreign-account or otherwise unreadable bucket all surface as the same location-lookup refusal rather than as a distinct "does not exist" refusal.

Emulator: the same request is refused HTTP 431 `InvalidParameterException` (status and code match) with the message "Unable to create new S3 endpoint config - bucket does not exist." — a different fixed text. The emulator decides the question with S3 `HeadBucket` and maps both "not an S3 bucket ARN" and "HeadBucket reported NotFound" onto that one literal, so no request can ever produce AWS's location-lookup wording.

Why: clients and operators match on this message. Ground Station's own troubleshooting guidance for a failing S3 recording config is driven by the GetBucketLocation sentence ("ensure that Ground Station has s3:GetBucketLocation permissions on the bucket"), so a caller that greps or asserts on it — or a test/runbook written against the real service — sees no match and concludes the refusal is a different fault than it is. Because status (431), code (`InvalidParameterException`) and `parameterName` are identical, the message is the only signal that differs, and it points the user at a non-existent bucket instead of at the bucket-location permission/lookup that actually failed.

Fix: the recording-bucket resolution path of this service's shared state helper, used by the CreateConfig handler's validation step — the constant that holds the refusal text and the branch that selects it, so the refusal returned for an unresolvable recording bucket carries AWS's fixed location-lookup sentence (and so that both the "bucketArn names no bucket" short-circuit and the S3-reported-absent branch answer the wording AWS answers for this error class).

Traces: groundstation-config-other-capability-types#task#6 (CreateConfig, expected 431 / actual 431, `$.message` value_mismatch).

## Evidence
Replay divergence (trace set simparity-wire-ga-prod-v1), fixture `groundstation-config-other-capability-types`, step `task#6`, operation CreateConfig, verdict mismatch, divergence_key `service=groundstation|CreateConfig|status=431->431|keys=$.message`:
  expected status 431, actual status 431 (match)
  detail: `$.message: value_mismatch (expected 'Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket.', got 'Unable to create new S3 endpoint config - bucket does not exist.')`
  affected: groundstation-config-other-capability-types#task#6 (1)

Expected message, as AWS words it for this error class (all fixed text, no request- or state-derived parts): "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket."
Actual emulator message: "Unable to create new S3 endpoint config - bucket does not exist."

Source of the emulator's text: `services/service-com-amazonaws-groundstation-groundstation/src/state.rs` lines ~449-457 define `RECORDING_BUCKET_DOES_NOT_EXIST = "Unable to create new S3 endpoint config - bucket does not exist."`, and `unresolvable_recording_bucket` (same file, lines ~478-517) returns it both when `bucket_name_of` rejects the ARN and when S3 `HeadBucket` reports `NotFound`. `services/service-com-amazonaws-groundstation-groundstation/src/ops/create_config.rs` lines ~176-184 call that helper and lift its result into `InvalidParameterException`, so the status (431), code and `parameterName` (`configData.s3RecordingConfig.bucketArn`) are already right and only the wording diverges.

Model evidence: `smithy_query(query="shape", shape="S3RecordingConfig,InvalidParameterException", model_path="services/service-com-amazonaws-groundstation-groundstation/smithy-model/com-amazonaws-groundstation-groundstation.json")` — `bucketArn` required, `InvalidParameterException` httpError 431; the model fixes status and code but not message text, so the recording is the authority for the wording. No open groundstation issue covers CreateConfig's S3 recording refusal (the open ones are operation stubs, ListContacts behavior, CreateDataflowEndpointGroup and CreateMissionProfile validation).

## Steps to Reproduce
1. Boot the emulator with the groundstation service mounted and S3 available on the same endpoint.
2. Do NOT create the bucket the config will name (or name a bucketArn whose bucket this account does not hold).
3. POST /config with body {"name":"<safe-name>","configData":{"s3RecordingConfig":{"bucketArn":"arn:aws:s3:::<bucket-this-account-does-not-hold>","roleArn":"arn:aws:iam::<account>:role/<role>"}}}.
4. Observe HTTP 431 InvalidParameterException with message "Unable to create new S3 endpoint config - bucket does not exist." where AWS answers the same 431 InvalidParameterException with "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket."
5. Equivalent replay: replay fixture `groundstation-config-other-capability-types`, step `task#6`.
