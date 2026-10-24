Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# ListMonitors state/nextToken refusal messages are Rust-debug formatted, not AWS-shaped

_Filed: 2026-10-24T05:26:27Z_
_Operation: ListMonitors | Class: error-response_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: when a request parameter is refused, the ValidationException `message` follows AWS's own construction. For an unrecognised enumerated value AWS renders the admissible set in its constraint template with bare values ("... failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, DELETING, ERROR, INACTIVE, PENDING]") and for a bad pagination cursor it uses its standard invalid-token sentence; in neither case does it quote the values or render a programming-language debug form.

Actual (emulator): `ListMonitors` builds its refusals with Rust `{:?}` formatting of internal constants, producing `state must be one of ["ACTIVE", "DELETING", "ERROR", "INACTIVE", "PENDING"], got "FOO"` (double-quoted values and a quoted echoed input — a Rust Debug artefact) and `nextToken does not name a page of this result set` for an unknown cursor. Status 400 and the error code are right; only the human/machine-readable message diverges, which is what SDK error-message assertions and recorded-trace comparisons key on. The same Rust-debug message construction is used for the probe `destination` refusal in `state.rs::validate_probe_spec` (`destination must be an IPv4 or IPv6 address, got "..."`) and the `destinationPort` floor message, so the fix belongs in the shared message construction rather than in one handler.

## Evidence

Spec: ListMonitors declares ValidationException (HTTP 400); `ListMonitorsInput$state` targets a plain string and `nextToken` a length-bounded string, so both refusals are authored by the handler rather than by the framework's constraint formatter. Handler: `src/ops/list_monitors.rs` handler lines ~171-177 (`format!("state must be one of {MONITOR_STATE_VALUES:?}, got {requested:?}")`) and lines ~199-215 (`nextToken does not name a page of this result set`); `MONITOR_STATE_VALUES` is a `&[&str]` in `src/ops/types/shared_types.rs`, so `{:?}` emits quoted elements. Prior art in this repo for the rendering rule: the fixed issues "networkmonitor validation-error messages diverge from AWS" and "NetworkFlowMonitor error messages not rendered in AWS's known forms (… sorted enum value set)" establish that AWS renders enum value sets without Rust quoting; neither covers ListMonitors. Tests: `tests/all/validation_message_wording.rs` pins the pattern/aggregation-period/TCP-port messages only — no test asserts these two.

## Steps to Reproduce

1. ListMonitors with query parameter state=FOO (a value outside MonitorState).
2. Observe HTTP 400 ValidationException with message `state must be one of ["ACTIVE", "DELETING", "ERROR", "INACTIVE", "PENDING"], got "FOO"` — quoted values and a quoted echoed input that AWS never emits.
3. ListMonitors with nextToken=notatoken (a cursor this service never vended).
4. Observe HTTP 400 ValidationException with message `nextToken does not name a page of this result set`, instead of AWS's invalid-next-token wording.
