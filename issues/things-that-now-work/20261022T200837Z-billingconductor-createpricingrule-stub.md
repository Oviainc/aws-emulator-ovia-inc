Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billingconductor.create-pricing-rule-client-token-reuse-still-payer-gated billingconductor.create-pricing-rule-concurrent-denial-consistency billingconductor.create-pricing-rule-requires-payer-account
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor.create-pricing-rule-client-token-reuse-still-payer-gated|run#8|AssertionError|assert code_1 == "AccessDeniedException", f"first call: unexpected error code: {code_1} ({message_1})"
Issue-Key: billingconductor.create-pricing-rule-concurrent-denial-consistency|run#13|AssertionError|assert all(code == "AccessDeniedException" for _, code, _ in errors), (
Issue-Key: billingconductor.create-pricing-rule-requires-payer-account|run#5|AssertionError|assert code == "AccessDeniedException", f"unexpected error code: {code} ({message})"
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Implement billingconductor CreatePricingRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePricingRule

## Description
`CreatePricingRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/create_pricing_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreatePricingRule` call, so each one unblocks only when `CreatePricingRule` works: `billingconductor.create-pricing-rule-client-token-reuse-still-payer-gated`, `billingconductor.create-pricing-rule-concurrent-denial-consistency`, `billingconductor.create-pricing-rule-requires-payer-account`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/create_pricing_rule.rs`. Replaying the corpus, the emulator refused the `CreatePricingRule` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_billingconductor_create_pricing_rule_client_token_reuse_still_payer_gated.py", line 39, in run
    assert code_1 == "AccessDeniedException", f"first call: unexpected error code: {code_1} ({message_1})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: first call: unexpected error code: NotImplemented (operation CreatePricingRule is scaffolded but not implemented in this emulator)

## Steps to Reproduce
Boot the billingconductor emulator
Call CreatePricingRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
