Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# AssociateAccounts accepts account IDs that are not linked accounts of the consolidated billing family

_Filed: 2026-10-24T16:46:37Z_
_Operation: AssociateAccounts | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateAccounts` is documented as connecting "an array of account IDs in a consolidated billing family to a predefined billing group. The account IDs must be a part of the consolidated billing family during the current month, and not already associated with another billing group." Two preconditions, and the emulator enforces only the second.

The handler refuses a duplicate account id in one request (`DUPLICATE_ACCOUNT`) and an account already owned by a different billing group (`ACCOUNTS_ALREADY_ASSOCIATED`), but it never asks whether the account is a member of the billing family at all: any string that satisfies the `AccountId` pattern (`^[0-9]{12}$`) is inserted into the group's `linked_account_ids` and into the `account_associations` index.

Expected: an account id that is not a linked account of the payer's consolidated billing family is refused with the modelled 400 `ValidationException` whose `Reason` is the one the model enumerates for an unusable account list (`ILLEGAL_ACCOUNTS` / `ILLEGAL_ACCOUNT_ID`), and the group's `Size` does not move.
Actual: HTTP 200, the group's `Size` grows, and `ListAccountAssociations` subsequently reports the fabricated account as `MONITORED` against that billing group.

## Evidence

Spec: `AssociateAccounts` operation documentation (smithy_query doc, shape="AssociateAccounts"): "The account IDs must be a part of the consolidated billing family during the current month, and not already associated with another billing group." `AccountGrouping$LinkedAccountIds` repeats the rule ("Account IDs must be a part of the consolidated billing family, and not associated with another billing group"). `ValidationExceptionReason` (smithy_query shape) enumerates `ILLEGAL_ACCOUNTS` and `ILLEGAL_ACCOUNT_ID` for exactly this class of refusal.

Handler: `src/ops/associate_accounts.rs` lines 192-269 — only `crate::state::first_duplicate` and `state.conflicting_association(...)` are consulted before `group.linked_account_ids.extend(to_add)` and `state.account_associations.insert(...)`. `src/state.rs` holds no registry of the billing family's linked accounts (`account_associations` is account->group for accounts earlier requests named; `unassociated_accounts` is only fed by `note_unassociated_account` on release), so membership is never decidable and never checked.

Tests: `tests/all/account_exclusivity_refusal.rs` and `tests/all/disassociate_accounts.rs` cover the already-associated and not-associated refusals; no test asserts a refusal for an account outside the billing family.

## Steps to Reproduce

1. Make the control plane reachable (designate the emulated account as the payer) and create a billing group.
2. Call AssociateAccounts with Arn = the billing group and AccountIds = ["999999999999"] — an account id that was never a linked account of the billing family and was never named by any earlier call.
3. Observe HTTP 200 with the group ARN instead of a 400 ValidationException carrying an ILLEGAL_ACCOUNTS-class Reason.
4. Call ListBillingGroups: the group's Size has grown by one. Call ListAccountAssociations: 999999999999 is reported as associated with (MONITORED by) that billing group.
