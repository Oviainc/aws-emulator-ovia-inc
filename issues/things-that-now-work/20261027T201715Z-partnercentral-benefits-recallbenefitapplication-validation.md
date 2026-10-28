Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a2ecdf5e6192d2ba8b13fcdc078fccacd8d14637

# RecallBenefitApplication persists an unbounded ClientToken and Reason, breaking the bounds this crate documents and its siblings enforce

_Filed: 2026-10-27T20:17:15Z_
_Operation: RecallBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `a2ecdf5e6192d2ba8b13fcdc078fccacd8d14637`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RecallBenefitApplicationInput` is the one write operation of this service whose `ClientToken` and `Reason` carry NO modelled `@length` (the model types both as a bare `String`), so the centralized constraint walk refuses nothing for them. The handler then persists both caller strings verbatim: `Reason` becomes the application's `StatusReason` and the token becomes the `SpentToken` kept on the record — with no length check anywhere on the path.

Two things in this crate say that cannot happen:

1. `crate::state` documents the stored token as bounded, and says so because *every* operation's token is modelled bounded — which is false for exactly this one. `SpentToken::client_token` (the field this handler writes) carries the doc "at most [`MAX_CLIENT_TOKEN_LEN`] bytes", and `MAX_CLIENT_TOKEN_LEN` is justified as "the modelled `@length(min: 1, max: 64)` every operation's `ClientToken` carries". A recall carrying a 1 MB token is stored whole, so the store's own byte accounting no longer holds.

2. The sibling write `AmendBenefitApplication` re-checks precisely these two kinds of string in-handler, with the stated reason that they are what the operation PERSISTS and the store's accounting cites the bounds — and refuses them as the modelled `ValidationException`. `RecallBenefitApplication`, whose model supplies no bound at all, skips the check the sibling applies when the model already supplies one.

The `Reason` half also leaves the read reporting a value outside its own modelled maximum: `GetBenefitApplicationOutput$StatusReason` is modelled `length {max: 1000}`, and `CancelBenefitApplicationInput$Reason` — the other operation that writes `StatusReason` — is modelled `length {max: 1000}` so the walk refuses a longer one there. Recall the same application with a 5 000-character `Reason` and `GetBenefitApplication` answers 200 with a `StatusReason` five times the modelled maximum.

## Evidence

Model (smithy_query query="operation" operation="RecallBenefitApplication"), input members, neither constrained:
  "ClientToken": "String"
  "Reason": {"target": "String", "required": true}
Compare the same two kinds of member elsewhere in the model:
  CancelBenefitApplicationInput: "Reason": {"target": "String", "length": {"max": 1000}, "pattern": "^[\\s\\S]{1,1000}$"}, "ClientToken": {... "length": {"min": 1, "max": 64} ...}
  GetBenefitApplicationOutput: "StatusReason": {"target": "String", "length": {"max": 1000}}

The handler persists both with no check — services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/recall_benefit_application.rs:257-271:
    application.set_status(
        crate::state::STATUS_PENDING_SUBMISSION,
        Some(input.reason),
    );
    application.revise(now);
    application.recall_client_token = input.client_token.map(|client_token| {
        crate::state::SpentToken { client_token, request_fingerprint: fingerprint }
    });
(there is no `length_clause`/`details_rejection`-style guard anywhere in this file)

The crate's own claim that this is impossible — src/state.rs:372-376:
    /// The longest `ClientToken` this service stores, which is the modelled
    /// `@length(min: 1, max: 64)` every operation's `ClientToken` carries (the patterns
    /// `^[a-zA-Z0-9-]{1,64}$` and `^[!-~]{1,64}$` say the same). The applied-write record
    /// keeps the token, so this bounds the bytes one record holds for it.
    pub const MAX_CLIENT_TOKEN_LEN: usize = 64;
and src/state.rs:1013-1015, the struct this handler writes:
    pub struct SpentToken {
        /// The request's `ClientToken`, at most [`MAX_CLIENT_TOKEN_LEN`] bytes.
        pub client_token: String,

The sibling that does apply the check — src/ops/amend_benefit_application.rs:464-480:
    // The two caller-chosen strings this operation PERSISTS, checked at the door of
    // the store whose byte accounting cites these bounds. ... this is the
    // enforcement the store itself relies on, not a restatement of the walk's job.
    if let Some(clause) = length_clause(&input.amendment_reason, 0, crate::state::MAX_AMENDMENT_REASON_LEN) {
        return Err(refused("amendmentReason", &clause));
    }
    if let Some(clause) = length_clause(&input.client_token, MIN_LEN, crate::state::MAX_CLIENT_TOKEN_LEN) {
        return Err(refused("clientToken", &clause));
    }

Test coverage: `tests/all/client_token_idempotency.rs:209-252` and `tests/all/cancel_client_token_idempotency.rs:172-173` pin that a token of `MAX_CLIENT_TOKEN_LEN + 1` is refused 400 on Update/Cancel; `tests/all/amend_benefit_application.rs:777,813` pin the same for Amend. `tests/all/recall_benefit_application.rs` exercises recall's token only for idempotency (line 495 `recall_is_idempotent_in_its_client_token`) and its `Reason` only for shape, so no test states a reason for recall accepting an unbounded value.

## Steps to Reproduce

1. Boot the emulator and grant the caller's account the partner entitlement (state::grant_partner_entitlement), as the lifecycle tests do
2. CreateBenefitApplication in catalog `AWS` for an ACTIVE benefit, then SubmitBenefitApplication so the record is IN_REVIEW (recallable)
3. Call RecallBenefitApplication with Catalog=AWS, Identifier=<the id>, ClientToken="t".repeat(100_000) and Reason="x".repeat(5000)
4. Observe HTTP 200: neither the 64-byte token bound the crate documents nor the 1000-character StatusReason maximum the model declares is applied (compare the same call shape on CancelBenefitApplication/AmendBenefitApplication, which answer 400 ValidationException)
5. Call GetBenefitApplication for the same id and observe StatusReason is 5000 characters, outside the modelled `GetBenefitApplicationOutput$StatusReason` `length {max: 1000}`; the 100 000-byte token stays on the stored record in `recall_client_token`
