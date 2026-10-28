Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# The client-token fingerprint is taken from the RAW accountIds list, so a retry that repeats an account id — which this crate says names the same account once — is answered 409 IdempotentParameterMismatchException instead of being replayed

_Filed: 2026-10-28T17:18:53Z_
_Operation: AssociateAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AssociateAccounts` normalises the request's `accountIds` through `crate::state::distinct_account_ids` — "each once and in the order the request first names them" — and then does the work and builds the response from that normalised list. The idempotency fingerprint, however, is computed two lines later from the UNNORMALISED input (`probe`, the whole input with only `client_token` blanked). So two requests this crate itself treats as naming the same accounts, acting identically and answering identically — `{"accountIds": ["111111111111"]}` and `{"accountIds": ["111111111111", "111111111111"]}` — digest to different fingerprints.

Under one client token the second of them is therefore classified `ParameterMismatch` and refused 409 `IdempotentParameterMismatchException` with "The request tied to this token is different!" — for a request that differs in no respect the service acts on. A retry is exactly the situation a client token exists for, and this refuses it.

The crate's own suite is what establishes that these two requests are the same request: `tests/all/account_id_list_rules.rs:169-265` runs `("the-same-id-twice", vec![MEMBER_B, MEMBER_B])` and `("the-documented-maximum-of-one-repeated-id", vec![MEMBER_A; 50])` beside `("one", vec![MEMBER_A])` and asserts all of them report and store each named account exactly once — but it sends no `clientToken`, so the fingerprint path is never exercised and the 409 reported here is pinned by nothing.

Scope: only the REPEATED-id shape is the defect. A reordered list (`["A","B"]` vs `["B","A"]`) legitimately yields a different fingerprint, because `distinct_account_ids` keeps "the order the request first names them" and the response's `accountIds` order follows it — so those two really are different requests. `DisassociateAccounts` carries the identical code (`src/ops/disassociate_accounts.rs:204-208`), so the root cause is shared; `AssociateAccounts` is named here because it is the path that writes the associations.

## Evidence

The normalisation and the raw fingerprint, adjacent in the same handler — `src/ops/associate_accounts.rs:203-210`:
```
        // The accounts this request names, each once and in the order the
        // request first names them (`crate::state::distinct_account_ids`).
        let requested = crate::state::distinct_account_ids(&input.account_ids);

        let mut probe = input.clone();
        probe.client_token = None;
        let request_fingerprint = crate::state::fingerprint(&probe);
        let client_token = input.client_token.as_deref();
```
(the same two steps, same order, in `src/ops/disassociate_accounts.rs:202-209`.)

What the crate says a repeated id means — `src/state.rs:475-489`:
```
/// The accounts an organization-association request names, each ONCE and in the
/// order the request first names them.
///
/// An id repeated in the list names the same account, and an account is
/// associated or it is not, so both association operations read their request's
/// list this way — once, here, so the two cannot disagree about what a repeated
/// id means or about the order they answer in.
pub fn distinct_account_ids(account_ids: &[String]) -> Vec<String> {
```

What the fingerprint is for, and that it digests the struct as sent — `src/state.rs:497-505`:
```
/// A stable digest of `value`'s debug rendering, used both as a request
/// fingerprint for the client-token ledger and as the filter fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its
/// members in declaration order, so the same request always digests to the same
/// string and a different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
```

The refusal it produces — `src/state.rs:3319-3321`:
```
            Some(record) if record.request_fingerprint != request_fingerprint => {
                TokenOutcome::ParameterMismatch
            }
```
worded `src/state.rs:565-570`:
```
/// The message every `IdempotentParameterMismatchException` of this service
/// carries — the service's own wire wording for "this client token was first
/// used with a different request", FIXED and derived from nothing in the
/// request.
pub const IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE: &str =
    "The request tied to this token is different!";
```
raised at `src/ops/associate_accounts.rs:267-278`.

Test coverage: that the two request shapes are equivalent is pinned — `tests/all/account_id_list_rules.rs:165-206` and `:223-237`:
```
/// A list inside the documented size is accepted by BOTH operations and acts on
/// each account it names ONCE, in the order the list first names it
...
        (
            "the-same-id-twice",
            vec![MEMBER_B.to_string(), MEMBER_B.to_string()],
        ),
...
        assert_eq!(
            reported_account_ids(&associated),
            expected,
            "case {case:?}/AssociateAccounts: expected each named account once in the order the \
             request first names it, got {associated}"
        );
```
and those calls send `&json!({"accountIds": requested})` with no `clientToken` (`:212-217`), so no test combines a repeated id with a client token. `tests/all/account_association.rs:330` and `:366` use client tokens but only with duplicate-free lists.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"} so the caller is past the enrollment gate.
2. Call AssociateAccounts with {"accountIds": ["111111111111", "111111111111"], "clientToken": "assoc-tok"} — 200, and the response's accountIds is ["111111111111"] (one entry), because the handler answers from distinct_account_ids (src/ops/associate_accounts.rs:205, 294-297).
3. Retry what the service just told you it did: AssociateAccounts with {"accountIds": ["111111111111"], "clientToken": "assoc-tok"} — the same token, and a list naming exactly the accounts the first call reported. Observe 409 IdempotentParameterMismatchException, "The request tied to this token is different!", because the fingerprint was taken from the raw list (src/ops/associate_accounts.rs:207-209) and ["111111111111","111111111111"] digests differently from ["111111111111"].
4. Reverse the order of the two calls (single id first, repeated id as the retry): the same 409 appears, so neither direction can be retried.
5. Confirm the two requests are equivalent everywhere else: drop the clientToken and send each shape on a fresh emulator — both answer the identical accountIds list and leave the identical stored enrollment, as tests/all/account_id_list_rules.rs:169-265 pins for "one" and "the-same-id-twice".
6. Repeat steps 2-3 against DisassociateAccounts (src/ops/disassociate_accounts.rs:204-208): the same 409.
