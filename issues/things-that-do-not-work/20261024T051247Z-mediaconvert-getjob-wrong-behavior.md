Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# Every submitted job terminates in ERROR ~750ms after CreateJob; no job ever reaches PROGRESSING or COMPLETE

_Filed: 2026-10-24T05:12:47Z_
_Operation: GetJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MediaConvert job whose Settings describe a valid transcode moves SUBMITTED -> PROGRESSING -> COMPLETE on the real service, and while it runs the Job projection carries CurrentPhase (PROBING/TRANSCODING/UPLOADING), JobPercentComplete, Timing.StartTime, and on completion OutputGroupDetails for each output group plus Timing.FinishTime and Status COMPLETE.

In the emulator every job is unconditionally flipped to ERROR a fixed 750ms after creation, whatever its settings, queue or priority: `ComAmazonawsMediaconvertMediaconvertState::settle` (src/state.rs, `settle_job`) sets `status = JOB_STATUS_ERROR` and a canned `NO_TRANSCODER_MESSAGE` for every job that is still SUBMITTED when its due instant passes. PROGRESSING and COMPLETE are unreachable states in this emulator, and `JobRecord::view` hardcodes `current_phase: None`, `job_percent_complete: None`, `output_group_details: None`, `job_engine_version_used: None`.

Expected: a job submitted with inputs and output groups that the service can process progresses out of SUBMITTED into PROGRESSING (CurrentPhase / JobPercentComplete / Timing.StartTime populated) and then reaches COMPLETE with Timing.FinishTime and one OutputGroupDetails entry per output group, with no ErrorMessage. Actual: GetJob/ListJobs/SearchJobs/GetJobsQueryResults report Status ERROR with an emulator-specific ErrorMessage within a second of CreateJob, so any client that polls for COMPLETE (the normal MediaConvert workflow) sees a hard failure, and any client that branches on PROGRESSING never sees it.

This is distinct from the two already-open symptoms of the same code path: the open bundle 20261022T110844Z cause 5 is about an ERROR job carrying ErrorCode 1404 and an input-naming message (i.e. the failure case being classified), and cause 4 is about a PAUSED queue holding a job in SUBMITTED. Neither asks for a successful job to ever reach PROGRESSING/COMPLETE, which is what is missing here.

## Evidence

Smithy model: `JobStatus` is SUBMITTED | PROGRESSING | COMPLETE | CANCELED | ERROR; `Job$CurrentPhase` -> JobPhase ("A job's phase can be PROBING, TRANSCODING OR UPLOADING"); `Job$JobPercentComplete` ("An estimate of how far your job has progressed"); `Job$OutputGroupDetails` ("List of output group details"); `Timing` ("Information about when jobs are submitted, started, and finished"). Handler/state: src/state.rs `const JOB_SETTLES_AFTER: Duration = Duration::from_millis(750)`, `settle_job()` -> `job.status = JOB_STATUS_ERROR` + `NO_TRANSCODER_MESSAGE` with no other outcome possible, and `JobRecord::view()` lines hardcoding current_phase/job_percent_complete/output_group_details to None. Test coverage: tests/all/job_lifecycle.rs `a_submitted_job_leaves_submitted_once_the_clock_makes_it_due` asserts the ERROR outcome and the canned message, i.e. the current divergent behavior is enshrined by a test and must be revised with the fix; no test anywhere asserts PROGRESSING or COMPLETE (job_lifecycle.rs line ~1406 even asserts "nothing is PROGRESSING").

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_queue(Name='q') then create_job(Role=<role>, Queue='q', Settings={'Inputs':[{'FileInput':'s3://bucket/in.mp4'}],'OutputGroups':[{'OutputGroupSettings':{'Type':'FILE_GROUP_SETTINGS','FileGroupSettings':{'Destination':'s3://bucket/out/'}},'Outputs':[{'ContainerSettings':{'Container':'MP4'},'VideoDescription':{'CodecSettings':{'Codec':'H_264'}}}]}]}).
3. Poll get_job(Id=<id>) once per second for a few seconds.
4. Expect the status to pass through PROGRESSING (with CurrentPhase and JobPercentComplete present) and settle on COMPLETE with Timing.FinishTime and OutputGroupDetails; observe Status 'ERROR' within ~1s with a generic errorMessage and no currentPhase/jobPercentComplete/outputGroupDetails.
5. Secondary: list_jobs(Status='COMPLETE') always returns an empty list no matter how many jobs were submitted.
