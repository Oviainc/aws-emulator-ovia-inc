Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8acf21a252327a123d13b159a352e2f9f2a954c2

# Resolve the agreement before the nextToken in ListAgreementInvoiceLineItems

_Filed: 2026-10-29T07:30:45Z_
_Operation: ListAgreementInvoiceLineItems | Class: error-response_

> _Note: this issue was identified against the code base at commit `8acf21a252327a123d13b159a352e2f9f2a954c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListAgreementInvoiceLineItems` parses and refuses its `nextToken` BEFORE it takes the store lock and resolves the required `agreementId`. The page-token scope is built from the agreement id (`invoice_line_item_scope(Self::NAME, &input.agreement_id, …)`), so no token this service ever minted can validate against the scope of an agreement the store does not hold — which means that whenever a request names an unknown agreement AND carries any `nextToken`, the handler answers its `ValidationException` with reason `INVALID_NEXT_TOKEN` on member `nextToken` and the modelled `ResourceNotFoundException` branch below it is unreachable.

That contradicts the handler's own stated rule for the member, and it diverges from the two sibling per-agreement paginated reads of this same crate (`GetAgreementTerms`, `GetAgreementEntitlements`), which resolve the agreement first and answer the modelled not-found whether or not a token was supplied. The caller is told to repair `nextToken` when the member actually at fault is `agreementId`, and the error SHAPE (which is what this service uses to distinguish a refused value from a lookup that found nothing — `state.rs:4230-4246`) is the wrong one.

## Evidence

The handler refuses the token before the store is even read — `src/ops/list_agreement_invoice_line_items.rs:196-235`:

```
196        // The token resumes THIS listing: one minted for a different agreement
197        // or a different set of filters would index into a different sequence.
198        let scope = crate::state::invoice_line_item_scope(
199            Self::NAME,
200            &input.agreement_id,
…
207        let offset = match input.next_token.as_deref() {
208            None => 0,
209            Some(token) => match crate::state::page_offset(&scope, token) {
210                Some(offset) => offset,
211                None => {
212                    return Err(
213                        ListAgreementInvoiceLineItemsErrorModeled::ValidationException(
214                            crate::state::validation_exception("INVALID_NEXT_TOKEN", "nextToken"),
…
222        let store = state.lock();
223
224        // "The unique identifier of the agreement": the invoices reported are
225        // the invoices OF that agreement, so an id naming none is a reference to
226        // a resource that does not exist — the modelled not-found this operation
227        // declares, at the status this service answers every lookup that found
228        // nothing with.
229        if !store.agreements.contains_key(&input.agreement_id) {
230            return Err(crate::state::not_found(
231                ListAgreementInvoiceLineItemsErrorModeled::ResourceNotFoundException(
232                    crate::state::agreement_not_found(&input.agreement_id),
```

The scope that makes the token unusable for an unknown agreement is keyed by that agreement id — `src/state.rs:1005-1029`: `pub fn invoice_line_item_scope(operation: &str, agreement_id: &str, …) -> String { … listing_scope(operation, &[agreement_id, group_by, …]) }`.

The siblings do it the other way round. `src/ops/get_agreement_terms.rs:165-191`:

```
165        let store = state.lock();
166        let Some(agreement) = store.agreements.get(&input.agreement_id) else {
…
169            return Err(crate::state::not_found(
170                GetAgreementTermsErrorModeled::ResourceNotFoundException(
171                    crate::state::agreement_not_found(&input.agreement_id),
…
179        let scope = crate::state::listing_scope(Self::NAME, &[&input.agreement_id]);
180        let offset = match input.next_token.as_deref() {
```

and `src/ops/get_agreement_entitlements.rs:171-201` (`store.settle_agreement(…)`, then the not-found, then `let scope = …; let offset = match input.next_token…`).

Existing coverage pins only the no-token case: `tests/all/invoice_line_item_listing.rs:445-462` — "A well-formed id is not refused as a member: it reaches the handler, which answers a reference to an agreement this store does not hold with the not-found this operation models" and asserts `answer.error_code() == "ResourceNotFoundException"`. No test in `tests/all/page_token_scopes.rs` or `tests/all/window_token_scopes.rs` exercises an unknown agreement together with a token, so nothing pins the current order.

## Steps to Reproduce

1. Accept an agreement request so an agreement A exists with at least one invoice (a prepaid term billed ON_ACCEPTANCE).
2. POST ListAgreementInvoiceLineItems {"agreementId": A, "groupBy": "INVOICE_ID", "maxResults": 1} and keep the returned nextToken T (or use any non-empty token value).
3. POST ListAgreementInvoiceLineItems {"agreementId": "agmt-absent", "groupBy": "INVOICE_ID", "nextToken": T}.
4. Observed: ValidationException with reason INVALID_NEXT_TOKEN naming member nextToken. Expected (as the same call without nextToken answers, per tests/all/invoice_line_item_listing.rs:448-462): ResourceNotFoundException naming the missing Agreement.
5. Compare GetAgreementTerms {"agreementId": "agmt-absent", "nextToken": T}: it answers ResourceNotFoundException, so two sibling reads disagree about the same pair of faults.
