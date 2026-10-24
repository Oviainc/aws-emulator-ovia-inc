Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a651a577464eaac3ffa428a0c9cc2aa4b0e9484e

# GetEnabledBaseline never reports driftStatusSummary for an OU-level enabled baseline

_Filed: 2026-10-24T08:24:49Z_
_Operation: GetEnabledBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a651a577464eaac3ffa428a0c9cc2aa4b0e9484e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetEnabledBaseline hardcodes `drift_status_summary: None` (and `parent_identifier: None`), so the `enabledBaselineDetails` body never carries a `driftStatusSummary` member no matter how long the enablement has been SUCCEEDED.

Expected (real AWS): once an EnabledBaseline is deployed, GetEnabledBaseline reports its drift summary — `"driftStatusSummary": {"types": {"inheritance": {"status": "IN_SYNC"}}}` for a baseline enabled at the OU level (the model documents "Amazon Web Services Control Tower detects inheritance drift on enabled baselines that apply at the OU level"), and a freshly enabled baseline whose configuration matches the OU's is IN_SYNC. Clients that read `enabledBaselineDetails.driftStatusSummary.types.inheritance.status` to decide whether a baseline needs re-enabling get nothing back from the emulator.

The emulator already computes exactly this kind of derived drift for control enablements — `EnableControl` seeds UNKNOWN and `state.settle` moves the three drift statuses to IN_SYNC when the enablement becomes due (src/state.rs:576-592), and GetEnabledControl reports them — so the enabled-baseline path is inconsistent with the enabled-control path for the same concept.

## Evidence

Spec/model: `EnabledBaselineDetails$driftStatusSummary` -> `EnabledBaselineDriftStatusSummary{types -> EnabledBaselineDriftTypes{inheritance -> EnabledBaselineInheritanceDrift{status}}}`, documented as "The drift status of the enabled baseline" / "Amazon Web Services Control Tower detects inheritance drift on enabled baselines that apply at the OU level". `EnabledBaselineSummary` carries the same member, and AWS's GetEnabledBaseline example response includes driftStatusSummary.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_enabled_baseline.rs lines 193-222 — `drift_status_summary: None, ... parent_identifier: None` with the comment "No drift is assessed and no parent enablement is modelled". State: `crate::state::EnabledBaseline` stores no drift field at all (src/state.rs, enabled-baseline record), so there is nothing for `settle` to materialise, unlike `EnabledControl` which does carry drift statuses (src/state.rs:576-592).

Tests: grepping tests/all for `driftStatusSummary` matches only enabled_control_lifecycle.rs — no enabled-baseline test asserts the member in either direction, so the behaviour is unpinned. No existing open/fixed issue covers GetEnabledBaseline's response body (the only GetEnabledBaseline entry in the backlog is a stale stub issue for an operation that is now implemented).

## Steps to Reproduce

1. POST /enable-baseline with a baseline ARN, version and an OU targetIdentifier; note the returned EnabledBaseline `arn`.
2. Wait past the operation settle span so the enablement reports statusSummary.status = SUCCEEDED.
3. POST /get-enabled-baseline with {"enabledBaselineIdentifier":"<arn>"} — the emulator's `enabledBaselineDetails` has no `driftStatusSummary` member at all; real AWS returns `driftStatusSummary.types.inheritance.status` (IN_SYNC for a newly enabled OU-level baseline).
