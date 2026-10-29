Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: groundstation.config-s3-recording-nonexistent-bucket-rejected groundstation.dataflow-endpoint-group-lifecycle groundstation.list-contacts-invalid-time-window-rejected groundstation.delete-config-referenced-by-mission-profile-rejected
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation.config-s3-recording-nonexistent-bucket-rejected|run#28|AssertionError|assert refused == (
Issue-Key: groundstation.dataflow-endpoint-group-lifecycle|run#6|AssertionError|assert refused == (
Issue-Key: groundstation.list-contacts-invalid-time-window-rejected|run#6|AssertionError|assert refused == (
Issue-Key: groundstation.delete-config-referenced-by-mission-profile-rejected|run#28|AssertionError|assert refused == (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Four Ground Station refusals carry invented message text: nonexistent S3 bucket, missing securityDetails, inverted ListContacts window and re-delete of a Config

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfig

## Description
Four net-new causes, all in groundstation, all the same shape of defect: the correct error CODE is returned but the message is the emulator's own prose rather than the fixed text AWS emits for that error class. A message is part of the requirement, so each is a divergence; they are bundled because they share one service, and each must be closed separately.

Cause 1 of 4: CreateConfig for an s3RecordingConfig whose bucketArn names a bucket that does not exist.
AWS: Smithy: CreateConfig errors InvalidParameterException (HTTP 431); S3RecordingConfig.bucketArn and .roleArn are @required. Recorded behavior: the refusal message is `Unable to create new S3 endpoint config - bucket does not exist.` — entirely fixed text, with no value from the request interpolated. AWS reserves a DIFFERENT fixed message, `Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket.`, for the distinct condition where the bucket's location cannot be read (missing s3:GetBucketLocation).
Emulator: refuses with the right code, InvalidParameterException, but emits the GetBucketLocation-permissions message for a bucket that simply is not there, conflating the two conditions.
Why: the two messages name different remediations. A client (or an operator reading the log) is told to fix an IAM policy when the actual fault is a missing bucket; a create-retry loop that treats the permissions text as "IAM still propagating" will keep retrying a call that can never succeed, instead of failing fast on a typo'd bucket name.
Fix: the S3RecordingConfig bucket-reference validation in the CreateConfig handler (services/service-com-amazonaws-groundstation-groundstation/src/ops/create_config.rs and its message helper in src/state.rs) must select the bucket-does-not-exist message when no such bucket exists, and keep the GetBucketLocation text for the case where the bucket exists but its location is unreadable.
Traces: groundstation.config-s3-recording-nonexistent-bucket-rejected.

Cause 2 of 4: CreateDataflowEndpointGroup with an endpointDetails entry that omits securityDetails.
AWS: Smithy: EndpointDetails.securityDetails is NOT marked @required (SecurityDetails itself requires subnetIds, securityGroupIds, roleArn); the service nevertheless refuses, with InvalidParameterException (431) and the fixed message `EndpointDetails securityDetails cannot be null` — fixed text only, no index or member path interpolated.
Emulator: right code, but the message is `endpointDetails[0] is a dataflow endpoint and names no securityDetails; a dataflow endpoint requires the subnetIds, securityGroupIds and roleArn Ground Station connects its streams with` — prose AWS never emits, which additionally interpolates the list index.
Why: callers and SDK wrappers surface this message verbatim; assertions, log-greps and user-facing error mapping keyed on AWS's sentence do not match.
Fix: the securityDetails presence gate in the CreateDataflowEndpointGroup handler (src/ops/create_dataflow_endpoint_group.rs) must emit AWS's fixed message.
Traces: groundstation.dataflow-endpoint-group-lifecycle (the create/get/delete and the re-delete assertions in that check pass; only this message diverges).

Cause 3 of 4: ListContacts with startTime after endTime.
AWS: Smithy: ListContacts.startTime and .endTime are @required Timestamps; errors InvalidParameterException (431). Recorded behavior: the inverted window is refused with the fixed message `Invalid request: End date must be after start date` — no request value interpolated.
Emulator: right code, message `endTime must not precede startTime`.
Why: same as above — the message is the contract a client reports; it also differs in phrasing a human operator would search for.
Fix: the time-window validation in the ListContacts handler (src/ops/list_contacts.rs).
Traces: groundstation.list-contacts-invalid-time-window-rejected (the correctly-ordered window succeeds, so only the refusal text diverges).

