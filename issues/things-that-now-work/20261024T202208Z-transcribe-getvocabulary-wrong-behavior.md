Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 225464d7b482e7fae5afe1559ca2b0fceec44ba9

# GetVocabulary reports a DownloadUri for a vocabulary that is still PENDING

_Filed: 2026-10-24T20:22:08Z_
_Operation: GetVocabulary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `225464d7b482e7fae5afe1559ca2b0fceec44ba9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetVocabulary` reports `DownloadUri` based only on where the vocabulary's terms came from, not on whether the vocabulary has finished being built.

Expected (AWS): `DownloadUri` is the location of the BUILT custom vocabulary ("The S3 location where the custom vocabulary is stored; use this URI to view or download the custom vocabulary"). While `VocabularyState` is `PENDING` the service is still processing the terms and there is no artifact to download, so the member is absent; it appears once the state is `READY`. The model agrees structurally: `CreateVocabularyResponse` and `UpdateVocabularyResponse` — the two answers that always report `PENDING` — have no `DownloadUri` member at all, while `GetVocabularyResponse` does.

Actual (emulator): `VocabularyRecord::download_uri()` returns `Some(..)` whenever the record carries no `file_uri` (i.e. for every vocabulary created from inline `Phrases`), with no reference to `self.state`. A `CreateVocabulary` followed immediately by `GetVocabulary` — inside the 500 ms `VOCABULARY_TRANSITION_DELAY` window, which is exactly what a create-then-read client does — therefore answers `VocabularyState: PENDING` together with a `DownloadUri`, a combination the real service does not emit. The emulator's own MEDICAL family gates the same member on `state == READY` (`MedicalVocabularyRecord::download_uri`), so the two families disagree about one rule.

## Evidence

Spec: `smithy_query(query="operation", operation="GetVocabulary")` — output `DownloadUri` (Uri, len 1..2000, pattern ^(s3://|http(s*)://).+$) documented as the location where the custom vocabulary is stored, beside `VocabularyState` whose values are `PENDING | READY | FAILED` ("If the state is READY, you can use the custom vocabulary in a StartTranscriptionJob request"); `CreateVocabulary`/`UpdateVocabulary`, whose answers are always `PENDING`, model no `DownloadUri` member.

Handler/state: `src/ops/get_vocabulary.rs` lines 203-220 passes `record.download_uri()` straight through; `src/state.rs` lines 1029-1033 — `self.file_uri.is_none().then(|| vocabulary_download_uri(..))`, which never consults `self.state`. Contrast `src/state.rs` lines 1147-1151, where the medical family uses `(self.state == VOCABULARY_STATE_READY).then(..)`.

Tests: `tests/all/vocabularies.rs` and `tests/all/vocabulary_file_uri.rs` assert the presence/absence of `DownloadUri` by content source (inline vs file), not by state; no test reads a vocabulary while it is still `PENDING` and asserts the member is absent, so no test covers this.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.CreateVocabulary` and body {"VocabularyName":"pending-vocab","LanguageCode":"en-US","Phrases":["Aardvark"]} — 200 with VocabularyState PENDING
2. Immediately (within the ~500 ms transition window) POST / with `X-Amz-Target: Transcribe.GetVocabulary` and body {"VocabularyName":"pending-vocab"}
3. The emulator answers VocabularyState PENDING together with a DownloadUri; AWS answers VocabularyState PENDING with no DownloadUri member
4. Repeat the GetVocabulary after the transition: both AWS and the emulator report READY with a DownloadUri — the divergence is confined to the PENDING read
