Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior

# An auto-renewing agreement past its end date is never renewed: RENEWED is a status no code path in this crate can produce

_Filed: 2026-10-28T19:00:12Z_
_Operation: DescribeAgreement | Class: wrong-behavior_

## Description

`AgreementStatus` documents the other half of what happens at an agreement's end date: "RENEWED – The agreement was renewed into a new agreement (for example, an auto-renewal)", and `AgreementViewSummary$endTimeBehaviorType` states what the agreement that reports `RENEW` does at that date — "a new agreement is created from the accepted terms of this agreement". This crate reports `RENEW` for an agreement whose accepted renewal term enables auto-renewal (`Agreement::end_time_behavior` in `src/state.rs`), but nothing ever performs that renewal: no code path mints the renewal agreement, and `RENEWED` is a status no writer in the crate assigns.

The expiry side is implemented: `Agreement::expire` moves an agreement whose stored `end_time` has passed to `EXPIRED`, dated at that end date, and every operation that reports or guards on the status materialises it first (`src/state.rs`, `expire_agreement`/`expire_agreements`). It deliberately leaves an auto-renewing agreement alone, because `EXPIRED` would contradict the `RENEW` behaviour the same agreement reports — so such an agreement keeps reporting `ACTIVE`, `endTimeBehavior {type: RENEW}` and `PROVISIONED` entitlements however long ago its end date passed.

## Expected

Once an auto-renewing agreement reaches its stored `endTime`, the renewal the clock owes is materialised: the source agreement reports `RENEWED`, and a new agreement exists carrying the same accepted terms, the same `initialAgreementId` (so the chain traces back), a `startTime` at the source's end date and an `endTime` a further contract length on — bounded by the renewal term's `maxRenewals` — with its own charges, invoice and entitlements, and reportable through `DescribeAgreement`, `GetAgreementTerms`, `GetAgreementEntitlements` and `SearchAgreements`.

## Actual

`src/state.rs`: `Agreement::expire` returns early for `auto_renew() == Some(true)`, and a grep for `RENEWED` across `src/` matches only the `AgreementStatus` value list in `src/ops/types/shared_types.rs`. The `Status` filter value `RENEWED` therefore matches no agreement, and an agreement whose end date passed years ago is still reported as active with a renewal that never happened.

## Steps to Reproduce

1. CreateAgreementRequest with a `configurableUpfrontPricingTermConfiguration` (`selectorValue` `P1M`) and a `renewalTermConfiguration` whose `enableAutoRenew` is true, then AcceptAgreementRequest it.
2. Once that agreement's `endTime` is in the past, DescribeAgreement it: `status` is `ACTIVE` and `endTimeBehavior` is still `{type: RENEW}`; expected `RENEWED`, beside the new agreement the renewal created.
3. SearchAgreements with filters `[{"name":"AgreementType","values":["PurchaseAgreement"]},{"name":"Status","values":["RENEWED"]}]` — always an empty page, for any store.
