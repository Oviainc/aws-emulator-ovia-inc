Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4ea1754a6043dc600bbe02b1a1a49ea418ab762f

# The tagging "validate ARN → take store → materialise expiry → resolve or not-found" sequence is written out three times

_Filed: 2026-10-27T14:41:37Z_
_Operation: UntagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4ea1754a6043dc600bbe02b1a1a49ea418ab762f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

state.rs states ONE rule for every operation that resolves a caller-supplied deployment-parameter ARN: judge the ARN on the input, take the store, materialise the expiry the clock has already made due, then resolve the record or answer the service's not-found failure. That rule is not implemented once — it is spelled out verbatim in each of the three tagging handlers (`untag_resource.rs`, `tag_resource.rs`, `list_tags_for_resource.rs`), including the identical `service_state(...)`-is-`None` branch with the identical `"the deployment-parameter store is unavailable"` message, the identical single clock sample, the identical `purge_expired(now)` call placement relative to the lookup, and the identical `parameters.get*(&input.resource_arn)` → `parameter_not_found()` fallback.

The copies are what let one of them silently diverge from the rule state.rs states: nothing in the crate makes the three agree, so adding or reordering a step (the ARN judgement, the expiry application, the not-found answer) has to be remembered three times. The shared pieces that already exist (`validate_resource_arn`, `purge_expired`, `parameter_not_found`) cover the individual steps but not the SEQUENCE the three handlers each re-derive.

Fix: hoist the sequence into one helper in `crate::state` (take `&*self.engine` plus the request's `resourceArn`, answer the resolved record or the refusal), and have the three handlers call it. Every response — the 400 `BadRequestException`, the 404 `NotFoundException`, the 200/204 success — and every existing test stay unchanged, because the helper would be the same steps in the same order.

## Evidence

The rule, stated once in the state module:

src/state.rs:10-14 — "//! A record also carries the EXPIRY the last write named\n//! (`expirationDate`), and that date is enforced here: every operation that\n//! resolves a deployment parameter first materialises the deletion the clock has\n//! made due ([`...::purge_expired`]),\n//! so no caller ever sees a parameter past its expiration date."

src/state.rs:661-664 — "/// Every operation that resolves a deployment parameter\n/// calls this first, so no caller sees a parameter the clock has already taken:\n/// the tagging operations answer the service's not-found failure for it\n/// ([`parameter_not_found`])"

Copy 1 — src/ops/untag_resource.rs:160-193:
"        crate::state::validate_resource_arn(&input.resource_arn)?;" … "        let Some(state) = crate::state::service_state(&*self.engine) else {\n            return Err(UntagResourceErrorModeled::InternalServerException(\n                InternalServerException {\n                    message: \"the deployment-parameter store is unavailable\".to_string()," … "        let now = ::std::time::SystemTime::now();\n        let mut store = state.lock();" … "        store.purge_expired(now);\n\n        let Some(record) = store.parameters.get_mut(&input.resource_arn) else {" … "            return Err(crate::state::parameter_not_found());"

Copy 2 — src/ops/tag_resource.rs:176-220: "        crate::state::validate_resource_arn(&input.resource_arn)?;" … "        let Some(state) = crate::state::service_state(&*self.engine) else {\n            return Err(TagResourceErrorModeled::InternalServerException(\n                InternalServerException {\n                    message: \"the deployment-parameter store is unavailable\".to_string()," … "        let now = ::std::time::SystemTime::now();\n        let mut store = state.lock();" … "        store.purge_expired(now);\n\n        let Some(record) = store.parameters.get_mut(&input.resource_arn) else {" … "            return Err(crate::state::parameter_not_found());"

Copy 3 — src/ops/list_tags_for_resource.rs:163-187: "        crate::state::validate_resource_arn(&input.resource_arn)?;" … "        let Some(state) = crate::state::service_state(&*self.engine) else {\n            return Err(ListTagsForResourceErrorModeled::InternalServerException(\n                InternalServerException {\n                    message: \"the deployment-parameter store is unavailable\".to_string()," … "        let now = ::std::time::SystemTime::now();\n        let mut store = state.lock();" … "        store.purge_expired(now);\n\n        let Some(record) = store.parameters.get(&input.resource_arn) else {" … "            return Err(crate::state::parameter_not_found());"

Test coverage that must stay unchanged (all three operations are driven over one table in each suite, so a single helper is pinned from three sides):
- tests/all/resource_arn_validation.rs:33-38 — "//! The three operations that bind `resourceArn` as a required `@httpLabel` … are each driven over the same table, through the production `MultiServiceRouter` and restJson1 serialization"
- tests/all/deployment_parameter_expiry.rs:248-307 — the expiry rows assert `ListTagsForResource`, `TagResource` and `UntagResource` each answer `NOT_FOUND_STATUS` / `NOT_FOUND_CODE` for an expired parameter.
- tests/all/not_found_error_code.rs — the not-found code/text for an ARN of this vendor naming no record.

## Steps to Reproduce

1. Open src/ops/list_tags_for_resource.rs:163-187, src/ops/tag_resource.rs:176-220 and src/ops/untag_resource.rs:160-193 side by side.
2. Observe the identical five-step sequence in each: `validate_resource_arn(&input.resource_arn)?` → `service_state(&*self.engine)` else `InternalServerException { message: "the deployment-parameter store is unavailable" }` → one `SystemTime::now()` sample + `state.lock()` → `store.purge_expired(now)` → `store.parameters.get*(&input.resource_arn)` else `parameter_not_found()`.
3. Compare with src/state.rs:10-14 and src/state.rs:661-664, which state that sequence as ONE rule of the service rather than as three per-handler preambles.
4. Note that no shared helper owns the sequence: `validate_resource_arn`, `purge_expired` and `parameter_not_found` are each shared, but their ordering and the store-unavailable branch are re-derived per handler, so a change to the rule has to be made three times.
5. Run the suites that pin the rule from all three operations (tests/all/resource_arn_validation.rs, tests/all/deployment_parameter_expiry.rs, tests/all/not_found_error_code.rs): they pass today and must pass unchanged after the sequence is hoisted into one helper.
