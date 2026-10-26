Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# An idempotency replay of StartExperiment reports a stale experimentReport: the clock-owed report generation is not materialised

_Filed: 2026-10-26T01:52:17Z_
_Operation: StartExperiment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartExperiment` is the one operation besides `GetExperiment` and `StopExperiment` that answers with the full `Experiment` shape, which carries `experimentReport`. On the `@idempotencyToken` REPLAY path it settles the store and then reports the already-started experiment straight out of the record:

    TokenReplay::Replay(id) => { if let Some(record) = store.experiments.get(&id) { let experiment = record.to_experiment(); ... return Ok(...) } }

It never calls `crate::state::materialise_experiment_report`, which is the write the clock owes an experiment that has ENDED. So when the retry arrives after the run elapsed, the replay reports `experimentReport.state.status: "pending"` with no `s3Reports`, and no report object has been written to the configured bucket — while a `GetExperiment` of the same experiment at the same instant reports the generation as `completed` with the object's ARN, because that handler does materialise it.

That is exactly the invariant the report machinery states for itself: the generation "was due at `now` whether or not anybody called", so two reads at the same instant must report the same report state. The replay IS a read of that experiment (it starts nothing), and it reports a different report state than the sibling read.

## Evidence

The replay path, no materialisation — services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:200-207:
```
            crate::state::TokenReplay::Replay(id) => {
                if let Some(record) = store.experiments.get(&id) {
                    let experiment = record.to_experiment();
                    // A replay starts nothing, so it produces no record of its own; what
                    // the settle above materialised is still delivered.
                    let deliveries = store.take_pending_log_deliveries();
                    return Ok((experiment, deliveries));
```
(the store was settled just above, at start_experiment.rs:188 `store.settle_experiments(now);`, so the replayed record CAN already be terminal), and `to_experiment` does report the generation — state.rs:1956-1959:
```
            experiment_report: self
                .report
                .as_ref()
                .map(ExperimentReportRecord::to_experiment_report),
```

The sibling read, which does materialise — src/ops/get_experiment.rs:13-17:
```
//! [`crate::state::materialise_experiment_report`] materialises the EXPERIMENT REPORT an
//! ended experiment that configured one owes — FIS generates it once the experiment has
//! ended, so it too was due whether or not anybody called. Two reads at the same instant
//! therefore report the same status and the same report, and this read changes nothing a
//! later caller would not have seen anyway.
```
and the helper's own doc — state.rs:4578-4582:
```
/// The SECOND write a reporting operation performs, beside
/// [`ExperimentRecord::settle`], and owed on the same terms: FIS generates the experiment
/// report once the experiment has ENDED, so the generation was due at `now` whether or not
/// anybody called, and two reads at the same instant report the same report state.
```
A grep of the crate shows the helper is called from exactly one place: `src/ops/get_experiment.rs:164`.

Test coverage: tests/all/experiment_state_reason_and_report.rs:655-662 pins "a second read must report the same generation" — but only through `get_experiment`; no test replays a `clientToken` after the run ended, so nothing pins the current divergence.

## Steps to Reproduce

1. Boot the emulator (with the object-storage service mounted) and point a `fis` client at it.
2. CreateExperimentTemplate with an `experimentReportConfiguration` whose `outputs.s3Configuration.bucketName` names an existing bucket, and a short `aws:fis:wait` action.
3. StartExperiment with clientToken='retry-me'; note the experiment id.
4. Wait until the run has ended on the emulator's compressed run clock.
5. Re-send the identical StartExperiment request with clientToken='retry-me' (the retry a client would send). Observe HTTP 200 with experiment.state.status terminal but experimentReport.state.status == 'pending' and no s3Reports, and no report object in the bucket.
6. Call GetExperiment on the same id at the same instant: experimentReport.state.status == 'completed' with s3Reports naming the written object — two reads of one experiment disagreeing about the report.
