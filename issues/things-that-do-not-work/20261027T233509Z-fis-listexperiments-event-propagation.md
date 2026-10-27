Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# ListExperiments settles the run's END without materialising the report generation that end makes due, so the S3 experiment report is never written on that path

_Filed: 2026-10-27T23:35:09Z_
_Operation: ListExperiments | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An experiment started from a template carrying an `experimentReportConfiguration` owes the clock TWO writes once its run ends: the terminal status (`ExperimentRecord::settle`) and the report generation that writes the report object to the configured Amazon S3 output (`materialise_experiment_report`). The crate states both are owed on the same terms — "the generation was due at `now` whether or not anybody called".

`GetExperiment` honours both: it calls `materialise_experiment_report` and then settles. `StartExperiment` honours both for an idempotency replay. `ListExperiments` honours only the first: it calls `store.settle_experiments(now)` — which is the call that MATERIALISES the end of every run that elapsed, i.e. exactly the event that makes the report generation due — and never calls the helper the crate already has for the second write. The same skip exists in `StopExperiment`, whose `settled_experiment_mut` can end the run before the handler refuses the stop.

Consequence: a client that polls `ListExperiments` for the terminal status (the natural poll for "are my experiments done?") sees `state.status: completed`, but the report object the configuration names is never written to the bucket and the experiment's `experimentReport.state.status` stays `pending` — until somebody happens to call `GetExperiment`, which then writes it. Two callers at the same instant therefore disagree about whether the report of an ended experiment exists, and the cross-service write to the emulator's own S3 service is skipped on the path that actually observed the end.

This is not about the report's content or about any cap: it is one reporting path skipping a helper its sibling reads already call for the same clock-owed write.

## Evidence

The rule the crate states for the generation, in the helper's own doc:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:4980-4984`
```
/// The SECOND write a reporting operation performs, beside
/// [`ExperimentRecord::settle`], and owed on the same terms: FIS generates the experiment
/// report once the experiment has ENDED, so the generation was due at `now` whether or not
/// anybody called, and two reads at the same instant report the same report state. An
/// experiment that configured no report owes nothing and nothing is written for it.
```

and in the module header:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:26-27`
```
//! not ended, and then — once the clock has ended the run, which is when FIS generates the
//! report — the report is WRITTEN to the Amazon S3 output the configuration names, by
```

The path that DOES it — `GetExperiment`:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/get_experiment.rs:164-165`
```
        crate::state::materialise_experiment_report(&*self.engine, ctx, &region, &input.id, now)
            .await;
```

The path that does NOT — `ListExperiments`, which settles every experiment (ending the runs that elapsed) and only drains log deliveries:

`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_experiments.rs:166-199`
```
        let (experiments, next_token, deliveries) = {
            let mut store = state.lock();
            store.settle_experiments(now);
...
            let deliveries = store.take_pending_log_deliveries();
```
There is no `materialise_experiment_report` call anywhere in `list_experiments.rs` (nor in `stop_experiment.rs`, whose `settled_experiment_mut(&input.id, now)` at `src/ops/stop_experiment.rs:155` can likewise end the run first).

The claim in the ListExperiments header that only one write is performed states what the handler does, not a reason it may skip the second:
`services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_experiments.rs:8-10`
```
//! next page starts at, and it arrives only when experiments remain. Reporting, with the
//! one write [`crate::state::ExperimentRecord::settle`] performs — so a status listed
//! here agrees with the status `GetExperiment` reports at the same instant.
```

Test coverage: `tests/all/experiment_state_reason_and_report.rs` exercises the report generation through `GetExperiment`; no test covers the report after an end materialised by `ListExperiments`.

## Steps to Reproduce

1. Create an S3 bucket in the emulator, then CreateExperimentTemplate with experimentReportConfiguration.outputs.s3Configuration.bucketName naming it (plus a short action, e.g. aws:fis:wait with duration PT1M).
2. StartExperiment from that template.
3. Poll ONLY ListExperiments until the experiment's state.status is terminal (completed).
4. List the bucket (s3:ListObjectsV2): no `<prefix>/<experimentId>/experiment-report.json` object exists, even though the run has ended.
5. Now call GetExperiment once for the same id: the object appears and experiment.experimentReport.state.status flips from `pending` to `completed` — proving the generation was owed at the instant ListExperiments already materialised the end.
