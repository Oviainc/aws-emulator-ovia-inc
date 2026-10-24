Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# The quote calculates no money: chargeSummary carries no currency, no agreement value for any committed term, and no charge amounts

_Filed: 2026-10-24T12:48:09Z_
_Operation: CreateAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateAgreementRequest is documented as the quote operation: it "captures the requested terms, CALCULATES CHARGES, and returns a summary" (operation doc), and ChargeSummary is "a detailed breakdown of charges that are associated with an agreement request". Expected: for a prepaid/committed purchase (ConfigurableUpfrontPricingTerm, FixedUpfrontPricingTerm, RecurringPaymentTerm, PaymentScheduleTerm) the response carries chargeSummary.currencyCode, chargeSummary.newAgreementValue, an amount on each expectedCharges[] entry, and an incrementalChargeAmount on each itemizedCharges[] entry.

Actual: crate::state::charge_summary() builds ChargeSummary with currency_code: None unconditionally, new_agreement_value only when NO requested term commits an amount (so it is '0' for usage-based terms and ABSENT for every committed/prepaid purchase — the exact case the spec says carries the total contract value), and every ExpectedCharge with amount: None / amount_after_tax: None / estimated_taxes: None and every ItemizedCharge with incremental_charge_amount: None. taxConfiguration is persisted on the request record but never read, so newAgreementValueAfterTax / estimatedTaxes are never produced either.

The same hole propagates to the agreement: DescribeAgreement's estimatedCharges reports agreementValue only when NO accepted term commits an amount (Agreement::estimated_charges()) and currencyCode never, and ListAgreementCharges reports every charge with amount and currencyCode absent (ComAmazonaws...State::charges_of() copies the quote's empty amount). So no operation of this emulated service ever reports a money amount for a purchase that costs money.

The module docs in src/state.rs justify this by saying the proposal's rate card lives in another API; a deterministic synthetic rate card derived from the accepted agreementProposalIdentifier and the configured dimension keys/quantities would let every modelled amount member be computed and kept consistent across quote → agreement → charges.

## Evidence

Spec: CreateAgreementRequest operation doc ("calculates charges"); ChargeSummary$newAgreementValue ("The total value of the agreement, which includes any amendments"), ChargeSummary$currencyCode, ExpectedCharge$amount, ItemizedCharge$incrementalChargeAmount, Charge$amount ("The amount of the charge"), EstimatedCharges$agreementValue ("the total known amount customer has to pay... This is the total contract value if accepted terms contain ConfigurableUpfrontPricingTerm or FixedUpfrontPricingTerm" and only "0 if the accepted terms contain UsageBasedPricingTerm without ConfigurableUpfrontPricingTerm or RecurringPaymentTerm"). Note the model's best-effort carve-out covers only the tax/invoicing members, not newAgreementValue or the charge amounts. Handler/state: src/ops/create_agreement_request.rs calls store.charge_summary(...); src/state.rs charge_summary() (currency_code: None, new_agreement_value gated on !committed, ExpectedCharge amount: None, ItemizedCharge incremental_charge_amount: None), Agreement::estimated_charges(), State::charges_of(). Tests: tests/all/agreement_lifecycle.rs:432 asserts charge["amount"].is_null() and lines 305/485 assert newAgreementValue is absent for a committed term — the current (divergent) behavior is pinned by tests, so a fix must update them.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:5}]}}}])
2. Observe chargeSummary: currencyCode absent, newAgreementValue absent, expectedCharges[0] carries only id and timing (no amount), itemizedCharges[0] carries no incrementalChargeAmount
3. Repeat with taxConfiguration supplied: observe estimatedTaxes and newAgreementValueAfterTax are still absent
4. AcceptAgreementRequest(agreementRequestId=<id>) then DescribeAgreement(agreementId=<id>): observe estimatedCharges = {} (no agreementValue, no currencyCode) although the acceptor committed to a 12-month prepaid term
5. ListAgreementCharges(agreementId=<id>): observe each item carries no amount, no currencyCode and no time
