Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transcribe.concurrent-create-vocabulary-same-name-one-wins transcribe.create-vocabulary-duplicate-name-conflict transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected transcribe.create-vocabulary-without-phrases-or-fileuri-rejected transcribe.delete-vocabulary-while-pending-rejected transcribe.get-vocabulary-not-found transcribe.list-vocabularies-name-contains-filter transcribe.list-vocabularies-state-equals-ready-reflects-vocabulary transcribe.tag-resource-list-untag-roundtrip transcribe.update-vocabulary-filter-words-reflected-in-download transcribe.update-vocabulary-resets-state-and-content transcribe.vocabulary-filter-content-roundtrip-via-download-uri transcribe.vocabulary-lifecycle-pending-to-ready transcribe.vocabulary-transitions-to-failed-on-unreachable-file transcribe.delete-transcription-job-while-in-progress-rejected transcribe.medical-transcription-job-lifecycle-completes transcribe.medical-vocabulary-lifecycle-pending-to-ready transcribe.native-speech-to-text-round-trip-via-polly transcribe.start-transcription-job-without-language-params-rejected transcribe.transcription-job-identify-language-reflects-detected-code transcribe.transcription-job-lifecycle-completes transcribe.transcription-job-output-written-to-custom-s3-bucket
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe.concurrent-create-vocabulary-same-name-one-wins|run#8|AssertionError|assert len(successes) == 1, f"expected exactly one winner among racers, got: {results}"
Issue-Key: transcribe|CreateVocabulary|AmbiguousService
Issue-Key: transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException creating filter without Words/FileUri, got {code!r}"
Issue-Key: transcribe.create-vocabulary-without-phrases-or-fileuri-rejected|run#4|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: transcribe|CreateVocabularyFilter|AmbiguousService
Issue-Key: transcribe.vocabulary-transitions-to-failed-on-unreachable-file|run#5|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for an unreachable file URI, got {code!r}"
Issue-Key: transcribe|PutObject|NotImplemented
Issue-Commit: 38b62e174af309db3e46216fa3b3f7ac7f6f5b84

# Transcribe is unusable behaviorally: every request is refused 500 AmbiguousService, and the remaining checks are blocked by an S3 PutObject stub (2 causes)

## Resolution note (both causes fixed; two trace ids remain blocked elsewhere)

Cause 1 is fixed in the two `transcribe`-signing services' `can_handle` seams, which now
claim through `service_common::claims_request` (signing name AND the service's own
modelled wire surface), plus the Transcribe custom-vocabulary slice they made reachable:
Create/Get/Update/Delete/List for vocabularies and vocabulary filters and the three
tagging operations, all served from this service's state. Cause 2 is fixed by the S3
object data plane: `PutObject` stores the object and reports the entity tag of the stored
bytes, and `GetObject` / `HeadObject` read it back.

TWO of this issue's trace ids are NOT closed by that work and need a follow-up elsewhere:

* `transcribe.vocabulary-filter-content-roundtrip-via-download-uri`
* `transcribe.update-vocabulary-filter-words-reflected-in-download`

Both fetch the vocabulary content back through the `DownloadUri` that `GetVocabulary` /
`GetVocabularyFilter` report. That URI names the service-managed S3 location the real
service presigns; this emulator computes it from the stored record but serves nothing at
it, exactly as a completed transcription job's `Transcript.TranscriptFileUri` already
does (documented in `services/service-com-amazonaws-transcribe-transcribe/src/state.rs`).
Making it readable means a Transcribe handler writing to, and a caller reading from, the
S3 object store — a cross-service call the framework routes through an SDK client this
workspace does not carry today; naming the S3 crate from the Transcribe crate would form
the cross-crate edge the callback seam exists to avoid. That belongs with the S3 object
data plane's own issue, not with the routing fix.

