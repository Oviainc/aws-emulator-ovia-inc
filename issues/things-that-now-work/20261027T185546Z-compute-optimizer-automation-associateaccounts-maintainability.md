Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 19fca1009697525153c46de34cf9f84b5fd316c3

# De-duplicate the accountIds size rule and the account-id de-duplication shared by AssociateAccounts and DisassociateAccounts

_Filed: 2026-10-27T18:55:46Z_
_Operation: AssociateAccounts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `19fca1009697525153c46de34cf9f84b5fd316c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateAccounts` and `DisassociateAccounts` each carry their own private copy of the same two pieces of logic:

1. the `accountIds` size rule — empty list refused with "Member must have length greater than or equal to 1" on `accountIds`, and a list longer than `crate::state::MAX_ACCOUNT_IDS_PER_REQUEST` refused with the same generated "less than or equal to {N}" wording;
2. the request-order de-duplication of `account_ids` into `requested`, with the same `HashSet` walk and the same comment.

The crate's own convention is that a rule two operations share lives once in `src/state.rs` — that is exactly what `recommended_action_types_violation`, `tag_keys_violation` and `preview_selection_violation` are, and the last one's doc says so in as many words. Two copies of one rule in two handlers can drift: a change to the bound, to the member path or to the wording has to be made twice, and the pair of tests that pin them would then disagree. Hoisting both into `src/state.rs` (as the crate already does for every other shared refusal) changes no response and no test: both copies produce byte-identical messages today.

## Evidence

Copy 1 — `src/ops/associate_accounts.rs:325-341`:
```
fn invalid_parameter(input: &AssociateAccountsInput) -> Option<String> {
    if input.account_ids.is_empty() {
        return Some(crate::state::constraint_report(
            "accountIds",
            "Member must have length greater than or equal to 1",
        ));
    }
    if input.account_ids.len() > crate::state::MAX_ACCOUNT_IDS_PER_REQUEST {
        return Some(crate::state::constraint_report(
            "accountIds",
            &format!(
                "Member must have length less than or equal to {}",
                crate::state::MAX_ACCOUNT_IDS_PER_REQUEST
            ),
        ));
    }
    None
}
```
Copy 2 — `src/ops/disassociate_accounts.rs:302-319`, the same body for `DisassociateAccountsInput` (identical member path, identical two messages, identical bound).
The de-duplication, also twice — `src/ops/associate_accounts.rs:201-210`:
```
        // The accounts this request names, each ONCE and in the order the request
        // first names them: an id repeated in the list names the same account,
        // and an account is associated or it is not.
        let mut seen: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
        let requested: Vec<String> = input
            .account_ids
            .iter()
            .filter(|account_id| seen.insert(account_id.as_str()))
            .cloned()
            .collect();
```
and `src/ops/disassociate_accounts.rs:200-209`, the same block with the same comment.
The crate's stated convention for a rule two operations share — `src/state.rs:1410-1417` (`preview_selection_violation`): "The selection a preview request names is read by both preview operations ([`previewed_actions`]), so what makes a selection refusable is judged here once as well: a request one preview operation cannot evaluate is one the other cannot evaluate either, and the two cannot drift apart over which requests they refuse or how they word the refusal."
Existing tests that pin both copies and must keep passing unchanged: `tests/all/account_association.rs:400` `the_documented_account_id_count_is_enforced` and `tests/all/account_disassociation.rs:499` `the_account_id_count_is_enforced`; the de-duplication is pinned by `tests/all/account_association.rs:146` `an_association_stores_every_account_the_request_names` and `tests/all/account_disassociation.rs:116` `a_disassociation_removes_the_association_of_every_account_the_request_names` (both table rows include "the same id twice").

## Steps to Reproduce

1. Read `src/ops/associate_accounts.rs:325-341` and `src/ops/disassociate_accounts.rs:302-319`: the same two clauses, the same member path and the same two messages, written twice.
2. Read `src/ops/associate_accounts.rs:201-210` and `src/ops/disassociate_accounts.rs:200-209`: the same de-duplication walk and comment, written twice.
3. Compare with `src/state.rs:1410-1417`, `src/state.rs:2595-2609` and `src/state.rs:417-443`, where every other refusal shared by two operations of this crate is written once.
4. Note that changing `MAX_ACCOUNT_IDS_PER_REQUEST`, the member path or either message requires editing both handlers, and that `tests/all/account_association.rs:400` and `tests/all/account_disassociation.rs:499` each pin only one copy.
