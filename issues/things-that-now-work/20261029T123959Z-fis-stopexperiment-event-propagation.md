Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 09793af671f727f5831b9f0700c1238f4e9cbb7e

# StopExperiment's final-state refusal materialises the run's END but returns without draining the log outbox, so the experiment-end record is never delivered

_Filed: 2026-10-29T12:39:59Z_
_Operation: StopExperiment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `09793af671f727f5831b9f0700c1238f4e9cbb7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopExperiment` settles the named experiment FIRST (`store.settled_experiment_mut(&input.id, now)` at `src/ops/stop_experiment.rs:155`). When that settle is the call that materialises the run's end — an experiment whose `run_span` had already elapsed, or whose `stopping` transition had already fallen due — `ExperimentRecord::settle` moves the record into a terminal status and PRODUCES the `experiment-end` log record for it, parking it on the experiment's outbox (`state.rs:1807` -> `log_state_change` -> `log_event` -> `self.pending_log_records.push(...)` at `state.rs:1644`).

The handler then discovers the record is terminal and returns `ValidationException` from INSIDE the guard block (`src/ops/stop_experiment.rs:165-170`), so `store.take_pending_log_deliveries()` at line 173 and `experiment_logs::deliver(...)` at line 179 are both skipped. The records the settle just produced are left parked in the store, undelivered.

That contradicts three statements the crate makes about this write:
* the handler's own comment two lines above the block says the experiment's log records ARE written after the guard is released;
* `take_pending_log_deliveries`'s contract says to call it "right after settling, with the records the settle produced still parked";
* `experiment_logs`'s module header says a record is produced by the transition that makes it true and then "An operation that has just settled the store then drains the outbox ... and writes what it drained".

Every sibling handler that settles honours it: `GetExperiment` (`src/ops/get_experiment.rs:171-187`), `ListExperiments` (`src/ops/list_experiments.rs:165-192`), `UpdateSafetyLeverState` (`src/ops/update_safety_lever_state.rs:192-219`) and `StopExperiment`'s own success path all drain and deliver after the same settle. Only this refusal path does not.

Consequence: a client whose experiment template configured a `logConfiguration` and whose only FIS traffic is `StopExperiment` on an experiment that already finished gets a 400, and the `experiment-end` record for that run never reaches the configured CloudWatch Logs log group or S3 bucket — the destination is missing the terminal record of a run whose end this very call materialised, until some other operation happens to drain the outbox.

This is NOT the already-open report-generation skip. The open issue `20261027T233509Z-fis-listexperiments-event-propagation` names `stop_experiment.rs` only for the missing `materialise_experiment_report` call (the S3 experiment REPORT); the undelivered experiment LOG records on the refusal path are a different cross-service write and are named in no existing issue — indeed that issue cites `take_pending_log_deliveries` as the write `ListExperiments` already honours.

## Evidence

