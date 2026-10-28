Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# AssociateFeed accepts an empty outputs array although the model documents "an array of one or more outputs"

_Filed: 2026-10-27T23:35:21Z_
_Operation: AssociateFeed | Class: validation_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateFeed`'s handler never inspects the length of `input.outputs`. A request carrying `"outputs": []` runs every other check vacuously (`first_duplicate_output_name` over an empty iterator yields `None`, `unresolved_output_reference` over an empty iterator yields `None`, `outputs_to_append(vec![])` yields `Ok(vec![])`, and the output-quota branch is guarded by `!to_append.is_empty()`), so the call falls straight through to the write and answers HTTP 200.

The model documents the member as carrying at least one output, and the two sibling handlers of this same crate that take a feed's output list DO enforce exactly that cardinality rule — each refuses an empty list with `crate::state::MISSING_OUTPUTS_MESSAGE` and a 400 `ValidationException`, a rule they derive from the weaker sentence on `CreateFeedRequest$outputs`. `AssociateFeed` is the one writer of a feed's output list that applies no cardinality check at all.

The crate's stated reason for exempting `AssociateFeed` (state.rs:397-399) addresses a different question — whether the call could EMPTY the stored list — not whether the request itself may carry zero outputs. A caller that genuinely wants to claim a feed without new outputs can echo an output the feed already holds: `Feed::outputs_to_append` already treats an output the feed holds exactly as described as "in place already" and appends nothing, so refusing the empty array does not remove that capability.

## Evidence

Smithy (smithy_query query="doc" shape="AssociateFeedRequest" member="outputs"): "An array of one or more outputs that you want to add to this feed now, to supplement any outputs that you specified when you created or updated the feed."

The handler has no length check — `src/ops/associate_feed.rs:224-246` goes straight from the ARN-resemblance check to the duplicate-name scan, and the only guard that mentions the list's length is the quota one at `src/ops/associate_feed.rs:400-402`: `if !to_append.is_empty() && feed.outputs.len() + to_append.len() > crate::state::MAX_OUTPUTS_PER_FEED {` — i.e. an empty list is explicitly waved past.

Both sibling setters in THIS crate enforce the rule:
* `src/ops/create_feed.rs:268-275`: `if input.outputs.is_empty() { return Err(CreateFeedErrorModeled::ValidationException(ValidationException { message: crate::state::MISSING_OUTPUTS_MESSAGE.to_string() }).into()); }`
* `src/ops/update_feed.rs:235-242`: `if input.outputs.is_empty() { return Err(UpdateFeedErrorModeled::ValidationException(ValidationException { message: crate::state::MISSING_OUTPUTS_MESSAGE.to_string() }).into()); }`

The crate's own exemption note, which addresses emptying the STORED list rather than the request's cardinality — `src/state.rs:397-399`: "`AssociateFeed` is not one of them: it APPENDS to the stored list (\"New outputs will be appended\"), so a call that appends nothing leaves a feed that already holds an output exactly as it was and cannot empty it."

The retry path that makes the refusal harmless for real callers — `src/state.rs:787-798` (`Feed::outputs_to_append`): `Some(held) if *held == candidate => {}` ("Already in place, exactly as this request describes it.").

Test coverage: no test asserts that an empty `outputs` is REFUSED; the suite instead relies on it being accepted as the "claim with no new outputs" idiom, e.g. `tests/all/feed_association.rs:701` `let (status, body) = associate(&router, &id, "source-media", json!([])).await;` and `tests/all/feed_output_validation.rs:268-272`, which asserts `status == 200` for `associate(&router, &id, json!([]), false)`. The empty-list rule itself is pinned only for the setters, in `tests/all/feed_output_validation.rs::create_feed_refuses_an_output_list_with_no_outputs` (:194-231) and `::update_feed_refuses_an_empty_output_list_and_changes_nothing` (:236-300).

This is a distinct gap from the already-fixed AssociateFeed reference-integrity issue (20261024T180343Z-elementalinference-associatefeed-validation), which covered unresolvable dictionary/fixture references only.

## Steps to Reproduce

1. POST /v1/feed with {"name":"empty-associate","outputs":[{"name":"crop1","outputConfig":{"cropping":{}},"status":"ENABLED"}]} and note the returned feed id F
2. POST /v1/feed/F/associate with {"associatedResourceName":"source-media","outputs":[],"dryRun":false}
3. Observe HTTP 200 and that GET /v1/feed/F now reports status ACTIVE with the association stored — the request carried zero outputs where the model documents "an array of one or more outputs"
4. For contrast, POST /v1/feed with "outputs": [] and PUT /v1/feed/F with "outputs": [] both answer 400 ValidationException with crate::state::MISSING_OUTPUTS_MESSAGE ("You must specify at least one output.")
5. Expected: the association request carrying an empty outputs array is refused with the same 400 ValidationException the other writers of a feed's output list answer with
