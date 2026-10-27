Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# payment_request_scope re-implements query_scope instead of calling it

_Filed: 2026-10-27T02:22:49Z_
_Operation: ListAgreementPaymentRequests | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`query_scope` is documented as "the one spelling of that rule" for building a page-token scope out of length-prefixed parts, and the sibling `charge_scope` documents itself as "Built from [`query_scope`] like every other scope in this module". `payment_request_scope` — the scope `ListAgreementPaymentRequests` binds its `nextToken` to — does not call it: it inlines the same computation by hand with `format!` over five `counted(..)` calls. The two copies are byte-for-byte equivalent today (`query_scope(&[a,b,c,d,e])` is exactly `counted(a)..counted(e)` concatenated), so replacing the hand-rolled `format!` with `query_scope(&[..])` leaves every response and every existing test unchanged — but while two copies of the rule exist they can drift (e.g. a future change to the delimiter or prefix in `counted`/`query_scope` would silently stop applying to one listing's tokens).

## Evidence

The single spelling of the rule — services/.../src/state.rs:3140-3149:
```
/// The scope a page token belongs to: every part of the query that selects what
/// the page indexes into, each part length-prefixed by [`counted`] so no value a
/// caller supplies can imitate the delimiters and make two different queries
/// hash alike.
///
/// The one spelling of that rule: [`search_scope`] and
/// [`billing_adjustment_scope`] both build their scope from it.
pub fn query_scope(parts: &[&str]) -> String {
    parts.iter().copied().map(counted).collect()
}
```

The second copy — services/.../src/state.rs:3185-3200:
```
pub fn payment_request_scope(
    party_type: &str,
    agreement_id: Option<&str>,
    agreement_type: Option<&str>,
    catalog: Option<&str>,
    status: Option<&str>,
) -> String {
    format!(
        "{}{}{}{}{}",
        counted(party_type),
        counted(agreement_id.unwrap_or_default()),
        counted(agreement_type.unwrap_or_default()),
        counted(catalog.unwrap_or_default()),
        counted(status.unwrap_or_default())
    )
}
```

The claim it falsifies — services/.../src/state.rs:3202-3219 (`charge_scope`): "Built from [`query_scope`] like every other scope in this module, so no value a caller supplies can imitate the delimiters and make two different listings hash alike." — followed by `query_scope(&[ ... ])`. `billing_adjustment_scope` (state.rs:2818-2841) and `invoice_line_item_scope` (state.rs:900-928) carry the same doc and also call `query_scope`.

Test coverage that must stay green: services/.../tests/all/payment_requests.rs:673-801 (`maxResults` bounds the page, the token resumes the same list, a token minted for a different `agreementId`/`status` is refused, a forged token is refused) and services/.../tests/all/payment_request_selector_resolution.rs:298-312.

## Steps to Reproduce

1. Open services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs and read `query_scope` at line 3147 and its doc at 3140-3146 ("The one spelling of that rule").
2. Read `payment_request_scope` at lines 3185-3200: it concatenates `counted(..)` through `format!` instead of calling `query_scope`.
3. Read `charge_scope` at 3202-3219, `billing_adjustment_scope` at 2824-2841 and `invoice_line_item_scope` at 906-928 — each documents itself as built from `query_scope` "like every other scope in this module" and calls it.
4. Observe that the rule now has two implementations in one crate; collapsing `payment_request_scope` onto `query_scope` produces the identical string and keeps tests/all/payment_requests.rs passing.
