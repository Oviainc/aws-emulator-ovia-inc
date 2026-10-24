Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 19809ff85dacb8a5011e45ecfee3e76273a4f2a9

# ResourceNotFoundException never carries its modelled Reason (RULE_NOT_FOUND)

_Filed: 2026-10-24T09:26:42Z_
_Operation: GetRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `19809ff85dacb8a5011e45ecfee3e76273a4f2a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every rule-addressed operation of this service answers an unknown rule with a 404 `ResourceNotFoundException` whose body carries only `Message`. The modelled error has a second member, `Reason`, targeting `ResourceNotFoundExceptionReason`, whose value set is exactly `[RULE_NOT_FOUND]` — i.e. the enum exists for precisely this one condition (the service has exactly one resource, a retention rule), so a 404 raised because the addressed rule does not exist is the only path that can populate it.

Expected: `GET /rules/{Identifier}` for an identifier no rule holds answers HTTP 404 with a body carrying both `Message` and `Reason: "RULE_NOT_FOUND"` — the same way this emulator already populates `ConflictException.Reason = INVALID_RULE_STATE` and `ValidationException.Reason = INVALID_PARAMETER_VALUE`, both single-purpose reason enums in the same model.

Actual: `crate::state::rule_not_found` sets `reason: None` for every 404 (GetRule, DeleteRule, UpdateRule, LockRule, UnlockRule, TagResource, UntagResource, ListTagsForResource), so clients that branch on `Reason` see it absent on the only path that can produce `RULE_NOT_FOUND`.

## Evidence

Spec: Smithy model `com.amazonaws.rbin#ResourceNotFoundException` has members `Message` and `Reason` -> `ResourceNotFoundExceptionReason` with values `[RULE_NOT_FOUND]` (smithy_query query=shape shape=ResourceNotFoundException,ResourceNotFoundExceptionReason). The sibling reason enums `ConflictExceptionReason = [INVALID_RULE_STATE]` and `ServiceQuotaExceededExceptionReason = [SERVICE_QUOTA_EXCEEDED]` are populated by this emulator's own handlers, so the 404 is the only modelled reason it leaves unset.

Handler: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs` — `pub fn rule_not_found(addressed: &str) -> ResourceNotFoundException { ResourceNotFoundException { message: Some(...), reason: None } }`, used by `src/ops/get_rule.rs` and every other rule-addressed handler.

Test coverage: the omission is deliberately pinned by `tests/all/retention_rule_fidelity.rs::an_absent_rule_answers_the_not_exist_message_composed_from_the_request`, which asserts `body.get("Reason").is_none_or(Value::is_null)` on the strength of an in-code claim that "Recycle Bin does not populate the modelled ResourceNotFoundExceptionReason on this path" — a claim with no citation in the model or the service documentation. If a recorded trace confirms the real service sends `Reason: RULE_NOT_FOUND`, both the helper and that assertion need to change together.

## Steps to Reproduce

1. GET /rules/0000000000x for an identifier no rule holds — observe HTTP 404 with a body containing only "Message".
2. Compare with a refused lock: PATCH /rules/{Identifier}/lock on a tag-level rule — the 409 body carries "Reason":"INVALID_RULE_STATE", showing the emulator populates every other single-value reason enum in this model.
3. Repeat the 404 through DELETE /rules/{Identifier}, PATCH /rules/{Identifier}, GET /tags/{ResourceArn} — every path omits "Reason":"RULE_NOT_FOUND".
