Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6c0e2feb75d7e0fede15568cd8941f97f9b3d17f

# StartTranscriptionJob reads the clock BEFORE awaiting the cross-service media resolution, so the reference-state check, the concurrency count and the job's own transition instants are all judged at a stale moment

_Filed: 2026-10-29T09:36:54Z_
_Operation: StartTranscriptionJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6c0e2feb75d7e0fede15568cd8941f97f9b3d17f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartTranscriptionJob` captures `now` and only then awaits `crate::state::resolve_media_objects`, which is an in-process call out to the emulator's own Amazon S3. Everything downstream of that await is then decided from an instant that is older than the moment the record is actually stored:

1. `store.unheld_reference_error(&region, now, &references)` materialises the referenced custom vocabulary's / custom language model's clock-owed transition AS OF the pre-hop instant. A vocabulary whose `PENDING -> READY` transition fell due DURING the S3 hop is therefore still seen as `PENDING`, and the start is refused with `resource_not_usable_message` ("... is in the PENDING state and can't be used") for a resource that was READY by the time the record would have been written.
2. `crate::state::admit_start(&mut store.transcription_jobs, &region, now, ...)` counts the jobs being processed as of the pre-hop instant, so jobs whose completion fell due during the hop are still counted as occupying a concurrency slot and the start can be refused `LimitExceededException` for slots that were already released.
3. `start_lifecycle(now, ...)` and `creation_time: now` stamp the record's `queued_until` / `completion_due` from the pre-hop instant, so a hop longer than `JOB_TRANSITION_DELAY` stores a job whose completion is ALREADY due — defeating the invariant the constant's own doc states.

`StartCallAnalyticsJob` has the same ordering. The two medical starts, and EVERY create of this crate that awaits a cross-service read, read `now` AFTER the await, so this is the inconsistent pair rather than the rule.

## Evidence

Pre-hop clock, then the await — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs:447-465:
```
447|        let now = ::std::time::SystemTime::now();
448|        let region = crate::state::request_region(ctx.parts());
...
460|        let media_verdict = crate::state::resolve_media_objects(
...
465|        .await;
```
and that same `now` is what every later decision uses — start_transcription_job.rs:500 `if let Some(message) = store.unheld_reference_error(&region, now, &references) {`, :550-556 `let admission = crate::state::admit_start(&mut store.transcription_jobs, &region, now, ...)`, :588-589 `let lifecycle = crate::state::start_lifecycle(now, admission == crate::state::StartAdmission::Queue);`, :642 `creation_time: now,`.

The SIBLING start does it the other way round — services/service-com-amazonaws-transcribe-transcribe/src/ops/start_medical_transcription_job.rs:351-370:
```
351|        let media_verdict = crate::state::resolve_media_objects(
...
356|        .await;
...
370|        let now = ::std::time::SystemTime::now();
```
So does `start_medical_scribe_job.rs:356-367` (`.await;` at 365, `let now = ...` at 367), `create_language_model.rs:233-235` (`resolve_training_data(...).await` at 234, `let now` at 235), `create_medical_vocabulary.rs:243-261` (`.await;` at 243, `let now` at 261), `create_vocabulary.rs:267-315`, `update_vocabulary.rs:234-278` and `update_medical_vocabulary.rs:224-242`. `start_call_analytics_job.rs:327-347` is the only other site that reads the clock first (`let now = ...` at 327, `resolve_media_objects(...).await` at 347) and then reuses it at :370 (`unheld_reference_error(&region, now, ...)`), :419 (`admit_start(..., now, ...)`) and :463/:486.

What the stale instant breaks, from the crate's own documentation — state.rs:615-618:
```
615| /// back still observes the non-terminal status (and a poll-until-done script makes
616| /// at least one real poll), and short enough that such a script terminates quickly.
617| /// The due instant is stored on the record; nothing ever sleeps for it.
618| pub const JOB_TRANSITION_DELAY: Duration = Duration::from_millis(500);
```
and the rule the reference check is supposed to follow — state.rs:1898-1902: "Takes `&mut self` and `now` for that reason: the state compared against is the one the CLOCK already owes the resource, materialised first exactly as every reader of that family materialises it, so a build that fell due before this request is not mistaken for one still running." A pre-hop `now` is precisely a build that fell due before the request being mistaken for one still running.

Test coverage: `tests/all/job_start_vocabulary_state.rs` and `tests/all/job_start_language_model_status.rs` cover the reference-state refusal, and `tests/all/job_concurrency_quota.rs:46-50` / `tests/all/queued_jobs_hold_no_concurrency_slot.rs:51-58` (`const QUEUE_AT: Duration = Duration::from_millis(300);` — "Inside the span the filled jobs are processing in (one `crate::state::JOB_TRANSITION_DELAY`, 500 ms)") cover the ceiling and the queue window; none of them holds the media resolution open, so all of them pass with the stale instant and none pins the ordering.

## Steps to Reproduce

1. Create a custom vocabulary (CreateVocabulary with Phrases), so its PENDING -> READY transition is due 500 ms later
2. Wait ~450 ms, then call Transcribe.StartTranscriptionJob naming that vocabulary in Settings.VocabularyName, with a Media.MediaFileUri whose S3 resolution takes longer than the remaining ~50 ms
3. The handler captured `now` at start_transcription_job.rs:447 before awaiting resolve_media_objects, so the vocabulary is still judged PENDING at line 500 and the start is refused 400 BadRequestException "The custom vocabulary ... is in the PENDING state and can't be used" even though it was READY before the record would have been stored
4. Repeat the same shape against Transcribe.StartMedicalTranscriptionJob with a medical vocabulary: that handler reads `now` at line 370, AFTER the await, and accepts the start
5. For the transition instants: fill the transcription-job ceiling, then make a start whose media hop exceeds 500 ms — the stored record carries completion_due <= the instant it was inserted, so the start answers IN_PROGRESS (or QUEUED) and the immediately following GetTranscriptionJob reports COMPLETED, with no poll ever observing the non-terminal status JOB_TRANSITION_DELAY exists to make observable
