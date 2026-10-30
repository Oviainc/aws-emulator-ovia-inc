Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# The enabled-baseline drift-summary projection is written out twice, once per reporting handler

_Filed: 2026-10-28T14:44:09Z_
_Operation: GetEnabledBaseline | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The projection that turns an enablement's stored `inheritance_drift_status` into the modelled `EnabledBaselineDriftStatusSummary` (`driftStatusSummary.types.inheritance.status`, absent when nothing is assessed) is written out twice, byte-identically, in `get_enabled_baseline.rs` and `list_enabled_baselines.rs`. Nothing ties the two copies together: a change to how the stored status becomes the reported member (a new drift type under `types`, a different absent/present rule) has to be made in both places, and the copies can silently drift so that `GetEnabledBaseline` and `ListEnabledBaselines` report different drift summaries for the SAME stored enablement. The list handler's own comment admits the coupling is maintained by hand — it says the member is "The drift summary `GetEnabledBaseline` reports for the same resource, read from the same stored status" — and an existing test exists purely to assert the two handlers agree.

This contradicts the discipline this crate states for every other rule two or more handlers share: `crate::state` holds the filter-matching rule and the pagination rule once each, precisely "so the rule lives here once rather than in each handler". The same duplication exists one level over for an enabled CONTROL's `DriftStatusSummary` (built from the three stored drift fields in both `get_enabled_control.rs` and `list_enabled_controls.rs`), so the fix is one shared projection per resource kind, in the place this crate already centralises shared rules.

The projection is pure — it reads only the stored status — so folding the copies into one helper leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1, `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_enabled_baseline.rs:208-219`:
```
                // Absent for an enablement whose inheritance drift is not assessed — a
                // baseline AWS detects no drift on, or a deployment the clock has not made
                // due yet — rather than reported as a status the model does not declare.
                drift_status_summary: enabled_baseline.inheritance_drift_status.map(|status| {
                    EnabledBaselineDriftStatusSummary {
                        types: Some(EnabledBaselineDriftTypes {
                            inheritance: Some(EnabledBaselineInheritanceDrift {
                                status: Some(status.as_wire().to_string()),
                            }),
                        }),
                    }
                }),
```

Copy 2, `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_enabled_baselines.rs:263-274` — the same body verbatim, with a comment that names the other copy as the thing it must match:
```
                // The drift summary `GetEnabledBaseline` reports for the same resource,
                // read from the same stored status: absent when no inheritance drift is
                // assessed for the enablement.
                drift_status_summary: enabled_baseline.inheritance_drift_status.map(|status| {
                    EnabledBaselineDriftStatusSummary {
                        types: Some(EnabledBaselineDriftTypes {
                            inheritance: Some(EnabledBaselineInheritanceDrift {
                                status: Some(status.as_wire().to_string()),
                            }),
                        }),
                    }
                }),
```

The crate's own stated rule for shared logic, `services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:61-66`:
```
//! The LIST operations over these stores all narrow their listing the same way, so the
//! rule lives here once rather than in each handler: [`Filter`] is one `filter` member as
//! a handler matches it, [`filter_set`] turns a sent member into that set and
//! [`selected`]/[`selected_optional`] test one stored record against it. Every list
//! operation of this crate reaches the rule through these, so narrowing by a filter
//! member cannot drift between them.
```

The same shape of duplication for a control's summary: `src/ops/get_enabled_control.rs:201-211` builds `DriftStatusSummary { drift_status, types: Some(EnabledControlDriftTypes { inheritance, resource }) }` from `control.drift_status` / `control.inheritance_drift_status` / `control.resource_drift_status`, and `src/ops/list_enabled_controls.rs:269-279` builds the identical structure from the same three fields.

Existing tests that pin the responses (and the agreement between the copies), so the fold must leave them unchanged:
- `tests/all/list_enabled_baselines.rs:267-268` asserts the listing's `driftStatusSummary` equals the detail's: `summary.get("driftStatusSummary").cloned(), detail.get("driftStatusSummary").cloned()`.
- `tests/all/enabled_baseline_drift.rs:101` `a_deployed_enablement_reports_the_inheritance_drift_of_its_baseline` and `tests/all/enabled_baseline_drift.rs:190` `the_listing_and_an_update_report_the_same_drift_summary`.
- `tests/all/in_flight_configuration_change_drift.rs:276` `a_baseline_change_in_flight_reports_no_drift_assessment` (asserts both the detail and the listing) and `:508` `a_control_change_in_flight_reports_unknown_drift`.
- `tests/all/enabled_control_lifecycle.rs:364,385` pin the control `driftStatusSummary` pair.

## Steps to Reproduce

1. grep -n 'EnabledBaselineDriftStatusSummary {' services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/*.rs — two construction sites are reported, get_enabled_baseline.rs:212 and list_enabled_baselines.rs:267.
2. Diff the two blocks (get_enabled_baseline.rs:211-219 against list_enabled_baselines.rs:266-274): the bodies are identical, including the `.map(|status| ...)` absent/present rule and the nesting under `types.inheritance.status`.
3. Change one copy only — e.g. report the member as present with no `types` — and observe that the sibling reporting operation keeps the old shape: nothing in the crate ties the two together, and only tests/all/list_enabled_baselines.rs:267 would notice.
4. Repeat for the control pair: grep -n 'DriftStatusSummary {' src/ops/get_enabled_control.rs src/ops/list_enabled_controls.rs — the same three stored fields are projected twice.
