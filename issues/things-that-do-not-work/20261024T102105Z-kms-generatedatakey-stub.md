Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.e2e-envelope-encryption kms.generate-data-key-spec-and-bytes-exclusivity kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt kms.sep-kms-key-scope-data-key-halves-correspond-and-rewrap-moves-key-scope kms__s3.kmsenvelope-client-side-envelope-through-s3-round-trips-the-exact-bytes-and-the-stored-object-is-ciphertext kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key
Issue-Key: kms|GenerateDataKey|NotImplemented
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms GenerateDataKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateDataKey

## Description
`GenerateDataKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/generate_data_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `GenerateDataKey` call, so each one unblocks only when `GenerateDataKey` works: `kms.e2e-envelope-encryption`, `kms.generate-data-key-spec-and-bytes-exclusivity`, `kms.sep-kms-key-scope-context-map-is-order-independent-on-decrypt`, `kms.sep-kms-key-scope-data-key-halves-correspond-and-rewrap-moves-key-scope`, `kms__s3.kmsenvelope-client-side-envelope-through-s3-round-trips-the-exact-bytes-and-the-stored-object-is-ciphertext`, `kms__s3.kmsenvelope-mismatched-encryption-context-is-refused-when-unwrapping-the-stored-data-key`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/generate_data_key.rs`. Replaying the corpus, the emulator refused the `GenerateDataKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GenerateDataKey operation: operation GenerateDataKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kms emulator
Call GenerateDataKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
