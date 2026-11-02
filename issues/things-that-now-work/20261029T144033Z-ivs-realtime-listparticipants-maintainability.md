Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f7cb83054c40d7ded49f98a292169f4a09dea4c9

# The "at most one of these members" decision is written out three times in three handlers, with only its wording shared

_Filed: 2026-10-29T14:40:33Z_
_Operation: ListParticipants | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f7cb83054c40d7ded49f98a292169f4a09dea4c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words a mutual-exclusion refusal in one place (`crate::state::mutually_exclusive_members_message`), but the DECISION that produces it — zip the member names with their presence flags, collect the ones the request carried, and refuse with a `ValidationException` when more than one was carried — is written out three times in three handlers. The three copies can drift in what they collect and when they refuse while all still producing the same sentence, which is exactly what the shared message helper exists to prevent at the wording level.

A shared helper in `crate::state` taking the operation name, the `(member, provided)` pairs and the exclusive set, and returning `Result<(), String>` built from the existing `mutually_exclusive_members_message`, would hold the rule once. Each call site keeps reading its own input members (including `ListParticipants`'s nuance that `filterByPublished` carries the model's `@default false`, so `false` is not a supplied filter), so every response and every existing test stays byte-for-byte unchanged.

## Evidence

Copy 1 — `src/ops/list_participants.rs:211-233`:
```
        let supplied: Vec<&str> = [
            ("filterByUserId", input.filter_by_user_id.is_some()),
            ("filterByPublished", input.filter_by_published),
            ("filterByState", input.filter_by_state.is_some()),
            (
                "filterByRecordingState",
                input.filter_by_recording_state.is_some(),
            ),
        ]
        .into_iter()
        .filter(|(_, provided)| *provided)
        .map(|(member, _)| member)
        .collect();
        if supplied.len() > 1 {
            return Err(crate::state::validation_failed(
                ListParticipantsErrorModeled::ValidationException,
                crate::state::mutually_exclusive_members_message(
                    Self::NAME,
                    &supplied,
                    &EXCLUSIVE_FILTERS,
                ),
            ));
        }
```

Copy 2 — `src/ops/list_ingest_configurations.rs:181-198`:
```
        let supplied: Vec<&str> = [
            ("filterByStageArn", input.filter_by_stage_arn.is_some()),
            ("filterByState", input.filter_by_state.is_some()),
        ]
        .into_iter()
        .filter(|(_, provided)| *provided)
        .map(|(member, _)| member)
        .collect();
        if supplied.len() > 1 {
            return Err(crate::state::validation_failed(
                ListIngestConfigurationsErrorModeled::ValidationException,
                crate::state::mutually_exclusive_members_message(
                    Self::NAME,
                    &supplied,
                    &EXCLUSIVE_FILTERS,
                ),
            ));
        }
```

Copy 3 — `src/ops/start_composition.rs:201-217`:
```
        if let Some(layout) = input.layout.as_ref() {
            let supplied: Vec<&str> = LAYOUT_MEMBERS
                .iter()
                .zip([layout.grid.is_some(), layout.pip.is_some()])
                .filter_map(|(member, given)| given.then_some(*member))
                .collect();
            if supplied.len() > 1 {
                return Err(crate::state::validation_failed(
                    StartCompositionErrorModeled::ValidationException,
                    crate::state::mutually_exclusive_members_message(
                        &format!("{}.layout", Self::NAME),
                        &supplied,
                        &LAYOUT_MEMBERS,
                    ),
                ));
            }
        }
```

Only the sentence is shared today — `src/state.rs:4985-4995`:
```
pub fn mutually_exclusive_members_message(
    operation: &str,
    supplied: &[&str],
    exclusive: &[&str],
) -> String {
    format!(
        "{operation} is required to have at most one of {}, found {}",
        exclusive.join(", "),
        supplied.join(", "),
    )
}
```

Tests that pin the behaviour and must stay unchanged: `tests/all/list_ingest_configurations.rs:738-810` ("Only one filter (by stage ARN or by state) can be used at a time": a request carrying both is refused, with the wording asserted), `tests/all/participants.rs:401+` ("The one filter a request may carry"), and `tests/all/composition_layout_default_lifetime.rs` / `tests/all/composition_destination_kinds.rs` for the layout and destination refusals.

## Steps to Reproduce

1. Open `src/ops/list_participants.rs:211-233` and `src/ops/list_ingest_configurations.rs:181-198`: the same collect-then-`len() > 1` refusal appears in both, differing only in the member list and the error enum.
2. Open `src/ops/start_composition.rs:201-217`: the same rule again, expressed with `zip`/`filter_map` instead of `into_iter`/`filter`/`map`.
3. Grep for `mutually_exclusive_members_message` — only the wording is centralised in `src/state.rs:4985`; the decision that calls it is not.
4. Run the existing suites (`tests/all/list_ingest_configurations.rs`, `tests/all/participants.rs`, `tests/all/composition_destination_kinds.rs`) and confirm they pass before and after collapsing the three copies into one `crate::state` helper.
