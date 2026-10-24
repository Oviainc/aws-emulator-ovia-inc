Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# CreateVocabulary refuses a VocabularyFileUri create with 400 instead of recording the vocabulary

_Filed: 2026-10-24T01:59:20Z_
_Operation: CreateVocabulary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A CreateVocabulary request that supplies its terms through `VocabularyFileUri` (the S3/HTTP text-file form) is refused with HTTP 400 BadRequestException and NOTHING is stored. The real service accepts such a request, answers HTTP 200 with `VocabularyState: PENDING`, and builds the vocabulary asynchronously so that a later GetVocabulary/ListVocabularies/DeleteVocabulary finds it.

Expected: `CreateVocabulary {VocabularyName, LanguageCode, VocabularyFileUri: "s3://bucket/vocab.txt"}` -> 200, body `{VocabularyName, LanguageCode, VocabularyState: "PENDING", LastModifiedTime}`, record stored and reachable by name (eventually READY).
Actual: 400 `BadRequestException` with message "The VocabularyFileUri <uri> can't be read by this emulator: it builds a custom vocabulary only from the terms a request lists in Phrases..." and no record created.

The stated reason (the emulator cannot read an object store) does not require refusing the request: no output shape of this service reports the vocabulary's terms at all (GetVocabulary returns only name/language/state/DownloadUri), so recording the vocabulary with the file URI remembered and an empty/unknown term list is wire-indistinguishable from the real service for every operation this crate serves. Refusing instead breaks the entire file-based lifecycle (create -> get -> update -> tag -> delete).

## Evidence

Spec: `CreateVocabulary` input models `VocabularyFileUri` (Uri, len 1..2000, pattern ^(s3://|http(s*)://).+$) as a first-class alternative to `Phrases` (smithy_query operation=CreateVocabulary); the operation doc states you can "either upload a text file ... and include the URI in your request, or ... include a list of terms directly in your request using the Phrases flag". Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/create_vocabulary.rs lines 225-255, the `ContentSource::File(uri)` branch returns `bad_request(...)` before anything is stored (helper `crate::state::content_source`, src/state.rs lines 556-568). Corroboration from a recorded trace: the already-filed (fixed) issue 20261022T175151Z-transcribe-vocabulary-and-tag-set-responses-drop-state names this exact branch as the hypothesis for DeleteVocabulary missing a vocabulary the AWS recording could delete, and states "AWS accepts such a create and builds the vocabulary asynchronously" — the refusal is still present in the code, so that cause was not closed. Tests: tests/all/vocabularies.rs lines 225-241 ("vocabulary from an s3 file this emulator cannot read", "vocabulary from an https file this emulator cannot read") ASSERT the 400, i.e. the suite currently locks in the divergence; no test asserts the accepting behavior.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateVocabulary and body {"VocabularyName":"from-s3","LanguageCode":"en-US","VocabularyFileUri":"s3://example-bucket/vocab.txt"}
2. Observe HTTP 400 with __type BadRequestException instead of HTTP 200 with VocabularyState PENDING
3. Call Transcribe.GetVocabulary with {"VocabularyName":"from-s3"} — the emulator reports the vocabulary as not found, where AWS reports PENDING and then READY
4. Call Transcribe.DeleteVocabulary / TagResource for the same name — every downstream step of a file-based vocabulary lifecycle diverges because the record was never created
