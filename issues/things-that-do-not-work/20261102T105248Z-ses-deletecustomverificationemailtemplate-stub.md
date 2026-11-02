Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.custom-verification-template-unverified-sender-and-absent-name-rejected ses.custom-verification-template-update-then-delete-then-not-found
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|DeleteCustomVerificationEmailTemplate|NotImplemented
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# Implement ses DeleteCustomVerificationEmailTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomVerificationEmailTemplate

## Description
`DeleteCustomVerificationEmailTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_custom_verification_email_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteCustomVerificationEmailTemplate` call, so each one unblocks only when `DeleteCustomVerificationEmailTemplate` works: `ses.custom-verification-template-unverified-sender-and-absent-name-rejected`, `ses.custom-verification-template-update-then-delete-then-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/delete_custom_verification_email_template.rs`. Replaying the corpus, the emulator refused the `DeleteCustomVerificationEmailTemplate` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteCustomVerificationEmailTemplate operation: operation DeleteCustomVerificationEmailTemplate is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call DeleteCustomVerificationEmailTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
