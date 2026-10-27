Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: c677c98df9a3a2556a69e92857be9faa7b7124d4

# The DownloadUri reported for a vocabulary filter (and vocabulary) serves nothing, so content cannot be fetched back

_Filed: 2026-10-24T10:18:51Z_
_Operation: GetVocabularyFilter | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `c677c98df9a3a2556a69e92857be9faa7b7124d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetVocabularyFilter` reports a `DownloadUri` computed from the stored record, and `GetVocabulary` does the same for a vocabulary built from inline `Phrases`, but nothing is ever written or served at those locations. A client that creates a filter with `Words`, reads `DownloadUri` and fetches it cannot retrieve the words it stored.

Expected (AWS): `DownloadUri` is a presigned location holding the filter's / vocabulary's text content — "use this URI to view or download the custom vocabulary filter" — so a GET of that URI returns the words/terms the create or update supplied, and a subsequent UpdateVocabularyFilter is reflected in what that URI serves.

Actual (emulator): the URI is a synthesized string (`crate::state::vocabulary_download_uri` / `VocabularyFilterRecord::download_uri`); the words are held only in process state and no operation of the service exposes them, so fetching the URI yields nothing. The stored words are therefore unobservable to any client.

The S3 object data plane needed to close this now exists (PutObject/GetObject are implemented and other service crates reach S3 in process through `ctx.callback().http_client()` + `aws_sdk_s3::Client`; see comprehend's `s3_client()`), so the create/update path can write the content object the reported URI names.

## Evidence

Spec: `GetVocabularyFilterResponse$DownloadUri` / `GetVocabularyResponse$DownloadUri` are documented as the location to "view or download" the custom vocabulary filter / vocabulary (smithy_query operation=GetVocabularyFilter, GetVocabulary). Handler/state: services/.../src/ops/get_vocabulary_filter.rs line 199 returns `Some(record.download_uri())` with the comment "Nothing is served there"; services/.../src/state.rs `VocabularyFilterRecord::download_uri` / `vocabulary_download_uri` only format a string, and the module header says the DownloadUri "is a location nothing serves". Corpus: behavioral check ids `transcribe.vocabulary-filter-content-roundtrip-via-download-uri` and `transcribe.update-vocabulary-filter-words-reflected-in-download` were explicitly left UNCLOSED by the resolution note of fixed issue 20261022T044732Z, which defers them to a follow-up that was never filed. Tests: tests/all/vocabularies.rs lines 159-162 and 727-730 assert only that the URI string names the right resource; no test fetches content from it.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateVocabularyFilter and {"VocabularyFilterName":"blocked","LanguageCode":"en-US","Words":["foo","bar"]}.
2. Call Transcribe.GetVocabularyFilter for "blocked" and take the reported DownloadUri.
3. Fetch that URI (S3 GetObject / HTTP GET): AWS serves the filter's word list; the emulator serves nothing — no object exists at that location.
4. Call Transcribe.UpdateVocabularyFilter with {"VocabularyFilterName":"blocked","Words":["baz"]} and re-fetch the DownloadUri: AWS reflects the new word list; the emulator still serves nothing.
5. Repeat with CreateVocabulary + Phrases and GetVocabulary's DownloadUri for the vocabulary family.
