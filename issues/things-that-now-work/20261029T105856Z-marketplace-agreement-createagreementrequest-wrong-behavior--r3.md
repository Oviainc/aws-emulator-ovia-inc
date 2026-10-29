Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e14af6b59464e58f15b7f39517598ee7f904626e

# An amendment subtracts the source quantity once per repeated dimensionKey, so a quote naming a dimension twice charges nothing for the units it adds

_Filed: 2026-10-29T10:58:56Z_
_Operation: CreateAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e14af6b59464e58f15b7f39517598ee7f904626e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`term_price` prices a `ConfigurableUpfrontPricingTerm` by walking `configuration.dimensions` and, for EACH element, looking the dimension's existing quantity up in `old_quantities` and subtracting it from that element's own `dimensionValue`. `DimensionList` carries no `@uniqueItems` (and no per-key uniqueness rule anywhere in the model), so one configuration — or two configurable-upfront terms in one request — may legitimately name the same `dimensionKey` more than once. When it does, the SAME source quantity is subtracted once per occurrence: the amount charged becomes sum(v_i - o) instead of (sum v_i) - o.

This contradicts the crate's own rule for what repeated dimension keys mean: `dimension_quantities` — the function that produces the very `old_quantities` map `term_price` reads — explicitly anticipates duplicates and SUMS them into one quantity per key ("one request may name the same dimension twice"). So the store's notion of "the quantity this dimension is bought at" is the sum of the occurrences, while the pricing loop treats each occurrence as a separate purchase measured against the whole existing quantity.

Two observable consequences, both inside one response:

1. `ChargeSummary$newAgreementValue` is computed from the raw sum of `dimensionValue` (state.rs:612-614), so it reports the value of all the units asked for, while `ExpectedCharge$amount` is computed from the per-occurrence differences (state.rs:615-617) and can be `0.00` for the same terms — the quote says the agreement is now worth N units but charges nothing for the ones it added. Because `saturating_sub` floors each difference at zero, a repeated dimension can never be charged more than one occurrence's worth above the source quantity.
2. Because `issue_acceptance_invoices` only invoices a charge whose amount `is_positive_amount` (state.rs:4562-4569), accepting such a quote issues NO invoice at all for the amendment, so the added units are never billed and `ListAgreementInvoiceLineItems` reports nothing to adjust.

Each repeated occurrence also emits its own `ItemizedCharge` claiming to be "the total incremental charge amount for this dimension" for the same `dimensionKey`, each with the same `oldQuantity`, so two line items describe one dimension and neither states its total.

## Evidence

The pricing loop that subtracts the source quantity per occurrence — services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:599-623:
```
            for dimension in &configuration.dimensions {
                ...
                let old_quantity = old_quantities
                    .get(&dimension.dimension_key)
                    .copied()
                    .unwrap_or_default();
                let added =
                    quantity(dimension.dimension_value).saturating_sub(quantity(old_quantity));
                let incremental = rate.map(|rate| rate.saturating_mul(added));
                committed = committed.zip(rate).map(|(total, rate)| {
                    total.saturating_add(rate.saturating_mul(quantity(dimension.dimension_value)))
                });
                charged = charged
                    .zip(incremental)
                    .map(|(total, incremental)| total.saturating_add(incremental));
```
(`committed` adds the raw `dimension_value`; `charged` adds the per-occurrence difference.)

The crate's own rule for repeated dimension keys, in the function that BUILDS `old_quantities` — state.rs:2013-2027:
```
fn dimension_quantities(terms: &[AcceptedTerm]) -> HashMap<String, i32> {
    ...
                for dimension in &configuration.dimensions {
                    // The quantities come from the request, and one request may
                    // name the same dimension twice, so the sum is bounded
                    // rather than left to overflow `i32`: ...
                    let total = quantities
                        .entry(dimension.dimension_key.clone())
                        .or_default();
                    *total = total.saturating_add(dimension.dimension_value);
```
and it is read through `charge_summary` at state.rs:5254-5256 (`dimension_quantities(&source.accepted_terms)`), which `CreateAgreementRequest` calls at src/ops/create_agreement_request.rs:329-333.

The model allows the duplicate input: `smithy_query(query="shape", shape="DimensionList")` → `{"type": "list", "length": {"min": 1}, "member": "Dimension"}` — no `uniqueItems`; and `ItemizedCharge$incrementalChargeAmount` is documented "<p>The total incremental charge amount for this dimension.</p>" with `oldQuantity` "<p>The existing quantity for this dimension from the source agreement. This value is <code>0</code> for NEW intent.</p>".

The charge that is then not invoiced — state.rs:4562-4569:
```
            .filter_map(|charge| {
                let id = charge.id.clone()?;
                let amount = charge
                    .amount
                    .clone()
                    .filter(|amount| is_positive_amount(amount))?;
```

Test coverage: tests/all/agreement_lifecycle.rs:833-883 (`an_amendment_is_quoted_against_the_source_agreements_quantities`) pins the amendment arithmetic only for DISTINCT dimension keys (`[("Users", 9), ("Storage", 100)]` against a source holding `[("Users", 4), ("Admins", 1)]`); a grep of tests/all for `oldQuantity`/`duplicate` shows no row that names one dimension key twice, which is why the double subtraction is unpinned.

## Steps to Reproduce

1. Create and accept an agreement over one ConfigurableUpfrontPricingTerm with selectorValue "P12M" and dimensions [{"dimensionKey":"Users","dimensionValue":4}] (as tests/all/agreement_lifecycle.rs's `agreement_over`/`upfront_term` helpers do); note the agreementId.
2. Call CreateAgreementRequest with intent=AMEND, sourceAgreementIdentifier=<that agreementId>, agreementProposalIdentifier="ap-offer1", and requestedTerms=[one ConfigurableUpfrontPricingTerm, selectorValue "P12M", dimensions [{"dimensionKey":"Users","dimensionValue":4},{"dimensionKey":"Users","dimensionValue":4}]] — i.e. 8 Users in total against a source that holds 4.
3. Read the response's chargeSummary: itemizedCharges carries TWO entries for dimensionKey "Users", each with oldQuantity 4 and incrementalChargeAmount "0.00", and expectedCharges[0].amount is "0.00" — although the amendment adds 4 Users above the source's 4.
4. Read newAgreementValue in the same response: it is the rate times 8 units, contradicting the 0.00 charge beside it.
5. Call AcceptAgreementRequest with the returned agreementRequestId, then ListAgreementInvoiceLineItems for the new agreementId: no invoice is issued for the amendment (the charge amount is not positive), so the added units are never billed.
