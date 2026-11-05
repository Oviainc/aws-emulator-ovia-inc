Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-error-paths ivs-channel-lifecycle ivs-ad-configuration-error-paths ivs-playback-key-pair-error-paths ivs-recording-configuration-error-paths ivs-stream-key-management
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|GetChannel|4xx->4xx|$.Code
Issue-Key: ivs|GetChannel|4xx->4xx|$.Message
Issue-Key: ivs|GetAdConfiguration|4xx->4xx|$.Code
Issue-Key: ivs|GetAdConfiguration|4xx->4xx|$.Message
Issue-Key: ivs|GetAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|DeleteAdConfiguration|4xx->4xx|$.Code
Issue-Key: ivs|DeleteAdConfiguration|4xx->4xx|$.Message
Issue-Key: ivs|DeleteAdConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|UpdateChannel|4xx->4xx|$.Code
Issue-Key: ivs|UpdateChannel|4xx->4xx|$.Message
Issue-Key: ivs|DeleteChannel|4xx->4xx|$.Code
Issue-Key: ivs|DeleteChannel|4xx->4xx|$.Message
Issue-Key: ivs|GetPlaybackKeyPair|4xx->4xx|$.Code
Issue-Key: ivs|GetPlaybackKeyPair|4xx->4xx|$.Message
Issue-Key: ivs|DeletePlaybackKeyPair|4xx->4xx|$.Code
Issue-Key: ivs|DeletePlaybackKeyPair|4xx->4xx|$.Message
Issue-Key: ivs|GetRecordingConfiguration|4xx->4xx|$.Code
Issue-Key: ivs|GetRecordingConfiguration|4xx->4xx|$.Message
Issue-Key: ivs|GetRecordingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|DeleteRecordingConfiguration|4xx->4xx|$.Code
Issue-Key: ivs|DeleteRecordingConfiguration|4xx->4xx|$.Message
Issue-Key: ivs|DeleteRecordingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivs|GetStreamKey|4xx->4xx|$.Code
Issue-Key: ivs|GetStreamKey|4xx->4xx|$.Message
Issue-Key: ivs|GetStreamKey|4xx->4xx|$.RequestID
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# IVS not-found error responses omit the Code and Message keys AWS sends (10 causes)

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetChannel

## Description
Replaying the wire corpus against the IVS emulator (service token `ivs`, crate services/service-com-amazonaws-ivs-amazoninteractivevideoservice) shows that every ResourceNotFoundException the service answers arrives with the correct HTTP status and an EMPTY error envelope: the recorded AWS response body carries `Code`, `Message` and a request id, and the replayed body carries none of them. This issue bundles the TEN per-operation causes below; they share one mechanism but each is a separate failing fixture step a fixer must re-replay, and each must be closed before submitting.

Shared AWS behavior (applies to all ten causes). Smithy (com.amazonaws.ivs, restJson1): ResourceNotFoundException is `errorKind: client`, `@httpError 404`, and is a declared error of GetChannel, UpdateChannel, DeleteChannel, GetAdConfiguration, DeleteAdConfiguration, GetPlaybackKeyPair, DeletePlaybackKeyPair, GetRecordingConfiguration, DeleteRecordingConfiguration and GetStreamKey; its detail member is named `exceptionMessage` with no @jsonName, and it also models the `x-amzn-ErrorType` response header. Recorded behavior for the wire envelope: the response AWS sends carries the error code under `Code` ('ResourceNotFoundException'), the text under `Message`, and a per-request id under `RequestID`. The message is the service's one not-found sentence: fixed text `Resource: ` + the ARN from the request + ` not found` — the ARN is the only variable part (e.g. `Resource: arn:aws:ivs:<region>:<account>:channel/<id> not found`); the request id value differs per run and is not itself a requirement, only the key's presence is.

Shared emulator behavior: the status is right (404) on every one of these, but the replayed body resolves neither `Code` nor `Message` (both reported `got None`), so a client sees a 404 with no error code and no text. crate::state::resource_not_found DOES build the sentence and route it through Scaffolded::stated (state.rs:836-848, scaffolded.rs:191-219/276-281/434-468), so the text exists in the process and is lost on the way to the wire — the body this emulator serves does not carry the `Code`/`Message` keys the recording has.

Why (all causes): an SDK or script cannot tell WHICH failure it got or show why — Error.Code and Error.Message are unpopulated, so every `except ResourceNotFoundException` branch, retry classifier and log line keyed on the code or the text is dead against this emulator while the same code against AWS works.

Fix (all causes): the service's error-response rendering — the body/headers written for a Scaffolded::Stated failure in crate::scaffolded (STATED_MESSAGE / STATED_ERROR_SCHEMA and the error serializer that is handed `shape_name()`), not the ten handlers, which already produce the correct sentence.

