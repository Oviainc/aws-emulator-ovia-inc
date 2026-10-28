Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# The three rules every setter of a feed's output list holds are written out verbatim in both CreateFeed and UpdateFeed

_Filed: 2026-10-27T23:35:21Z_
_Operation: CreateFeed | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateFeed` and `UpdateFeed` are the two operations that SET a feed's whole output list, and each one carries its own copy of the same three-check sequence, in the same order, over the same constants, mapping each failure to the same modelled error class:

1. empty list -> 400 `ValidationException` with `crate::state::MISSING_OUTPUTS_MESSAGE`
2. a name listed twice (`crate::state::first_duplicate_output_name`) -> 409 `ConflictException` with `crate::state::duplicate_output_name_message`
3. more than `crate::state::MAX_OUTPUTS_PER_FEED` entries -> 402 `ServiceQuotaExceededException` with `crate::state::too_many_outputs_message()`

The crate has already factored the predicates and the message strings into `crate::state`; what is still duplicated is the LOGIC on top of them — which checks a setter runs, in which order, and which error class each one produces. Because the two copies are independent, one can be changed without the other, and the two operations would then disagree about what a feed's output list may look like, which is precisely the drift `crate::state::first_duplicate_output_name`'s own doc says the shared scan exists to prevent ("so none of them can disagree about what a feed's outputs may look like").

A single classifier in `crate::state` (e.g. one returning which of the three refusals a supplied list earns, with the handler lifting it into its own error enum) would leave every response byte and every existing test unchanged, since the predicates, the order and the messages are already named constants and helpers.

## Evidence

Copy 1 — `src/ops/create_feed.rs:268-301`:
```
if input.outputs.is_empty() {
    return Err(CreateFeedErrorModeled::ValidationException(
        ValidationException { message: crate::state::MISSING_OUTPUTS_MESSAGE.to_string() },
    ).into());
}
...
if let Some(duplicate) = crate::state::first_duplicate_output_name(
    input.outputs.iter().map(|output| output.name.as_str()),
) {
    return Err(CreateFeedErrorModeled::ConflictException(ConflictException {
        message: crate::state::duplicate_output_name_message(duplicate),
    }).into());
}
...
if input.outputs.len() > crate::state::MAX_OUTPUTS_PER_FEED {
    return Err(CreateFeedErrorModeled::ServiceQuotaExceededException(
        ServiceQuotaExceededException { message: crate::state::too_many_outputs_message() },
    ).into());
}
```

Copy 2 — `src/ops/update_feed.rs:235-270`, the same three branches in the same order:
```
if input.outputs.is_empty() {
    return Err(UpdateFeedErrorModeled::ValidationException(
        ValidationException { message: crate::state::MISSING_OUTPUTS_MESSAGE.to_string() },
    ).into());
}
...
if let Some(duplicate) = crate::state::first_duplicate_output_name(
    input.outputs.iter().map(|output| output.name.as_str()),
) {
    return Err(UpdateFeedErrorModeled::ConflictException(ConflictException {
        message: crate::state::duplicate_output_name_message(duplicate),
    }).into());
}
...
if input.outputs.len() > crate::state::MAX_OUTPUTS_PER_FEED {
    return Err(UpdateFeedErrorModeled::ServiceQuotaExceededException(
        ServiceQuotaExceededException { message: crate::state::too_many_outputs_message() },
    ).into());
}
```

The crate states the single-rule intent it is drifting from — `src/state.rs:316-319`: "every operation that supplies a feed's output list goes through this one scan — `CreateFeed` and `UpdateFeed`, which REPLACE the list, and `AssociateFeed`, which appends to it — so none of them can disagree about what a feed's outputs may look like." and `src/state.rs:369-372`: "It binds EVERY operation that sets a feed's output list, not only the append: `CreateFeed` and `UpdateFeed` each REPLACE the list".

Existing tests that pin all three rules for BOTH carriers (so a shared classifier is verifiable without changing any assertion):
* `tests/all/output_provenance.rs:541-604` `an_output_list_past_the_feed_output_quota_is_refused`, parameterised `for carrier in [ListCarrier::Create, ListCarrier::Update]` (:545) — the test's own `ListCarrier` enum (:336) exists precisely because one rule is held by two operations
* `tests/all/output_provenance.rs:420-535` `an_output_list_naming_one_output_twice_is_refused`, asserting `json!(duplicate_output_name_message(duplicate))` (:512)
* `tests/all/feed_output_validation.rs:194-231` `create_feed_refuses_an_output_list_with_no_outputs` and `:236-300` `update_feed_refuses_an_empty_output_list_and_changes_nothing`, both asserting `MISSING_OUTPUTS_MESSAGE` (:222, :283)
* `tests/all/error_message_rule.rs:432-447`, asserting the quota refusal states the bound

## Steps to Reproduce

1. Open services/service-com-amazonaws-elementalinference-elementalinference/src/ops/create_feed.rs and read lines 268-301
2. Open services/service-com-amazonaws-elementalinference-elementalinference/src/ops/update_feed.rs and read lines 235-270
3. Observe the same three predicates, in the same order, each mapped to the same modelled error class and the same shared message helper — two independent copies of one rule set
4. Change (for example) the order of the duplicate-name and quota checks in one file only, and observe that nothing in the crate forces the other setter to agree
5. Expected: the sequence is stated once in crate::state (as the page-size rule and the duplicate-name scan already are) and both handlers lift the single result into their own error enum, leaving every response and every test above unchanged
