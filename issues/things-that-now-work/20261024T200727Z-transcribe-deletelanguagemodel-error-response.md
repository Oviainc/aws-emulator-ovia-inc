Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# DeleteLanguageModel answers an empty 200 for a model the account does not hold instead of the modelled not-found refusal

_Filed: 2026-10-24T20:07:27Z_
_Operation: DeleteLanguageModel | Class: error-response_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler treats a missing record as an already-satisfied delete: when `language_model_mut` returns `None` it logs "the delete is already satisfied" and answers this operation's ordinary empty success.

Expected (AWS): deleting a custom language model the account holds nothing under is refused with the service's absent-resource failure — BadRequestException, whose own model documentation is "Your request didn't pass one or more validation tests. This can occur when the entity you're trying to delete doesn't exist …" — on the wire status this service uses for client errors (400), with the message naming the custom language model that could not be found.

Actual (emulator): HTTP 200 with an empty body, so a caller that deletes a typo'd model name, or deletes twice, cannot tell the resource never existed. This also makes the delete family internally inconsistent: `DeleteCallAnalyticsCategory`, `DeleteTranscriptionJob` and `DeleteMedicalScribeJob` all refuse an absent resource through `crate::state::not_found_message` over `BadRequestException`, while `DeleteLanguageModel` (and `DeleteVocabulary`/`DeleteVocabularyFilter`, already filed separately) silently succeed — one service answering two different outcomes for one condition.

## Evidence

Spec: `BadRequestException` is declared on DeleteLanguageModel (smithy_query operation=DeleteLanguageModel: BadRequestException, InternalFailureException, LimitExceededException) and its own documentation names the absent-entity case ("This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state"), which is the sentence the handler itself quotes two branches earlier to refuse an IN_PROGRESS model. The same emulator uses that exact branch for the absent resource in ops/delete_call_analytics_category.rs lines 165-187, ops/delete_transcription_job.rs lines 186-196 and ops/delete_medical_scribe_job.rs lines 188-195. Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_language_model.rs lines 188-239 — the `None` arm of the `match stored_status` returns `Ok(DeleteLanguageModelOutput)`. Tests: tests/all/language_models.rs lines 353-420 assert the CURRENT idempotent-success behaviour ("A delete is idempotent: the same call again is this operation's ordinary empty success"), so the fix must update that row; nothing asserts the refusal AWS gives.

## Steps to Reproduce

1. POST / X-Amz-Target: Transcribe.DeleteLanguageModel {"ModelName":"never-created"} — the emulator answers 200 with an empty body; AWS refuses with its absent-resource BadRequestException
2. POST / X-Amz-Target: Transcribe.CreateLanguageModel for `doomed`, wait for its status to leave IN_PROGRESS, then DeleteLanguageModel {"ModelName":"doomed"} (200), then DeleteLanguageModel {"ModelName":"doomed"} again — the second call also answers 200 instead of reporting that the model no longer exists
3. Contrast: POST / X-Amz-Target: Transcribe.DeleteCallAnalyticsCategory {"CategoryName":"never-created"} — the same emulator answers 400 BadRequestException for the identical condition
