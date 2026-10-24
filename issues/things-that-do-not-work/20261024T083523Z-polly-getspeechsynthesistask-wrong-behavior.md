Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: cdf8bb57609ff190f9228b1bff313b67e030c44d

# A synthesis task never expires: GetSpeechSynthesisTask keeps reporting a task older than the documented 72-hour retention

_Filed: 2026-10-24T08:35:23Z_
_Operation: GetSpeechSynthesisTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cdf8bb57609ff190f9228b1bff313b67e030c44d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Polly retains a SpeechSynthesisTask for 72 hours after it is started; after that the task is gone, so GetSpeechSynthesisTask answers `SynthesisTaskNotFoundException` (HTTP 400, with the null message body this emulator already reproduces) and ListSpeechSynthesisTasks no longer reports it.

Expected: a task whose `CreationTime` is more than 72 hours in the past is not reported — GetSpeechSynthesisTask refuses it with SynthesisTaskNotFoundException and it drops out of ListSpeechSynthesisTasks (and no longer consumes a page slot or shifts a NextToken cursor).

Actual: the task store has no retention at all. `ComAmazonawsPollyParrotV1State::synthesis_tasks` keeps every entry forever; the only clock-driven transition modeled is `pending_status` → terminal status (`settle_task_status`, src/state.rs lines 152-166). src/ops/get_speech_synthesis_task.rs looks the task up by id (line 159) and reports it regardless of age, and src/ops/list_speech_synthesis_tasks.rs lists it regardless of age (lines 254-270), so a task with a 7-day-old CreationTime still reads back 200.

## Evidence

Spec grounding: the model's StartSpeechSynthesisTask documentation states "The SpeechSynthesisTask object is available for 72 hours after starting the asynchronous synthesis task" (reproduced in src/ops/start_speech_synthesis_task.rs lines 57-59); GetSpeechSynthesisTask models SynthesisTaskNotFoundException for a well-formed TaskId that names no task, which is what an expired task becomes. Handler: src/ops/get_speech_synthesis_task.rs lines 152-174 (no age check) and src/state.rs (StoredSynthesisTask carries `creation_time` and a `pending_status` clock hook, but nothing removes an aged entry). Test coverage: tests/all/synthesis_tasks.rs and tests/all/error_semantics.rs seed tasks with an explicit creation time (tests/all/support.rs `stored_task(task_id, created)`) and assert ordering, filtering, paging and status settling — no test asserts that a task older than the retention window is gone, so the gap is uncovered even though the seeding helper makes it directly observable.

## Steps to Reproduce

1. Seed (or start, once StartSpeechSynthesisTask is implemented) a synthesis task whose CreationTime is 96 hours in the past
2. Call GET /v1/synthesisTasks/{TaskId} — observe HTTP 200 with the full SynthesisTask, where real Polly answers HTTP 400 SynthesisTaskNotFoundException because the object is only available for 72 hours
3. Call GET /v1/synthesisTasks — observe the 96-hour-old task still listed and still occupying a page slot
