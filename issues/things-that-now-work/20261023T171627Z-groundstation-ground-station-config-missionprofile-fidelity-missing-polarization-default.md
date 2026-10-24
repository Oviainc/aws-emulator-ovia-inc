Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: groundstation.antenna-downlink-config-default-polarization groundstation.config-s3-recording-nonexistent-bucket-rejected groundstation.delete-config-referenced-by-mission-profile-rejected groundstation.uplink-echo-config-rejects-wrong-type-arn groundstation.delete-config-then-get-not-found groundstation.mission-profile-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation.antenna-downlink-config-default-polarization|run#9|KeyError|assert spectrum["polarization"] == "NONE", (
Issue-Key: groundstation.config-s3-recording-nonexistent-bucket-rejected|run#22|AssertionError|assert good is not None, f"CreateConfig kept rejecting a real, permissioned bucket: {last_err}"
Issue-Key: groundstation.delete-config-referenced-by-mission-profile-rejected|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: groundstation.uplink-echo-config-rejects-wrong-type-arn|run#7|AssertionError|assert refused == (
Issue-Key: groundstation.delete-config-then-get-not-found|run#18|AssertionError|assert update_message == expected_update, (
Issue-Key: groundstation.mission-profile-lifecycle|run#29|AssertionError|assert mutate_refused == (
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Ground Station Config/MissionProfile fidelity: missing polarization default, broken S3 bucket check, no DeleteConfig referential integrity, wrong UplinkEcho ARN-type error code, wrong mutation not-found messages

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfig

## Description
Five distinct root causes in the Ground Station emulator, all reached through CreateConfig / GetConfig / DeleteConfig / UpdateConfig / UpdateMissionProfile. Each must be closed separately.

=== Cause 1 of 5: SpectrumConfig.polarization server default NONE is never applied ===
Failing check: groundstation.antenna-downlink-config-default-polarization
AWS: Smithy model com-amazonaws-groundstation-groundstation, shape SpectrumConfig, member `polarization` -> target Polarization, `default: "NONE"` with addedDefault+clientOptional (enum values RIGHT_HAND, LEFT_HAND, NONE). A modeled default on a non-required member means the server materializes it: when CreateConfig omits polarization inside antennaDownlinkConfig.spectrumConfig, the stored Config carries polarization = NONE and GetConfig returns that member populated. An explicitly supplied polarization (e.g. LEFT_HAND) must persist unchanged.
Emulator: GetConfig on a Config created with polarization omitted returns a spectrumConfig object with NO `polarization` key at all; the check fails with KeyError: 'polarization' when reading configData.antennaDownlinkConfig.spectrumConfig["polarization"].
Why: a client that reads spectrumConfig["polarization"] — which the model guarantees is always present because it has a default — crashes with a key/missing-field error instead of seeing NONE. Round-tripping a Config read from the emulator back into CreateConfig/UpdateConfig also silently loses the field.
Fix: CreateConfig's request deserialization/validation must apply the modeled member default for SpectrumConfig.polarization before persisting (and the serializer must emit it), in the CreateConfig handler / the Config state model for the groundstation service crate.

