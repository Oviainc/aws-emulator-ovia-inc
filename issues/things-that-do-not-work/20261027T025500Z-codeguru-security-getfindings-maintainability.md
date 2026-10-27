Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# The "nextToken is not a token this service issued" refusal is built three times instead of once in state.rs

_Filed: 2026-10-27T02:55:00Z_
_Operation: GetFindings | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All three paginated readers of this crate refuse a token they cannot decode by hand-constructing the identical `ValidationException` — same `errorCode`, same `message`, same `reason`, same single `ValidationExceptionField { name: "nextToken", message: ... }` — in three different files. The rule "a token this service did not issue is a `nextToken` field validation failure" is therefore implemented three times and the three copies can drift (one message reworded, one `reason` changed) while the others stay.

This crate already centralises exactly this kind of refusal in `crate::state`: `interval_order_refusal` is documented as "The ONE place the relation between the start and the end of a requested interval is decided", and `tag_quota_refusal` is "Shared by every writer of the tag store, so the bound is reported the same way whichever call reaches it". The token refusal is the one request-level refusal of this crate that has no such shared constructor, even though `encode_page_token`/`decode_page_token` — the helpers whose contract it reports — already live in `state.rs`.

Moving the construction into one `state.rs` helper (taking the member name) leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/get_findings.rs:188-201`:
```
                    return Err(GetFindingsErrorModeled::ValidationException(
                        ValidationException {
                            error_code: "ValidationException".to_string(),
                            message: "nextToken is not a token this service issued".to_string(),
                            reason: "fieldValidationFailed".to_string(),
                            field_list: Some(vec![ValidationExceptionField {
                                name: "nextToken".to_string(),
                                message:
                                    "pass back the nextToken from the previous page unchanged"
                                        .to_string(),
                            }]),
                        },
                    )
```
Copy 2 — `src/ops/list_scans.rs:188-198`:
```
                    return Err(ListScansErrorModeled::ValidationException(ValidationException {
                        error_code: "ValidationException".to_string(),
                        message: "nextToken is not a token this service issued".to_string(),
                        reason: "fieldValidationFailed".to_string(),
                        field_list: Some(vec![ValidationExceptionField {
                            name: "nextToken".to_string(),
                            message: "pass back the nextToken from the previous page unchanged"
                                .to_string(),
                        }]),
                    })
```
Copy 3 — `src/ops/list_findings_metrics.rs:360-371`:
```
                    return Err(ListFindingsMetricsErrorModeled::ValidationException(
                        ValidationException {
                            error_code: "ValidationException".to_string(),
                            message: "nextToken is not a token this service issued".to_string(),
                            reason: "fieldValidationFailed".to_string(),
                            field_list: Some(vec![ValidationExceptionField {
                                name: "nextToken".to_string(),
                                message: "pass back the nextToken from the previous page unchanged"
                                    .to_string(),
                            }]),
                        },
                    )
```
The shared-helper pattern this crate otherwise follows — `src/state.rs:963-975`: "The ONE place the relation between the start and the end of a requested interval is decided ... pub fn interval_order_refusal(" and `src/state.rs:227-229`: "Shared by every writer of the tag store, so the bound is reported the same way whichever call reaches it. pub fn tag_quota_refusal(". The token helpers already live beside them: `src/state.rs:1387-1397` (`encode_page_token` / `decode_page_token`).

Tests that pin the three responses and must stay green: `tests/all/scan_lifecycle.rs:1146-1169` (`ListScans(foreign nextToken)` and `GetFindings(foreign nextToken)` -> 400 `ValidationException`) and `tests/all/findings_metrics.rs:715-719` ("case {label:?}: an unissued nextToken is refused").

## Steps to Reproduce

1. Open `src/ops/get_findings.rs:188`, `src/ops/list_scans.rs:188` and `src/ops/list_findings_metrics.rs:360` side by side and compare the three `ValidationException` literals: all four fields are identical in each.
2. Compare with `src/state.rs:975` (`interval_order_refusal`) and `src/state.rs:229` (`tag_quota_refusal`), which the handlers call instead of building the refusal inline.
3. Change the message in one copy only and observe that the other two paginated operations keep the old wording, with no test catching the divergence.
