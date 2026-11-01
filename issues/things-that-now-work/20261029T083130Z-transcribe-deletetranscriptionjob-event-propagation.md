Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 6946203d9f2a769569cfa156ac7c42bcf6c300bc

# A job delete leaves the transcript document it wrote into the service-managed bucket behind, although every vocabulary-family delete of this crate withdraws its own service-managed file

_Filed: 2026-10-29T08:31:30Z_
_Operation: DeleteTranscriptionJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `6946203d9f2a769569cfa156ac7c42bcf6c300bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate writes two kinds of file into ONE bucket of its own — `crate::state::service_managed_bucket`: a custom vocabulary's/filter's built text file (the location `DownloadUri` names) and the transcript of a job that named no output bucket of its own (the location `Transcript.TranscriptFileUri` names). `state.rs` single-homes that bucket name precisely so "the two cannot drift".

The three vocabulary-family deletes close the loop: after removing the record they AWAIT a withdrawal of the file the resource left in that bucket (`store_vocabulary_content(.., &[])`, whose own doc says the empty-terms call is "the resource is being deleted, so nothing is served there and any file an earlier build left is withdrawn"). `DeleteVocabulary`, `DeleteVocabularyFilter` and `DeleteMedicalVocabulary` all scope their store guard explicitly so that await can happen, and `tests/all/vocabulary_download_isolation.rs` pins it ("a deleted resource's content is withdrawn").

The two job deletes whose family CAN write into that same service-managed bucket — `DeleteTranscriptionJob` (service-managed whenever the start carried no `OutputBucketName`) and `DeleteCallAnalyticsJob` (service-managed whenever the start carried no `OutputLocation`) — remove the record and return, awaiting nothing. The transcript document that `GetTranscriptionJob`/`GetCallAnalyticsJob` stored at `aws-transcribe-<region>-prod/<account>/<job>.json` stays in the emulator's own Amazon S3 after the job that owned it is gone and after Transcribe stops reporting that location at all. Two consequences inside this crate's own model of itself:

* the same bucket is governed by two different rules depending only on which of this crate's families put the file there, which is exactly the drift the single-homed bucket name exists to prevent;
* the service-managed objects a caller accumulates are bounded by nothing. `MAX_RETAINED_JOB_RECORDS`/`evict_oldest_finished_jobs` bound the RECORDS a Region keeps, and a record evicted or deleted takes its reported location out of every answer — but the object at that location is never reclaimed, so start→poll→delete in a loop grows the object store without limit.

Scope note for the fixer: only the SERVICE-MANAGED branch is this service's to withdraw. A bucket the request named belongs to the caller — `store_job_outputs` already says "a bucket the REQUEST named belongs to the caller, who created it before starting the job, so nothing is created there" — so a job whose reported location is a caller bucket must be left exactly as it is, and `DeleteMedicalTranscriptionJob`/`DeleteMedicalScribeJob` (whose `OutputBucketName` is `@required`, so they never write service-managed output) need no change at all. `crate::state::output_object_address` / `is_path_style_s3_host` already exist to tell a reported URI that addresses this emulator's own store from one that does not.

## Evidence

The sibling rule, with the awaited withdrawal — services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary.rs:187-189 and :254-264:
"        // The guard is SCOPED: the store lock is released before this handler awaits\n        // the withdrawal of the file the vocabulary's `DownloadUri` named, which is a\n        // call out to another emulated service and reaches none of this state."
"        // The vocabulary is gone, so the location its `DownloadUri` named serves\n        // nothing: the file the build left is withdrawn with it\n        // (`crate::state::store_vocabulary_content`).\n        crate::state::store_vocabulary_content(\n            ctx,\n            &region,\n            crate::state::RESOURCE_TYPE_VOCABULARY,\n            &input.vocabulary_name,\n            &[],\n        )\n        .await;"
(the same withdrawal at src/ops/delete_vocabulary_filter.rs:226-228 and src/ops/delete_medical_vocabulary.rs:258-260)

The helper's own statement that a DELETE is one of the two callers of the withdrawing branch — src/state.rs:3742-3752:
"/// Store `terms` as the text file the `DownloadUri` of the resource `name` of\n/// `resource_type` names — or WITHDRAW that file when there are no terms."
"/// An EMPTY list of terms is the other direction of the same rule:\n/// nothing was built (the file the terms were to be read from was not served, or the\n/// resource is being deleted), so nothing is served there and any file an earlier\n/// build left is withdrawn."

The path that skips it — src/ops/delete_transcription_job.rs:218-226 (the whole tail of the handler; nothing is awaited after the removal):
"        store\n            .transcription_jobs\n            .remove(&region, &input.transcription_job_name);\n        ::tracing::info!(\n            operation = \"DeleteTranscriptionJob\",\n            job = %input.transcription_job_name,\n            \"transcription job deleted\"\n        );\n        Ok(DeleteTranscriptionJobOutput)"

and the same tail in the other family that writes service-managed output — src/ops/delete_call_analytics_job.rs:205-213:
"        store\n            .call_analytics_jobs\n            .remove(&region, &input.call_analytics_job_name);\n        ::tracing::info!(\n            operation = \"DeleteCallAnalyticsJob\",\n            job = %input.call_analytics_job_name,\n            \"call analytics job deleted\"\n        );"

The crate's own statement that BOTH kinds of file live in the one bucket, so the two rules cannot legitimately differ — src/state.rs:3645-3651:
"/// The bucket a Region's service-managed files live in: the built vocabulary and\n/// vocabulary-filter files the reported `DownloadUri` names\n/// ([`vocabulary_download_uri`]), and the transcript of a job that named no output\n/// bucket of its own ([`output_uri`]). ONE home for the name, so the two cannot drift.\npub fn service_managed_bucket(region: &str) -> String {"

and src/state.rs:5112-5119:
"/// The URI of one output file of a job, in the caller's bucket when the request\n/// named one and in a service-managed bucket otherwise (model doc for\n/// `Transcript$TranscriptFileUri`).\n///\n/// The service-managed branch names [`service_managed_bucket`], the one bucket this\n/// service's own files live in — the same one a custom vocabulary's `DownloadUri`\n/// names ([`vocabulary_download_uri`])."

The write that leaves the object behind is real and awaited on the read path — src/ops/get_transcription_job.rs:216:
"        if !crate::state::store_job_outputs(ctx, &region, &outputs).await {"
with the caller-bucket carve-out the fix has to preserve, src/state.rs:5344-5346:
"/// The service-managed bucket is this service's own, so a write into it stands it up\n/// first ([`create_service_managed_bucket`]); a bucket the REQUEST named belongs to the\n/// caller, who created it before starting the job, so nothing is created there."

Test coverage: the sibling half IS pinned — tests/all/vocabulary_download_isolation.rs:14 ("and a delete of one withdraws only its own"), :274-277 ("/// A delete withdraws only the content of the resource it names" / `async fn a_delete_withdraws_only_its_own_content()`) and :296 ("case {:?}: a deleted resource's content is withdrawn, so a fetch of \\"). The job half is pinned by NOTHING: tests/all/job_output_objects.rs, the suite that asserts a finished job's reported location serves its document, contains no occurrence of "delete" at all, and tests/all/transcription_jobs.rs:1094 / tests/all/call_analytics_job_delete.rs:156 assert only that the deleted job leaves the LISTING, never what its reported output location serves afterwards. So no existing expectation has to change.

## Steps to Reproduce

1. Assemble the emulator with transcribe and Amazon S3 on one engine, as tests/all/media_object_resolution.rs does
2. PutObject s3://example-media/audio.wav, then POST / with X-Amz-Target: Transcribe.StartTranscriptionJob and body {"TranscriptionJobName":"j1","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://example-media/audio.wav"}} — no OutputBucketName, so the job's output is service-managed (crate::state::output_uri's None branch)
3. Wait past crate::state::JOB_TRANSITION_DELAY and POST / with X-Amz-Target: Transcribe.GetTranscriptionJob {"TranscriptionJobName":"j1"} — the answer reports TranscriptionJobStatus COMPLETED and Transcript.TranscriptFileUri https://s3.us-east-1.amazonaws.com/aws-transcribe-us-east-1-prod/<account>/j1.json, and the handler has just stored the document there (get_transcription_job.rs:216)
4. S3 GetObject on bucket aws-transcribe-us-east-1-prod, key <account>/j1.json — the transcript document is served
5. POST / with X-Amz-Target: Transcribe.DeleteTranscriptionJob {"TranscriptionJobName":"j1"} — 200, and GetTranscriptionJob now answers 400 BadRequestException "The requested job couldn't be found."
6. S3 GetObject on bucket aws-transcribe-us-east-1-prod, key <account>/j1.json again — the document is STILL served, for a job this service no longer holds and no longer reports a location for
7. Compare the sibling family: CreateVocabulary {"VocabularyName":"v1","LanguageCode":"en-US","Phrases":["a"]}, wait for READY, GetVocabulary to read its DownloadUri, S3 GetObject it (served), then DeleteVocabulary {"VocabularyName":"v1"} and S3 GetObject the same key — NoSuchKey, because delete_vocabulary.rs:257-264 withdrew it (tests/all/vocabulary_download_isolation.rs:274-296)
8. Repeat steps 2-5 in a loop with fresh job names: every iteration adds one permanently unreclaimable object to the service-managed bucket, although crate::state::evict_oldest_finished_jobs keeps the job RECORDS bounded at MAX_RETAINED_JOB_RECORDS
9. The same sequence with StartCallAnalyticsJob/GetCallAnalyticsJob/DeleteCallAnalyticsJob and no OutputLocation leaves the same orphan (delete_call_analytics_job.rs:205-213)
