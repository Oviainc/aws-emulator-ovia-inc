Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-not-found-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-not-found-error-paths transcribe-vocabulary-lifecycle transcribe-transcription-job-workflow transcribe-vocabulary-filter-lifecycle
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|UpdateVocabulary|4xx->4xx|$.Message
Issue-Key: transcribe|UpdateVocabulary|4xx->4xx|$.__type
Issue-Key: transcribe|UpdateVocabulary|4xx->4xx|status
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->4xx|$.Message
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->4xx|$.__type
Issue-Key: transcribe|UpdateVocabularyFilter|4xx->4xx|status
Issue-Key: transcribe|ListTagsForResource|4xx->4xx|$.Message
Issue-Key: transcribe|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: transcribe|ListTagsForResource|4xx->4xx|status
Issue-Key: transcribe|TagResource|4xx->4xx|$.Message
Issue-Key: transcribe|TagResource|4xx->4xx|$.__type
Issue-Key: transcribe|TagResource|4xx->4xx|status
Issue-Key: transcribe|UntagResource|4xx->4xx|$.Message
Issue-Key: transcribe|UntagResource|4xx->4xx|$.__type
Issue-Key: transcribe|UntagResource|4xx->4xx|status
Issue-Key: transcribe|GetTranscriptionJob|4xx->4xx|$.Message
Issue-Key: transcribe|GetTranscriptionJob|4xx->4xx|$.__type
Issue-Key: transcribe|GetTranscriptionJob|4xx->4xx|status
Issue-Key: transcribe|GetVocabularyFilter|4xx->4xx|$.Message
Issue-Key: transcribe|GetVocabularyFilter|4xx->4xx|$.__type
Issue-Key: transcribe|GetVocabularyFilter|4xx->4xx|status
Issue-Key: transcribe|GetVocabulary|4xx->4xx|$.Message
Issue-Key: transcribe|GetVocabulary|4xx->4xx|$.__type
Issue-Key: transcribe|GetVocabulary|4xx->4xx|status
Issue-Key: transcribe|CreateVocabularyFilter|4xx->4xx|$.Message
Issue-Key: transcribe|CreateVocabularyFilter|4xx->4xx|status
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Transcribe client errors answer the modeled @httpError instead of the HTTP 400 AWS returns

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateVocabulary

## Description
This issue bundles NINE causes, all one fault in this service's error channel. A fixer must close all nine before submitting.

