Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 59eb119668c46536c53893c00cd680650c32abdf

# The patient-insights-job clock keeps transitioning jobs under a domain whose deletion has already been recorded, which state.rs says it must not

_Filed: 2026-10-28T22:48:11Z_
_Operation: StartPatientInsightsJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `59eb119668c46536c53893c00cd680650c32abdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsConnecthealthConnecthealthState::advance_due_patient_insights_jobs` walks `self.domains.values_mut()` with NO liveness filter, so it applies due status transitions (and fills `insights_output`, and moves `updated_at`) to jobs held under a domain that a `DeleteDomain` has already taken out of `ACTIVE`.

That contradicts the rule the same module states for exactly these records. `Domain::is_live`'s doc says that from the instant a delete is recorded the domain "is gone as far as every reader of what it CONTAINS — its subscriptions and the sessions and jobs under them — and every transition on them is concerned", and it names itself the ONE place that rule is stated, reached through `live_domain`/`live_domain_mut`. The sibling write path honours it for the other contained record kind: `DeleteDomain` ends every subscription's lifecycle in the same call that records the deletion (`subscription.status = SubscriptionStatus::Deleted`, "The subscriptions go with the domain"). Nothing does the equivalent for the jobs, and the clock helper skips the `is_live()`/`live_domain` helper the crate already has.

So after `DeleteDomain` on a domain with a `webAppSetupConfiguration` (which stays in `DELETING` for `DOMAIN_DELETION_SPAN` = 500 ms, longer than `PATIENT_INSIGHTS_JOB_STEP_SPAN` = 400 ms), the next `StartPatientInsightsJob`/`GetPatientInsightsJob` call in ANY domain advances that doomed domain's jobs — a `SUBMITTED` job can be driven to `IN_PROGRESS` and even to `SUCCEEDED` with a derived `insightsOutput` URI, for a domain the service is tearing down ("Deletes a Domain and all associated resources"). Severity is LOW because no reader can project the result today: `GetPatientInsightsJob` resolves through `live_domain`, so the record is unreadable until the purge drops it. The defect is the write itself — a transition the crate's own stated rule forbids, performed on state that a recorded deletion has retired.

## Evidence

The rule, in the state module that declares itself its single home — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:360-376`:
```
    /// Whether this record is still live: a `DeleteDomain` already recorded against it
    /// has taken it out of `ACTIVE`, and from that instant it is gone as far as every
    /// reader of what it CONTAINS — its subscriptions and the sessions and jobs under
    /// them — and every transition on them is concerned.
    ///
    /// The ONE place this rule is stated, beside [`Subscription::is_live`]: every reader
    /// that needs a live domain goes through it
    /// ([`ComAmazonawsConnecthealthConnecthealthState::live_domain`], ...
    pub fn is_live(&self) -> bool {
        self.status == DomainStatus::Active
    }
```

The code that breaks it — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:984-990` (no `is_live()` / `live_domain` filter anywhere):
```
    pub fn advance_due_patient_insights_jobs(&mut self, now: SystemTime) {
        for domain in self.domains.values_mut() {
            for job in domain.patient_insights_jobs.values_mut() {
                job.apply_due_transitions(now);
            }
        }
    }
```

The transition it performs, including the terminal write of `insightsOutput` — `services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:627-641`:
```
                JobStatus::InProgress => {
                    self.status = JobStatus::Succeeded;
                    self.insights_output = Some(InsightsOutput {
                        uri: patient_insights_output_uri(
```

The sibling treatment that DOES stop a contained record's lifecycle in the deleting call — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/delete_domain.rs:377-381`:
```
            // The subscriptions go with the domain ("and all associated resources").
            for subscription in domain.subscriptions.values_mut() {
                subscription.status = crate::state::SubscriptionStatus::Deleted;
                subscription.last_updated_at = now;
            }
```

The call site in this operation's handler — `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/start_patient_insights_job.rs:210-214`:
```
        // What the CLOCK already made due, before anything is resolved or written: a
        // domain whose DELETING span has elapsed is purged, and every job whose next
        // status transition has fallen due is moved on.
        store.purge_due_deletions(now);
        store.advance_due_patient_insights_jobs(now);
```

The window in which it happens, from the crate's own constants — `src/state.rs:50` (`DOMAIN_DELETION_SPAN: Duration = Duration::from_millis(500)`) and `src/state.rs:125` (`PATIENT_INSIGHTS_JOB_STEP_SPAN: Duration = Duration::from_millis(400)`).

Test coverage: `tests/all/start_patient_insights_job_write_discipline.rs:62-71` deliberately excludes the status and last-updated instants from its store snapshot ("the status and last-updated instant the CLOCK owns are deliberately left out"), and `tests/all/patient_insights_job_lifecycle.rs` drives transitions only under a live domain — so no test pins what the clock does to a job whose domain is in `DELETING`.

## Steps to Reproduce

1. POST /domain with a webAppSetupConfiguration (ehrRole, idcInstanceId, idcRegion) so the domain's deletion cannot complete inside the DeleteDomain call; capture domainId.
2. POST /domain/{domainId}/patient-insights-job with the required members (patientContext, insightsContext, encounterContext, userContext, inputDataConfig, outputDataConfig) and capture jobId; the stored job is SUBMITTED with next_transition_at = now + 400 ms.
3. Immediately DELETE /domain/{domainId}: the response reports status DELETING and the record stays in the store for DOMAIN_DELETION_SPAN (500 ms), with every subscription of the domain already marked DELETED.
4. Wait ~450 ms (past PATIENT_INSIGHTS_JOB_STEP_SPAN, inside DOMAIN_DELETION_SPAN) and issue any StartPatientInsightsJob or GetPatientInsightsJob request (it may address a different, live domain).
5. Inspect the shared store (as tests/all/start_patient_insights_job_write_discipline.rs does): the job under the DELETING domain has been advanced to IN_PROGRESS with updated_at rewritten — a transition on a record contained by a domain that state.rs:360-363 says is already gone for every transition, while the sibling subscriptions were stopped by DeleteDomain.
