Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: billing.get-billing-preferences-access-denied-without-billing-access billing.update-billing-preferences-access-denied-rejected
Issue-Shape-Id: com.amazonaws.billing#AWSBilling
Issue-Key: billing.get-billing-preferences-access-denied-without-billing-access|run#3|AssertionError|assert code == "AccessDeniedException", (feature, code, message)
Issue-Key: billing|GetBillingPreferences|NotImplemented
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# Implement billing GetBillingPreferences: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBillingPreferences

## Description
`GetBillingPreferences` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetBillingPreferences` call, so each one unblocks only when `GetBillingPreferences` works: `billing.get-billing-preferences-access-denied-without-billing-access`, `billing.update-billing-preferences-access-denied-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_preferences.rs`. Replaying the corpus, the emulator refused the `GetBillingPreferences` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_billing_get_billing_preferences_access_denied_without_billing_access.py", line 27, in run
    assert code == "AccessDeniedException", (feature, code, message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('BILLING_ALERTS', 'NotImplemented', 'operation GetBillingPreferences is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the billing emulator
Call GetBillingPreferences and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
