Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.duplicate-template-create-rejected-and-the-original-body-still-renders sesv2.test-render-email-template-assembles-a-multipart-alternative-mime-message sesv2.test-render-email-template-does-not-html-escape-substituted-values sesv2.test-render-email-template-mime-encodes-non-ascii-rendering-data sesv2.test-render-email-template-selects-the-mime-type-from-the-stored-parts sesv2.test-render-with-incomplete-template-data-rejected-complete-data-renders sesv2.updating-a-template-changes-what-the-renderer-produces
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: sesv2|TestRenderEmailTemplate|NotImplemented
Issue-Key: sesv2.test-render-with-incomplete-template-data-rejected-complete-data-renders|run#10|AssertionError|assert code == 'BadRequestException', (
Issue-Commit: de2a758d888a26724f6774bb8d661e6e5775621b

# Implement ses TestRenderEmailTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `de2a758d888a26724f6774bb8d661e6e5775621b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestRenderEmailTemplate

## Description
`TestRenderEmailTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/test_render_email_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `TestRenderEmailTemplate` call, so each one unblocks only when `TestRenderEmailTemplate` works: `ses.duplicate-template-create-rejected-and-the-original-body-still-renders`, `ses.test-render-email-template-assembles-a-multipart-alternative-mime-message`, `ses.test-render-email-template-does-not-html-escape-substituted-values`, `ses.test-render-email-template-mime-encodes-non-ascii-rendering-data`, `ses.test-render-email-template-selects-the-mime-type-from-the-stored-parts`, `ses.test-render-with-incomplete-template-data-rejected-complete-data-renders`, `ses.updating-a-template-changes-what-the-renderer-produces`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/test_render_email_template.rs`. Replaying the corpus, the emulator refused the `TestRenderEmailTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestRenderEmailTemplate operation: operation TestRenderEmailTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call TestRenderEmailTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