=== Cause 2 of 5: CreateConfig rejects a real, existing, permissioned S3 bucket for s3RecordingConfig ===
Failing check: groundstation.config-s3-recording-nonexistent-bucket-rejected
AWS: Smithy: CreateConfig input configData -> ConfigTypeData union member s3RecordingConfig -> S3RecordingConfig {bucketArn required, roleArn required (pattern ^arn:[a-z0-9-.]{1,63}:iam::[0-9]{12}:role/[\w+=,.@-]{1,64}$), prefix}; modeled errors include InvalidParameterException (client, httpError 431). Recorded behavior: CreateConfig verifies the referenced bucket EXISTS and is reachable — a Config whose bucketArn names a bucket the account actually owns is ACCEPTED (HTTP 200, configType "s3-recording"), and only a bucketArn naming a nonexistent bucket is refused with InvalidParameterException (recorded message example: "Unable to create new S3 endpoint config - bucket does not exist.").
Emulator: CreateConfig refuses the positive case too. For a bucket just created through the emulator's own S3 (with bucket tagging set and an IAM role granting s3:GetBucketLocation/PutObject/ListBucket), every one of 10 attempts over ~30s returned InvalidParameterException with the message "Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket." — i.e. the bucket-location/existence probe never succeeds, so no S3RecordingConfig can ever be created.
Why: S3 recording is unusable in the emulator. A client cannot create any s3-recording Config, so the whole downlink-to-S3 workflow (Config -> MissionProfile -> contact) cannot be built, and the failure is reported as a caller parameter error (431) rather than as the emulator's own inability to resolve the bucket.
Fix: the bucket-existence/location resolution behind CreateConfig's s3RecordingConfig validation (the CreateConfig handler's S3 reference check) must consult the emulator's own S3 bucket state and accept an existing bucket, reserving InvalidParameterException for a bucketArn that truly names no bucket.

=== Cause 3 of 5: DeleteConfig does not enforce referential integrity against MissionProfile references ===
Failing check: groundstation.delete-config-referenced-by-mission-profile-rejected
AWS: Smithy: DeleteConfig DELETE /config/{configType}/{configId}, modeled errors InvalidParameterException (client, httpError 431) and ResourceNotFoundException (client, httpError 434). Recorded behavior: while a MissionProfile still references the Config (here as its trackingConfigArn), DeleteConfig is REFUSED with InvalidParameterException naming the referencing mission-profile ARN(s) (recorded message example: "Config cannot be deleted; it is currently being used by the following Mission Profiles: arn:aws:groundstation:<region>:<account>:mission-profile/<id>"). The Config and the MissionProfile both remain intact, and DeleteConfig only succeeds once the referencing MissionProfile has been deleted.
Emulator: DeleteConfig on the still-referenced tracking Config returns HTTP 200 with the full ConfigIdResponse (configId/configType/configArn) and deletes it — the reference held by the live MissionProfile is ignored entirely.
Why: the emulator lets a client destroy a Config that a live MissionProfile depends on, leaving the MissionProfile pointing at a dangling trackingConfigArn. Code that relies on the refusal (deleting a mission profile first, or surfacing the "still in use" error to a user) takes the wrong branch, and teardown ordering bugs that real AWS catches pass silently here.
Fix: DeleteConfig's handler must consult MissionProfile state for inbound references (trackingConfigArn, telemetrySinkConfigArn, dataflowEdges) before removing the Config and return InvalidParameterException (431) listing the referencing mission-profile ARNs when any exist.

=== Cause 4 of 5: UplinkEchoConfig antennaUplinkConfigArn wrong-type reference gets the wrong error code ===
Failing check: groundstation.uplink-echo-config-rejects-wrong-type-arn
AWS: Smithy: CreateConfig configData -> ConfigTypeData member uplinkEchoConfig -> UplinkEchoConfig {enabled, antennaUplinkConfigArn}; modeled errors InvalidParameterException (client, httpError 431) and ResourceNotFoundException (client, httpError 434). Recorded behavior: when antennaUplinkConfigArn points at a Config that EXISTS but is of the wrong type (an antenna-downlink Config), the reference is type-checked and the call is refused with error code InvalidParameterException (HTTP 431); recorded message example: 'Invalid ARN provided. Config Type must be of type "antenna-uplink".'
Emulator: returns error code ResourceNotFoundException (HTTP 434) instead, with the message "Unable to find AntennaUplink Config with ARN: arn:aws:groundstation:<region>:<account>:config/antenna-downlink/<id>" — i.e. it treats a real-but-wrong-type Config as a missing resource rather than as an invalid parameter.
Why: the HTTP status class and error code both diverge (431 InvalidParameterException vs 434 ResourceNotFoundException). Clients branch on the code: retry/wait-for-eventual-consistency logic keyed on ResourceNotFoundException will spin on a request that can never succeed, while validation handling keyed on InvalidParameterException never fires.
Fix: the reference resolution in CreateConfig's uplinkEchoConfig validation must distinguish "no such Config" from "Config found but configType != antenna-uplink" and classify the latter as InvalidParameterException.

