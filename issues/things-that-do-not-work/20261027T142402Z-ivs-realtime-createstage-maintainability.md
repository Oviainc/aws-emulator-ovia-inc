Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# Collapse the five copies of the "mint a fresh id, re-mint on collision" loop into one state helper

_Filed: 2026-10-27T14:24:02Z_
_Operation: CreateStage | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Five creating handlers in this crate each hand-roll the same three-step rule: take `store.next_sequence()`, build an ARN from `crate::state::mint_id(sequence, <ID_LEN>)`, then loop `while store.<map>.contains_key(&arn)` re-minting from a fresh sequence. The logic is identical in every copy; only the map, the ARN constructor and the id-length constant differ. Each copy carries its own comment pointing at `CreateStage` as the canonical one ("the uniqueness rule `CreateStage` applies to a minted stage ARN"), which is exactly the signal that one rule lives in five places and the copies can drift — e.g. one of them could be changed to re-use the original `sequence` for the record while re-minting the id, or lose the collision loop entirely, with nothing in the crate noticing.

`crate::state` is already where this crate puts rules shared by more than one handler (`issue_token_record`, `mint_ingest_credential`, `resolved_stage_association`, `encode_after`/`decode_after`), and `issue_token_record` already performs this very re-mint loop for participant ids inside `state.rs`. One `state` helper parameterised by the lookup and the ARN constructor would hold the rule once. The ids minted, the ARNs reported and every existing test stay byte-for-byte unchanged: the helper would return the same `(sequence, arn)` pair the inline loops compute today.

## Evidence

Copy 1 — `src/ops/create_stage.rs:243-254`:
```
        let sequence = store.next_sequence();
        let mut arn = crate::state::stage_arn(
            &region,
            &crate::state::mint_id(sequence, crate::state::STAGE_ID_LEN),
        );
        while store.stages.contains_key(&arn) {
            let retry = store.next_sequence();
            arn = crate::state::stage_arn(
                &region,
                &crate::state::mint_id(retry, crate::state::STAGE_ID_LEN),
            );
        }
```
Copy 2 — `src/ops/create_ingest_configuration.rs:281-292` (comment at :278-280 names the first as the source of the rule: "the uniqueness rule `CreateStage` applies to a minted stage ARN"):
```
        let sequence = store.next_sequence();
        let mut arn = crate::state::ingest_configuration_arn(
            &region,
            &crate::state::mint_id(sequence, crate::state::INGEST_CONFIGURATION_ID_LEN),
        );
        while store.ingest_configurations.contains_key(&arn) {
            let retry = store.next_sequence();
            arn = crate::state::ingest_configuration_arn(
                &region,
                &crate::state::mint_id(retry, crate::state::INGEST_CONFIGURATION_ID_LEN),
            );
        }
```
Copy 3 — `src/ops/create_encoder_configuration.rs:234-245` (`while store.encoder_configurations.contains_key(&arn) {`).
Copy 4 — `src/ops/create_storage_configuration.rs:208-219` (`while store.storage_configurations.contains_key(&arn) {`).
Copy 5 — `src/ops/start_composition.rs:407-414` (`while store.compositions.contains_key(&arn) {`), whose comment at :404-406 again says "the uniqueness rule every other mint of this service applies".

The same rule already exists as a `state` function for participant ids — `src/state.rs:3946-3964`:
```
    // A fresh id per issuance, re-minted on the (vanishingly unlikely) chance it already
    // names a participant of this stage … the same uniqueness rule `CreateStage` applies
    // to a minted stage ARN
    let mut participant_id = mint_id(state.next_sequence(), PARTICIPANT_ID_LEN);
    while in_flight
```

Existing tests that cover the five copies and must stay unchanged: `tests/all/stages.rs`, `tests/all/ingest_configurations.rs`, `tests/all/encoder_configurations.rs`, `tests/all/storage_configurations.rs`, `tests/all/compositions.rs` (round-trip suites that assert the minted ARN is reported back by the matching `Get…`), plus `tests/all/region_scoped_resources.rs`.

## Steps to Reproduce

1. Read `src/ops/create_stage.rs:243-254` and `src/ops/create_ingest_configuration.rs:281-292`: the two blocks are the same loop with the map, the ARN constructor and the id-length constant substituted.
2. Confirm the same block in `src/ops/create_encoder_configuration.rs:234-245`, `src/ops/create_storage_configuration.rs:208-219` and `src/ops/start_composition.rs:407-414`.
3. Note `src/state.rs:3946-3964`, where the identical rule is already expressed once inside `issue_token_record`, and that `crate::state` is where this crate keeps every other rule shared by two or more handlers.
4. Hoisting the loop into one `crate::state` helper leaves every minted ARN and every response identical, so the test suites above pass unchanged.
