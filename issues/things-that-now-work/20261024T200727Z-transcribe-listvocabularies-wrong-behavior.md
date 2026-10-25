Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# Transcribe resource stores are not Region-scoped: a vocabulary/job created through one Region's endpoint is readable, listable and mutable through every other Region

_Filed: 2026-10-24T20:07:27Z_
_Operation: ListVocabularies | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every resource family of this emulator (`transcription_jobs`, `medical_transcription_jobs`, `call_analytics_jobs`, `medical_scribe_jobs`, `vocabularies`, `medical_vocabularies`, `vocabulary_filters`, `language_models`, `call_analytics_categories`) is a single flat map keyed by resource NAME only. The Region the request was signed for is read (`crate::state::request_region`) and stored on the record, but it is used solely to build an output URI/ARN — no lookup, listing, uniqueness check or quota count ever consults it.

Expected (AWS): Amazon Transcribe resources are Regional. A custom vocabulary created in `us-east-1` is not returned by `ListVocabularies`/`GetVocabulary` against the `eu-west-1` endpoint (the eu-west-1 account holds nothing of that name), the published quota is "100 custom vocabularies per account per Region", and a name is unique per Region — so `CreateVocabulary` with the same name in a second Region succeeds rather than answering ConflictException.

Actual (emulator): the resource is global to the process.
1. `CreateVocabulary` signed for us-east-1, then `ListVocabularies`/`GetVocabulary` signed for eu-west-1 returns it (and reports the us-east-1 `DownloadUri`).
2. `CreateVocabulary` with the same name signed for a second Region is refused 400 ConflictException although that Region holds no such resource.
3. The per-account ceilings (`MAX_VOCABULARIES`, `MAX_TRANSCRIPTION_JOBS`, …) are counted across all Regions at once, so a caller hits `LimitExceededException` sooner than AWS would.
4. `UpdateVocabulary`/`UpdateVocabularyFilter` overwrite `record.region` with the CALLING Region (update_vocabulary.rs: `record.region = region;`), so an update issued from another Region silently migrates the resource and every later-reported `DownloadUri`/service-bucket URI moves with it.
The same holds for every job family: `ListTranscriptionJobs`, `ListCallAnalyticsJobs`, `ListMedicalTranscriptionJobs` and the Get* readers all answer across Regions.

This is distinct from the already-open ListTagsForResource/validation issue, which is about the region/account SEGMENTS of a tagging ARN being discarded by `parse_resource_arn`; here it is the store itself that has no Region dimension, so even correctly scoped, properly signed calls cross Regions.

## Evidence

Spec/well-established behavior: Transcribe ARNs are region-qualified — `TranscribeArn`'s own pattern pins the Region segment (`^arn:aws(-[^:]+)?:transcribe:[a-zA-Z0-9-]*:[0-9]{12}:...`, smithy_query shape=TranscribeArn) and the emulator builds a resource's ARN from the stored `region` (`crate::state::resource_arn`); AWS publishes Transcribe quotas per account per Region (the emulator's own state.rs comment for `MAX_VOCABULARIES` says "100 custom vocabularies per account per Region"), and a regional service endpoint only serves resources of that Region. Handler/state: services/service-com-amazonaws-transcribe-transcribe/src/state.rs — `ComAmazonawsTranscribeTranscribeState` holds `vocabularies`, `transcription_jobs`, … as name-keyed maps; `vocabulary_mut`, `vocabularies_newest_first`, `jobs_newest_first`, `tags`/`tags_mut` take no region argument; `request_region` (state.rs ~line 2203) feeds only URI/ARN construction. Handlers: ops/list_vocabularies.rs lines 202-236 iterate every stored record regardless of the caller's scope; ops/create_vocabulary.rs lines 286-296 do the conflict check on the global map; ops/update_vocabulary.rs line 276 rewrites the stored region. Tests: tests/all/transcription_jobs.rs `the_service_managed_transcript_uri_is_in_the_signed_region` asserts only that the transcript URI reflects the signed Region; no test in tests/all/ creates a resource in one Region and asserts it is invisible in another, so nothing covers this.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateVocabulary and a SigV4 Authorization header whose credential scope names us-east-1: {"VocabularyName":"shared","LanguageCode":"en-US","Phrases":["Aardvark"]}
2. POST / with X-Amz-Target: Transcribe.ListVocabularies signed for eu-west-1 (empty body) — the emulator returns the `shared` vocabulary created in us-east-1; AWS returns an empty list for eu-west-1
3. POST / with X-Amz-Target: Transcribe.GetVocabulary signed for eu-west-1 {"VocabularyName":"shared"} — the emulator answers 200 with a us-east-1 DownloadUri; AWS answers the not-found refusal
4. POST / with X-Amz-Target: Transcribe.CreateVocabulary signed for eu-west-1 with the same name — the emulator answers 400 ConflictException; AWS creates a second, independent eu-west-1 vocabulary
5. POST / with X-Amz-Target: Transcribe.UpdateVocabulary signed for eu-west-1 {"VocabularyName":"shared","LanguageCode":"en-US","Phrases":["Zebra"]}, then GetVocabulary signed for us-east-1 — the stored region has moved to eu-west-1 and the reported DownloadUri now names eu-west-1
6. Repeat with StartTranscriptionJob + ListTranscriptionJobs/GetTranscriptionJob across two Regions for the job families