=== Cause 5 of 5: mutation paths reuse the read-path not-found message instead of AWS's account-qualified one ===
Failing checks: groundstation.delete-config-then-get-not-found, groundstation.mission-profile-lifecycle
AWS: Smithy: UpdateConfig (PUT /config/{configType}/{configId}) and UpdateMissionProfile (PUT /missionprofile/{missionProfileId}) both model ResourceNotFoundException (client, httpError 434). Recorded behavior: the read and the mutation paths produce DIFFERENT not-found messages, each composed from the request members plus the caller's account id. GetConfig on a deleted id -> "No config found with id {configId} and type {configType}"; UpdateConfig on the same dead id -> "No config found with ID {configId} and type {configType} for customer {accountId}". GetMissionProfile on a deleted id -> "MissionProfile not found"; UpdateMissionProfile on the same dead id -> "No mission profile found with ID {missionProfileId} for customer {accountId}". The requirement is that composition rule, not any one recording's literal id values.
Emulator: the error code is correct (ResourceNotFoundException) on both mutation paths, but the message is the read-path one. UpdateConfig on a deleted config returned "No config found with id <id> and type tracking" (the GetConfig wording, no "for customer <account>" suffix, lowercase "id"); UpdateMissionProfile on a deleted profile returned "MissionProfile not found" (the GetMissionProfile wording, no id and no account). So Update* shares the Get* not-found error construction instead of building its own.
Why: the mutation-path message is request-derived and account-qualified in AWS; a client (or an operator reading logs) that uses it to identify WHICH id and WHICH account a failed write referred to gets a message with that information stripped. Error code and status are correct, so impact is limited to diagnosability.
Fix: give UpdateConfig and UpdateMissionProfile their own ResourceNotFoundException construction in their handlers (rather than delegating to the Get path's lookup error), composing the message from the request's configId/configType or missionProfileId plus the caller account id.

## Evidence
Suite: simparity-behavioral-aws-v1, 6/20 passed (30.0%); groundstation 6/20, 14 failed (7 of those are ALREADY FILED scaffold-stub blockers for ListConfigs, CreateDataflowEndpointGroup, GetMinuteUsage, ListTagsForResource, TagResource and are excluded here).

Cause 1 — groundstation.antenna-downlink-config-default-polarization [error]:
  expected: GetConfig -> configData.antennaDownlinkConfig.spectrumConfig["polarization"] == "NONE" when polarization omitted at CreateConfig
  actual:   KeyError: 'polarization' (member absent from the response)
  (Smithy SpectrumConfig.polarization default = "NONE", addedDefault, clientOptional)

Cause 2 — groundstation.config-s3-recording-nonexistent-bucket-rejected [mismatch]:
  expected: CreateConfig(s3RecordingConfig -> existing permissioned bucket) -> 200, configType "s3-recording"; only a nonexistent bucket -> InvalidParameterException
  actual:   AssertionError: CreateConfig kept rejecting a real, permissioned bucket: An error occurred (InvalidParameterException) when calling the CreateConfig operation: Unable to create new S3 endpoint config - unable to get location of bucket. Ensure that Ground Station has s3:GetBucketLocation permissions on the bucket. (10 attempts over ~30s, all refused)

Cause 3 — groundstation.delete-config-referenced-by-mission-profile-rejected [mismatch]:
  expected: DeleteConfig on a Config still referenced as a MissionProfile's trackingConfigArn -> InvalidParameterException (431), message example "Config cannot be deleted; it is currently being used by the following Mission Profiles: <mission-profile arn>"
  actual:   AssertionError: expected the call to be refused, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'configId': '7d308db6-...', 'configType': 'tracking', 'configArn': 'arn:aws:groundstation:us-west-2:000000000000:config/tracking/7d308db6-...'}

