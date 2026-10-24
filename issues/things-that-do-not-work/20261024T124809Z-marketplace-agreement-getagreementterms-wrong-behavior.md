Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Accepted terms are synthesized from the request: the term type is guessed from the term id and every payload member beyond type/id/configuration is absent

_Filed: 2026-10-24T12:48:09Z_
_Operation: GetAgreementTerms | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetAgreementTerms reports the accepted terms of an agreement. Expected: each AcceptedTerm carries the term as the agreement holds it — the type resolved from the proposal's term, plus the term's own payload (e.g. ValidityTerm$agreementStartDate/agreementEndDate, ConfigurableUpfrontPricingTerm$currencyCode and $rateCards, FixedUpfrontPricingTerm$price/$duration, LegalTerm$documents, SupportTerm$refundPolicy, UsageBasedPricingTerm$rateCards, VariablePaymentTerm$currencyCode/$maxTotalChargeAmount).

Actual: crate::state::accepted_term() constructs the accepted term from the acceptor's RequestedTerm alone, filling only `type`, `id` and (for the three configurable kinds) `configuration`, with `..Default::default()` for everything else — so every modelled payload member is absent for every agreement. Worse, the TYPE itself is guessed: term_kind() reads the kind from the configuration union when one is present, otherwise it string-matches requestedTerms[].id against TERM_KINDS and falls back to DEFAULT_TERM_KIND = "LegalTerm". A real proposal term id (e.g. "abc123term" or "term-1") therefore comes back as an accepted legalTerm the acceptor never requested, and a caller cannot distinguish a genuine legal term from an unresolved one.

## Evidence

Spec: GetAgreementTerms documents the accepted term as carrying TermType, TermID and Configuration, and the AcceptedTerm union members in the model each declare further payload members (ValidityTerm$agreementStartDate/$agreementEndDate, ConfigurableUpfrontPricingTerm$rateCards/$currencyCode, FixedUpfrontPricingTerm$price, LegalTerm$documents, UsageBasedPricingTerm$rateCards, VariablePaymentTerm$currencyCode/$maxTotalChargeAmount). Handler/state: src/ops/get_agreement_terms.rs returns agreement.accepted_terms verbatim; src/state.rs accepted_term() (every arm `..Default::default()`) and term_kind()/DEFAULT_TERM_KIND (id string match, LegalTerm fallback). Test coverage: tests/all/agreement_lifecycle.rs exercises GetAgreementTerms only for type/id/configuration round-tripping; no test asserts any other payload member or the type-resolution rule.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"term-1"}, {id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:2}]}}}]) then AcceptAgreementRequest
2. GetAgreementTerms(agreementId=<id>) -> observe the first term reported as {legalTerm:{type:"LegalTerm", id:"term-1"}} with no documents, although nothing in the request said it was a legal term
3. Observe the configurableUpfrontPricingTerm carries only type/id/configuration: no currencyCode, no rateCards, no price
4. Observe no validityTerm with agreementStartDate/agreementEndDate is reported for a 12-month prepaid purchase
