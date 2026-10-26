Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.set-sms-attributes-invalid-name-rejected sns.set-sms-attributes-invalid-sms-type-rejected sns.sms-attributes-custom-value-round-trip sns.sms-attributes-filter-exactness-and-clear-via-empty
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|GetSMSAttributes|NotImplemented
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# Implement sns GetSMSAttributes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSMSAttributes

## Description
`GetSMSAttributes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/get_sms_attributes.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `GetSMSAttributes` call, so each one unblocks only when `GetSMSAttributes` works: `sns.set-sms-attributes-invalid-name-rejected`, `sns.set-sms-attributes-invalid-sms-type-rejected`, `sns.sms-attributes-custom-value-round-trip`, `sns.sms-attributes-filter-exactness-and-clear-via-empty`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/get_sms_attributes.rs`. Replaying the corpus, the emulator refused the `GetSMSAttributes` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetSMSAttributes operation: operation GetSMSAttributes is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sns emulator
Call GetSMSAttributes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