Cause 4 — groundstation.uplink-echo-config-rejects-wrong-type-arn [mismatch]:
  expected: ('InvalidParameterException', 'Invalid ARN provided. Config Type must be of type "antenna-uplink".')  [HTTP 431]
  actual:   ('ResourceNotFoundException', 'Unable to find AntennaUplink Config with ARN: arn:aws:groundstation:us-west-2:000000000000:config/antenna-downlink/d7161e49-...')  [HTTP 434]

Cause 5 — groundstation.delete-config-then-get-not-found [mismatch]:
  GetConfig after delete: code+message MATCH (ResourceNotFoundException, "No config found with id <id> and type tracking")
  UpdateConfig on the same dead id: code MATCHES (ResourceNotFoundException) but
  expected message: 'No config found with ID <id> and type tracking for customer <accountId>'
  actual message:   'No config found with id <id> and type tracking'
Cause 5 — groundstation.mission-profile-lifecycle [mismatch]:
  GetMissionProfile after delete: ('ResourceNotFoundException', 'MissionProfile not found') MATCHES
  UpdateMissionProfile on the same dead id: code MATCHES but
  expected message: 'No mission profile found with ID <profileId> for customer <accountId>'
  actual:           ('ResourceNotFoundException', 'MissionProfile not found')

## Steps to Reproduce
Boot the emulator and point a botocore groundstation client at it.

Cause 1: CreateConfig(name=<unique>, configData={"antennaDownlinkConfig":{"spectrumConfig":{"centerFrequency":{"value":7850.0,"units":"MHz"},"bandwidth":{"value":1.0,"units":"MHz"}}}}) with polarization OMITTED, then GetConfig(configId, configType) and read configData.antennaDownlinkConfig.spectrumConfig["polarization"] -> KeyError instead of "NONE". Repeat with polarization="LEFT_HAND" to confirm an explicit value does persist.

Cause 2: CreateBucket a bucket in the emulator's S3 (LocationConstraint = client region outside us-east-1), CreateRole + PutRolePolicy granting s3:GetBucketLocation/s3:PutObject/s3:ListBucket on it, then CreateConfig(configData={"s3RecordingConfig":{"bucketArn":"arn:aws:s3:::<bucket>","roleArn":<role>,"prefix":"sim2real/"}}) -> InvalidParameterException "unable to get location of bucket" on every attempt, where AWS returns 200.

Cause 3: CreateConfig trackingConfig (autotrack REQUIRED), CreateConfig antennaDownlinkConfig, CreateConfig dataflowEndpointConfig; CreateMissionProfile(minimumViableContactDurationSeconds=30, dataflowEdges=[[downlinkArn, endpointArn]], trackingConfigArn=<tracking config arn>); then DeleteConfig(configId=<tracking config>, configType="tracking") -> 200 instead of InvalidParameterException.

Cause 4: CreateConfig an antennaDownlinkConfig and keep its configArn; CreateConfig(configData={"uplinkEchoConfig":{"enabled":True,"antennaUplinkConfigArn":<that antenna-downlink configArn>}}) -> ResourceNotFoundException/434 instead of InvalidParameterException/431.

Cause 5: CreateConfig trackingConfig; DeleteConfig it; GetConfig (message matches); then UpdateConfig(configId, configType, name, configData) on the dead id and compare the message to 'No config found with ID <id> and type <type> for customer <account>'. Likewise CreateMissionProfile / DeleteMissionProfile / UpdateMissionProfile(missionProfileId=<dead id>, name=...) and compare to 'No mission profile found with ID <id> for customer <account>'.
