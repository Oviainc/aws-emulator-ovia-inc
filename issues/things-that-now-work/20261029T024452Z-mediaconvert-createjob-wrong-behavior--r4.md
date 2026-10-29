Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1

# A job that reaches a terminal status through `settle` keeps the clock-owed hop its own field doc says must be cleared, unlike the CancelJob path

_Filed: 2026-10-29T02:44:52Z_
_Operation: CreateJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `cd4f62d4622d99c848b6778d8f7c3e7dcd5cf5b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`JobRecord` carries two clock-owed instants, `settles_at` and `hops_at`, each mirrored by an entry in the store's `pending` / `pending_hops` indexes. Both fields document an invariant tying them to the job's status and to its queue's status. Two write paths in `state.rs` break that invariant:

1. `settle_job` moves a job out of `SUBMITTED` into `ERROR` and clears ONLY `settles_at`. It never clears `hops_at` and never removes the job's `pending_hops` entry, so a job in the terminal `ERROR` status is left carrying a due instant for a hop, with a matching index entry, which `hops_at`'s own doc says must be `None` "once the job has left `SUBMITTED`". The sibling terminal transition, `cancel_job`, does exactly the clearing `settle_job` skips — same field, same index, with a comment stating the rule.

2. `accept_job` arms `settles_at` (and the `pending` entry) unconditionally, without consulting the queue's status, so a job submitted to a `PAUSED` queue is stored with a due instant that `settles_at`'s own doc says must be `None` "while the job's queue is `PAUSED` and nothing is therefore due at all". `settle_job`'s own `held` branch is the code that enforces that rule on every other path.

Neither case changes a response today: `hop_job` guards on `waiting = job.status == JOB_STATUS_SUBMITTED` and so refuses to move a terminal job, and `settle_job`'s `held` branch retroactively clears the paused job's instant, so both states self-heal on the next `settle`. The defect is that the stored record is left in a state its own invariant forbids and that the clock indexes disagree with the record they index — the exact failure mode `accept_job`'s doc claims to prevent ("Owns the id, the `pending` entry, the first hop's `pending_hops` entry and the idempotency entry together, so the four cannot be left inconsistent"). Any future reader of `hops_at`/`next_hop` (for example a projection of pending hops, or a `DeleteQueue` guard that consults hop destinations) inherits a stale value.

## Evidence

The invariants, stated on the fields themselves:

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:3056-3060`
```
    /// When the clock owes this job its next hop — the instant it has waited
    /// `hop_destinations[next_hop]`'s `WaitMinutes` in the queue it entered last. `None`
    /// once no hop is left, or once the job has left `SUBMITTED` and is no longer waiting
    /// in a queue at all.
    pub hops_at: Option<SystemTime>,
```

`services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:3079-3083`
```
    /// When the clock owes this job its exit from `SUBMITTED`; `None` once the job has
    /// reached a status nothing further is due from, and `None` while the job's queue is
    /// `PAUSED` and nothing is therefore due at all (see
    /// [`ComAmazonawsMediaconvertMediaconvertState::settle`]).
    pub settles_at: Option<SystemTime>,
```

The path that breaks the first one — `settle_job`, `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:4446-4468`:
```
            if job.status == JOB_STATUS_SUBMITTED {
                job.status = JOB_STATUS_ERROR.to_string();
                ...
                job.timing.start_time = Some(due);
                job.timing.finish_time = Some(due);
            }
            job.settles_at = None;
        }
    }
```
(no `hops_at`, no `next_hop`, no `pending_hops` removal)

The sibling that gets it right — `cancel_job`, `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:4790-4802`:
```
        job.status = JOB_STATUS_CANCELED.to_string();
        job.timing.finish_time = Some(now);
        // A cancelled job is terminal, so the clock owes it nothing further.
        let due = job.settles_at.take();
        let hop_due = job.hops_at.take();
        job.next_hop = job.hop_destinations.len();
        if let Some(due) = due {
            self.pending.remove(&(due, id.to_string()));
        }
        // A cancelled job is in no queue's backlog, so it hops nowhere either.
        if let Some(hop_due) = hop_due {
            self.pending_hops.remove(&(hop_due, id.to_string()));
        }
```

The path that breaks the second one — `accept_job`, `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:4695-4707`, whose own doc claims the four are kept consistent:
```
    /// Owns the id, the `pending` entry, the first hop's `pending_hops` entry and the
    /// idempotency entry together, so the four cannot be left inconsistent by a caller that
    /// forgets one.
    pub fn accept_job(&mut self, mut record: JobRecord, now: SystemTime) -> AcceptOutcome {
        ...
        let due = now + self.job_settles_after;
        record.id = id.clone();
        // SUBMITTED is not terminal, so the clock owes this job its exit from it.
        record.settles_at = Some(due);
        self.pending.insert((due, id.clone()));
```
The rule it skips is the one `settle_job` applies on every later pass, `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:4436-4445`:
```
        let held = self.jobs.get(id).is_some_and(|job| {
            job.status == JOB_STATUS_SUBMITTED && self.queue_is_paused(&job.queue)
        });
        ...
            if held {
                // The queue is paused, so this job does not begin: it keeps SUBMITTED
                // and nothing is due from it while the pause lasts.
                job.settles_at = None;
                return;
            }
```

Test coverage: the hopping suite deliberately prevents a job from ever reaching `ERROR` while its hops are live, so no existing row exercises case 1 — `services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/queue_hopping.rs:45-47`:
```
/// Longer than any row advances the clock by, so no row's job leaves `SUBMITTED` while its
/// hops are being materialised: the rows are about hopping, not about the settle span.
const HELD_IN_SUBMITTED: Duration = Duration::from_secs(100 * 24 * 60 * 60);
```
`tests/all/queue_pause_gating.rs` covers the pause/unpause behaviour (which is unaffected, because `settle_job`'s `held` branch repairs the state), and `tests/all/job_lifecycle.rs` covers the `SUBMITTED` → `ERROR` transition — neither asserts anything about `hops_at`/`pending_hops`, so both cases are currently untested.

## Steps to Reproduce

1. Create a custom queue and leave it ACTIVE.
2. `CreateJob` on that queue with `hopDestinations: [{ "queue": "<another-queue>", "waitMinutes": 1 }]`. `accept_job` stores `settles_at = now + 750ms` and `hops_at = now + 60s`, inserting one entry into `pending` and one into `pending_hops`.
3. Wait past the settle span (or hand `settle` an instant 1s ahead, as `tests/all/queue_hopping.rs::advance` does) and read the job: it is now `ERROR`.
4. Inspect the stored record through `engine.state::<ComAmazonawsMediaconvertMediaconvertState>()`: the job's `hops_at` is still `Some(created_at + 60s)` and `pending_hops` still holds `(created_at + 60s, <job id>)`, although `hops_at`'s doc says it must be `None` "once the job has left `SUBMITTED`" and `cancel_job` clears exactly these two for the other terminal transition.
5. For the second case: `UpdateQueue` a custom queue to `status: PAUSED`, then `CreateJob` on it. Inspect the stored record immediately: `settles_at` is `Some(now + 750ms)` with a matching `pending` entry, although `settles_at`'s doc says it must be `None` "while the job's queue is `PAUSED` and nothing is therefore due at all".
