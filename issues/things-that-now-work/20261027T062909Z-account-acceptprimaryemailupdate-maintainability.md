Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c61e5d5a111548c41d0df12f114137f026337222

# The "an account-id member naming the caller's own account is refused" rule is written out four times instead of going through state::target_account_id

_Filed: 2026-10-27T06:29:09Z_
_Operation: AcceptPrimaryEmailUpdate | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c61e5d5a111548c41d0df12f114137f026337222`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states the own-account refusal rule once, in `state::target_account_id`, and `state.rs` documents that helper as the single place every operation taking an account id resolves through. Fifteen of the eighteen handlers do go through it. The three handlers whose account-id member is `@required` — `AcceptPrimaryEmailUpdate`, `StartPrimaryEmailUpdate` and `GetPrimaryEmail` — instead inline a hand-written copy of the same rule (`if input.account_id == crate::state::default_account_id() { return Err(...cannot_target_own_account()) }`), so the rule now exists in four places in this crate's own code.

The copies can drift: a change to how "the caller's own account" is decided (or to which refusal that case earns) has to be made in `state::target_account_id` and in each of the three handlers, and nothing ties them together. `target_account_id(Some(&input.account_id))` already answers exactly what the three inline copies compute — `Err(cannot_target_own_account())` when the named id equals `default_account_id()`, `Ok(named.to_string())` otherwise — so routing the three required-member handlers through it leaves every response byte-identical (the same `cannot_target_own_account()` payload, the same 403 `AccessDeniedException`) and every existing test passing.

The duplication also contradicts the crate's own documentation of the helper, which says the rule is the same "for a member that is required as for one that is optional" and names `target_account_id` as that one place — while the three required-member handlers are precisely the ones that bypass it.

## Evidence

The single statement of the rule — services/service-com-amazonaws-account-account/src/state.rs:1168-1174:
```
pub fn target_account_id(requested: Option<&str>) -> Result<String, AccessDeniedException> {
    match requested {
        None => Ok(default_account_id().to_string()),
        Some(named) if named == default_account_id() => Err(cannot_target_own_account()),
        Some(named) => Ok(named.to_string()),
    }
}
```
The module doc that claims it is the ONE resolution — state.rs:19-25:
```
//! Most Account Management operations take an OPTIONAL `AccountId`: absent, the
//! call lands on "the Amazon Web Services account of the identity used to call
//! the operation" ... Present, it names a MEMBER
//! account of the caller's organization, which the caller's own account is not:
//! that is [`target_account_id`], the one resolution every operation taking an
//! account id goes through.
```
and state.rs:1458-1461, on `cannot_target_own_account`:
```
/// ... the rule is the same for a read of a member account as for a write
/// of one, and the same for a member that is required as for one that is
/// optional ([`target_account_id`]).
```
Copy 1 — src/ops/accept_primary_email_update.rs:193-198:
```
        if input.account_id == crate::state::default_account_id() {
            return Err(AcceptPrimaryEmailUpdateErrorModeled::AccessDeniedException(
                crate::state::cannot_target_own_account(),
            )
            .into());
        }
```
Copy 2 — src/ops/start_primary_email_update.rs:193-198:
```
        if input.account_id == crate::state::default_account_id() {
            return Err(StartPrimaryEmailUpdateErrorModeled::AccessDeniedException(
                crate::state::cannot_target_own_account(),
            )
            .into());
        }
```
Copy 3 — src/ops/get_primary_email.rs:181-186:
```
        if input.account_id == crate::state::default_account_id() {
            return Err(GetPrimaryEmailErrorModeled::AccessDeniedException(
                crate::state::cannot_target_own_account(),
            )
            .into());
        }
```
The sibling form, for contrast — src/ops/get_primary_email_update_status.rs:176-183 (same service, same rule, via the helper):
```
        let account_id = match crate::state::target_account_id(input.account_id.as_deref()) {
            Ok(account_id) => account_id,
            Err(denied) => {
                return Err(
                    GetPrimaryEmailUpdateStatusErrorModeled::AccessDeniedException(denied).into(),
                )
            }
        };
```
Tests that cover all four copies and must stay unchanged — tests/all/own_account_target_refusal.rs:275-295 builds `required_rows()` for exactly these three operations (`GetPrimaryEmail`, `StartPrimaryEmailUpdate`, `AcceptPrimaryEmailUpdate`), and tests/all/own_account_target_refusal.rs:300 `naming_the_callers_own_account_is_refused_by_every_operation`, :346 `the_standalone_call_is_served_for_every_optional_member`, :367 `a_member_account_the_store_holds_is_not_refused` and :402 `a_refused_self_naming_call_writes_nothing` pin the 403 `AccessDeniedException`, the message not echoing the id, and that a refused call writes nothing.

## Steps to Reproduce

1. Open services/service-com-amazonaws-account-account/src/state.rs:1168-1174 and read `target_account_id`, then its module doc at state.rs:19-25 claiming it is "the one resolution every operation taking an account id goes through".
2. Open src/ops/accept_primary_email_update.rs:193-198, src/ops/start_primary_email_update.rs:193-198 and src/ops/get_primary_email.rs:181-186 side by side: each inlines the same `input.account_id == default_account_id()` comparison plus `cannot_target_own_account()` instead of calling the helper.
3. Compare with src/ops/get_primary_email_update_status.rs:176-183 (and the twelve other handlers), which decide the identical rule by calling `crate::state::target_account_id(...)`.
4. Confirm the replacement is response-identical: `target_account_id(Some(&input.account_id))` returns `Err(cannot_target_own_account())` exactly when the named id equals `default_account_id()`, and `Ok(named.to_string())` otherwise — the same string the three handlers already use.
5. Run `cargo test -p service-com-amazonaws-account-account own_account_target_refusal` to see the tests covering all four copies, which must keep passing unchanged after the rule is stated once.
