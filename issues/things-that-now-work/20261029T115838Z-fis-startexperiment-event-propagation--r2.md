Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: fb07764462a6e3726541f141027927b0567b370d

# StartExperiment's store-wide settle ends other experiments' runs while its own comment claims "neither reaches" a report generation, so their S3 reports are never written on that path

_Filed: 2026-10-29T11:58:38Z_
_Operation: StartExperiment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `fb07764462a6e3726541f141027927b0567b370d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A fresh `StartExperiment` settles EVERY experiment in the store before it takes its quotas, which is the call that materialises the END of every run whose span had already elapsed. Ending a run is exactly what makes the report generation due (`claim_report_generation` is gated on `status.is_terminal()`), and the crate states that generation is owed "whether or not anybody called". But the handler calls `materialise_experiment_report` only for the id an idempotency REPLAY resolved to — and its own comment asserts that a start which mints a new experiment owes nothing, "so neither reaches it". That assertion is false for the OTHER experiments the same handler's sweep just ended: their terminal status is materialised by this call, their log records are drained and delivered by this call, and their report object is never written.

Consequence: a client whose only FIS traffic is StartExperiment (start A; later start B) sees experiment A reported `completed`/`failed` by the call that started B, while `A.experimentReport.state.status` stays `pending` and no `<prefix>/<A>/experiment-report.json` object exists in the configured bucket — until somebody happens to call GetExperiment, which then writes it. The cross-service write is skipped on the very path that observed the end.

This is NOT the already-filed gap: the open ListExperiments issue names only ListExperiments and StopExperiment, and the open UpdateSafetyLeverState issue calls that handler "the only store-WIDE settle besides ListExperiments" while asserting "StartExperiment honours both for an idempotency replay". Both issues therefore treat StartExperiment as a path that honours the rule; the store-wide sweep inside `start_under_guard` is named in neither, and the handler's own comment is the second side of the contradiction.

## Evidence

The handler's comment claiming nothing is owed on the fresh path — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:438-449`:
```
        // A replay of a token that already started an experiment reports THAT experiment,
        // so the report generation the clock owes it is materialised first: the generation
        // was due at `now` whether or not anybody called, ...
        // guard; a token that started nothing owes nothing, and a start that mints a new
        // experiment has not ended, so neither reaches it.
        if let Some(id) = self.replayed_experiment_id(&input, &actions_mode) {
            crate::state::materialise_experiment_report(&*self.engine, ctx, &region, &id, now)
                .await;
        }
```

The code that contradicts it — the same handler settles the WHOLE store, deliberately to end runs the clock already finished — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:221-226`:
```
        let mut store = state.lock();

        // Settle first: an experiment whose run already elapsed is no longer active, so
        // the quota below is taken against the experiments that are genuinely running
        // and not against ones the clock finished before this call.
        store.settle_experiments(now);
```

which is the store-wide sweep — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:2386-2390`:
```
    pub fn settle_experiments(&mut self, now: SystemTime) {
        for record in self.experiments.values_mut() {
            record.settle(now);
        }
    }
```

The rule the crate states for the second write — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:5237-5241`:
```
/// The SECOND write a reporting operation performs, beside
/// [`ExperimentRecord::settle`], and owed on the same terms: FIS generates the experiment
/// report once the experiment has ENDED, so the generation was due at `now` whether or not
/// anybody called, and two reads at the same instant report the same report state. An
/// experiment that configured no report owes nothing and nothing is written for it.
```
and it is gated on exactly the status that settle produces — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:1884-1887`:
```
    pub fn claim_report_generation(&mut self, now: SystemTime) -> Option<ReportGeneration> {
        if !self.status.is_terminal() || !self.report.as_ref()?.claimable_at(now) {
            return None;
        }
```

The handler DOES honour the other clock-owed cross-service write for those same ended runs, which is what shows the asymmetry — `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/start_experiment.rs:385-386` and `454-456`:
```
        let deliveries = store.take_pending_log_deliveries();
...
        crate::state::experiment_logs::deliver(ctx, &region, deliveries).await;
```

The two existing issues that exclude this path, by their own text: `20261029T025246Z-fis-updatesafetyleverstate-event-propagation.md` — "this handler — the only store-WIDE settle besides `ListExperiments`" and "the only two call sites in the crate are `src/ops/get_experiment.rs:164` and `src/ops/start_experiment.rs:447`"; `20261027T233509Z-fis-listexperiments-event-propagation.md` — "`StartExperiment` honours both for an idempotency replay".

Test coverage: `tests/all/start_experiment_replay.rs` covers the report only on the REPLAY path (its `report_key` rows at :106-130); `tests/all/experiment_state_reason_and_report.rs` covers it through `GetExperiment`. No test covers the report of an experiment whose end was materialised by a FRESH `StartExperiment` of a different experiment.

## Steps to Reproduce

1. Create an S3 bucket in the emulator.
2. CreateExperimentTemplate T1 with experimentReportConfiguration.outputs.s3Configuration.bucketName naming that bucket (plus a short action, e.g. aws:fis:wait with duration PT1M).
3. StartExperiment from T1 -> experiment A.
4. Wait until A's run span has elapsed on the emulator's compressed run clock, calling NOTHING in the meantime (no GetExperiment, no ListExperiments).
5. StartExperiment again with a FRESH clientToken (from T1 or any other template) -> experiment B. The sweep at start_experiment.rs:226 settles A, so A is now terminal.
6. GetExperiment(A): state.status is terminal (completed), but experimentReport.state.status is still `pending`.
7. ListObjectsV2 on the bucket: no `<prefix>/A/experiment-report.json` object exists even though A's run ended.
8. Call GetExperiment(A) once more: the object appears and experimentReport.state.status flips to `completed` — proving the generation was owed at the instant the fresh StartExperiment already materialised A's end.
