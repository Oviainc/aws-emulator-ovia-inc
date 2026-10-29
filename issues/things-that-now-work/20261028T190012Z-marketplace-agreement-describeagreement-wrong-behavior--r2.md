Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# An agreement past its own stored endTime is still reported ACTIVE: EXPIRED is a status no code path in this crate can ever produce

_Filed: 2026-10-28T19:00:12Z_
_Operation: DescribeAgreement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeAgreement` reports `status` straight off the stored record and never consults the agreement's own `endTime`, which the acceptance stored as the instant the agreement's accepted terms commit it until. The model states the transition explicitly for this very member: `EXPIRED` – "The agreement ended on the defined end date." No code path in this crate ever assigns that status: `Agreement::leave_active` is the single writer of an agreement's status and is called exactly twice — `cancel` (CANCELLED) and `replace` (REPLACED) — so an agreement whose `endTime` has passed stays `ACTIVE` forever.

This contradicts the rule this crate states for exactly this situation. A due instant stored on a record is materialised by the operations that report it: `settle_billing_adjustments` turns a `PENDING` adjustment `COMPLETED` at its stored `completes_at` because "the processing is owed at `completes_at` whether or not anybody calls, so a reporting operation materialises it before it reports", and `Agreement::entitlement_state` derives an entitlement's status from the agreement and `now` without any write at all. The agreement's own `end_time` is the same kind of stored due instant, and no reader materialises or derives anything from it.

The consequences are visible across the crate: `SearchAgreements`' documented `Status` filter value `EXPIRED` can never match any agreement; `AgreementViewSummary$status` reports `ACTIVE` for an ended agreement; `end_time_behavior()` keeps reporting `type: EXPIRE` with reason `NO_RENEWAL_TERM` for an agreement whose end date is already in the past; its entitlements keep reporting `PROVISIONED`/`AGREEMENT_ACTIVE`; and `SendAgreementPaymentRequest` and `SendAgreementCancellationRequest` keep accepting new requests under an agreement that has ended, because their `inactive_agreement()` guard tests the status that was never moved.

## Evidence

The model states the transition — `smithy_query(query="doc", shape="DescribeAgreementOutput", member="status")`: "<code>ACTIVE</code> – The terms of the agreement are active. … <code>EXPIRED</code> – The agreement ended on the defined end date."

The handler reports the stored status with no clock involved — `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/describe_agreement.rs:185-200`:
```
        // A pure report: it reads the stored agreement and writes nothing.
        let store = state.lock();
        let Some(agreement) = store.agreements.get(&input.agreement_id) else {
...
        Ok(agreement.to_description(estimated_charges))
```
and `src/state.rs:2355` renders it verbatim: `status: Some(self.status.clone()),`.

Nothing can ever set EXPIRED — `src/state.rs:2186-2194` is the single writer:
```
    #[must_use]
    fn leave_active(&mut self, status: &str, now: SystemTime) -> bool {
        if self.status != STATUS_ACTIVE {
            return false;
        }
```
and its only two callers are `src/state.rs:2206` `self.leave_active(STATUS_CANCELLED, now)` and `src/state.rs:2220` `self.leave_active(STATUS_REPLACED, now)`. A grep for `EXPIRED` across `src/` matches only the enum value list (`src/ops/types/shared_types.rs:165`) — no handler and no state writer uses it. Its own doc even names the status it can never reach — `src/state.rs:2180-2182`: "one that has already reached a terminal status (cancelled, replaced, expired) keeps it".

The due instant IS stored — `src/state.rs:2119-2123`:
```
    /// "The field is null for pay-as-you-go agreements, which don't have end
    /// dates": an agreement ends when the duration its accepted terms commit it
    /// for runs out ([`agreement_end_time`]) ...
    pub end_time: Option<SystemTime>,
```
and the rule the crate applies to a stored due instant — `src/state.rs:4578-4592`:
```
    /// Materialise every billing adjustment status change the clock has already
    /// made due by `now`.
    ///
    /// A request is submitted `PENDING` and AWS Marketplace processes it; the
    /// processing is owed at `completes_at` whether or not anybody calls, so a
    /// reporting operation materialises it before it reports.
    pub fn settle_billing_adjustments(&mut self, now: SystemTime) {
```
called by both billing readers (`src/ops/get_billing_adjustment_request.rs:180-182`, `src/ops/list_billing_adjustment_requests.rs:244-247`), and the clock-derived projection that needs no write at all — `src/state.rs:2287-2307` `entitlement_state(..., now)`, which already branches on `now < self.start_time` but never on `self.end_time`.

Test coverage: `tests/all/agreement_end_dates.rs` asserts only the endTime an acceptance computes (line 449: "the agreements that expire on their end date") and `tests/all/agreement_lifecycle.rs` asserts the ACTIVE→CANCELLED/REPLACED transitions; no test states a reason for an ended agreement staying ACTIVE.

## Steps to Reproduce

1. CreateAgreementRequest with a `configurableUpfrontPricingTermConfiguration` whose `selectorValue` is `P1M` (the shortest contract length `CONTRACT_LENGTHS` publishes, 30 days), then AcceptAgreementRequest it.
2. DescribeAgreement the new agreement: `endTime` = acceptance + 30 days, `status` = `ACTIVE`, `endTimeBehavior` = `{type: EXPIRE, reasonCode: NO_RENEWAL_TERM}`.
3. Once that `endTime` is in the past (or, equivalently, by inspection: `leave_active` is the only writer of `Agreement::status` and is only ever called with `STATUS_CANCELLED`/`STATUS_REPLACED`), DescribeAgreement again. Expected per the member's own documentation: `EXPIRED` – "The agreement ended on the defined end date." Actual: `ACTIVE`, unchanged, with `endTime` still in the past.
4. SearchAgreements with filters `[{"name":"AgreementType","values":["PurchaseAgreement"]},{"name":"Status","values":["EXPIRED"]}]` — always an empty page, for any store, since no record can carry that status.
5. GetAgreementEntitlements on the same agreement — still `PROVISIONED`/`AGREEMENT_ACTIVE`; and SendAgreementPaymentRequest under its `VariablePaymentTerm` is still accepted, because the `inactive_agreement()` guard tests a status the clock never moved.
