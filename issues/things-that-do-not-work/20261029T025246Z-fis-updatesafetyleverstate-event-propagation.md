Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: a2d8365760a86e4759c409db3cefda62d7096263

# Engaging the safety lever settles every experiment — ending the runs that elapsed — without materialising the report generation that end makes due, so the S3 experiment report is never written on that path

_Filed: 2026-10-29T02:52:46Z_
_Operation: UpdateSafetyLeverState | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a2d8365760a86e4759c409db3cefda62d7096263`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An experiment started from a template carrying an `experimentReportConfiguration` owes the clock TWO writes once its run ends: the terminal status (`ExperimentRecord::settle`) and the report generation that writes the report document to the configured Amazon S3 output (`materialise_experiment_report`). The crate states both are owed on exactly the same terms — the generation "was due at `now` whether or not anybody called".

`UpdateSafetyLeverState` is the one operation of this service that settles EVERY experiment in the store: engaging the lever calls `ComAmazonawsFisFaultinjectionsimulatorState::stop_experiments_for_safety_lever`, whose first act per experiment is `record.settle(now)`. That settle is exactly the call that MATERIALISES the end of every run whose span had already elapsed — i.e. precisely the event that makes the report generation due. The handler then honours the OTHER clock-owed cross-service write for those same experiment moves (it drains the log outbox and writes the records to CloudWatch Logs / S3, which its own module header promises) but never calls `materialise_experiment_report`: that helper appears nowhere in `update_safety_lever_state.rs`.

Consequence: a client that engages the safety lever (the natural "stop everything" control) ends up with experiments reported `completed`/`failed` by the very call it made, while `experiment.experimentReport.state.status` stays `pending` and no `<prefix>/<experimentId>/experiment-report.json` object exists in the configured bucket — until somebody happens to call `GetExperiment`, which then writes it. Two callers at the same instant therefore disagree about whether the report of an ended experiment exists, and the cross-service write is skipped on the path that actually observed the end.

This is not about the report's content and not about any cap: it is one path that settles skipping a helper the crate already has for the second write that settle makes due, while its sibling read performs both. (A sibling issue already names `ListExperiments` and `StopExperiment` for the same skip; this handler — the only store-WIDE settle besides `ListExperiments`, and the one that also forces stops — is named in neither.)

## Evidence

The rule the crate states for the generation, in the helper's own doc:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:5237-5241`
```
/// The SECOND write a reporting operation performs, beside
/// [`ExperimentRecord::settle`], and owed on the same terms: FIS generates the experiment
/// report once the experiment has ENDED, so the generation was due at `now` whether or not
/// anybody called, and two reads at the same instant report the same report state. An
/// experiment that configured no report owes nothing and nothing is written for it.
```

and the generation is gated on exactly the terminal status that settle produces:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:1884-1887`
```
    pub fn claim_report_generation(&mut self, now: SystemTime) -> Option<ReportGeneration> {
        if !self.status.is_terminal() || !self.report.as_ref()?.claimable_at(now) {
            return None;
        }
```

What `UpdateSafetyLeverState` does to the store — a settle of EVERY experiment:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:2457-2462`
```
    pub fn stop_experiments_for_safety_lever(&mut self, now: SystemTime) {
        for record in self.experiments.values_mut() {
            record.settle(now);
            record.request_stop_for(now, STOPPED_BY_SAFETY_LEVER);
        }
    }
```

called from the handler:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/update_safety_lever_state.rs:197-203`
```
            // Engaging the lever stops the experiments that are running. Done before the
            // lever's own state is written, and on the same clock a `StopExperiment` puts an
            // experiment on, so the lever reports `engaging` exactly while the experiments it
            // stopped are on their way to `stopped`.
            if engaging {
                store.stop_experiments_for_safety_lever(now);
            }
```

The handler explicitly takes responsibility for the cross-service writes those experiment moves make due — but only for the LOG half:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/update_safety_lever_state.rs:5-8`
```
//! Engaging the lever STOPS the experiments that are running, which is a move of each
//! stopped experiment's own lifecycle — so this call also delivers those experiments'
//! records to the destinations their templates configured. See
//! [`crate::state::experiment_logs`].
```
`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/update_safety_lever_state.rs:211-219`
```
            let deliveries = store.take_pending_log_deliveries();
            (safety_lever, deliveries)
        };
...
        crate::state::experiment_logs::deliver(ctx, &region, deliveries).await;
```
A grep of `update_safety_lever_state.rs` for `materialise_experiment_report` returns nothing; the only two call sites in the crate are `src/ops/get_experiment.rs:164` and `src/ops/start_experiment.rs:447`.

The sibling that honours BOTH writes:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/get_experiment.rs:164-165`
```
        crate::state::materialise_experiment_report(&*self.engine, ctx, &region, &input.id, now)
            .await;
```

Test coverage: `tests/all/safety_lever.rs` exercises the lever and the stops it performs, and `tests/all/experiment_state_reason_and_report.rs` exercises the report generation through `GetExperiment`; no test covers the report after an end materialised by `UpdateSafetyLeverState`.

## Steps to Reproduce

1. Create an S3 bucket in the emulator, then CreateExperimentTemplate with experimentReportConfiguration.outputs.s3Configuration.bucketName naming it (and a short action, e.g. aws:fis:wait with duration PT1M).
2. StartExperiment from that template.
3. Wait until the run's span has elapsed on the emulator's compressed run clock, but call NOTHING in the meantime (no GetExperiment, no ListExperiments).
4. Call UpdateSafetyLeverState with state.status=engaged. The response is 200 and the experiment is now terminal — the settle inside stop_experiments_for_safety_lever materialised the end of the run.
5. List the bucket (s3:ListObjectsV2): no `<prefix>/<experimentId>/experiment-report.json` object exists, even though the run has ended.
6. Now call GetExperiment once for the same id: the object appears and experiment.experimentReport.state.status flips from `pending` to `completed` — proving the generation was owed at the instant UpdateSafetyLeverState already materialised the end.
