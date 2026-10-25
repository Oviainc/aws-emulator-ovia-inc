Issue-Severity: LOW
Issue-Gap-Class: parameter-ignored
Issue-Commit: 5b028d5a37462f071f2cc1a7c0f151a69190cfd2

# OutputEncryptionKMSKeyId and KMSEncryptionContext are dropped by StartTranscriptionJob while every sibling job family records them

_Filed: 2026-10-25T00:40:41Z_
_Operation: StartTranscriptionJob | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `5b028d5a37462f071f2cc1a7c0f151a69190cfd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartTranscriptionJobRequest` models `OutputEncryptionKMSKeyId` and `KMSEncryptionContext`, but the handler never reads either member and `TranscriptionJobRecord` has no field for them, so both are silently discarded at the boundary. The record's own documentation claims the opposite ("One stored transcription job: exactly what the request asked for"), and the three sibling job families of this same crate all keep exactly these members, each with a comment explaining why they are recorded even though nothing is encrypted.

The effect is that the transcription-job record is the only job record in the crate that is NOT "exactly what the request asked for": a caller's choice of output encryption key is accepted and then forgotten, so nothing downstream (a future `GetTranscriptionJob` projection, a future output-location or encryption behaviour, or a state dump) can ever report what the request chose. Every other family needed no such amputation.

## Evidence

Smithy (`smithy_query(query="operation", operation="StartTranscriptionJob")`) lists both members on the input shape:
```
"OutputEncryptionKMSKeyId": {"target": "KMSKeyId", "length": {"min": 1, "max": 2048}, ...},
"KMSEncryptionContext": {"target": "KMSEncryptionContextMap", "length": {"min": 1, "max": 10}, "type": "map"}
```

Handler: `src/ops/start_transcription_job.rs:521-562` builds the record from the input and names neither member — no `input.output_encryption_kms_key_id` and no `input.kms_encryption_context` appears anywhere in the file (the construction goes `output_bucket_name: input.output_bucket_name, output_key: input.output_key, region, settings: input.settings, ...`).

The record contradicts its own doc, `src/state.rs:1873-1921`:
```
/// One stored transcription job: exactly what the request asked for, plus the
/// lifecycle facts the service itself owns (status, timestamps, transcript location).
pub struct TranscriptionJobRecord {
```
…and the field list that follows (lines 1877-1920) has no KMS member at all.

Siblings in the SAME crate keep both, documented as a deliberate rule — `src/state.rs:2067-2072` (`MedicalTranscriptionJobRecord`):
```
    /// `OutputEncryptionKMSKeyId`: the key the transcript would be encrypted with.
    /// Recorded as asked for; nothing encrypts anything, because nothing is written.
    pub output_encryption_kms_key_id: Option<String>,
    /// `KMSEncryptionContext`, recorded for the same reason as
    /// [`Self::output_encryption_kms_key_id`].
    pub kms_encryption_context: Option<std::collections::HashMap<String, String>>,
```
and `src/state.rs:2199-2204` (`MedicalScribeJobRecord`, identical pair), and `src/state.rs:2337-2339` (`CallAnalyticsJobRecord`, `output_encryption_kms_key_id` — the only one of the two its start models). The corresponding handlers store them: `src/ops/start_medical_transcription_job.rs:401-402` (`output_encryption_kms_key_id: input.output_encryption_kms_key_id, kms_encryption_context: input.kms_encryption_context`), `src/ops/start_medical_scribe_job.rs:388-389`, `src/ops/start_call_analytics_job.rs:377`.

Test coverage: `tests/all/transcription_jobs.rs:316` (`a_started_job_reads_back_exactly_as_it_was_started`) walks `start_requests()` (line 244) and asserts the read-back is the record that was started; no row of it sets `OutputEncryptionKMSKeyId` or `KMSEncryptionContext`, so no test exercises either member for this family.

## Steps to Reproduce

1. POST / with `X-Amz-Target: Transcribe.StartTranscriptionJob` and body {"TranscriptionJobName":"kms-1","LanguageCode":"en-US","Media":{"MediaFileUri":"s3://DOC-EXAMPLE-BUCKET/audio.wav"},"OutputBucketName":"example-output","OutputEncryptionKMSKeyId":"arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab","KMSEncryptionContext":{"team":"speech"}} — 200
2. Grep the handler/state for the two members: `src/ops/start_transcription_job.rs` never mentions `output_encryption_kms_key_id` or `kms_encryption_context`, and `crate::state::TranscriptionJobRecord` declares no such field, so neither value exists in the store after step 1
3. Repeat step 1 against `Transcribe.StartMedicalTranscriptionJob` (adding the required OutputBucketName/Specialty/Type) and observe that the same two members ARE stored on `MedicalTranscriptionJobRecord` — the two families of the same crate disagree about recording the same modelled members
