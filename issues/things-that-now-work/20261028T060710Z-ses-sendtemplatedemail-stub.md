Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.send-templated-email-validation-order
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.send-templated-email-validation-order|run#16|AssertionError|assert code_missing == "MessageRejected", (
Issue-Commit: 3b03347fcb752edcb4010bfe3eb49b815ec10bd1

# Implement ses SendTemplatedEmail: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3b03347fcb752edcb4010bfe3eb49b815ec10bd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTemplatedEmail

## Description
`SendTemplatedEmail` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_templated_email.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SendTemplatedEmail` call, so each one unblocks only when `SendTemplatedEmail` works: `ses.send-templated-email-validation-order`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/send_templated_email.rs`. Replaying the corpus, the emulator refused the `SendTemplatedEmail` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_ses_send_templated_email_validation_order.py", line 69, in run
    assert code_missing == "MessageRejected", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected MessageRejected (identity gate before template lookup) for a MISSING template, got 'NotImplemented': 'operation SendTemplatedEmail is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ses emulator
Call SendTemplatedEmail and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
