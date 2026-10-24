Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# CreateJob stores JobEngineVersion and HopDestinations without ever acting on them

_Filed: 2026-10-24T05:12:47Z_
_Operation: CreateJob | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two documented CreateJob members are persisted and echoed back but never have the effect the service documents.

(a) `JobEngineVersion` is stored verbatim as `job_engine_version_requested` and never resolved against the account's Job engine version catalogue (`state.job_engine_versions`, the same catalogue ListVersions reports). The model states the rule for this member explicitly on `Job$JobEngineVersionUsed`: "Requests to create jobs with an expired version result in a regular job, as if no specific Job engine version was requested. When you request an invalid version, the response for this property will be empty. Requests to create jobs with an invalid version result in a 400 error message, and no job is created." Expected: a version string that is not in the catalogue (e.g. 'not-a-version' or '1999-01-01') is BadRequestException (400) and no job is stored; a version that is in the catalogue but expired creates a regular job with JobEngineVersionUsed empty; a valid requested version creates a job whose JobEngineVersionUsed names the version actually used. Actual: any string is accepted with 201, the job is persisted, and `JobRecord::view()` hardcodes `job_engine_version_used: None` for every job, so the requested/used distinction never materialises.

(b) `HopDestinations` is stored and projected back, but no hop ever happens: `JobRecord::view()` hardcodes `queue_transitions: Some(Vec::new())` with the comment "A job is never hopped between queues here", and nothing in `settle`/the `pending` index consults `hop_destinations`. Expected: once a job has waited the hop destination's WaitMinutes in its original queue it moves to the destination queue (adopting the hop's Priority when given) and Job.QueueTransitions records SourceQueue/DestinationQueue/Timestamp. Actual: the job stays on its original queue and QueueTransitions is always []. (Relatedly, WaitMinutes is documented as "Valid range is 1 to 4320 minutes, inclusive" and the handler accepts 0 or 99999 unchecked.)

## Evidence

Smithy model: CreateJob input `JobEngineVersion` (__string) and `HopDestinations` (__listOfHopDestination); `Job$JobEngineVersionUsed` doc carries the invalid-version-is-a-400 / expired-version-falls-back rule quoted above; `HopDestination` doc "Configuration for a destination queue to which the job can hop once a customer-defined minimum wait time has passed" with `WaitMinutes` "Valid range is 1 to 4320 minutes, inclusive"; `QueueTransition` doc "Description of the source and destination queues between which the job has moved, along with the timestamp of the move"; ListVersions/`JobEngineVersion` is the catalogue the requested version must be checked against. Handler: src/ops/create_job.rs builds the record with `job_engine_version_requested: input.job_engine_version` and `hop_destinations: input.hop_destinations.unwrap_or_default()` and never reads `state.job_engine_versions`; src/state.rs `JobRecord::view()` hardcodes `job_engine_version_used: None` and `queue_transitions: Some(Vec::new())`. Test coverage: tests/all/job_engine_versions.rs covers only the ListVersions projection; no test submits a job with a JobEngineVersion or a HopDestination and asserts an effect, so neither symptom is asserted either way. Not covered by the open CreateJob issues (20261024T010653Z-...-createjob-validation is Settings/JobTemplate resolution; ...-createjob-wrong-behavior is the account input policy; 20261022T110844Z cause 3 is JobTemplate settings merging).

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. list_versions() and note the catalogue labels the account publishes.
3. create_job(Role=<role>, JobEngineVersion='not-a-version', Settings={valid inputs+outputGroups}) — expect BadRequestException (400) and no job created; observe HTTP 201 with jobEngineVersionRequested 'not-a-version'.
4. create_job(Role=<role>, JobEngineVersion=<a label returned by list_versions>, Settings={...}) then get_job(Id=<id>) — expect jobEngineVersionUsed to name the version used; observe the member absent.
5. create_job(Role=<role>, Queue='q1', HopDestinations=[{'Queue':'q2','WaitMinutes':1,'Priority':10}], Settings={...}) and poll get_job past one minute — expect the job on queue q2 with one QueueTransition recorded; observe queue still q1 and queueTransitions [].
