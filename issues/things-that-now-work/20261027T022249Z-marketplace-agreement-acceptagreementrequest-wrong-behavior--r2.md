Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# Accepting a REPLACE request overwrites the source agreement's terminal status

_Filed: 2026-10-27T02:22:49Z_
_Operation: AcceptAgreementRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the accepted agreement request carries `intent: REPLACE`, the handler writes `source.status = STATUS_REPLACED` directly on the source agreement with NO check that the source is still ACTIVE. Every other path in this crate that moves an agreement's status refuses to move one that has already reached a terminal status: `Agreement::cancel` is documented and implemented as "the one transition" that "keeps" a terminal status "rather than being moved backwards", and `AcceptAgreementCancellationRequest` goes through it for exactly that reason. The REPLACE path bypasses that single writer and assigns the status unconditionally.

Consequence: an agreement the acceptor already CANCELLED (or that a previous replacement already left REPLACED) is silently rewritten to REPLACED by a later `AcceptAgreementRequest`. The cancellation recorded on the agreement is lost — `DescribeAgreement` and `SearchAgreements` then report REPLACED for an agreement that was cancelled, and the `Status` search filter no longer finds it under CANCELLED. The crate's own sibling rule and its own test for the sibling say this must not happen.

The defect is the unguarded write, not the absence of a new refusal: the guarded writer the crate already owns (`Agreement::cancel`'s "only an ACTIVE agreement is moved" rule) is simply not applied here.

## Evidence

The unguarded write — services/.../src/ops/accept_agreement_request.rs:333-341:
```
        if request.intent == crate::state::INTENT_REPLACE {
            if let Some(source) = request
                .source_agreement_identifier
                .as_deref()
                .and_then(|source| store.agreements.get_mut(source))
            {
                source.status = crate::state::STATUS_REPLACED.to_string();
                source.updated_at = now;
```
(no `source.status == STATUS_ACTIVE` test anywhere in the block).

The rule it breaks — services/.../src/state.rs:2040-2053:
```
    /// Only an [`STATUS_ACTIVE`] agreement is cancelled; one that has already
    /// reached a terminal status (cancelled, replaced, expired) keeps it rather
    /// than being moved backwards. ...
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if self.status != STATUS_ACTIVE {
            return false;
        }
```

The sibling that honours it — services/.../src/ops/accept_agreement_cancellation_request.rs:285-292:
```
        // ... and an agreement that has already reached a
        // terminal status (cancelled, replaced, expired) keeps it rather than
        // being moved backwards ...
        let cancelled = store
            .agreements
            .get_mut(&input.agreement_id)
            .is_some_and(|agreement| agreement.cancel(now));
```

The invariant is pinned for the sibling but never for this path — services/.../tests/all/cancellation_request_decisions.rs:392-397,424-428:
```
/// An agreement that has already left `ACTIVE` keeps the status it reached: ...
/// cancellation never moves an agreement backwards into a status it has already
/// passed.
#[tokio::test]
async fn accepting_a_request_over_an_already_cancelled_agreement_keeps_its_status() {
...
    assert_eq!(
        agreement_status(&router, &agreement_id).await,
        AGREEMENT_CANCELLED,
        "the agreement keeps the terminal status it already reached"
    );
```
No test in tests/all/ exercises a REPLACE accepted over a non-ACTIVE source (grep for `REPLACE` in tests/all shows only replacements of active agreements, e.g. tests/all/inactive_agreement_refusals.rs:151-177).

Nothing stops the sequence: `CreateAgreementRequest` only checks that the source exists — services/.../src/ops/create_agreement_request.rs:277-288 `if !store.agreements.contains_key(source) { ... not_found ... }` — and `AcceptAgreementRequest` likewise only resolves it, services/.../src/ops/accept_agreement_request.rs:192-203.

## Steps to Reproduce

1. CreateAgreementRequest with intent NEW and some requestedTerms, then AcceptAgreementRequest on the returned agreementRequestId — note the agreement id A.
2. CreateAgreementRequest with intent REPLACE, agreementProposalIdentifier set and sourceAgreementIdentifier = A — note the agreement request id ar-2. (Accepted: the source only has to exist.)
3. CancelAgreement with agreementId = A. DescribeAgreement(A) now reports status CANCELLED.
4. AcceptAgreementRequest with agreementRequestId = ar-2. It returns 200.
5. DescribeAgreement(A) again: status is now REPLACED. Expected per Agreement::cancel's documented rule and the sibling's pinned behaviour: A keeps the terminal status CANCELLED it already reached.
