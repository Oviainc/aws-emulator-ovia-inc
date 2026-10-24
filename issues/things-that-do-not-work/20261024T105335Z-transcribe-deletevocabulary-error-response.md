Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# DeleteVocabulary answers an empty 200 for a vocabulary the account does not hold instead of the modelled not-found failure

_Filed: 2026-10-24T10:53:35Z_
_Operation: DeleteVocabulary | Class: error-response_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler treats a name the store holds no vocabulary under as an already-satisfied delete and answers this operation's ordinary empty success (HTTP 200, empty body). The real service has a not-found failure mode for exactly this operation: the model declares `NotFoundException` (HTTP 404) AND `BadRequestException` (HTTP 400) on `DeleteVocabulary`, and `BadRequestException`'s own documentation names the condition — "This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)". AWS answers that refusal ("The requested vocabulary couldn't be found. Check the vocabulary name and try your request again."); the emulator answers success, so a client that relies on the delete to tell it whether the resource existed (a teardown that must distinguish "removed" from "was never there", or a retry loop that asserts a name is unknown) sees the opposite outcome.

This is also internally inconsistent: `DeleteTranscriptionJob` (delete_transcription_job.rs lines 190-196), `DeleteMedicalScribeJob` (lines 188-195) and `DeleteCallAnalyticsCategory` (lines 165-187) all refuse an absent resource with `BadRequestException`, citing the same `BadRequestException` sentence — and those two operations do NOT even model `NotFoundException`, while `DeleteVocabulary` does.

Expected: a 4xx modelled client error (`BadRequestException` carrying this service's not-found text, or the modelled `NotFoundException`) with nothing changed. Actual: HTTP 200 with an empty body.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteVocabulary")` lists errors BadRequestException (400), InternalFailureException (500), LimitExceededException (429) and NotFoundException (404) — a not-found failure mode is modelled for this operation specifically, which is meaningless if an absent name were an ordinary success; `BadRequestException` doc: "Your request didn't pass one or more validation tests. This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)." Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs lines 223-240 — the `None` branch logs "no custom vocabulary of that name; the delete is already satisfied" and falls through to `Ok(DeleteVocabularyOutput)`. Tests: tests/all/vocabulary_delete_and_tag_sets.rs lines 150-191 (`deleting_a_resource_this_account_does_not_hold_succeeds`) and lines 196-228 (`a_delete_is_idempotent_once_the_resource_is_gone`) PIN the current empty-success behaviour, so the divergence is asserted rather than untested — those assertions have to move with the fix.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.DeleteVocabulary and body {"VocabularyName":"never-created"} on a fresh emulator
2. Observe HTTP 200 with an empty body; AWS answers a 4xx modelled client error (BadRequestException "The requested vocabulary couldn't be found. Check the vocabulary name and try your request again.")
3. Create a vocabulary, wait out its PENDING span, delete it once (200), then delete the same name again: the second delete also answers 200 where AWS reports the resource as absent
