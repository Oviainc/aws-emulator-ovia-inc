Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b17317436abe1a5dae1bd1120020acb35c8cff22

# The transcription-job family is the only one whose clock-materialising lookup is hand-rolled in two handlers instead of single-homed in state.rs

_Filed: 2026-10-26T04:24:05Z_
_Operation: GetTranscriptionJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b17317436abe1a5dae1bd1120020acb35c8cff22`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` single-homes the "look the record up and materialise the transition the clock already owed it" rule once per family — `medical_transcription_job_mut`, `medical_scribe_job_mut`, `call_analytics_job_mut`, `vocabulary_mut`, `medical_vocabulary_mut`, `language_model_mut` — and each of those carries the doc that explains why it is one function: "The single lookup for this family, so every operation that reads or deletes a ... job sees the same status: the one the clock already owed it."

The non-medical transcription-job family has no such helper, so the identical `transcription_jobs.get_mut(&region, name)` + `record.advance_clock(now)` pair is written out twice, in GetTranscriptionJob and DeleteTranscriptionJob. The two copies can drift (e.g. one gaining the advance and the other losing it), which is precisely the failure mode the other six helpers exist to prevent, and DeleteTranscriptionJob's own comment already claims the ordering invariant the helper would guarantee. Extracting a `transcription_job_mut(&mut self, region, name, now)` beside its six siblings and calling it from both handlers changes no response and no test.

## Evidence

Copy 1 — services/service-com-amazonaws-transcribe-transcribe/src/ops/get_transcription_job.rs:173-195:
```
        let Some(record) = store
            .transcription_jobs
            .get_mut(&region, &input.transcription_job_name)
        else {
...
        record.advance_clock(now);
```
Copy 2 — services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_transcription_job.rs:186-199:
```
        // The clock-owed transitions are materialised FIRST, so a job whose completion
        // was already due is deleted rather than refused for a status it had left.
        let Some(record) = store
            .transcription_jobs
            .get_mut(&region, &input.transcription_job_name)
        else {
...
        record.advance_clock(now);
```
The rule every OTHER family single-homes — services/service-com-amazonaws-transcribe-transcribe/src/state.rs:1256-1271:
```
    /// The MEDICAL transcription job named `name`, with every clock-owed transition of
    /// it already materialised, or `None` when that Region has started no such job.
    ///
    /// The single lookup for this family, so every operation that reads or deletes a
    /// medical transcription job sees the same status: the one the clock already owed
    /// it.
    pub fn medical_transcription_job_mut(
...
        let record = self.medical_transcription_jobs.get_mut(region, name)?;
        record.advance_clock(now);
        Some(record)
```
and the siblings that consume theirs: delete_medical_scribe_job.rs:190-191 `store.medical_scribe_job_mut(&region, &input.medical_scribe_job_name, now)`, delete_call_analytics_job.rs:172-173 `store.call_analytics_job_mut(...)`, get_vocabulary.rs:192 `store.vocabulary_mut(...)`, get_medical_vocabulary.rs:188 `store.medical_vocabulary_mut(...)`, describe_language_model.rs:177 `store.language_model_mut(...)`, delete_vocabulary.rs:192-194 `store.vocabulary_mut(...)`, delete_language_model.rs:191-193 `store.language_model_mut(...)`.

Existing tests that pin the behaviour and must stay green unchanged: services/service-com-amazonaws-transcribe-transcribe/tests/all/transcription_jobs.rs (job read-after-start and completion reads, e.g. :476 "// A completed job is terminal: a later read reports the same instants.", and the non-terminal delete refusal at :936-961), services/service-com-amazonaws-transcribe-transcribe/tests/all/delete_requires_the_entity.rs, services/service-com-amazonaws-transcribe-transcribe/tests/all/job_concurrency_quota.rs and services/service-com-amazonaws-transcribe-transcribe/tests/all/state_roundtrip.rs.

## Steps to Reproduce

1. grep -n 'transcription_jobs' services/service-com-amazonaws-transcribe-transcribe/src/state.rs — note there is no `transcription_job_mut` beside `medical_transcription_job_mut` (state.rs:1262), `medical_scribe_job_mut` (:1295), `call_analytics_job_mut` (:1328), `vocabulary_mut` (:1358), `medical_vocabulary_mut` (:1391) and `language_model_mut` (:1431).
2. Read src/ops/get_transcription_job.rs:173-195 and src/ops/delete_transcription_job.rs:186-199 — the same `transcription_jobs.get_mut(...)` + `record.advance_clock(now)` pair appears in both.
3. Confirm the fix is behaviour-preserving: add `transcription_job_mut(&mut self, region, name, now)` to state.rs mirroring `medical_transcription_job_mut` and call it from both handlers; `cargo test -p service-com-amazonaws-transcribe-transcribe` passes with every assertion and response byte unchanged.
