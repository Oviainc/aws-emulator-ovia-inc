Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# The conditional-ETag precondition rule is implemented four times, once per conditional writer

_Filed: 2026-10-27T04:32:42Z_
_Operation: UpdateChannel | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a presented `ETag` that is not the resource's committed tag means somebody else committed first, so refuse with `ConflictException` / `CONFLICTING_OPERATION` and write nothing" is spelled out separately in each of the four handlers that bind the `x-amzn-update-if-match` header: UpdateChannel, UpdateChannelGroup, UpdateOriginEndpoint and CancelHarvestJob. Each carries its own copy of the comparison branch (and its own comment restating the rule); only the refusal SENTENCE is shared (`crate::state::conflicting_operation()`). Four copies of one precondition can drift — e.g. one of them comparing with `as_ref()` against `&String` and another with `as_deref()`, or a fifth conditional writer added later reading the rule a fifth way — while the crate already centralises every other cross-handler rule in `crate::state` (`manifest_set_violation`, `channel_input_type_violation`, `speke_role_arn_rejection`, `invalid_pagination_token`, `page`/`page_size`). Collapsing the branch into one predicate beside `conflicting_operation()` leaves every response byte-identical.

## Evidence

Four copies of the same branch in this crate:
- services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/update_channel.rs:279-291: "// The precondition: a presented entity tag that is not the committed one means\n// somebody else committed first, which is what the model's `ConflictException` /\n// `CONFLICTING_OPERATION` describes.\nif input.e_tag.as_deref().is_some_and(|presented| presented != stored_etag) { return Err(UpdateChannelErrorModeled::ConflictException(crate::state::conflicting_operation()).into()); }"
- src/ops/update_channel_group.rs:202-215: "// `ETag` travels in `x-amzn-update-if-match` and is the caller's claim about\n// which version of the resource it is updating ...\nif input.e_tag.as_deref().is_some_and(|presented| presented != group.etag) { return Err(UpdateChannelGroupErrorModeled::ConflictException(crate::state::conflicting_operation()).into()); }"
- src/ops/update_origin_endpoint.rs:453-469: "// `ETag` is the precondition the caller read from an earlier response ...\nif input.e_tag.as_ref().is_some_and(|expected| expected != &endpoint.etag) { return Err(UpdateOriginEndpointErrorModeled::ConflictException(crate::state::conflicting_operation()).into()); }"
- src/ops/cancel_harvest_job.rs:217-226: "if input.e_tag.as_deref().is_some_and(|presented| presented != job.etag) { return Err(CancelHarvestJobErrorModeled::ConflictException(crate::state::conflicting_operation()).into()); }"

The shared half already exists: `crate::state::conflicting_operation()` is the one sentence all four use ("One sentence for the whole stale-entity-tag class, so this operation words the refusal exactly as the other conditional updates do." — update_origin_endpoint.rs:464-466).

Existing tests that pin the behaviour and must stay green unchanged: tests/all/update_channel.rs:377 (ConflictExceptionType CONFLICTING_OPERATION on a stale tag), tests/all/update_roundtrip.rs:15 ("`ETag` in `x-amzn-update-if-match` is a precondition"), tests/all/refusal_text_and_entity_tags.rs:857-885 (stale tag refused, current tag accepted, stored tag unchanged), tests/all/rejection_rule_and_negative_paths.rs:1095-1124 (lost-update race -> CONFLICTING_OPERATION), tests/all/control_plane_validation.rs:988-1016 (endpoint precondition).

## Steps to Reproduce

1. Open src/ops/update_channel.rs:279-291, src/ops/update_channel_group.rs:202-215, src/ops/update_origin_endpoint.rs:453-469 and src/ops/cancel_harvest_job.rs:217-226 side by side.
2. Observe the same precondition branch written four times, differing only in the enum variant, the record it reads `etag` from, and `as_deref()` vs `as_ref()`.
3. Note that the refusal body is already shared (`crate::state::conflicting_operation()`) while the rule that selects it is not.
