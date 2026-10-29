Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 38258527b35574f7302fd0c1e5aa4d86ad9e0201

# The three doors that accept HopDestinations refuse a bad wait and an unknown destination queue in opposite orders, so one hop list answers 400 from CreateJob and 404 from the two template doors

_Filed: 2026-10-29T07:34:02Z_
_Operation: CreateJobTemplate | Class: error-response_

> _Note: this issue was identified against the code base at commit `38258527b35574f7302fd0c1e5aa4d86ad9e0201`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`HopDestinations` is accepted by exactly three operations, and `crate::state` owns both tests those doors apply to it: `hop_wait_problem` (a `WaitMinutes` that is absent or outside 1..4320 -> `BadRequestException`) and the destination-queue lookup through `holds_queue` (a `Queue` naming no stored queue -> `NotFoundException`). Both helper docs state that the point of putting them in one place is that the three doors cannot diverge.

The doors apply the two tests in OPPOSITE orders:

* `CreateJob` runs `hop_wait_problem` first (before it re-takes the store lock) and the queue lookup afterwards, so a hop list that is wrong in both ways answers `400 BadRequestException` naming `WaitMinutes`.
* `CreateJobTemplate` and `UpdateJobTemplate` run the queue lookup first and `hop_wait_problem` after it, so the same hop list answers `404 NotFoundException` naming `hopDestinations`.

So one and the same `hopDestinations` value — e.g. `[{"queue": "no-such-queue", "waitMinutes": 0}]` — gets a different status and a different error code depending on which of the three doors it was sent to, although the crate deliberately gave them one shared pair of tests. The crate's own test harness shows the intent: `tests/all/blank_resource_name.rs` builds ONE row table of hop lists and asserts the SAME outcome for all three operations from it; every row in that table carries a valid `waitMinutes: 10`, so no case combines the two problems and the divergence is untested.

No state is corrupted either way (all three doors refuse and store nothing), hence LOW — but a client that branches on 400 vs 404, or that reads which member the refusal names, sees two different answers to one request body.

## Evidence

The shared tests, with the stated reason for sharing them — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:1086-1092`:
```rust
/// `HopDestination$WaitMinutes`: "Required for setting up a job to use queue hopping.
/// Minimum wait time in minutes until the job can hop to the destination queue. Valid range
/// is 1 to 4320 minutes, inclusive." The member is a bare `__integer` in the model, so
/// nothing refuses a missing or out-of-range wait before a handler runs. Every operation
/// that accepts hop destinations takes this one test, so what they accept cannot drift
/// apart;
```
and `src/state.rs:1124-1126`:
```rust
/// One function for the three operations that accept hop destinations, exactly as
/// [`hop_wait_problem`] is, so what they accept cannot drift apart.
```

`CreateJob` — the wait test first, at `src/ops/create_job.rs:434-443`:
```rust
        if let Some(problem) = input
            .hop_destinations
            .as_deref()
            .and_then(crate::state::hop_wait_problem)
        {
            return Err(CreateJobErrorModeled::BadRequestException(BadRequestException {
                message: Some(problem),
            })
            .into());
        }
```
and the destination lookup only afterwards, at `src/ops/create_job.rs:523-530`:
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

`CreateJobTemplate` — the opposite order, lookup first at `src/ops/create_job_template.rs:275-284`:
```rust
        if crate::state::hop_destination_queues(&hop_destinations)
            .any(|reference| !state.holds_queue(reference))
        {
            return Err(CreateJobTemplateErrorModeled::NotFoundException(
                NotFoundException {
                    message: Some(crate::state::not_found_message("hopDestinations")),
                },
            )
            .into());
        }
```
then the wait test at `src/ops/create_job_template.rs:285-295`:
```rust
        // And every hop destination has to state a wait inside the range the member
        // publishes, or it sets up no hop at all — the same test `CreateJob` takes, so a
        // template cannot store a hop a job could not use.
        if let Some(problem) = crate::state::hop_wait_problem(&hop_destinations) {
```

`UpdateJobTemplate` follows the template order too — `src/ops/update_job_template.rs:274-287`: `if hop_queue_missing { ... NotFoundException ... }` then `if let Some(problem) = hop_wait_problem { ... BadRequestException ... }`.

The crate's harness asserting the three doors answer identically — `tests/all/blank_resource_name.rs:629-630` and `:744-768`:
```
/// Every row is driven through all three operations that carry `HopDestinations`, so the
/// rule is asserted wherever it holds.
```
```rust
            Expect::BlankRefusal => {
                for (operation, reply) in [
                    ("CreateJob", &job),
                    ("CreateJobTemplate", &template),
                    ("UpdateJobTemplate", &updated),
                ] {
                    assert_blank_refusal(&format!("{operation}: {case}"), "HopDestinations.Queue", reply);
```
and every row of that table carries a valid wait (`tests/all/blank_resource_name.rs:658`, `:667-668`, `:675`, `:680`, `:685` — all `"waitMinutes": 10` / `20`), so no existing assertion covers a list that is wrong in both ways.

## Steps to Reproduce

1. Read `src/state.rs:1086-1092` and `:1124-1126`: both hop-destination tests are deliberately single functions shared by the three doors "so what they accept cannot drift apart".
2. Read `src/ops/create_job.rs:434-443` (wait test) and `:523-530` (destination lookup): the wait test runs first.
3. Read `src/ops/create_job_template.rs:275-284` (destination lookup) and `:285-295` (wait test): the order is reversed; `src/ops/update_job_template.rs:274-287` matches the template order.
4. Create a queue `q` and a job template `t`. Send `POST /2017-08-29/jobs` with `{"role": R, "queue": "q", "hopDestinations": [{"queue": "no-such-queue", "waitMinutes": 0}], "settings": <valid>}` and observe `400 BadRequestException` naming `WaitMinutes`.
5. Send the same `hopDestinations` value to `POST /2017-08-29/jobTemplates` (and to `PUT /2017-08-29/jobTemplates/t`) and observe `404 NotFoundException` naming `hopDestinations` — one request body, two different statuses and codes across doors the crate gave one shared pair of tests.
6. Confirm nothing pins the divergence: every row of the hop table in `tests/all/blank_resource_name.rs:654-687` carries a valid `waitMinutes`, so no case combines an out-of-range wait with an unknown destination queue.
