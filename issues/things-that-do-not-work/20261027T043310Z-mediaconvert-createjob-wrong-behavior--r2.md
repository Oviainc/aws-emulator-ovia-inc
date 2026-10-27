Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# A job that asks for accelerated transcoding reports no AccelerationStatus at all, although the model states its initial value

_Filed: 2026-10-27T04:33:10Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`JobRecord::acceleration_status()` reports `NOT_APPLICABLE` only when the job's `AccelerationSettings.Mode` is `DISABLED`, and reports NOTHING (the member is left absent) for `ENABLED` and `PREFERRED`. The model states what the member carries in exactly those two cases: for jobs whose mode is `ENABLED` or `PREFERRED` "AccelerationStatus is one of the other states. AccelerationStatus is IN_PROGRESS initially, while the service determines whether the input files and job settings are compatible with accelerated transcoding."

So a `CreateJob` with `accelerationSettings.mode = ENABLED` answers 201 with a `Job` that has no `accelerationStatus` member, and every later `GetJob`/`ListJobs`/`SearchJobs` projection of that job is missing it too — a client reading `accelerationStatus` to tell an accelerated job from a non-accelerated one sees nothing rather than the one state the model names for a job that has just been submitted. The value is not invented: `IN_PROGRESS` is the value the member's own documentation gives for a job the service has not yet decided about, and `AccelerationStatus`'s value set already declares it.

The handler comment under the code states the reason — "the status is decided by probing the input, which never happens here, so it stays absent" — but the model sentence puts `IN_PROGRESS` BEFORE that determination ("initially, while the service determines whether ..."), so the absent member contradicts the model rather than the probe.

## Evidence

Smithy, `Job$AccelerationStatus` (smithy_query(query="doc",shape="Job",member="AccelerationStatus")):
```text
Describes whether the current job is running with accelerated transcoding. For jobs that have Acceleration (AccelerationMode) set to DISABLED, AccelerationStatus is always NOT_APPLICABLE. For jobs that have Acceleration (AccelerationMode) set to ENABLED or PREFERRED, AccelerationStatus is one of the other states. AccelerationStatus is IN_PROGRESS initially, while the service determines whether the input files and job settings are compatible with accelerated transcoding.
```
`AccelerationStatus` values: ["NOT_APPLICABLE", "IN_PROGRESS", "ACCELERATED", "NOT_ACCELERATED"]; `AccelerationMode` values: ["DISABLED", "ENABLED", "PREFERRED"].

The code — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:2930`:
```rust
    /// `NOT_APPLICABLE` whenever acceleration is off, per `AccelerationStatus`'s doc.
    /// For a job that asked for acceleration the status is decided by probing the
    /// input, which never happens here, so it stays absent.
    fn acceleration_status(&self) -> Option<String> {
        (self.acceleration_settings.mode == ACCELERATION_MODE_DISABLED)
            .then(|| ACCELERATION_STATUS_NOT_APPLICABLE.to_string())
    }
```
which `JobRecord::view` is the only reader of (`src/state.rs:2866`: `acceleration_status: self.acceleration_status(),`), so every job projection this service builds — `CreateJob` (`src/ops/create_job.rs:576`), `GetJob` (`src/ops/get_job.rs:173`), `ListJobs`, `SearchJobs`, `GetJobsQueryResults` — omits the member for an accelerated job.

Test coverage: `tests/all/resource_projection_defaults.rs:676` loops every `AccelerationMode` value through `CreateJob` and asserts `accelerationSettings`, `billingTagsSource`, `priority`, `statusUpdateInterval`, `shareStatus` and `messages` — it asserts nothing about `accelerationStatus`, and `grep accelerationStatus tests/` finds no match anywhere, so no test pins the absence.

## Steps to Reproduce

1. POST /2017-08-29/jobs with {"role":"<role>","accelerationSettings":{"mode":"ENABLED"},"settings":{<one input, one complete output>}} — observe HTTP 201.
2. Read the response body's /job: there is no `accelerationStatus` member.
3. GET /2017-08-29/jobs/{id} — the projection still carries no `accelerationStatus`, although the model says a job with mode ENABLED carries IN_PROGRESS initially.
4. Repeat with "mode":"PREFERRED" — same omission. With "mode":"DISABLED" the member is reported as NOT_APPLICABLE, so only the two acceleration modes the model describes are missing it.
