Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6

# The eligibility-to-refusal branch is written out twice inside PutRawMessageContent::handle

_Filed: 2026-10-24T22:47:34Z_
_Operation: PutRawMessageContent | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f4a68ad00bfdddbe3121beaf6a0646fa4e966cf6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutRawMessageContent::handle` decides "may this message take an update, and if not which refusal does the caller get" twice: once in the first critical section (before the S3 reference is resolved) and once in the second critical section (before the bytes are installed). The second check is deliberate and must stay — the lock is released while the MIME content is validated, so the eligibility has to be re-established under the write guard — but the MAPPING from `Eligibility` to the refusal is copy-pasted, not shared: both copies spell out `Eligibility::Missing => return Err(refuse_missing(..))`, `Eligibility::Frozen => return Err(refuse_frozen(..))`, `Eligibility::Updatable => {}` with the same three arguments in the same order.

That is one rule in two places, so the copies can drift: a change to which refusal a state produces (or a new `Eligibility` variant) applied to the first `match` alone leaves the second one deciding differently, and because the second `match` is the one that guards the write, a drift there is invisible on the happy path and only shows up on a racing request. The crate already shows the alternative it prefers for exactly this situation one level down: the two refusals are themselves factored into `refuse_missing`/`refuse_frozen`, and the not-found text lives in a single `crate::state::unknown_message_id` because, as the handler comment says, "both operations render it from the one place that states it".

The fix is confined to this crate and changes no response: fold the branch into one helper (e.g. `fn refuse_if_not_updatable(state, akid, message_id, region) -> Result<(), PutRawMessageContentError>`) and call it with `?` from both critical sections. Every wire answer, status, error type and message stays byte-identical, and the existing tests keep passing unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs:174-186:
```
        let resolved = {
            let state = store.lock();
            match eligibility(&state, &input.message_id) {
                Eligibility::Missing => {
                    return Err(refuse_missing(&akid, &input.message_id, &request_region))
                }
                Eligibility::Frozen => {
                    return Err(refuse_frozen(&akid, &input.message_id, &request_region))
                }
                Eligibility::Updatable => {}
            }
```

Copy 2 — same file, put_raw_message_content.rs:239-249:
```
            let mut state = store.lock();
            match eligibility(&state, &input.message_id) {
                Eligibility::Missing => {
                    return Err(refuse_missing(&akid, &input.message_id, &request_region))
                }
                Eligibility::Frozen => {
                    return Err(refuse_frozen(&akid, &input.message_id, &request_region))
                }
                Eligibility::Updatable => {}
            }
```
The two blocks are byte-identical apart from the guard binding (`let state` vs `let mut state`), and the second one is introduced by the comment at put_raw_message_content.rs:234-237: "SECOND critical section: install the bytes. The lock was released while the content was validated, so the eligibility the first section checked is re-checked here".

The crate's own single-source convention, which this branch does not follow — put_raw_message_content.rs:310-312: "The text is the service's own rule — the fixed prefix plus the rejected id — and both operations render it from the one place that states it, `crate::state::unknown_message_id`." (the same comment is repeated at get_raw_message_content.rs:150-153), with the helper at src/state.rs:34-36.

Test coverage that pins the behaviour a refactor must not change: tests/all/raw_message_content.rs:526 `put_to_an_unknown_message_is_not_found`, :582 `an_unknown_message_id_is_refused_with_the_id_it_was_asked_about`, :678 `put_to_a_frozen_message_is_refused_and_changes_nothing` (asserts HTTP 400 + `x-amzn-errortype: MessageFrozen` and that the stored content is untouched), and :372 `put_replaces_the_stored_content_and_the_next_get_reports_it` for the Updatable path.

## Steps to Reproduce

1. Open services/service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice/src/ops/put_raw_message_content.rs and read lines 174-186 and 239-249: the same three-arm `match eligibility(&state, &input.message_id)` with the same `refuse_missing`/`refuse_frozen` calls appears twice.
2. Simulate the drift: change the first copy only (for example make `Eligibility::Frozen` fall through to `Updatable`, or add a fourth variant to `enum Eligibility` at lines 279-286 and handle it in the first `match` only). The first copy now encodes a different rule from the copy that actually guards `state.replace_message_content` at line 253, and nothing in the crate ties them together.
3. Confirm the fix is response-neutral: extract one `refuse_if_not_updatable(&state, &akid, &input.message_id, &request_region)?` helper, call it from both critical sections, and run `cargo test -p service-com-amazonaws-workmailmessageflow-giraffemessageintransitservice` — tests/all/raw_message_content.rs (`put_to_an_unknown_message_is_not_found`, `put_to_a_frozen_message_is_refused_and_changes_nothing`, `put_replaces_the_stored_content_and_the_next_get_reports_it`) must pass unchanged.