Cause 4 of 4: a SECOND DeleteConfig on an id already deleted reuses GetConfig's not-found wording.
AWS: Smithy: DeleteConfig errors ResourceNotFoundException (434). Recorded behavior distinguishes the two operations' messages: GetConfig answers `No config found with id {configId} and type {configType}` while DeleteConfig answers `Config {configId} with type {configType} not found`. In both the bracketed parts are values taken from the request (the configId and the configType label); the rest is fixed text, and the two templates are different.
Emulator: DeleteConfig on a dead id answers ResourceNotFoundException with GetConfig's template, `No config found with id <configId> and type <configType>` — src/ops/delete_config.rs:159-166 builds it from the shared helper `crate::state::config_not_found_message`, the same helper the read path uses.
Why: a client that distinguishes a failed read from a failed mutation by message (or an operator correlating logs) sees the read's wording on a delete; the two operations become indistinguishable in logs and in any message-based assertion.
Fix: give DeleteConfig its own not-found message template in its handler (or a delete-specific helper alongside config_not_found_message in src/state.rs), keeping GetConfig's existing wording — which this same check verifies and which passes today.
Traces: groundstation.delete-config-referenced-by-mission-profile-rejected (the referential-integrity refusal, the ARN-intact reads and the GetConfig-after-delete message in that check all pass; only the re-delete message diverges).

## Evidence
Cause 1 — check groundstation.config-s3-recording-nonexistent-bucket-rejected:
  expected: ("InvalidParameterException", "Unable to create new S3 endpoint config - bucket does not exist.")
  actual:   ("InvalidParameterException", "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket.")

Cause 2 — check groundstation.dataflow-endpoint-group-lifecycle:
  expected: ("InvalidParameterException", "EndpointDetails securityDetails cannot be null")
  actual:   ("InvalidParameterException", "endpointDetails[0] is a dataflow endpoint and names no securityDetails; a dataflow endpoint requires the subnetIds, securityGroupIds and roleArn Ground Station connects its streams with")

Cause 3 — check groundstation.list-contacts-invalid-time-window-rejected:
  expected: ("InvalidParameterException", "Invalid request: End date must be after start date")
  actual:   ("InvalidParameterException", "endTime must not precede startTime")

Cause 4 — check groundstation.delete-config-referenced-by-mission-profile-rejected:
  expected: ("ResourceNotFoundException", "Config {configId} with type {configType} not found")
  actual:   ("ResourceNotFoundException", "No config found with id 17dd4143-24ef-493a-b332-8498e5c82e77 and type tracking")
The uuid and the type token in the actual message are the values the run supplied for configId/configType — they fill the template; the divergence is the template's fixed text ("No config found with id … and type …" instead of "Config … with type … not found"). Source: src/ops/delete_config.rs:159-166 calling crate::state::config_not_found_message, the helper GetConfig also uses.

In causes 1-3 the expected messages have no variable parts at all: every word is fixed text AWS emits for that error class.

## Steps to Reproduce
Cause 1: create an S3 bucket plus an IAM role trusted by groundstation.amazonaws.com with s3:GetBucketLocation/PutObject/ListBucket on it; CreateConfig with configData={"s3RecordingConfig": {"bucketArn": "arn:aws:s3:::<that-bucket>", "roleArn": <role>, "prefix": "sim2real/"}} succeeds; repeat with bucketArn naming a bucket that was never created and read Error.Message.

Cause 2: CreateDataflowEndpointGroup(endpointDetails=[{"endpoint": {"name": <name>, "address": {"name": "10.0.0.5", "port": 55888}}}]) with securityDetails omitted; read Error.Message.

Cause 3: ListContacts(statusList=["COMPLETED"], startTime=now, endTime=now-1h); read Error.Message. (The same call with startTime=now-1h, endTime=now succeeds.)

Cause 4: CreateConfig a trackingConfig; DeleteConfig it once (succeeds); DeleteConfig the same configId/configType again and read Error.Message. Compare with GetConfig on the same dead id, whose message is correct today.