The SAME missing seam leaves one more gap, in the other direction: a custom vocabulary or
vocabulary filter whose content has to be READ from a `VocabularyFileUri` /
`VocabularyFilterFileUri` is refused with the operations' modelled `BadRequestException`
("can't be accessed"). That is the right answer for the unreachable URI this issue
records, and it is now also the answer for an `s3://` URI naming an object the S3 slice
really holds since `PutObject` works — AWS would build the vocabulary from it. Making a
file-backed vocabulary buildable needs the same cross-service read as the `DownloadUri`
gap above, so it belongs with the same follow-up.

Two further trace ids of this report are served by operations that are still SCAFFOLDED
(each with its own stub issue), so they are not closed here either:
`transcribe.medical-vocabulary-lifecycle-pending-to-ready` (`CreateMedicalVocabulary`,
`GetMedicalVocabulary` — and a medical vocabulary's content is file-only, so it also needs
the read above) and `transcribe.medical-transcription-job-lifecycle-completes`
(`StartMedicalTranscriptionJob`, `GetMedicalTranscriptionJob`). The resolved record is
therefore "both causes of this issue fixed", not "22/22 checks served".

> _Note: this issue was identified against the code base at commit `38b62e174af309db3e46216fa3b3f7ac7f6f5b84`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVocabulary

## Description
All 22 behavioral transcribe checks fail (0/22 passed). They cluster into 2 distinct root causes, both of which must be closed before this issue is submitted.

Cause 1 of 2: the request router refuses every Amazon Transcribe request as 'AmbiguousService' because two bootstrapped services share the sigv4 signing name 'transcribe'.
Failing checks: transcribe.concurrent-create-vocabulary-same-name-one-wins, transcribe.create-vocabulary-duplicate-name-conflict, transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected, transcribe.create-vocabulary-without-phrases-or-fileuri-rejected, transcribe.delete-vocabulary-while-pending-rejected, transcribe.get-vocabulary-not-found, transcribe.list-vocabularies-name-contains-filter, transcribe.list-vocabularies-state-equals-ready-reflects-vocabulary, transcribe.tag-resource-list-untag-roundtrip, transcribe.update-vocabulary-filter-words-reflected-in-download, transcribe.update-vocabulary-resets-state-and-content, transcribe.vocabulary-filter-content-roundtrip-via-download-uri, transcribe.vocabulary-lifecycle-pending-to-ready, transcribe.vocabulary-transitions-to-failed-on-unreachable-file.
  AWS: Amazon Transcribe (Smithy: services/service-com-amazonaws-transcribe-transcribe, protocol awsJson1_1, dispatch by X-Amz-Target 'Transcribe.<Operation>'; the modeled @http/@httpLabel bindings are inert under an RPC protocol) answers each of these operations with its modeled 200 output or one of its modeled client errors. Smithy: CreateVocabulary.errors = BadRequestException(400), ConflictException(409), LimitExceededException(429), InternalFailureException(500); CreateVocabularyFilter.errors = BadRequestException(400), ConflictException(409), LimitExceededException(429); GetVocabulary.errors = BadRequestException(400), NotFoundException(404), LimitExceededException(429); ListVocabularies.errors = BadRequestException(400), LimitExceededException(429); TagResource.errors = BadRequestException(400), ConflictException(409), NotFoundException(404). CreateVocabulary output = {VocabularyName, LanguageCode, VocabularyState, LastModifiedTime, FailureReason}. 'AmbiguousService' is not a modeled error of this service and 500 is not a modeled client-error status for any of these operations. Concurrency expectation (recorded behavior only, per the checks): N concurrent CreateVocabulary calls for the same name produce exactly one success and ConflictException for every other racer; a missing-Phrases/missing-VocabularyFileUri CreateVocabulary and a missing-Words/missing-VocabularyFilterFileUri CreateVocabularyFilter are refused with BadRequestException (HTTP 400); an unreachable VocabularyFileUri is likewise refused with BadRequestException.
  Emulator: every call returns HTTP 500 with body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'transcribe': transcribe, transcribe; the request is rejected rather than served by an arbitrary one"} — botocore reports e.g. "An error occurred (AmbiguousService) when calling the CreateVocabulary operation (reached max retries: 3)". Error-class checks report 'expected BadRequestException, got AmbiguousService'; the race check reports 0 winners and 8 AmbiguousService results. Two registered services both declare sigv4Name 'transcribe' (Amazon Transcribe, endpointPrefix 'transcribe', awsJson1_1; and Transcribe Streaming, endpointPrefix 'transcribestreaming', restJson1), and the router resolves by signing name alone, so it fails closed.
  Why: a client gets an unmodeled 500 server fault for every Transcribe call, including ones that must succeed and ones that must be a well-formed 4xx. SDKs retry the 500 (3 retries observed) and then surface an error no caller catches; code handling ConflictException / BadRequestException / NotFoundException never runs, so create-conflict, not-found, validation and every vocabulary/job lifecycle are all broken. The service is bootstrapped but unreachable.
  Fix: the shared request-dispatch / service-resolution layer that maps an inbound request to an emulated service. Disambiguate the two 'transcribe'-signed services by endpointPrefix/Host, by protocol shape (awsJson1_1 X-Amz-Target vs restJson1 method+URI), or by which service actually binds the requested operation; reject as ambiguous only when no candidate claims the request. Residual per-operation fidelity defects are masked today and will surface after this is fixed. NOTE: open issue 20261022T040822Z-transcribe-router-rejects-every-request-as-ambiguousservice.md describes the same router fault from the WIRE corpus; it does not carry these behavioral check ids, so the checks listed here must be re-replayed under this issue.
  Traces: the 14 check ids listed above.

