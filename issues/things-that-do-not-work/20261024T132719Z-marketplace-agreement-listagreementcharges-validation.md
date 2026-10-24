Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# catalog and agreementType values are never validated: an unknown catalog or agreement type answers 200 with an empty page instead of ValidationException (INVALID_CATALOG / INVALID_AGREEMENT_TYPE)

_Filed: 2026-10-24T13:27:19Z_
_Operation: ListAgreementCharges | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`catalog` is "the catalog in which the charges were created" and AWS Marketplace exposes exactly one catalog, `AWSMarketplace`; `agreementType` is "filter to retrieve charges of a specific agreement type", whose documented values are `PurchaseAgreement` and `VendorInsightsAgreement` (DescribeAgreementOutput$agreementType). Both members are modelled as pattern-constrained plain strings, so nothing before the handler can check which VALUE they carry — the service's own ValidationExceptionReason enum models `INVALID_CATALOG` and `INVALID_AGREEMENT_TYPE` precisely because the service refuses a value outside the known set with a 400 ValidationException.

Expected: `ListAgreementCharges(catalog="NotARealCatalog")` and `ListAgreementCharges(agreementType="Nonsense")` are refused 400 ValidationException with reason INVALID_CATALOG / INVALID_AGREEMENT_TYPE.

Actual: the handler treats both members purely as string equality filters against the stored agreement (`input.catalog.as_deref().is_some_and(|catalog| catalog != agreement.catalog)`), so an unknown value simply selects nothing and the call answers 200 with `items: []` and no nextToken. A client that misspells the catalog cannot tell "you asked for a catalog that does not exist" from "this account has no charges"; negative-path handling keyed on ValidationException never fires. Nothing in the crate validates these two members: `grep INVALID_CATALOG|INVALID_AGREEMENT_TYPE src/` matches only the generated reason-value list in src/ops/types/shared_types.rs.

The same untreated members appear on the sibling listings and should be fixed with one shared rule: ListAgreementCancellationRequests (`catalog`, `agreementType`), ListAgreementPaymentRequests (`catalog`, `agreementType`), ListBillingAdjustmentRequests (`catalog`, `agreementType`), and SearchAgreements' `catalog` member plus its `AgreementType` filter value (state::matches_filter does plain equality on the value, and state::validate_filters checks only the PartyType value).

## Evidence

Spec: ValidationExceptionReason (smithy_query shape=ValidationExceptionReason) models INVALID_CATALOG and INVALID_AGREEMENT_TYPE; ListAgreementChargesInput$catalog/$agreementType docs; DescribeAgreementOutput$agreementType documents the two agreement types; src/state.rs CATALOG="AWSMarketplace" records the single catalog the emulator mints agreements in. Handler: src/ops/ops/list_agreement_charges.rs lines ~203-219 filter on equality only — the handler DOES validate the agreementId member (ValidationException INVALID_AGREEMENT_ID when the store holds no such agreement) but applies no comparable rule to catalog or agreementType. Same pattern in src/ops/list_agreement_cancellation_requests.rs (~228-241), src/ops/list_agreement_payment_requests.rs (~221-230), src/ops/list_billing_adjustment_requests.rs (~238-246), and src/state.rs matches_filter("AgreementType"). Tests: tests/all/validation_messages.rs covers sort/partyType/nextToken refusals only; no test asserts a catalog or agreementType refusal, and the filter tables in tests/all/agreement_charges.rs assert that an unknown value selects nothing (200).

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:5}]}}}]) then AcceptAgreementRequest to store one agreement with charges
2. ListAgreementCharges(catalog="NotARealCatalog") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_CATALOG)
3. ListAgreementCharges(agreementType="Nonsense") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_TYPE)
4. ListAgreementPaymentRequests(partyType="Acceptor", catalog="NotARealCatalog") and ListAgreementCancellationRequests(partyType="Acceptor", agreementType="Nonsense") and ListBillingAdjustmentRequests(catalog="NotARealCatalog") -> same 200-with-empty-page instead of the modelled refusal
5. SearchAgreements(catalog="NotARealCatalog", filters=[{name:"AgreementType", values:["Nonsense"]}]) -> observe 200 with an empty agreementViewSummaries instead of ValidationException
