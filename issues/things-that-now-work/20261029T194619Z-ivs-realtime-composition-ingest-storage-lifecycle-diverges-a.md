Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs-realtime.stop-composition-sets-stopped-state-and-endtime ivs-realtime.stop-composition-not-found-rejected ivs-realtime.stop-composition-terminal-state-conflict-rejected ivs-realtime.start-composition-too-many-destinations-rejected ivs-realtime.start-composition-idempotency-token-accepted ivs-realtime.delete-encoder-configuration-in-use-rejected ivs-realtime.disconnect-participant-unknown-id-is-noop-success ivs-realtime.start-participant-replication-source-not-found-rejected ivs-realtime.update-ingest-configuration-redundant-ingest-toggle ivs-realtime.update-ingest-configuration-not-found-rejected ivs-realtime.create-storage-configuration-nonexistent-bucket-not-found ivs-realtime.storage-configuration-creates-bucket-policy
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|GetComposition|ResourceNotFoundException
Issue-Key: ivs-realtime.start-composition-idempotency-token-accepted|run#13|AssertionError|raise AssertionError(
Issue-Key: ivs-realtime.delete-encoder-configuration-in-use-rejected|run#20|AssertionError|assert in_use.get("Message") == (
Issue-Key: ivs-realtime|DisconnectParticipant|ResourceNotFoundException
Issue-Key: ivs-realtime.start-participant-replication-source-not-found-rejected|run#7|AssertionError|assert error["Code"] == "ConflictException", f"expected ConflictException, got {error!r}"
Issue-Key: ivs-realtime.update-ingest-configuration-redundant-ingest-toggle|run#9|AssertionError|assert enabled["redundantIngest"] is True, (
Issue-Key: ivs-realtime.update-ingest-configuration-not-found-rejected|run#9|AssertionError|assert updated["redundantIngest"] is True, updated["redundantIngest"]
Issue-Key: ivs-realtime.create-storage-configuration-nonexistent-bucket-not-found|_client_error#12|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ivs-realtime|GetBucketPolicy|NoSuchBucketPolicy
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# ivs-realtime composition/ingest/storage lifecycle diverges: a stopped composition is reclaimed in 1s, idempotencyToken is not a dedup key, and 5 more (7 causes)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopComposition

## Description
Seven distinct root causes in the ivs-realtime emulator, all grounded in failing behavioral checks of this suite. A fixer must close all seven before submitting.

Cause 1 of 7 — a composition stopped by StopComposition becomes unreadable within ~1 second, so the STOPPED state and its endTime can never be observed.
AWS: recorded behavior only (the model states only that a composition is ephemeral and that the resource is deleted when StopComposition is called; it publishes no reclaim span). The recording shows that after a successful StopComposition on an ACTIVE composition, GetComposition keeps answering 200 with composition.state="STOPPED" and a populated composition.endTime, and ListCompositions keeps listing it, for long enough that a caller polling every 3 seconds for up to 60 seconds reliably observes the STOPPED record; a second StopComposition on that terminal composition is refused with 409 ConflictException, not 404.
Emulator: the record is deleted one second after the stop. src/ops/stop_composition.rs:188 calls begin_stop(now); src/state.rs:1623 sets COMPOSITION_STOP_SPAN = 500ms, src/state.rs:1496-1521 reaches STOPPED at stop+500ms, and src/state.rs:1701-1706 (is_reclaimed) removes the record at stop+1000ms. Every poll that lands later than one second after the stop gets 404 ResourceNotFoundException "Resource: <composition arn> not found" from GetComposition, which is what the four checks below crashed on.
Why: a client that stops a composition and then reads it back — the documented way to wait for a stop to complete before deleting the encoder/storage configurations it referenced — gets a hard 404 instead of STOPPED/endTime, and can never distinguish "stopped" from "never existed". It also makes the modeled 409 on a repeat stop unreachable.
Fix: the composition lifecycle in src/state.rs (the reclaim rule CompositionRecord::is_reclaimed / materialize_due and COMPOSITION_STOP_SPAN) must keep a stopped composition readable for a span a polling caller can observe, and GetComposition/ListCompositions must report it as STOPPED with endTime.
Traces: ivs-realtime.stop-composition-sets-stopped-state-and-endtime, ivs-realtime.stop-composition-not-found-rejected, ivs-realtime.stop-composition-terminal-state-conflict-rejected, ivs-realtime.start-composition-too-many-destinations-rejected (all four fail on GetComposition answering ResourceNotFoundException for the composition they just stopped).

