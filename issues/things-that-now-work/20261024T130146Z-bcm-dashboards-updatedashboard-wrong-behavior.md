Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# UpdateDashboard silently erases the stored description when the member is omitted

_Filed: 2026-10-24T13:01:46Z_
_Operation: UpdateDashboard | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateDashboard treats its two OPTIONAL members inconsistently, and the treatment of `description` destroys persisted data.

Expected: an update that supplies only the @required members (`arn` + `name`) — the shape of a plain rename — leaves every optional member of the dashboard as it was stored, so a later GetDashboard still reports the description the create persisted. This service states its own convention for unsetting an optional member explicitly: the sibling update operation `UpdateScheduledReport` models `clearWidgetIds` and `clearWidgetDateRangeOverride` (boolean, @default false, required) precisely because an OMITTED optional member there means "leave unchanged" and clearing needs a dedicated flag. UpdateDashboard models no `clearDescription` flag at all, so omission cannot be the way a caller clears a description.

Actual: `src/ops/update_dashboard.rs` assigns `dashboard.description = input.description;` unconditionally, so a rename-only UpdateDashboard writes `None` over the stored description and the dashboard's description is gone for good (there is no way to recover it other than re-sending it). In the same handler the other optional member is handled the OPPOSITE way — `if let Some(widgets) = widgets { dashboard.widgets = widgets; }` — so an omitted `widgets` list is preserved while an omitted `description` is wiped. One of the two must be wrong; the clear-flag convention this service's model carries says the preserving branch is the correct one and the description branch is the divergence.

## Evidence

Spec: `UpdateDashboardRequest` members (smithy_query shape=UpdateDashboardRequest) — `arn` and `name` are @required, `description` and `widgets` are optional, and nothing in the operation doc ("Updates an existing dashboard's properties, including its name, description, and widget configurations.") says an omitted member is a clear. `UpdateScheduledReportRequest` in the SAME model declares `clearWidgetIds` and `clearWidgetDateRangeOverride` (boolean, @default false, required) — explicit clear flags are how this service unsets an optional member, and UpdateDashboard has none.

Handler: `src/ops/update_dashboard.rs` (handle body, lines ~233-237): `dashboard.name = input.name; dashboard.description = input.description; if let Some(widgets) = widgets { dashboard.widgets = widgets; }` — unconditional overwrite for description, conditional for widgets.

Tests: the divergence is currently PINNED rather than covered — `tests/all/dashboard_lifecycle.rs` case "name only, widgets absent" creates a dashboard with description "The original description", updates with only `arn`+`name`, and asserts at line 818-822 `description must be the updated one (absent clears it)` while asserting the widgets survive. So the existing suite locks in the inconsistency; closing this issue means changing that assertion, not adding one.

## Steps to Reproduce

1. CreateDashboard with name "Before Update", description "The original description" and one widget; note the returned arn
2. GetDashboard on that arn and observe description == "The original description"
3. UpdateDashboard with ONLY {arn, name: "After Update"} (both @required members, no description, no widgets)
4. GetDashboard on the arn again: expect name "After Update", description still "The original description" and the original widget; observe the description member absent entirely while the widgets were preserved
5. Contrast with UpdateScheduledReport's modelled clearWidgetIds / clearWidgetDateRangeOverride flags: this service clears an optional member only when a caller asks for it explicitly
