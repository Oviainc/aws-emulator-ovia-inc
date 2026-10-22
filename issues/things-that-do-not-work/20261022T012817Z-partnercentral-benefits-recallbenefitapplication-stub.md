Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-benefits.recall-benefit-application-malformed-identifier-rejected
Issue-Shape-Id: com.amazonaws.partnercentralbenefits#PartnerCentralBenefitsService
Issue-Key: partnercentral-benefits.recall-benefit-application-malformed-identifier-rejected|run#3|AssertionError|assert positive == ("AccessDeniedException", _DENIED), (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement partnercentral-benefits RecallBenefitApplication: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecallBenefitApplication

## Description
`RecallBenefitApplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/recall_benefit_application.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `RecallBenefitApplication` call, so each one unblocks only when `RecallBenefitApplication` works: `partnercentral-benefits.recall-benefit-application-malformed-identifier-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/recall_benefit_application.rs`. Replaying the corpus, the emulator refused the `RecallBenefitApplication` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_partnercentral_benefits_recall_benefit_application_malformed_identifier_rejected.py", line 35, in run
    assert positive == ("AccessDeniedException", _DENIED), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: positive control: a well-formed but nonexistent identifier should pass input validation and be refused at authorization, got ('NotImplemented', 'operation RecallBenefitApplication is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the partnercentral-benefits emulator
Call RecallBenefitApplication and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
