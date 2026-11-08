Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 424f227b7e616b3d6e5534953489c6aa083295fd

# The ARN-addressed resolution is mapped to refusals three times, once per tag handler, although the crate already shares such a mapping through a refusal trait

_Filed: 2026-11-08T07:49:48Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `424f227b7e616b3d6e5534953489c6aa083295fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three ARN-addressed operations each carry their own hand-written copy of the same two-step judgement and its refusal classification: parse the `arn` as an Interconnect connection ARN and refuse an unparsable one as `InterconnectValidationException` with `not_a_connection_arn_message`, then judge the ARN's scope with `arn_scope_violation` and refuse a mismatch as `InterconnectValidationException`; the two writers then add a third identical step, resolving `connection_in_for_arn_mut` and refusing an absent resource as `InterconnectClientException` with `arn_resource_absent_message`. The rules and the wording already live once in `state.rs`; what is duplicated is WHICH refusal each verdict calls for — the per-operation branch sequence — in three (and for the absent-resource verdict, two) copies that can drift apart.

The crate already solved exactly this problem for the sibling judgement: `direct_connect::AttachPointRefusal` exists so that "the rules deciding WHICH refusal a verdict calls for and what it says live once" and "a handler therefore cannot classify or word one of these refusals differently from its neighbour", with each handler supplying only which variant of its own error enum each refusal travels in. (An earlier maintainability issue, 20261028T013820Z-interconnect-acceptconnectionproposal-maintainability, introduced that trait for the attach-point verdict mapped twice.) The ARN-addressed judgement is the same shape of problem — one rule, several per-operation error enums — and has no such channel.

The fix is a verdict + refusal-trait pair (or one generic helper parameterised over the three refusals) for the ARN-addressed resolution, with each handler supplying only its own enum variants. Every status, code, message and existing test stays byte-identical: the listing keeps answering HTTP 200 with the empty tag set for an in-scope ARN that resolves to nothing, and the two writers keep their 400 `InterconnectClientException`.

## Evidence

The precedent the crate already set for one verdict mapped per handler — services/service-com-amazonaws-interconnect-interconnect/src/state/direct_connect.rs:191-215:
```
/// The refusals an attach-point judgement can call for, in the error channel of the
/// operation that asked for it.
///
/// Every operation of this service carries its own modelled error enum, so the rules
/// deciding WHICH refusal a verdict calls for and what it says live once — in
/// [`judge_attach_point`] for the gateway resource and in
/// [`crate::state::judge_attach_point_in_store`] for the member's own constraints and the
/// ARN's resource — and an operation that resolves an attach point supplies only this:
/// which variant of its own channel each refusal travels in. A handler therefore cannot
/// classify or word one of these refusals differently from its neighbour.
pub trait AttachPointRefusal: Sized {
```
Copy 1 — src/ops/tag_resource.rs:174-198 and :227-234:
```
        let Some(parsed) = crate::state::parse_connection_arn(&input.arn) else {
            return Err(TagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException {
                    message: crate::state::not_a_connection_arn_message(&input.arn),
                },
            )
            .into());
        };
...
        let region = crate::state::request_region(ctx);
        if let Some(message) = crate::state::arn_scope_violation(&parsed, REQUEST_SHAPE) {
            return Err(TagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException { message },
            )
            .into());
        }
...
        let Some(stored) = guard.connection_in_for_arn_mut(&region, &input.arn) else {
            return Err(TagResourceErrorModeled::InterconnectClientException(
                InterconnectClientException {
                    message: crate::state::arn_resource_absent_message(),
                },
            )
            .into());
        };
```
Copy 2 — src/ops/untag_resource.rs:174-198 and :228-235 (identical but for the enum name):
```
        let Some(parsed) = crate::state::parse_connection_arn(&input.arn) else {
            return Err(UntagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException {
                    message: crate::state::not_a_connection_arn_message(&input.arn),
                },
            )
            .into());
        };
...
        if let Some(message) = crate::state::arn_scope_violation(&parsed, REQUEST_SHAPE) {
            return Err(UntagResourceErrorModeled::InterconnectValidationException(
                InterconnectValidationException { message },
            )
            .into());
        }
...
        let Some(stored) = guard.connection_in_for_arn_mut(&region, &input.arn) else {
            return Err(UntagResourceErrorModeled::InterconnectClientException(
                InterconnectClientException {
                    message: crate::state::arn_resource_absent_message(),
                },
            )
            .into());
        };
```
Copy 3 — src/ops/list_tags_for_resource.rs:177-203 (the first two steps again):
```
        let Some(parsed) = crate::state::parse_connection_arn(&input.arn) else {
            return Err(
                ListTagsForResourceErrorModeled::InterconnectValidationException(
                    InterconnectValidationException {
                        message: crate::state::not_a_connection_arn_message(&input.arn),
                    },
                )
                .into(),
            );
        };
...
        if let Some(message) = crate::state::arn_scope_violation(&parsed, REQUEST_SHAPE) {
            return Err(
                ListTagsForResourceErrorModeled::InterconnectValidationException(
                    InterconnectValidationException { message },
                )
                .into(),
            );
        }
```
Tests that pin the behaviour a fix must leave unchanged: tests/all/arn_scope_refusals.rs (the whole rule over a table of ARNs "crossed with all three operations", arn_scope_refusals.rs:27-32), tests/all/arn_scope_mismatch_wording.rs, tests/all/arn_scope_is_relative_to_the_request.rs, tests/all/arn_region_segment_is_not_a_scope.rs, tests/all/tag_resource.rs, tests/all/list_tags_for_resource.rs, tests/all/resource_tags.rs.

## Steps to Reproduce

1. Open src/ops/tag_resource.rs:174-198 and src/ops/untag_resource.rs:174-198 side by side: the parse step and the scope step are the same two branches, the same two helper calls and the same refusal class, written twice, differing only in the error enum name.
2. Add src/ops/list_tags_for_resource.rs:177-203 as the third copy of the same two branches.
3. Compare tag_resource.rs:227-234 with untag_resource.rs:228-235: the absent-resource verdict is mapped to `InterconnectClientException` + `arn_resource_absent_message()` twice.
4. Read src/state/direct_connect.rs:191-215: the crate already has a refusal-trait channel for exactly this situation (one verdict, several per-operation error enums), used by create_connection.rs:124-145 and accept_connection_proposal.rs:116-139, so the ARN-addressed verdict is the one left mapped by hand per handler.
5. Confirm the fix is behaviour-preserving by re-running tests/all/arn_scope_refusals.rs, arn_scope_mismatch_wording.rs and resource_tags.rs — every status, code and message must be unchanged.
