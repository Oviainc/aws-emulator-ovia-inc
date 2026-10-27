Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# De-duplicate the test harness logic copied into all seven suites of this crate

_Filed: 2026-10-27T16:46:52Z_
_Operation: Sample | Class: maintainability_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every integration suite of this crate re-implements the same HTTP-reply reduction logic privately: a `Reply` struct plus the `error_code()` computation that strips the namespace/shape-id decoration off `x-amzn-errortype`, the `message()` computation that reads the modelled `Message` member out of a restJson1 error document, and the `mounted_service()` / `create_job()` engine-and-job-directory setup. There are SEVEN copies of `error_code()` (one per suite file) and five copies of `mounted_service()`/`create_job()`.

This is one rule — "how this service's wire answer is reduced to (status, error code, Message)" and "how a job comes to exist for a test of this crate" — implemented once per file, so the copies can and already do drift: `trajectory_lifecycle.rs` carries a `json()` accessor where the other six carry `message()`, and `create_job` exists in two different spellings (one formats the ARN from a `category`/`name` pair, the other parses an existing ARN with `service_common::arn::resource_id_of_type`). A change to how the emulator renders `x-amzn-errortype` or the error envelope has to be chased through seven identical bodies, and a suite whose copy is missed keeps asserting against the old reduction.

The single integration target (`tests/all.rs`) already declares each suite with an explicit `#[path]`, so one shared `tests/all/<helper>.rs` module declared the same way is the natural home: the assertions, the expected statuses/messages and every response stay byte-for-byte the same — only the duplicated helper bodies collapse into one.

## Evidence

Copy 1 — `tests/all/job_resolution.rs:71-96`:
```
impl Reply {
    /// The bare error code out of `x-amzn-errortype`, which may carry a namespace or a
    /// Coral shape-id suffix alongside the name.
    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_default();
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message` member of a restJson1 error document, read under the name the MODEL
    /// declares (`Message`) and no other.
    fn message(&self) -> String { ... }
```

Copy 2 — `tests/all/trajectory_lifecycle.rs:57-69`, character-for-character the same computation:
```
impl Reply {
    /// The bare error code out of an `x-amzn-errortype` value, which may carry a
    /// namespace or a shape-id URL alongside the name.
    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_default();
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
```

Copy 3 — `tests/all/modelled_rejections.rs:85-94`, same body again. `grep -n 'fn error_code'` over `tests/` lists SEVEN copies, one per suite:
```
tests/all/trajectory_lifecycle.rs:60
tests/all/job_resolution.rs:74
tests/all/sampling_path_is_shared.rs:67
tests/all/rejected_value_echo.rs:95
tests/all/job_arn_is_the_callers_job.rs:76
tests/all/refused_sampling_leaves_no_trajectory.rs:72
tests/all/modelled_rejections.rs:85
```

The setup helpers are duplicated the same way — `mounted_service()` at `tests/all/job_resolution.rs:106-113`, `tests/all/job_arn_is_the_callers_job.rs:108-115`, `tests/all/refused_sampling_leaves_no_trajectory.rs:104-111`, `tests/all/trajectory_lifecycle.rs:84-91`, `tests/all/sampling_path_is_shared.rs:99-106` — and the copies have ALREADY drifted: `tests/all/job_resolution.rs:119-127` builds the job ARN from the pair
```
fn create_job(engine: &engine::InMemoryEngine, category: &str, name: &str) {
    engine.state::<JobDirectory>().lock().register(JobRecord { ... arn: format!("arn:aws:sagemaker:us-east-1:000000000000:job/{category}/{name}"), });
```
while `tests/all/sampling_path_is_shared.rs:111-121` parses one instead
```
fn create_job(engine: &engine::InMemoryEngine, job_arn: &str) {
    let resource = ::service_common::arn::resource_id_of_type(job_arn, "job")
```

The shared target that makes one helper module reachable is `tests/all.rs:5-9`: "Put a new suite at `tests/all/<name>.rs` and declare it below ... A suite that is not declared here compiles nowhere", with every suite already declared via `#[path = "all/<name>.rs"]`.

Tests covered by the duplicated helpers (all must keep passing unchanged): `job_arn_is_the_callers_job`, `job_resolution`, `modelled_rejections`, `refused_sampling_leaves_no_trajectory`, `rejected_value_echo`, `sampling_path_is_shared`, `trajectory_lifecycle`.

## Steps to Reproduce

1. cd services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice
2. grep -n 'fn error_code' tests/all/*.rs — seven identical bodies, one per suite
3. grep -n 'fn mounted_service\|fn create_job' tests/all/*.rs — five copies of the engine/router setup and two divergent spellings of create_job
4. Observe tests/all.rs: every suite is declared with an explicit #[path], so a single shared helper module can be declared the same way without touching any assertion
