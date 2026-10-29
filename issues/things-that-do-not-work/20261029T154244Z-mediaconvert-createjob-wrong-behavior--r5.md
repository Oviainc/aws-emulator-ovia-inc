Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: de2d8767cb82ce68bb0f28dff69738907970fd37

# CreateJob runs the outbound S3 input probe before two purely-local refusals, contradicting its own "every purely-local refusal is answered here, before any outbound work" rule

_Filed: 2026-10-29T15:42:44Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `de2d8767cb82ce68bb0f28dff69738907970fd37`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateJob` takes the store's guard once, resolves the template/queue/policy/engine-version, releases the guard, then performs a REAL outbound probe of the job's `Settings.Inputs[].FileInput` against the emulated S3 (up to `MAX_PROBED_INPUTS` = 20 `HeadObject` calls over the in-process callback), and only afterwards re-takes the guard and decides the remaining refusals.

The comment over that first guarded block states the rule the ordering is supposed to follow: "Every purely-local refusal is answered here, before any outbound work: a request naming a template or a queue this account does not hold costs no probe." Two refusals that are purely local — they need nothing but the guarded store this crate already holds — are nonetheless decided only AFTER the probe:

* the hop-destination 404 (`state.holds_queue(reference)`, `create_job.rs:523`), and
* the settings-graph 400 (`state.job_settings_problem(&settings)`, `create_job.rs:552`), which reads only `self.presets` plus the graph the request carried.

So a `CreateJob` whose `Settings` describe no output group, or whose output names a preset this account does not hold, or whose `HopDestinations` name a queue that does not exist, still issues up to twenty `HeadObject` calls to the emulated S3 before being refused — outbound work on a request that was already decidable locally, which is precisely what the handler's own rule says must not happen. The same handler demonstrates that the settings refusal needs no new information after the probe: its comment at `create_job.rs:546` notes the template reference "was resolved against this same guarded store above".

The effect is observable from outside this service: the emulated S3 receives (and logs/counts) object reads for a job that is never created.

## Evidence

The stated rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs:308`:
```rust
// The job template this request inherits from, and the queue the job is sent to,
// both read under the store's guard — which is then RELEASED, because the input
// probe below calls another emulated service in-process and this service's state
// is never locked across that call. Every purely-local refusal is answered here,
// before any outbound work: a request naming a template or a queue this account
// does not hold costs no probe.
let (template, queue, policy, job_engine_version_used) = {
    let mut store = state.lock();
```

The outbound work — `src/ops/create_job.rs:481`:
```rust
let unreadable_input =
    crate::state::unreadable_job_input(ctx, &region, &input_uris).await;
```
which issues real `HeadObject` calls, `src/state.rs:3046`:
```rust
for (uri, bucket, key) in locations {
    match client.head_object().bucket(bucket).key(key).send().await {
```
bounded by `MAX_PROBED_INPUTS` (`state.rs:569`: `const MAX_PROBED_INPUTS: usize = 20;`).

The two purely-local refusals that come AFTER it — `src/ops/create_job.rs:523`:
```rust
if crate::state::hop_destination_queues(&hop_destinations)
    .any(|reference| !state.holds_queue(reference))
{
    return Err(CreateJobErrorModeled::NotFoundException(NotFoundException {
        message: Some(crate::state::not_found_message("hopDestinations")),
    })
    .into());
}
```
and `src/ops/create_job.rs:552`:
```rust
if let Some(problem) = state.job_settings_problem(&settings) {
    return Err(CreateJobErrorModeled::BadRequestException(BadRequestException {
        message: Some(problem.message()),
    })
    .into());
}
```

That second one reads nothing but the guarded store — `src/state.rs:1401`:
```rust
pub fn job_settings_problem(&self, settings: &JobSettings) -> Option<SettingsProblem> {
    if settings.inputs.as_deref().unwrap_or_default().is_empty() {
        return Some(SettingsProblem::NoInputs);
    }
    self.output_groups_problem(settings.output_groups.as_deref().unwrap_or_default())
}
```
…and `holds_queue` is a single map probe, `src/state.rs:4985`:
```rust
pub fn holds_queue(&self, reference: &str) -> bool {
    self.queues
        .contains_key(queue_name_from_reference(reference))
}
```

Test coverage: `tests/all/create_job_validation.rs` and `tests/all/input_probe.rs` pin the refusals and the probe's verdict respectively; neither asserts anything about how many S3 reads a refused submission costs, so the ordering is unpinned.

## Steps to Reproduce

1. Mount mediaconvert beside the emulated S3 so the in-process probe can reach it, and create a bucket with no objects in it.
2. Submit a job whose inputs are S3 URIs and whose `Settings` cannot be stored: `POST /2017-08-29/jobs` with `{"role": "arn:aws:iam::<account>:role/r", "settings": {"inputs": [{"fileInput": "s3://bucket/a.mp4"}, {"fileInput": "s3://bucket/b.mp4"}], "outputGroups": []}}`.
3. Observe HTTP 400 `BadRequestException` ("Your settings must specify at least one entry in OutputGroups…") — the correct refusal.
4. Inspect the emulated S3's received requests: two `HeadObject` calls were issued for `a.mp4` and `b.mp4` before the local refusal was decided, although `job_settings_problem` needed only the guarded store the handler had already taken at create_job.rs:315.
5. Repeat with a valid `Settings` but `{"hopDestinations": [{"queue": "no-such-queue", "waitMinutes": 5}]}`: the 404 for `hopDestinations` is likewise decided only after the probe.
