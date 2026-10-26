Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.send-bulk-email-missing-default-template-data-rejected sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default
Issue-Key: sesv2|SendBulkEmail|NotImplemented
Issue-Related: 20261023T183615Z-sesv2-is-unreachable-every-request-is-refused-ambiguousservice.md
Issue-Commit: 2e41c53f62212c879ccde9aca1b2ab6c6cf6a645

# Implement sesv2 SendBulkEmail: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e41c53f62212c879ccde9aca1b2ab6c6cf6a645`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendBulkEmail

## Description
`SendBulkEmail` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_bulk_email.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `SendBulkEmail` call, so each one unblocks only when `SendBulkEmail` works: `sesv2.send-bulk-email-missing-default-template-data-rejected`, `sesv2__sns__sqs.send-bulk-email-per-entry-replacement-headers-override-the-template-default`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_bulk_email.rs`. Replaying the corpus, the emulator refused the `SendBulkEmail` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sesv2_send_bulk_email_missing_default_template_data_rejected.py", line 44, in run
    assert err == expected, f'expected {expected!r}, got {err!r}'
           ^^^^^^^^^^^^^^^
AssertionError: expected ('BadRequestException', 'The default template data does not exist'), got ('NotImplemented', 'operation SendBulkEmail is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sesv2 emulator
Call SendBulkEmail and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
