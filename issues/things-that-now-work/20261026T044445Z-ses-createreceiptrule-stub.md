Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
Issue-Key: ses|CreateReceiptRule|NotImplemented
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# Implement ses CreateReceiptRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReceiptRule

## Description
`CreateReceiptRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_receipt_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreateReceiptRule` call, so each one unblocks only when `CreateReceiptRule` works: `s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers`, `s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule`, `s3__ses__sesv2.raw-multipart-attachment-arrives-byte-exact`, `s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ses-simpleemailservice/src/ops/create_receipt_rule.rs`. Replaying the corpus, the emulator refused the `CreateReceiptRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateReceiptRule operation: operation CreateReceiptRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ses emulator
Call CreateReceiptRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
