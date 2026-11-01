Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: payment-cryptography-data.e2e-card-issuance-transaction-settlement-pipeline payment-cryptography-data.generate-verify-pin-data-visa-pvv-roundtrip payment-cryptography-data.translate-pin-data-iso-format-roundtrip payment-cryptography-data.verify-pin-data-wrong-pin-rejected
Issue-Shape-Id: com.amazonaws.paymentcryptographydata#PaymentCryptographyDataPlane
Issue-Key: payment-cryptography-data|GeneratePinData|NotImplemented
Issue-Commit: 6df8ee8a64c175c1ec46c5456c673946e1d299f9

# Implement payment-cryptography-data GeneratePinData: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6df8ee8a64c175c1ec46c5456c673946e1d299f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GeneratePinData

## Description
`GeneratePinData` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane/src/ops/generate_pin_data.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `GeneratePinData` call, so each one unblocks only when `GeneratePinData` works: `payment-cryptography-data.e2e-card-issuance-transaction-settlement-pipeline`, `payment-cryptography-data.generate-verify-pin-data-visa-pvv-roundtrip`, `payment-cryptography-data.translate-pin-data-iso-format-roundtrip`, `payment-cryptography-data.verify-pin-data-wrong-pin-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-paymentcryptographydata-paymentcryptographydataplane/src/ops/generate_pin_data.rs`. Replaying the corpus, the emulator refused the `GeneratePinData` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GeneratePinData operation: operation GeneratePinData is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the payment-cryptography-data emulator
Call GeneratePinData and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