Cause 2 of 7 — StartComposition's idempotencyToken is accepted but is not a dedup key.
AWS: Smithy: StartCompositionRequest$idempotencyToken carries @idempotencyToken (CompositionClientToken, length 1..64); StartComposition models ConflictException (409). Recorded: replaying the IDENTICAL idempotencyToken against the same stage and destinations is refused with 409 ConflictException and message "A composition resource already exists with the given attributes" (fixed text, no variable part), and no second composition is created; a DIFFERENT token on the same wiring still succeeds with a distinct composition.
Emulator: the replay succeeds and mints a second composition (the check's replay "unexpectedly succeeded instead of raising ConflictException").
Why: a client retrying StartComposition after a timeout — the exact purpose of an @idempotencyToken member — silently creates duplicate compositions, double-billing the stage and double-writing to the recording destination.
Fix: the StartComposition handler's admission step (src/ops/start_composition.rs) plus the composition records in src/state.rs must index the token per account/region and refuse a replay with the modeled ConflictException.
Traces: ivs-realtime.start-composition-idempotency-token-accepted.

Cause 3 of 7 — the in-use refusal of DeleteEncoderConfiguration is worded with the emulator's own sentence.
AWS: Smithy: DeleteEncoderConfiguration models ConflictException (409); its doc says it "Ensures that no Compositions are using this template". Recorded message: "Resource: <encoder configuration arn> is attached to a composition" — fixed text "Resource: " + the ARN from the request + " is attached to a composition"; it names no composition and no state.
Emulator: same code (ConflictException) but message "Resource: <encoder arn> is in use: composition <composition arn> is ACTIVE", i.e. different fixed text plus two values AWS does not include (the composition ARN and its state).
Why: a client or test that matches the refusal text, or logs it for operators, sees a sentence AWS never sends; the extra composition ARN also leaks a resource identity AWS deliberately omits here.
Fix: the message builder for this refusal (crate::state's composition/in-use message helper used by src/ops/delete_encoder_configuration.rs).
Traces: ivs-realtime.delete-encoder-configuration-in-use-rejected.

Cause 4 of 7 — DisconnectParticipant refuses an unknown participantId instead of no-op succeeding.
AWS: Smithy: DisconnectParticipant input is stageArn + participantId (ParticipantTokenId, pattern ^[a-zA-Z0-9-_]*$) + reason, output is an empty structure; ResourceNotFoundException is modeled (404) for the stage. Recorded: against a stage that exists but a participantId with no connected participant, AWS answers HTTP 200 with an empty body — IVS does not require the participant to exist — and the stage stays readable.
Emulator: 404 ResourceNotFoundException "Resource: <participantId> not found" (here the request's participantId value is interpolated into a not-found sentence).
Why: a client cleaning up a stage (a moderation "kick" that races the participant leaving on its own) gets a hard failure where AWS succeeds, so idempotent disconnect loops break.
Fix: src/ops/disconnect_participant.rs — resolve only the stage, and return the empty 200 output when no such participant is connected.
Traces: ivs-realtime.disconnect-participant-unknown-id-is-noop-success.

Cause 5 of 7 — StartParticipantReplication answers 404 where AWS answers 409 for a participant that is not publishing.
AWS: Smithy: StartParticipantReplication models both ConflictException (409) and ResourceNotFoundException (404). Recorded: with both stages existing and a participantId that is not connected/publishing on the source stage, AWS refuses with ConflictException and message "<participantId> is not publishing" — the participantId from the request followed by the fixed text " is not publishing" — i.e. "nothing to replicate" is a state conflict, not a missing-resource lookup.
Emulator: 404 ResourceNotFoundException "Resource: <participantId> not found".
Why: the two classes are handled differently by clients (a 409 is a retry-after-the-publisher-joins condition; a 404 reads as a bad identifier), and botocore raises a different exception class, so error handling written against AWS does not fire.
Fix: src/ops/start_participant_replication.rs — classify a non-publishing source participant as the modeled ConflictException with that message. Note the open issue 20261029T133251Z-ivs-realtime-startparticipantreplication-error-response.md covers a DIFFERENT input (a replica participant the stage reports as publishing); it does not cover this unknown/not-publishing case.
Traces: ivs-realtime.start-participant-replication-source-not-found-rejected.

Cause 6 of 7 — UpdateIngestConfiguration silently ignores redundantIngest.
AWS: Smithy: UpdateIngestConfigurationRequest$redundantIngest (RedundantIngest boolean, @default false) is one of the two mutable members of the request, and the response carries the updated IngestConfiguration. Recorded: UpdateIngestConfiguration(arn, redundantIngest=true) answers with ingestConfiguration.redundantIngest=true and the value persists — an independent GetIngestConfiguration reports true; flipping it back to false persists too.
Emulator: the handler deliberately drops the member — src/ops/update_ingest_configuration.rs:223-241 comments that "The request's redundantIngest is therefore not persisted either" and writes only stage_arn — so the response and every later read keep reporting redundantIngest=false.
Why: a client enabling redundant ingest gets a 200 that claims nothing changed, and its backup ingest path is never configured; the request is accepted and lost rather than refused.
Fix: src/ops/update_ingest_configuration.rs must persist the request's redundantIngest onto the record (and the record/report path in src/state.rs must surface it).
Traces: ivs-realtime.update-ingest-configuration-redundant-ingest-toggle, ivs-realtime.update-ingest-configuration-not-found-rejected (the second check's not-found half passes; it fails on the same positive control).

Cause 7 of 7 — CreateStorageConfiguration accepts a bucket that does not exist and never amends the bucket policy.
AWS: Smithy: CreateStorageConfiguration models ValidationException (400) and its doc states that "When a StorageConfiguration is created, IVS will modify the S3 bucketPolicy of the provided bucket". Recorded: a syntactically valid s3.bucketName naming no bucket of this account/region is refused with 400 ValidationException and the message "bucket not found in this region" (fixed text, no variable part), and nothing is persisted; against a bucket that does exist, the call succeeds and the bucket's policy afterwards contains a statement with Principal.Service = "ivs-composite.<region>.amazonaws.com", Effect "Allow", Resource "arn:aws:s3:::<bucket>/*", s3:PutObject among the actions and Condition.Bool["aws:SecureTransport"] = "true" (region and bucket come from the request/environment; the rest is the fixed template).
Emulator: the create returns 200 with a minted storage-configuration ARN for a bucket that was never created (recorded response body showed storageConfiguration.s3.bucketName echoed back), and GetBucketPolicy on a bucket a storage configuration was created for still answers NoSuchBucketPolicy "The bucket policy does not exist".
Why: a client that mistypes or forgets to create its recording bucket gets a successful create locally and silently loses every recording; and IVS-as-writer permissions are never granted, so any downstream check of the grant fails.
Fix: src/ops/create_storage_configuration.rs — resolve the bucket through the shared S3 service state before minting anything, refuse with the modeled ValidationException otherwise, and write the IVS grant statement into that bucket's policy. The open issue 20261024T100718Z-ivs-realtime-createstorageconfiguration-validation.md describes the same two gaps from a static read; this entry grounds them in two replayed checks, which that issue does not carry.
Traces: ivs-realtime.create-storage-configuration-nonexistent-bucket-not-found, ivs-realtime.storage-configuration-creates-bucket-policy.

## Evidence
Suite: simparity-behavioral-aws-v1, ivs-realtime 40/69 passed; the 12 checks below failed.

Cause 1: [error] ivs-realtime.stop-composition-sets-stopped-state-and-endtime / .stop-composition-not-found-rejected / .stop-composition-terminal-state-conflict-rejected / .start-composition-too-many-destinations-rejected — all four: "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetComposition operation: Resource: arn:aws:ivs:<region>:<account>:composition/<id> not found", raised while polling GetComposition after a StopComposition that returned 200. Expected: state "STOPPED" with a populated endTime (and 409 ConflictException "Resource: <arn> is in a transitioning state" on a repeat stop). Code: src/state.rs:1623 COMPOSITION_STOP_SPAN = 500ms; src/state.rs:1701-1706 is_reclaimed at stop + 2*span.

Cause 2: [mismatch] ivs-realtime.start-composition-idempotency-token-accepted — "AssertionError: replaying idempotencyToken 'sim2real-idem-a-<id>' unexpectedly succeeded instead of raising ConflictException". Expected 409 ConflictException "A composition resource already exists with the given attributes".

Cause 3: [mismatch] ivs-realtime.delete-encoder-configuration-in-use-rejected — expected Message "Resource: <encoder arn> is attached to a composition"; actual {'Code': 'ConflictException', 'Message': 'Resource: arn:aws:ivs:us-west-2:000000000000:encoder-configuration/0000031fdfb8 is in use: composition arn:aws:ivs:us-west-2:000000000000:composition/0000052085be is ACTIVE'}.

Cause 4: [error] ivs-realtime.disconnect-participant-unknown-id-is-noop-success — "botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DisconnectParticipant operation: Resource: no-such-participant not found". Expected HTTP 200, empty body.

Cause 5: [mismatch] ivs-realtime.start-participant-replication-source-not-found-rejected — "AssertionError: expected ConflictException, got {'Message': 'Resource: no-such-participant not found', 'Code': 'ResourceNotFoundException'}". Expected 409 ConflictException, Message "no-such-participant is not publishing" (the participantId supplied by the request + " is not publishing").

Cause 6: [mismatch] ivs-realtime.update-ingest-configuration-redundant-ingest-toggle — "AssertionError: expected redundantIngest True in UpdateIngestConfiguration response, got False"; and [mismatch] ivs-realtime.update-ingest-configuration-not-found-rejected — "AssertionError: False" on `updated["redundantIngest"] is True`. Handler source states the member is intentionally not persisted: src/ops/update_ingest_configuration.rs:229-241.

Cause 7: [mismatch] ivs-realtime.create-storage-configuration-nonexistent-bucket-not-found — "AssertionError: expected a rejection, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ..., 'storageConfiguration': {'arn': 'arn:aws:ivs:us-west-2:000000000000:storage-configuration/000002423a74', 's3': {'bucketName': 'sim2real-ivsrt-missing-<id>'}}}"; expected 400 ValidationException "bucket not found in this region". [error] ivs-realtime.storage-configuration-creates-bucket-policy — "botocore.exceptions.ClientError: An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist" after a successful CreateStorageConfiguration against that bucket.

## Steps to Reproduce
Cause 1: CreateStage; CreateEncoderConfiguration; create an S3 bucket and CreateStorageConfiguration over it; StartComposition(stageArn, destinations=[s3 destination]); poll GetComposition until ACTIVE; StopComposition(arn); poll GetComposition every 3s. Expected: state STOPPED with endTime, then a repeat StopComposition refused 409 ConflictException. Actual: GetComposition answers 404 ResourceNotFoundException about one second after the stop.
Cause 2: with the same wiring, StartComposition(..., idempotencyToken="t1") → 200; call it again with idempotencyToken="t1" and identical stage/destinations. Expected 409 ConflictException "A composition resource already exists with the given attributes" and no new composition; actual 200 with a second composition ARN.
Cause 3: start a composition that references an encoder configuration and wait for ACTIVE, then DeleteEncoderConfiguration(arn=<encoder arn>). Expected 409 with Message "Resource: <encoder arn> is attached to a composition"; actual the "... is in use: composition <arn> is ACTIVE" sentence.
Cause 4: CreateStage, then DisconnectParticipant(stageArn=<stage>, participantId="no-such-participant"). Expected HTTP 200; actual 404 ResourceNotFoundException.
Cause 5: CreateStage twice (source, destination), then StartParticipantReplication(sourceStageArn, destinationStageArn, participantId="no-such-participant"). Expected 409 ConflictException "no-such-participant is not publishing"; actual 404 ResourceNotFoundException.
Cause 6: CreateIngestConfiguration(name, ingestProtocol="RTMPS") → redundantIngest false; UpdateIngestConfiguration(arn, redundantIngest=true). Expected the response and a following GetIngestConfiguration to report redundantIngest=true; actual false in both.
Cause 7: CreateStorageConfiguration(name, s3={"bucketName": "<name of a bucket that was never created>"}). Expected 400 ValidationException "bucket not found in this region" and nothing persisted; actual 200 with a minted ARN. Then create a bucket, CreateStorageConfiguration over it, and GetBucketPolicy on that bucket. Expected a statement with Principal.Service "ivs-composite.<region>.amazonaws.com", Resource "arn:aws:s3:::<bucket>/*", s3:PutObject and Condition Bool aws:SecureTransport=true; actual NoSuchBucketPolicy.
