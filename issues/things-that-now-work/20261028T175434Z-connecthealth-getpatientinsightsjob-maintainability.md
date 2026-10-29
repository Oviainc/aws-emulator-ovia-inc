Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 492929116435f45e81bb91743fd26ee706bd36d7

# The clock-materialisation preamble is copied into all 15 handlers and has already drifted: 13 of them skip advance_due_patient_insights_jobs, which state.rs documents as running in every one

_Filed: 2026-10-28T17:54:34Z_
_Operation: GetPatientInsightsJob | Class: maintainability_

> _Note: this issue was identified against the code base at commit `492929116435f45e81bb91743fd26ee706bd36d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/state.rs` states ONE rule for what a handler does before it reads anything: materialise everything the CLOCK already made due. That rule has two halves — `purge_due_deletions` (a `Domain` whose `DELETING` span has elapsed) and `advance_due_patient_insights_jobs` (a job whose next status transition has fallen due) — and the module documents BOTH as running at the top of every handler. The rule is not implemented once, however: it is re-typed as a 2-line preamble inside each of the 15 handler bodies, and the two copies have already diverged. Thirteen handlers call only `purge_due_deletions`; the two patient-insights handlers call both. The doc on `advance_due_patient_insights_jobs` therefore states a crate-wide rule the code under it does not keep, which is exactly the drift a single-homed rule exists to prevent — and it is the kind of false invariant a later handler that reads job state would rely on.

The repair is to state the preamble once (e.g. one `materialise_due(now)` on `ComAmazonawsConnecthealthConnecthealthState` that both halves live in, called by every handler) so the two halves cannot be applied unevenly again, or to narrow the doc to the handlers that actually run it. Either way every response and every existing test stays byte-identical: `PatientInsightsJob::apply_due_transitions` stamps each transition with the instant it FELL DUE (`self.updated_at = due;`, state.rs:641) rather than the instant somebody looked, so advancing a job earlier in a sibling handler cannot change any value a later `GetPatientInsightsJob` reports. This is a maintainability finding only — no behaviour changes, which is why it is filed LOW.

## Evidence

Both halves are documented as universal, in the same module and in the same words:

- services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:958-959 (on `purge_due_deletions`): "/// Called at the top of every handler, including the reporting ones: it writes only /// what was owed at this instant whether or not anybody called."
- services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:980-983 (on `advance_due_patient_insights_jobs`): "/// Called at the top of every handler beside [`Self::purge_due_deletions`], including /// the reporting ones: it writes only what was owed at this instant whether or not /// anybody called, so a job observed late reads exactly as it would have at the /// moment its transition fell due."

The second sentence is false — only 2 of the 15 handlers in this crate call it (grep for `advance_due_patient_insights_jobs|purge_due_deletions` under src/):

- services/service-com-amazonaws-connecthealth-connecthealth/src/ops/get_patient_insights_job.rs:205-206 — `store.purge_due_deletions(now);` / `store.advance_due_patient_insights_jobs(now);` (both)
- services/service-com-amazonaws-connecthealth-connecthealth/src/ops/start_patient_insights_job.rs:213-214 — both
- purge only, no advance: get_domain.rs:204, list_domains.rs:181, list_subscriptions.rs:183, get_subscription.rs:161, create_domain.rs:289, create_subscription.rs:171, activate_subscription.rs:164, deactivate_subscription.rs:163, delete_domain.rs:327 (and :403), tag_resource.rs:214, untag_resource.rs:209, list_tags_for_resource.rs:168, get_medical_scribe_listening_session.rs:188

That the fix is response-neutral is pinned by the stamping rule the helper itself implements — services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:611-613: "Each transition is stamped with the instant it FELL DUE rather than the instant somebody happened to look, so the record reads the same however late the first read comes" — and state.rs:641 `self.updated_at = due;`.

Test coverage that must stay unchanged: tests/all/patient_insights_job_lifecycle.rs (status progression + clientToken idempotency), tests/all/patient_insights_job_terminal_output.rs (terminal `insightsOutput`), tests/all/start_patient_insights_job_write_discipline.rs ("A start is IDEMPOTENT on `clientToken` at every point of a job's life"), tests/all/domain_liveness_agreement.rs and tests/all/domain_response_projection.rs (the purge half, across the reporting handlers).

## Steps to Reproduce

1. Read services/service-com-amazonaws-connecthealth-connecthealth/src/state.rs:977-990 and note the doc sentence "Called at the top of every handler beside [`Self::purge_due_deletions`], including the reporting ones".
2. Run `grep -rn 'advance_due_patient_insights_jobs\|purge_due_deletions' services/service-com-amazonaws-connecthealth-connecthealth/src/ops/` and count: 15 handlers call `purge_due_deletions`, only get_patient_insights_job.rs:206 and start_patient_insights_job.rs:214 call `advance_due_patient_insights_jobs`.
3. Open any of the other 13 handlers (e.g. src/ops/get_domain.rs:201-204) and observe the preamble is re-typed per handler with only one of the two owed classes materialised.
4. Confirm the divergence is invisible on the wire (so a single-homed preamble is response-neutral): state.rs:620-642 stamps each transition at the instant it fell due, so `GetPatientInsightsJob` reports the same values whether or not a sibling handler advanced the job first.