Shared root cause: Amazon Transcribe is an awsJson1_1 (RPC) service, so the REST-shaped traits in its model are inert on the wire; its recorded client-error responses are HTTP 400, and a missing resource is reported with the BadRequestException SHAPE. The emulator instead (a) selects NotFoundException for a missing resource and (b) derives the HTTP status from the error shape's @httpError trait (NotFoundException 404, ConflictException 409). Smithy grounding: smithy_query(operation=GetVocabulary/UpdateVocabulary/...) lists BadRequestException @httpError 400, NotFoundException @httpError 404, ConflictException @httpError 409; BadRequestException's own model documentation states its causes include 'This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)' (quoted verbatim in the emulator's own op header, services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs lines 57-60).

Shared Fix: the handlers' modeled-error selection plus the error serializer for com-amazonaws-transcribe-transcribe — the not-found branches in services/service-com-amazonaws-transcribe-transcribe/src/ops/*.rs must raise BadRequestException, and the error status for this awsJson1_1 service must not be read off the per-shape @httpError. Note on messages: AWS's recorded texts name the resource KIND ('vocabulary', 'vocabulary filter', 'job') and do NOT interpolate the caller's name/ARN, whereas every emulator message below does; the HTTP status and the __type are the requirements here, the quoted texts are examples.

Cause 1 of 9: UpdateVocabulary on a vocabulary that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again." (recorded response; consistent with BadRequestException's model doc).
Emulator: HTTP 404, __type NotFoundException (src/ops/update_vocabulary.rs lines 259-268).
Why: an SDK client branches on the error code, so code that catches BadRequestException/400 for an unknown vocabulary never matches, and a retry/異 classifier sees a 404 the service never emits.
Fix: UpdateVocabulary's handler error selection.
Traces: transcribe-not-found-error-paths#task#1 (same cluster: transcribe-vocabulary-lifecycle#task#4).

Cause 2 of 9: UpdateVocabularyFilter on a vocabulary filter that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested vocabulary filter couldn't be found. Check the vocabulary filter name and try your request again."
Emulator: HTTP 404, __type NotFoundException.
Why: as cause 1, for the vocabulary-filter family.
Fix: UpdateVocabularyFilter's handler error selection.
Traces: transcribe-not-found-error-paths#task#2.

Cause 3 of 9: ListTagsForResource for an ARN naming a transcription job that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested job couldn't be found. Check the job name and try your request again."
Emulator: HTTP 404, __type NotFoundException, message built from the ARN (src/ops/list_tags_for_resource.rs lines 185-196, over state.rs tags_mut).
Why: a tag reader that handles BadRequestException/400 for an unknown resource sees an unmodeled-for-this-wire 404 instead.
Fix: ListTagsForResource's handler error selection.
Traces: transcribe-not-found-error-paths#task#5.

Cause 4 of 9: TagResource for an ARN naming a transcription job that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested job couldn't be found. Check the job name and try your request again."
Emulator: HTTP 404, __type NotFoundException, message built from the ARN.
Why: as cause 3, on the write path.
Fix: TagResource's handler error selection.
Traces: transcribe-not-found-error-paths#task#6.

Cause 5 of 9: UntagResource for an ARN naming a transcription job that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested job couldn't be found. Check the job name and try your request again."
Emulator: HTTP 404, __type NotFoundException, message built from the ARN.
Why: as cause 3, on the untag path.
Fix: UntagResource's handler error selection.
Traces: transcribe-not-found-error-paths#task#7.

Cause 6 of 9: GetTranscriptionJob for a job that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested job couldn't be found. Check the job name and try your request again."
Emulator: HTTP 404, __type NotFoundException.
Why: poll-until-done loops classify an unknown job by this error; a 404/NotFoundException is not what the service emits, so such a loop mis-classifies it as a transport/routing failure.
Fix: GetTranscriptionJob's handler error selection.
Traces: transcribe-transcription-job-workflow#task#1.

Cause 7 of 9: GetVocabularyFilter for a filter that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested vocabulary filter couldn't be found. Check the vocabulary filter name and try your request again."
Emulator: HTTP 404, __type NotFoundException.
Why: as cause 1, on the read path for filters.
Fix: GetVocabularyFilter's handler error selection.
Traces: transcribe-vocabulary-filter-lifecycle#task#1 (same cluster: transcribe-vocabulary-filter-lifecycle#teardown#9).

Cause 8 of 9: GetVocabulary for a vocabulary that does not exist.
AWS: HTTP 400, __type BadRequestException, e.g. "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again."
Emulator: HTTP 404, __type NotFoundException.
Why: a create-if-absent pattern probes with GetVocabulary and keys off BadRequestException/400; a 404 breaks it.
Fix: GetVocabulary's handler error selection.
Traces: transcribe-vocabulary-lifecycle#task#1.

Cause 9 of 9: CreateVocabularyFilter for a name that already exists — right shape, wrong status.
AWS: HTTP 400 carrying __type ConflictException (the recorded response's __type matched; only the status and message diverged), e.g. "The requested vocabulary filter name already exists. Use a different vocabulary filter name."
Emulator: HTTP 409 (the shape's modeled @httpError), message "The requested vocabulary filter name <name> already exists. Use a different name and try your request again."
Why: a client that treats 409 as a retryable/conditional-write outcome and 400 as terminal takes the wrong branch on a duplicate-name create; conversely a 4xx-code-based policy never sees the 400 AWS sends.
Fix: the error status serialization for this awsJson1_1 service (same seam as causes 1-8), for ConflictException.
Traces: transcribe-vocabulary-filter-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator, 9 divergence clusters:
- UpdateVocabulary (transcribe-not-found-error-paths#task#1): expected 400 / __type BadRequestException, got 404 / __type NotFoundException.
- UpdateVocabularyFilter (#task#2): expected 400 / BadRequestException, got 404 / NotFoundException.
- ListTagsForResource (#task#5): expected 400 / BadRequestException, got 404 / NotFoundException.
- TagResource (#task#6): expected 400 / BadRequestException, got 404 / NotFoundException.
- UntagResource (#task#7): expected 400 / BadRequestException, got 404 / NotFoundException.
- GetTranscriptionJob (transcribe-transcription-job-workflow#task#1): expected 400 / BadRequestException, got 404 / NotFoundException.
- GetVocabularyFilter (transcribe-vocabulary-filter-lifecycle#task#1, also teardown#9): expected 400 / BadRequestException, got 404 / NotFoundException.
- GetVocabulary (transcribe-vocabulary-lifecycle#task#1): expected 400 / BadRequestException, got 404 / NotFoundException.
- CreateVocabularyFilter duplicate name (transcribe-vocabulary-filter-lifecycle#task#3): expected 400, got 409 (__type ConflictException agreed).
Model: Transcribe 2017-10-26, protocol awsJson1_1; BadRequestException @httpError 400, NotFoundException @httpError 404, ConflictException @httpError 409; BadRequestException is documented to cover an entity that 'doesn't exist'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Fixture transcribe-not-found-error-paths, steps task#1, task#2, task#5, task#6, task#7: call UpdateVocabulary / UpdateVocabularyFilter / ListTagsForResource / TagResource / UntagResource naming a vocabulary, filter or transcription-job ARN that was never created. Each answers HTTP 404 with __type NotFoundException instead of HTTP 400 with __type BadRequestException.
3. Fixture transcribe-transcription-job-workflow step task#1 (GetTranscriptionJob), transcribe-vocabulary-filter-lifecycle step task#1 (GetVocabularyFilter), transcribe-vocabulary-lifecycle step task#1 (GetVocabulary): same 404/NotFoundException instead of 400/BadRequestException.
4. Fixture transcribe-vocabulary-filter-lifecycle step task#3: CreateVocabularyFilter with the name created at task#2. It answers HTTP 409 instead of HTTP 400 (ConflictException body otherwise correct).