The refusal returns from inside the guard block, before the drain — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/stop_experiment.rs:149-179`:
```
        // The guard is taken, spent and RELEASED inside this block, so the experiment's
        // log records are written after it rather than while the store is locked.
        let (experiment, deliveries) = {
            let mut store = state.lock();
            // Settled first, so a stop lands on the status the clock already owed rather
            // than on a stale one.
            let Some(record) = store.settled_experiment_mut(&input.id, now) else {
...
            if record.status.is_terminal() {
                return Err(StopExperimentErrorModeled::ValidationException(
                    crate::state::final_state_refusal(&input.id),
                )
                .into());
            }
            record.request_stop(now);
            let experiment = record.to_experiment();
            let deliveries = store.take_pending_log_deliveries();
            (experiment, deliveries)
        };

        // The stop is part of the run, so it is one of the records the run delivers to the
        // destination the template configured.
        crate::state::experiment_logs::deliver(ctx, &region, deliveries).await;
```

That lookup settles, and the settle is what produces the end record — `src/state.rs:2373-2381`:
```
    pub fn settled_experiment_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ExperimentRecord> {
        let record = self.experiments.get_mut(id)?;
        record.settle(now);
        Some(record)
    }
```
`src/state.rs:1792-1807` (the `running` -> terminal branch of `settle`):
```
                ExperimentStatus::Running => {
...
                    self.end_time = Some(due);
                    self.transition_due = None;
                    self.log_state_change(due);
                }
```
and `log_state_change` parks the record — `src/state.rs:1748-1753`:
```
        let log_type = if terminal {
            experiment_logs::LOG_TYPE_EXPERIMENT_END
        } else {
            experiment_logs::LOG_TYPE_EXPERIMENT_STATE_CHANGE
        };
        self.log_event(log_type, at, details);
```
`src/state.rs:1644-1649`:
```
        self.pending_log_records.push(experiment_logs::LogRecord {
            log_type,
            at,
            details,
        });
        self.log_records_produced += 1;
```

The contract the refusal path breaks — `src/state.rs:2392-2397`:
```
    /// Take every experiment log record the store holds and has not delivered, as one
    /// delivery per experiment ([`experiment_logs::PendingDelivery`]).
    ///
    /// Call it right after settling, with the records the settle produced still parked,
    /// and then WRITE what it hands back with the state guard released
    /// ([`experiment_logs::deliver`]).
```
and `src/state/experiment_logs.rs:51-58`:
```
//! A record is PRODUCED by the same state transition that makes it true — the start that
//! mints the experiment, or [`crate::state::ExperimentRecord::settle`] materialising a
//! move the clock already owed — and parked in an outbox on the experiment record
//! ([`crate::state::ExperimentRecord::pending_log_records`], bounded by
//! [`MAX_PENDING_LOG_RECORDS`]). An operation that has just settled the store then drains
//! the outbox ([...take_pending_log_deliveries])
//! and writes what it drained, with the state guard already released.
```

The sibling that does it after the same settle — `src/ops/get_experiment.rs:171-187`:
```
        let (experiment, deliveries) = {
            let mut store = state.lock();
            let Some(record) = store.settled_experiment_mut(&input.id, now) else {
...
            let deliveries = store.take_pending_log_deliveries();
            (experiment, deliveries)
        };
...
        crate::state::experiment_logs::deliver(ctx, &region, deliveries).await;
```

Test coverage: `tests/all/experiment_run.rs:327-355` exercises the final-state refusal, but it calls `get_experiment(&router, id)` at line 328 FIRST, which settles and drains before the stop runs, so the refusal never performs the settle; and none of its rows configures a `logConfiguration`. `tests/all/experiment_logging.rs:695-751` covers only an ACCEPTED stop ("RULE: a stop is part of the run, so the destination reports it"). No test covers a stop whose own settle materialised the run's end.

## Steps to Reproduce

1. Create a CloudWatch Logs log group (or an S3 bucket) in the emulator.
2. CreateExperimentTemplate with a `logConfiguration` naming that destination plus `logSchemaVersion`, and one short action (e.g. `aws:fis:wait` with `duration` `PT1M`).
3. StartExperiment from that template and note the experiment id.
4. Wait until the run span has elapsed on the emulator's compressed run clock, calling NOTHING in the meantime (no GetExperiment, no ListExperiments, no other FIS call).
5. Call StopExperiment on that id. It answers HTTP 400 ValidationException ("Cannot modify the state of an experiment in final state: <id>.") — the settle inside `settled_experiment_mut` has just moved the experiment to `completed` and produced its `experiment-end` log record.
6. Read the configured destination (logs:GetLogEvents on the stream named after the experiment, or s3:ListObjectsV2 under the configured prefix): the `experiment-end` record is absent, although the call that just answered is the one that materialised the end.
7. Call GetExperiment on the same id once: the `experiment-end` record now appears at the destination, proving it had been produced and left parked by the refused stop.
