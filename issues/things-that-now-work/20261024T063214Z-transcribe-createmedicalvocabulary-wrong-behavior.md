Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Every custom medical vocabulary settles in FAILED and never reaches READY, so the family can never be used

_Filed: 2026-10-24T06:32:14Z_
_Operation: CreateMedicalVocabulary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateMedicalVocabulary` models `VocabularyFileUri` as `@required` and publishes no inline alternative, so a medical vocabulary's terms can ONLY come from the table file at that URI. The handler records the request (good) but the clock-owed transition it schedules is hard-wired to failure: `MedicalVocabularyRecord::advance_clock` always moves `PENDING -> FAILED` and stamps a `FailureReason` saying "this emulator reads no object store and makes no outbound request". Consequently EVERY medical vocabulary this emulator holds ends `FAILED`, `GetMedicalVocabulary` never reports a `DownloadUri` (it is gated on `state == READY`, which is unreachable), and `ListMedicalVocabularies` can never return a row for `StateEquals: READY`.

Expected (real service): a create whose `VocabularyFileUri` names an object that exists answers 200 with `VocabularyState: PENDING`, and a later `GetMedicalVocabulary` reports `VocabularyState: READY`, no `FailureReason`, and a `DownloadUri` for the built table. Only a URI that cannot be read (missing object, no permission) ends `FAILED` with a `FailureReason`. Actual: `PENDING` then unconditionally `FAILED`, for every request, including one whose file exists in the companion object store.

This is not a platform limitation: other service crates in this workspace resolve S3 objects in-process through the shared emulator (see `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs::s3_client` / `read_plugin_file`, and its tests `custom_plugin_file_location.rs` — "a custom plugin file the object store answers for is taken on and reads back" vs "... the object store cannot read is refused"). The same seam would let this family build a vocabulary whose file is present and fail only the one whose file is not. The non-medical `VocabularyRecord::advance_clock` has the identical hard-wired failure for a `CreateVocabulary`/`UpdateVocabulary` that supplies `VocabularyFileUri` instead of `Phrases`, and the fix belongs in one place.

Because `StartMedicalTranscriptionJob` requires a READY vocabulary to reference, no medical workflow can ever use a custom medical vocabulary against this emulator.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateMedicalVocabulary")` — `VocabularyFileUri` is `@required` (len 1..2000, pattern `^(s3://|http(s*)://).+$`); output `VocabularyState` targets `VocabularyState` whose values are `PENDING | READY | FAILED`, and `GetMedicalVocabulary`'s `DownloadUri` is documented "use this URI to view or download the custom medical vocabulary" — a value that is unreachable here. Handler/state: `src/ops/create_medical_vocabulary.rs` lines 256-265 build the record via `MedicalVocabularyRecord::pending(..)`; `src/state.rs` `MedicalVocabularyRecord::advance_clock` (lines ~683-698) unconditionally sets `VOCABULARY_STATE_FAILED` + `file_not_read_failure_reason(..)`, and `download_uri()` (lines ~716-720) is `(self.state == VOCABULARY_STATE_READY).then(..)`, i.e. always `None`. The type's own doc header states "WHY THIS FAMILY NEVER REACHES READY". Platform counter-example: `services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/state.rs` reads an S3 object in-process (no socket) to decide whether a create is accepted. Tests: `tests/all/medical_vocabularies.rs` and `tests/all/vocabulary_file_uri.rs` assert the current FAILED outcome ("a vocabulary whose file is never read must end FAILED"), so no test asserts the AWS outcome — they encode the divergence and would need updating with the fix.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.CreateMedicalVocabulary` and body {"VocabularyName":"med-vocab","LanguageCode":"en-US","VocabularyFileUri":"s3://DOC-EXAMPLE-BUCKET/med-terms.txt"} (with that object present in the companion S3 emulator) — 200 with VocabularyState PENDING
2. Wait past the state transition (~500 ms) and call `Transcribe.GetMedicalVocabulary` with {"VocabularyName":"med-vocab"}
3. Emulator answers VocabularyState FAILED, a FailureReason naming the file it did not read, and no DownloadUri; AWS answers VocabularyState READY, no FailureReason, and a DownloadUri
4. Call `Transcribe.ListMedicalVocabularies` with {"StateEquals":"READY"} — the emulator returns an empty list for every medical vocabulary ever created
5. The same happens for `Transcribe.CreateVocabulary` with `VocabularyFileUri` instead of `Phrases` (shared `advance_clock` failure path in src/state.rs)
