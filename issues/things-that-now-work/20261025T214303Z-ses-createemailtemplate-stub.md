Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.concurrent-template-create-exactly-one-body-renders sesv2.duplicate-template-create-rejected-and-the-original-body-still-renders sesv2.email-template-contract-limits-rejected sesv2.test-render-email-template-assembles-a-multipart-alternative-mime-message sesv2.test-render-email-template-does-not-html-escape-substituted-values sesv2.test-render-email-template-mime-encodes-non-ascii-rendering-data sesv2.test-render-email-template-selects-the-mime-type-from-the-stored-parts sesv2.test-render-with-incomplete-template-data-rejected-complete-data-renders sesv2.updating-a-template-changes-what-the-renderer-produces
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: sesv2|CreateEmailTemplate|NotImplemented
Issue-Related: 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# Implement ses CreateEmailTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEmailTemplate

## Description
`CreateEmailTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateEmailTemplate` call, so each one unblocks only when `CreateEmailTemplate` works: `ses.concurrent-template-create-exactly-one-body-renders`, `ses.duplicate-template-create-rejected-and-the-original-body-still-renders`, `ses.email-template-contract-limits-rejected`, `ses.test-render-email-template-assembles-a-multipart-alternative-mime-message`, `ses.test-render-email-template-does-not-html-escape-substituted-values`, `ses.test-render-email-template-mime-encodes-non-ascii-rendering-data`, `ses.test-render-email-template-selects-the-mime-type-from-the-stored-parts`, `ses.test-render-with-incomplete-template-data-rejected-complete-data-renders`, `ses.updating-a-template-changes-what-the-renderer-produces`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs`. Replaying the corpus, the emulator refused the `CreateEmailTemplate` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ses_concurrent_template_create_exactly_one_body_renders.py", line 106, in run
    assert len(winners) == 1, (
           ^^^^^^^^^^^^^^^^^
AssertionError: exactly one of 5 simultaneous creates of the same template name may be accepted, got 0 winners []; losers reported [(0, {'Message': 'operation CreateEmailTemplate is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}), (1, {'Message': 'operation CreateEmailTemplate is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}), (2, {'Message': 'operation CreateEmailTemplate is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}), (3, {'Message': 'operation CreateEmailTemplate is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}), (4, {'Message': 'operation CreateEmailTemplate is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'})]

## Steps to Reproduce
Boot the ses emulator
Call CreateEmailTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