Cause 2 of 2: every Transcribe check that first has to stage media or vocabulary content in S3 is blocked because S3 PutObject answers NotImplemented.
Failing checks: transcribe.delete-transcription-job-while-in-progress-rejected, transcribe.medical-transcription-job-lifecycle-completes, transcribe.medical-vocabulary-lifecycle-pending-to-ready, transcribe.native-speech-to-text-round-trip-via-polly, transcribe.start-transcription-job-without-language-params-rejected, transcribe.transcription-job-identify-language-reflects-detected-code, transcribe.transcription-job-lifecycle-completes, transcribe.transcription-job-output-written-to-custom-s3-bucket.
  AWS: S3 PutObject stores the object and answers HTTP 200 with an ETag header; the object is then readable and usable as a Transcribe Media.MediaFileUri / VocabularyFileUri (Smithy: StartTranscriptionJob.Media is required and carries the s3:// URI; CreateMedicalVocabulary/CreateVocabulary accept VocabularyFileUri matching ^(s3://|http(s*)://).+$). Only with that upload succeeding can Transcribe be asked to transcribe or to build a vocabulary from a file.
  Emulator: the staging call fails with HTTP 501 'An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator', so the check errors before any Transcribe operation is exercised. These checks therefore say nothing yet about Transcribe fidelity; they are blocked on the dependency.
  Why: no client workflow that transcribes real media or builds a file-backed vocabulary can be emulated end to end — the input object can never be created, so job lifecycle, language identification, custom output-bucket delivery and in-progress-delete rejection are all unreachable.
  Fix: the S3 PutObject handler (the object data plane of the S3 emulator). Open issue 20261021T221645Z-bootstrap-service-com-amazonaws-s3-amazons3.md tracks standing up Amazon S3, and 20261022T022901Z-mwaa-environment-lifecycle-tagging-round-trip-and-rest.md / 20261022T031134Z-b2bi-17-b2bi-capability-partnership-transformer-job-checks-blocked.md record the same PutObject stub blocking other services; none of them carries these transcribe check ids. Once PutObject stores and returns objects, re-replay the 8 checks above: any Transcribe-side divergence they then expose needs its own issue.
  Traces: the 8 check ids listed above.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: transcribe 0/22 passed (0%), 4 mismatched, 18 errored, 0 skipped, 0 unloadable. No failing check was marked ALREADY FILED or OWNED by an open issue.

Cause 1 (expected vs actual):
- transcribe.get-vocabulary-not-found: expected the modeled not-found rejection from GetVocabulary; actual ClientError 'AmbiguousService' on the setup CreateVocabulary call, HTTP 500, after 3 retries.
- transcribe.create-vocabulary-without-phrases-or-fileuri-rejected: AssertionError "expected BadRequestException, got 'AmbiguousService'".
- transcribe.create-vocabulary-filter-without-words-or-fileuri-rejected: AssertionError "expected BadRequestException creating filter without Words/FileUri, got 'AmbiguousService'".
- transcribe.vocabulary-transitions-to-failed-on-unreachable-file: AssertionError "expected BadRequestException for an unreachable file URI, got 'AmbiguousService'".
- transcribe.concurrent-create-vocabulary-same-name-one-wins: AssertionError "expected exactly one winner among racers"; all 8 racers returned ('AmbiguousService', "2 emulated services all claim requests for 'transcribe': transcribe, transcribe; the request is rejected rather than served by an arbitrary one") instead of 1 success + 7 ConflictException.
- transcribe.create-vocabulary-duplicate-name-conflict, transcribe.delete-vocabulary-while-pending-rejected, transcribe.list-vocabularies-name-contains-filter, transcribe.list-vocabularies-state-equals-ready-reflects-vocabulary, transcribe.tag-resource-list-untag-roundtrip, transcribe.update-vocabulary-resets-state-and-content, transcribe.vocabulary-lifecycle-pending-to-ready: botocore.exceptions.ClientError (AmbiguousService) calling CreateVocabulary (reached max retries: 3).
- transcribe.update-vocabulary-filter-words-reflected-in-download, transcribe.vocabulary-filter-content-roundtrip-via-download-uri: same AmbiguousService refusal calling CreateVocabularyFilter.
Requirement is the status + error code: 200 with the modeled output body on success paths, 400/BadRequestException and 409/ConflictException on the rejection paths — never 500/AmbiguousService. The quoted AmbiguousService message and the checks' expected ConflictException wording ('The requested job name already exists. Use a different job name.') are examples.

Cause 2 (expected vs actual): all 8 checks error with botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject is scaffolded but not implemented in this emulator. Expected: HTTP 200 from PutObject with the object stored and readable, so the Transcribe call under test can run.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-behavioral-aws-v1 behavioral suite; the transcribe service reports 0/22.
3. Cause 1 minimal repro: with any AWS SDK pointed at the emulator, call transcribe.create_vocabulary(VocabularyName='v1', LanguageCode='en-US', Phrases=['Aardvark']). Expected HTTP 200 with {VocabularyName, LanguageCode, VocabularyState, LastModifiedTime}; actual HTTP 500 {"__type":"AmbiguousService", "message":"2 emulated services all claim requests for 'transcribe': transcribe, transcribe; ..."}. Repeat with transcribe.get_vocabulary(VocabularyName='does-not-exist'): expected 404 NotFoundException (or the recorded 400 BadRequestException rejection), actual 500 AmbiguousService. Repeat the same name concurrently from 8 threads: expected 1 success + 7 ConflictException(409).
4. Confirm the collision: services/service-com-amazonaws-transcribe-transcribe (awsJson1_1, endpointPrefix 'transcribe') and services/service-com-amazonaws-transcribestreaming-transcribe (restJson1, endpointPrefix 'transcribestreaming') both declare sigv4Name 'transcribe'.
5. Cause 2 minimal repro: s3.create_bucket(...) then s3.put_object(Bucket=..., Key='media/sample.wav', Body=b'...'). Expected HTTP 200 with an ETag; actual HTTP 501 NotImplemented ('operation PutObject is scaffolded but not implemented'). This is the call that blocks transcribe.transcription-job-lifecycle-completes and the other 7 checks before any Transcribe operation is reached.
6. After fixing both causes, re-replay all 22 check ids listed in trace_ids.