Cause 1 of 10: GetChannel on an ARN the store does not hold. Fixture ivs-channel-error-paths, step task#1 (the same cluster covers ivs-channel-lifecycle#teardown#7). Expected 404 with Code 'ResourceNotFoundException' and Message 'Resource: <channel arn> not found'; got 404 with neither key.
Cause 2 of 10: GetAdConfiguration on an unknown ad-configuration ARN. Fixture ivs-ad-configuration-error-paths, step task#2. Expected Code 'ResourceNotFoundException', Message 'Resource: <ad-configuration arn> not found', plus RequestID; got none.
Cause 3 of 10: DeleteAdConfiguration on an unknown ad-configuration ARN. Fixture ivs-ad-configuration-error-paths, step task#4. Same expected envelope; got none.
Cause 4 of 10: UpdateChannel on an unknown channel ARN. Fixture ivs-channel-error-paths, step task#3.
Cause 5 of 10: DeleteChannel on an unknown channel ARN. Fixture ivs-channel-error-paths, step task#4.
Cause 6 of 10: GetPlaybackKeyPair on an unknown playback-key ARN. Fixture ivs-playback-key-pair-error-paths, step task#3. Expected Message 'Resource: <playback-key arn> not found'.
Cause 7 of 10: DeletePlaybackKeyPair on an unknown playback-key ARN. Fixture ivs-playback-key-pair-error-paths, step task#4.
Cause 8 of 10: GetRecordingConfiguration on an unknown recording-configuration ARN. Fixture ivs-recording-configuration-error-paths, step task#2.
Cause 9 of 10: DeleteRecordingConfiguration on an unknown recording-configuration ARN. Fixture ivs-recording-configuration-error-paths, step task#3.
Cause 10 of 10: GetStreamKey on a stream-key ARN that was just deleted. Fixture ivs-stream-key-management, step task#7. Expected Message 'Resource: <stream-key arn> not found'.

Related open issues, neither of which covers these steps: 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md (behavioral corpus, Cause 1 — modelled bodies rendered under `exceptionMessage` so Error.Message is empty) and the resolved 20261022T071923Z-ivs-9-behavioral-divergences-empty-error-message-on.md. The replay reports no owner for any of the ten keys above, so they are filed here.

## Evidence
Replay summary: trace_set simparity-wire-aws-v1, emulator_up true, 84 steps, 32 passed, 47 mismatched, score 38.1%.

Per cause (expected -> actual), all with the correct 404 status:
- GetChannel, ivs-channel-error-paths#task#1: $.Code expected 'ResourceNotFoundException' got None; $.Message expected 'Resource: arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist not found' got None. Cluster also contains ivs-channel-lifecycle#teardown#7.
- GetAdConfiguration, ivs-ad-configuration-error-paths#task#2: $.Code None; $.Message expected 'Resource: arn:aws:ivs:us-west-2:000000000000:ad-configuration/does-not-exist not found' got None; $.RequestID None.
- DeleteAdConfiguration, ivs-ad-configuration-error-paths#task#4: same three keys missing.
- UpdateChannel, ivs-channel-error-paths#task#3 and DeleteChannel, ivs-channel-error-paths#task#4: $.Code and $.Message missing, expected 'ResourceNotFoundException' / 'Resource: <channel arn> not found'.
- GetPlaybackKeyPair, ivs-playback-key-pair-error-paths#task#3 and DeletePlaybackKeyPair, #task#4: $.Code and $.Message missing, expected 'Resource: arn:aws:ivs:us-west-2:000000000000:playback-key/does-not-exist not found'.
- GetRecordingConfiguration, ivs-recording-configuration-error-paths#task#2 and DeleteRecordingConfiguration, #task#3: $.Code, $.Message ('Resource: <recording-configuration arn> not found') and $.RequestID missing.
- GetStreamKey, ivs-stream-key-management#task#7: $.Code, $.Message ('Resource: <stream-key arn> not found') and $.RequestID missing.

The ARNs and the request ids above are one run's values; the requirement is the key set plus the fixed sentence 'Resource: <arn> not found' with the ARN taken from the request.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), or by hand with an SDK/curl against the data listener in region us-west-2:
1. POST /GetChannel {"arn":"arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist"} -> 404 with a body that carries no Code and no Message (fixture ivs-channel-error-paths#task#1).
2. Repeat for /UpdateChannel, /DeleteChannel, /GetAdConfiguration, /DeleteAdConfiguration, /GetPlaybackKeyPair, /DeletePlaybackKeyPair, /GetRecordingConfiguration, /DeleteRecordingConfiguration with an ARN of the matching resource type that was never created.
3. CreateChannel, then DeleteStreamKey the returned stream key, then POST /GetStreamKey with that ARN -> 404 with the same empty envelope (fixture ivs-stream-key-management#task#7).
In each case compare the body keys against the recording: Code, Message, RequestID.
