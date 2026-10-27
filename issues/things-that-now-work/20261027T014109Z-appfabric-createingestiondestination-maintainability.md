Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: bed9f8e2d85c054efb957ff95014081869f003aa

# The ingestion-destination `Active` status is written as a bare literal in two handlers, against this crate's own rule of naming every modelled state constant in state.rs

_Filed: 2026-10-27T01:41:09Z_
_Operation: CreateIngestionDestination | Class: maintainability_

> _Note: this issue was identified against the code base at commit `bed9f8e2d85c054efb957ff95014081869f003aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

state.rs names every other modelled enum state it writes as a constant and states why: so the handlers that write a state and the ones that read it "cannot drift apart over a bare literal" (INGESTION_STATE_ENABLED/DISABLED, APP_AUTHORIZATION_STATUS_PENDING_CONNECT/CONNECTED, RESULT_STATUS_IN_PROGRESS/FAILED). The `IngestionDestinationStatus` rule — an accepted destination is `Active` with no `statusReason` — is instead written twice as a bare string literal, once in CreateIngestionDestination and once in UpdateIngestionDestination, so the two copies can drift (and neither is tied to the generated INGESTION_DESTINATION_STATUS_VALUES list). Pulling the one rule into a named constant in state.rs beside its siblings leaves every response byte and both existing assertions unchanged.

## Evidence

Rule stated in this crate — src/state.rs:38-46: "The two values the model's `IngestionState` enum carries ... Named here so the handlers that write the state and the ones that read it cannot drift apart over a bare literal." `pub const INGESTION_STATE_ENABLED: &str = "enabled";` / `pub const INGESTION_STATE_DISABLED: &str = "disabled";` (same pattern at state.rs:92 and 97 for the app-authorization statuses, and 158/162 for the result statuses).
Copy 1 — src/ops/create_ingestion_destination.rs:339-343: "// IngestionDestinationStatus = Active | Failed. A destination this emulator accepted is usable straight away, so it is stored in the terminal `Active` state and carries no status reason." then `status: "Active".to_string(), status_reason: None,`.
Copy 2 — src/ops/update_ingestion_destination.rs:224-228: "// A destination this emulator accepted is usable, so an accepted update leaves it in the terminal `Active` state with no status reason" then `destination.status = "Active".to_string(); destination.status_reason = None;`.
The modelled value list already exists as src/ops/types/shared_types.rs:239 `pub const INGESTION_DESTINATION_STATUS_VALUES: &[&str] = &["Active", "Failed"];`, which neither copy references.
Test coverage (must stay green and unchanged): tests/all/ingestion_destination_lifecycle.rs:426 "expected a created destination to be Active" and :463 "expected an accepted update to leave the destination Active".

## Steps to Reproduce

1. grep -n '"Active"' services/service-com-amazonaws-appfabric-fabricfrontendservice/src — two independent literals, in create_ingestion_destination.rs:342 and update_ingestion_destination.rs:227
2. grep -n 'pub const INGESTION_STATE_\|APP_AUTHORIZATION_STATUS_\|RESULT_STATUS_' services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs — every other written state is a named constant with the stated anti-drift rationale
3. Change one copy only (e.g. to "active") and observe that nothing in the crate ties the two together: only the two assertions in tests/all/ingestion_destination_lifecycle.rs catch the divergence, and only for the path that was changed
