Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# ListAccountAssociations reports only accounts earlier requests named, and never AccountName/AccountEmail

_Filed: 2026-10-24T04:59:11Z_
_Operation: ListAccountAssociations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the operation "list[s] linked accounts that are linked to the payer account for the specified time period ... The response will optionally include the billing group that's associated with the linked account" (operation documentation). So the roster is the consolidated billing family: every linked account of the payer is reported, each as an `AccountAssociationsListElement` carrying `AccountId`, `AccountName`, `AccountEmail` and `BillingGroupArn` (absent/UNMONITORED when the account is in no billing group).

Actual: the handler builds its roster from `state.linked_accounts()`, which is `account_associations` (accounts a previous `CreateBillingGroup`/`AssociateAccounts` happened to name) plus a bounded tail of accounts released by a delete and primary accounts that were not members. Consequences a client observes:
- On a store where no billing group has been created, `ListAccountAssociations` answers 200 with an EMPTY `LinkedAccounts` list even though the organization has linked accounts; `Filters.Association=UNMONITORED` can therefore never report an account the emulator was not told about by an earlier billingconductor write.
- `AccountName` and `AccountEmail` are hardcoded `None` for every element, so they are missing from every response, for every account, forever.
- The roster is also capped (`MAX_UNASSOCIATED_ACCOUNTS`) and loses the oldest released account, so an account that was once in a deleted group silently disappears from the listing instead of staying UNMONITORED.

The account roster (ids, names, emails) exists in the emulator: the Organizations emulator persists `AccountRecord { id, email, name }` per account. billingconductor does not consult it, so this is a cross-service wiring gap as much as a response-content one.

## Evidence

Spec: `smithy_query(query="doc",shape="ListAccountAssociations")` — "This is a paginated call to list linked accounts that are linked to the payer account for the specified time period ... The response will optionally include the billing group that's associated with the linked account"; `AccountAssociationsListElement` declares `AccountId`, `BillingGroupArn`, `AccountName`, `AccountEmail` (the latter two `@sensitive`, both documented output).

Handler: `src/ops/list_account_associations.rs:205-241` iterates `state.linked_accounts()` and constructs each element with `account_name: None, account_email: None` (comment: "The account's name and email live in Organizations, which this emulator has not been told; absent rather than invented"). `src/state.rs:639-649` `linked_accounts()` derives the roster purely from `account_associations` + the `unassociated_accounts` tail; `src/state.rs:619-631` caps and evicts that tail.

Tests: no test asserts the roster for an account that was never named in a billingconductor write, and none asserts `AccountName`/`AccountEmail`; `tests/all/billing_group_core_loop.rs` only checks accounts it itself associated.

## Steps to Reproduce

1. Boot the emulator (with the payer account designated so the call is served)
2. POST /list-account-associations with an empty body -> 200 with an empty LinkedAccounts list, although the account's consolidated billing family has linked accounts
3. POST /create-billing-group with AccountGrouping.LinkedAccountIds=["111122223333"] and PrimaryAccountId="111122223333"
4. POST /list-account-associations again -> the account now appears, but the element carries no AccountName and no AccountEmail
5. POST /list-account-associations with Filters={"Association":"UNMONITORED"} -> empty, even for org accounts that genuinely belong to no billing group
