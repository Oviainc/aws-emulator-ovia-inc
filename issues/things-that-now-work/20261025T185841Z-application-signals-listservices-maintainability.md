Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# The "request names another account" rule is written out five times instead of once

_Filed: 2026-10-25T18:58:41Z_
_Operation: ListServices | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

One rule — "an account id the request supplies that is not this emulator's account selects nothing" — is implemented verbatim in five places in this crate: four list handlers and one helper in the state module. The copies are byte-identical expressions over different input members, each with its own paragraph of the same comment, so a change to how this store decides whether a request names another account (or to the account source) has to be made in five places and can drift between the reads that must agree about it.

The fix is to hoist the predicate into `crate::state` beside the store it is a property of (e.g. `pub fn names_other_account(account: Option<&str>) -> bool`) and call it from the five sites. Every response and every existing test stays exactly as it is — the predicate is the same expression in all five copies.

## Evidence

Five copies of the identical expression:

- `src/ops/list_services.rs:193-196`: `let other_account = input\n    .aws_account_id\n    .as_deref()\n    .is_some_and(|account| account != ::engine::emulator_account_id());`
- `src/ops/list_service_states.rs:198-201`: same expression over `input.aws_account_id`.
- `src/ops/list_service_level_objectives.rs:206-209`: same expression over `input.slo_owner_aws_account_id`.
- `src/ops/list_grouping_attribute_definitions.rs:180-183`: `let other_account = input\n    .aws_account_id\n    .as_deref()\n    .is_some_and(|account| account != ::engine::emulator_account_id());`
- `src/state.rs:2619-2622` (`audited_entity_attributes`): `if entity\n    .aws_account_id\n    .as_deref()\n    .is_some_and(|account| account != ::engine::emulator_account_id())`

The comment above each copy is the same sentence, e.g. `src/ops/list_services.rs:189-192` ("This emulator holds exactly one account's state, so a request that asks for another account's selects nothing...") and `src/ops/list_service_states.rs:194-197` ("This emulator holds exactly one account's state, so a request that asks for another account's selects nothing...").

Existing tests that cover each copy and must stay unchanged: `tests/all/list_services.rs:572` and `tests/all/list_services.rs:599` (ListServices with `AwsAccountId`), `tests/all/list_service_states.rs:821` and `:848` (ListServiceStates with `AwsAccountId`), `tests/all/slo_lifecycle.rs:832-838` ("SloOwnerAwsAccountId naming another account" / "naming this account"), `tests/all/grouping_configuration.rs:446-477` (ListGroupingAttributeDefinitions with `AwsAccountId`), `tests/all/audit_findings.rs:363` (`"AwsAccountId": "999988887777"`).

## Steps to Reproduce

1. Grep the crate for `emulator_account_id`: `src/ops/list_services.rs:196`, `src/ops/list_service_states.rs:201`, `src/ops/list_service_level_objectives.rs:209`, `src/ops/list_grouping_attribute_definitions.rs:183` and `src/state.rs:2622` each re-implement the same predicate.
2. Observe that the four handler copies also repeat the same explanatory comment paragraph, so the rule exists in five editable places.
3. Hoist the predicate into `crate::state` and call it from all five sites; re-run the tests named in the evidence — responses are unchanged.
