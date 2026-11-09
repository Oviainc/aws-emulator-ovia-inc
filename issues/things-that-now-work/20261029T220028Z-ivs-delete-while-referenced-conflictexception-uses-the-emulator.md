Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.delete-recording-configuration-referenced-by-channel-rejected ivs.delete-recording-configuration-referenced-conflict-rejected ivs.delete-ad-configuration-associated-with-channel-rejected
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs.delete-recording-configuration-referenced-by-channel-rejected|run#12|AssertionError|assert error["Message"] == (
Issue-Key: ivs.delete-recording-configuration-referenced-conflict-rejected|run#18|AssertionError|assert error["Message"] == (
Issue-Key: ivs.delete-ad-configuration-associated-with-channel-rejected|run#23|AssertionError|assert error["Message"] == (
Issue-Commit: 6e02dc63462b16cdda1be20178ebf3ce3788d85e

# IVS delete-while-referenced ConflictException uses the emulator's own wording, not AWS's "Unable to perform ivs:<Op> while … is still attached to a channel" (2 causes)

> _Note: this issue was identified against the code base at commit `6e02dc63462b16cdda1be20178ebf3ce3788d85e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecordingConfiguration

## Description
Two net-new root causes in the IVS emulator (crate services/service-com-amazonaws-ivs-amazoninteractivevideoservice), both on the 409 ConflictException a delete answers while a channel still references the resource. On both the HTTP status (409) and the error code (ConflictException) are CORRECT and the body does carry a message — only the message TEXT diverges from the sentence AWS words for this error class. Each cause is a separate operation and must be closed separately.

No already-filed IVS issue covers these: 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md (empty Error.Message, InsertAdBreak stub, IAM, S3-bucket and tagging resolution), 20261029T172005Z-ivs-tagging-and-ad-recording-configuration-admission-diverge.md (tag-write statuses, ad/recording-configuration ADMISSION, BatchGetChannel per-item not-found text), 20261029T172005Z-ivs-not-found-error-responses-omit-the-code.md and 20261029T172005Z-ivs-validation-quota-and-not-broadcasting-error-bodies.md (empty Code/Message envelopes on not-found / validation / quota / not-broadcasting) all describe other error classes and other operations; none names the delete-while-referenced ConflictException text. 20261022T071923Z-ivs-createplaybackrestrictionpolicy-stub.md is unrelated.

=== Cause 1 of 2: DeleteRecordingConfiguration's referenced-by refusal is worded as "Resource: <arn> is still referenced by: <channel arn>" ===
Failing checks: ivs.delete-recording-configuration-referenced-by-channel-rejected, ivs.delete-recording-configuration-referenced-conflict-rejected.
AWS: Smithy DeleteRecordingConfiguration (com.amazonaws.ivs, restJson1) — POST /DeleteRecordingConfiguration, input arn: RecordingConfigurationArn (required, @length 0..128, @pattern ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:recording-configuration/[a-zA-Z0-9-]+$), success @http code 204, modelled errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400. The operation's own documentation states the rule: "If you try to delete a recording configuration that is associated with a channel, you will get an error (409 ConflictException). To avoid this, for all channels that reference the recording configuration, first use UpdateChannel to set the recordingConfigurationArn field to an empty string, then use DeleteRecordingConfiguration." Recorded behavior for the TEXT: the refusal message is the service's attachment sentence — fixed text `Unable to perform ivs:DeleteRecordingConfiguration while `, then the recording configuration's ARN taken from the request's `arn`, then fixed text ` is still attached to a channel`. The fixed parts (including the `ivs:<OperationName>` action token) are the requirement; the ARN is the request's value and is the only variable part. Note what the sentence does NOT contain: AWS does not name the referencing channel.
Emulator: status 409 and code ConflictException are right, but the message is `Resource: <recording-configuration arn> is still referenced by: <channel arn>` — a different sentence entirely, and it additionally discloses the referencing channel's ARN, which the AWS message never carries. The text comes from the shared rule crate::state::resource_referenced_message, which formats "Resource: {arn} is still referenced by: {referenced_by}" and is handed to Scaffolded::stated by crate::state::resource_referenced.
Why: a client that surfaces or text-matches the IVS conflict message (the documented remediation path is to clear recordingConfigurationArn with UpdateChannel, which is exactly what the AWS sentence hints at) sees prose the service never emits, so message assertions, log triage and user-facing copy diverge from AWS. The extra channel ARN also makes the emulator leak a resource identifier the real response withholds, so a test that asserts the message mentions only the deleted resource passes against AWS and fails here.
Fix: the conflict message rule for this operation — the shared builder in crate::state (resource_referenced_message / resource_referenced) must produce the `Unable to perform ivs:<OperationName> while <arn> is still attached to a channel` form and must not include the referencing ARN; the call site is the DeleteRecordingConfiguration handler's reference check.
Traces: ivs.delete-recording-configuration-referenced-by-channel-rejected, ivs.delete-recording-configuration-referenced-conflict-rejected; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs:761-763 (resource_referenced_message) and :883-899 (resource_referenced), services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/delete_recording_configuration.rs:202-208.

=== Cause 2 of 2: DeleteAdConfiguration's referenced-by refusal has the same wrong sentence, and AWS's form for this operation also names the resource KIND ===
Failing check: ivs.delete-ad-configuration-associated-with-channel-rejected.
AWS: Smithy DeleteAdConfiguration — POST /DeleteAdConfiguration, input arn: AdConfigurationArn (required, @length 0..128, @pattern ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$), success @http code 204, modelled errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400. Recorded behavior for the TEXT: fixed text `Unable to perform ivs:DeleteAdConfiguration while ad configuration `, then the ad configuration's ARN from the request's `arn`, then fixed text ` is still attached to a channel`. So the template is the same attachment sentence as cause 1 but with the resource kind (`ad configuration `) spelled before the ARN — which is why one shared format string cannot serve both without the per-operation parts being parameterised. The same check also pins the surrounding not-found texts, which the emulator already matches (`Resource: <arn> not found` from GetAdConfiguration before and after the delete), confirming only the conflict sentence diverges.
Emulator: status 409 and code ConflictException are right; the message is `Resource: arn:aws:ivs:<region>:<account>:ad-configuration/<id> is still referenced by: arn:aws:ivs:<region>:<account>:channel/<id>` — the same shared "still referenced by" rule, so it is wrong in both the fixed wording and in disclosing the channel ARN.
Why: identical to cause 1, for the SSAI surface: a client detaching an ad configuration (delete the channel, or clear adConfigurationArn) cannot match on the message AWS returns, and the emulator again reveals the referencing channel that the real refusal withholds.
Fix: the DeleteAdConfiguration handler's reference-check message, via the same shared rule in crate::state — the operation name and the resource-kind prefix must both be supplied per operation so this operation emits `Unable to perform ivs:DeleteAdConfiguration while ad configuration <arn> is still attached to a channel`.
Traces: ivs.delete-ad-configuration-associated-with-channel-rejected; services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/delete_ad_configuration.rs:192-198, services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs:761-763.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): 53/76 passed (69.7%); ivs 53/76, 23 failed. 20 of those failures are already filed (one CreatePlaybackRestrictionPolicy scaffold stub blocking 7 checks, and 13 checks OWNED by 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md) and are excluded here. The 3 remaining failures are the two causes above, all three [mismatch] on the conflict MESSAGE only.

Cause 1 — DeleteRecordingConfiguration while a channel references it:
  Expected: HTTP 409, Code 'ConflictException', Message 'Unable to perform ivs:DeleteRecordingConfiguration while <recording-configuration arn> is still attached to a channel' (fixed text around the request's arn).
  Actual:   HTTP 409, Code 'ConflictException', Message 'Resource: arn:aws:ivs:us-west-2:000000000000:recording-configuration/15fdf783f2a8 is still referenced by: arn:aws:ivs:us-west-2:000000000000:channel/16979cc3d16b'
  (ivs.delete-recording-configuration-referenced-by-channel-rejected: AssertionError: unexpected DeleteRecordingConfiguration conflict message)
  Actual, second check: 'Resource: arn:aws:ivs:us-west-2:000000000000:recording-configuration/a3e64154d2a8 is still referenced by: arn:aws:ivs:us-west-2:000000000000:channel/b62de3a7b6b4'
  (ivs.delete-recording-configuration-referenced-conflict-rejected: same assertion)
  The ARNs in both are that run's generated ids — per-run values, not requirements. The requirement is the fixed text, the absence of any referencing-channel ARN, and the `ivs:DeleteRecordingConfiguration` action token.
  Everything else in both checks passed: the ConflictException code, the configuration surviving the rejected delete, the delete succeeding after the reference is cleared (UpdateChannel recordingConfigurationArn="" or DeleteChannel), and the subsequent 'Resource: <arn> not found' on Get and on the repeat delete.

Cause 2 — DeleteAdConfiguration while a channel references it via adConfigurationArn:
  Expected: HTTP 409, Code 'ConflictException', Message 'Unable to perform ivs:DeleteAdConfiguration while ad configuration <ad-configuration arn> is still attached to a channel'.
  Actual:   HTTP 409, Code 'ConflictException', Message 'Resource: arn:aws:ivs:us-west-2:000000000000:ad-configuration/a773da0d2ec3 is still referenced by: arn:aws:ivs:us-west-2:000000000000:channel/f99955f3810d'
  (ivs.delete-ad-configuration-associated-with-channel-rejected: AssertionError: unexpected DeleteAdConfiguration conflict message)
  The check's positive control passed (deleting an UNREFERENCED ad configuration succeeds and GetAdConfiguration then answers ResourceNotFoundException with 'Resource: <arn> not found'), so the divergence is confined to the conflict sentence.

Shared source evidence: both handlers call crate::state::resource_referenced(..., &input.arn, channel_arn), which builds its text from crate::state::resource_referenced_message -> format!("Resource: {arn} is still referenced by: {referenced_by}") (state.rs:761-763). One rule, two operations, two different AWS sentences.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it with region us-west-2.

Cause 1 (DeleteRecordingConfiguration):
1. s3.create_bucket(Bucket=<b>, CreateBucketConfiguration={'LocationConstraint':'us-west-2'}).
2. rc = ivs.create_recording_configuration(name=<n>, destinationConfiguration={'s3':{'bucketName':<b>}})['recordingConfiguration']; poll ivs.get_recording_configuration until state == 'ACTIVE'.
3. ivs.create_channel(name=<c>, recordingConfigurationArn=rc['arn']).
4. ivs.delete_recording_configuration(arn=rc['arn']) -> ClientError. Inspect response['Error']: Code is 'ConflictException' (correct) but Message is 'Resource: <rc arn> is still referenced by: <channel arn>' instead of 'Unable to perform ivs:DeleteRecordingConfiguration while <rc arn> is still attached to a channel'.
5. (Controls, which already pass) ivs.get_recording_configuration(arn=rc['arn']) still returns it; ivs.update_channel(arn=<channel arn>, recordingConfigurationArn='') or delete the channel, then the identical delete succeeds and Get/second-delete answer 'Resource: <rc arn> not found'.

Cause 2 (DeleteAdConfiguration):
1. Create a probe channel to read the account/region IVS playback-URL prefix (everything before '/api/' of channel['playbackUrl']), then mediatailor.put_playback_configuration(Name=<m>, AdDecisionServerUrl='https://example.com/ads?abc={aws.random}', VideoContentSourceUrl=<prefix>, InsertionMode='PLAYER_SELECT').
2. ad = ivs.create_ad_configuration(name=<a>, mediaTailorPlaybackConfigurations=[{'playbackConfigurationArn': <mt arn>}])['adConfiguration'].
3. ivs.create_channel(name=<c>, adConfigurationArn=ad['arn']).
4. ivs.delete_ad_configuration(arn=ad['arn']) -> ClientError with Code 'ConflictException' and Message 'Resource: <ad arn> is still referenced by: <channel arn>' instead of 'Unable to perform ivs:DeleteAdConfiguration while ad configuration <ad arn> is still attached to a channel'.
