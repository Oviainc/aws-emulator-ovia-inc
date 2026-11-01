Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# Accepting an AMEND request modifies nothing: the amended agreement keeps its old terms, value and lastUpdateTime

_Filed: 2026-10-29T10:40:56Z_
_Operation: AcceptAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAgreementRequest` prices an amendment correctly (the quote reports `oldQuantity`/`newQuantity`/`incrementalChargeAmount` against the source agreement), but `AcceptAgreementRequest` never applies that amendment to the agreement it acts upon. The handler mints a brand-new agreement id for EVERY intent and the only write it makes to the source is the `REPLACE` transition; the `AMEND` branch does not exist. So after an accepted `AMEND`:

* the amended agreement's `accepted_terms` still hold the pre-amendment dimensions, so `GetAgreementTerms` reports the old configuration;
* its `updated_at` is untouched, so `AgreementViewSummary$lastUpdateTime` still reports the acceptance instant;
* `DescribeAgreement` on it reads `estimated_charges` from its ORIGINAL quote, so `EstimatedCharges$agreementValue` excludes the units the amendment bought;
* it stays `ACTIVE` beside the newly minted agreement, so one chain (`initialAgreementId`) now holds two ACTIVE agreements, each granting its own licence for the same resources.

The model states the opposite in three places: the intent itself modifies the existing agreement, an amendment is given as an example of what updates an agreement's `lastUpdateTime`, and `agreementValue` is documented to include the added cost after an amendment. Note the fix is NOT to move the source out of `ACTIVE` (the crate's `leave_active` guard, added by a previous fixed issue, must stay): the amendment has to be applied to the existing record.

## Evidence

Smithy, `CreateAgreementRequestInput$intent` (smithy_query doc): "<code>AMEND</code> – Modifies an existing agreement with terms that are accepted in the request."

Smithy, `AgreementViewSummary$lastUpdateTime` (smithy_query doc): "The date and time when the agreement was last updated. An agreement is updated when any of its attributes or accepted terms change. Amendments, renewals, and a party changing whether the agreement renews are all examples."

Smithy, `EstimatedCharges$agreementValue` (smithy_query doc): "In case a customer has amended an agreement, by purchasing more units of any dimension, this will include both the original cost as well as the added cost incurred due to addition of new units."

Handler — a new agreement is minted whatever the intent, `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/accept_agreement_request.rs:232`:
```
        let id = store.mint_agreement_id();
```
and the only write to the source, same file lines 349-363:
```
        // `AgreementStatus` documents the status the agreement a replacement
        // acts upon carries from then on: "REPLACED – ..." What an AMEND leaves its source
        // in is not documented, so the emulator does not move it.
        ...
        if request.intent == crate::state::INTENT_REPLACE {
            if let Some(source) = request
                .source_agreement_identifier
                ...
                if source.replace(now) {
```
There is no `INTENT_AMEND` branch anywhere in the file (grep for `INTENT_AMEND` in `src/ops/` hits only `create_agreement_request.rs:195`, the "required if the intent is not AMEND" check).

The crate's own state helper set confirms nothing applies an amendment: `src/state.rs:2283` `pub fn replace(&mut self, now: SystemTime) -> bool` and `src/state.rs:2298` `pub fn renew(...)` exist, but there is no writer that re-states a source agreement's `accepted_terms`, `end_time` or `updated_at`.

The quote side already computes the amendment, so the data exists — `tests/all/agreement_lifecycle.rs:874-882`:
```
    assert_eq!(
        reported,
        vec![
            ("Users".to_string(), 9, 4),
            ("Storage".to_string(), 100, 0),
        ],
        "an amendment reports the source agreement's quantities as oldQuantity, got {:?}",
```

Existing coverage pins the defect: `tests/all/replacement_source_status.rs:252` iterates `for intent in [REPLACE, AMEND]` and lines 287-293 assert the source is untouched by an AMEND:
```
            let after = last_update_time(&router, &source).await;
            if expected == state.status() {
                assert_eq!(
                    after, before,
                    "case {case}: a call that moves nothing leaves lastUpdateTime at {before}, \
                     got {after}"
                );
```
Its stated reason (`tests/all/replacement_source_status.rs:22-23`: "`AMEND`, which documents no move at all") is about the STATUS transition only; the `lastUpdateTime` doc sentence quoted above contradicts it for this member, and the `intent` doc contradicts leaving the record unmodified at all.

## Steps to Reproduce

1. POST CreateAgreementRequest with intent=NEW and a ConfigurableUpfrontPricingTerm configuring Users=4 at selectorValue P12M; AcceptAgreementRequest the returned agreementRequestId to get agreement A.
2. DescribeAgreement(A) and note `estimatedCharges.agreementValue`; SearchAgreements and note A's `lastUpdateTime`; GetAgreementTerms(A) and note the configured Users quantity.
3. POST CreateAgreementRequest with intent=AMEND, sourceAgreementIdentifier=A and the same term configuring Users=9 — the returned chargeSummary correctly reports oldQuantity 4 / newQuantity 9.
4. AcceptAgreementRequest that amendment request. It answers 200 with a NEW agreementId B.
5. DescribeAgreement(A) again: status is still ACTIVE, `estimatedCharges.agreementValue` is still the pre-amendment total, and GetAgreementTerms(A) still reports Users=4.
6. SearchAgreements again: A's `lastUpdateTime` is unchanged, and both A and B are ACTIVE under the same `initialAgreementId`.
