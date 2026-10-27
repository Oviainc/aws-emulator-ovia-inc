Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c

# ListAgreementCancellationRequests accepts a page token minted by ListAgreementPaymentRequests (scope parts in a different order)

_Filed: 2026-10-27T18:06:20Z_
_Operation: ListAgreementCancellationRequests | Class: pagination_

> _Note: this issue was identified against the code base at commit `15c0cfc29e2643a7b1edb3bac08891ffd2eb9b9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every listing of this crate binds its `nextToken` to a hash of "every parameter of the call that selects what the page indexes into", and the crate states that a token therefore "resumes exactly that query and no other". `ListAgreementCancellationRequests` builds that scope INLINE from its five selectors in the order (partyType, agreementId, status, agreementType, catalog), while the sibling `ListAgreementPaymentRequests` builds the same five-selector scope through `crate::state::payment_request_scope` in the order (partyType, agreementId, agreementType, catalog, status). Both scopes are the concatenation of the same five length-prefixed parts, so the two different listings hash to the SAME scope whenever the permuted parts coincide — in particular for the common call that supplies only the required `partyType`, where both scopes are literally `"8:Acceptor0:0:0:0:"`.

Consequence: a `nextToken` minted by `ListAgreementPaymentRequests {partyType: "Acceptor"}` is ACCEPTED by `ListAgreementCancellationRequests {partyType: "Acceptor"}` (and vice versa) and is used as a raw offset into the OTHER listing's sequence. Instead of the `ValidationException` / `INVALID_NEXT_TOKEN` the crate promises for a foreign token, the caller gets HTTP 200 with cancellation requests resumed at the payment-request offset — i.e. the first N items of the list are silently skipped (or, when the offset happens to be past the end, a confusing 400 for a token the caller did consider valid).

The crate already knows the fix shape: `GetAgreementEntitlements` puts the OPERATION NAME in its scope for exactly this reason, and the other five scopes are built by named helpers in `state.rs`; only this listing hand-rolls its scope, and it drifted from its sibling's part order.

## Evidence

The inline scope of this listing — `services/.../src/ops/list_agreement_cancellation_requests.rs:274-280`:
```rust
        let scope = crate::state::query_scope(&[
            &input.party_type,
            input.agreement_id.as_deref().unwrap_or_default(),
            input.status.as_deref().unwrap_or_default(),
            input.agreement_type.as_deref().unwrap_or_default(),
            input.catalog.as_deref().unwrap_or_default(),
        ]);
```
The sibling's helper — `services/.../src/state.rs:3322-3336`:
```rust
pub fn payment_request_scope(
    party_type: &str, agreement_id: Option<&str>, agreement_type: Option<&str>,
    catalog: Option<&str>, status: Option<&str>,
) -> String {
    query_scope(&[party_type, agreement_id.unwrap_or_default(), agreement_type.unwrap_or_default(), catalog.unwrap_or_default(), status.unwrap_or_default()])
}
```
and it is used verbatim at `services/.../src/ops/list_agreement_payment_requests.rs:265-271`.

The scope is a plain concatenation of length-prefixed parts, so a permutation of equal parts is the same string — `services/.../src/state.rs:3280-3282`: `pub fn query_scope(parts: &[&str]) -> String { parts.iter().copied().map(counted).collect() }`, and `services/.../src/state.rs:3025-3027`: `fn counted(value: &str) -> String { format!("{}:{value}", value.len()) }`. With only `partyType` supplied both listings therefore hash `"8:Acceptor" + "0:" + "0:" + "0:" + "0:"`, and `page_offset` (`src/state.rs:1273-1282`) compares nothing but that digest.

The rule this breaks is the crate's own. `services/.../src/ops/get_agreement_entitlements.rs:181-184`: "// The token resumes THIS agreement's entitlements: the scope names the operation as well as the agreement, so a token minted for the same agreement's terms cannot resume this list." — and it does `crate::state::query_scope(&[Self::NAME, &input.agreement_id])`. `services/.../src/state.rs:3276-3279` likewise claims "The one spelling of that rule: every page-token scope in this module — search_scope, payment_request_scope, charge_scope, billing_adjustment_scope and invoice_line_item_scope — builds its scope from it" (this handler's scope is not among them).

Test coverage stops exactly short of the collision: `tests/all/page_token_scopes.rs:5-10` states "the token a listing mints belongs to the scope of the query that minted it … so it resumes exactly that query and no other" but drives only `ListAgreementPaymentRequests` and `ListAgreementCharges` (whose scopes have 5 vs 3 parts, so they cannot collide), and `tests/all/agreement_cancellation_requests.rs:625-626` (`a_token_from_another_listing_is_refused`) only varies parameters WITHIN the cancellation listing — no test offers a payment-request token to the cancellation listing.

## Steps to Reproduce

1. Create and accept an agreement request that carries a `VariablePaymentTerm` so payment requests can be sent (as `tests/all/payment_requests.rs` does).
2. Send two payment requests under it (`SendAgreementPaymentRequest`) and send two cancellation requests over two agreements (`SendAgreementCancellationRequest`), so each listing has at least two items for `partyType: "Acceptor"`.
3. Call `ListAgreementPaymentRequests` with `{"partyType": "Acceptor", "maxResults": 1}` and keep the returned `nextToken`.
4. Call `ListAgreementCancellationRequests` with `{"partyType": "Acceptor", "maxResults": 1, "nextToken": <that token>}`.
5. Expected (the crate's own rule, as `tests/all/agreement_cancellation_requests.rs:625-626` states it): HTTP 400 `ValidationException` with reason `INVALID_NEXT_TOKEN`.
6. Actual: HTTP 200 — the token's offset (1) is applied to the cancellation list, so the FIRST cancellation request is silently skipped and the second is reported. The reverse direction (a cancellation token offered to `ListAgreementPaymentRequests`) behaves the same way.
