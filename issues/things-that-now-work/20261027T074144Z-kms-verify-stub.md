Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from kms.kms-asymmetric-signature-verifies-locally-against-the-published-public-key kms.sep-kms-key-scope-signature-from-one-key-fails-verify-under-another-rejected kms.sign-message-type-raw-and-digest-agree-only-when-the-digest-matches kms.sign-secp256k1-digest-cross-verify kms.sign-verify-ecc kms.sign-verify-rsa
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|Verify|NotImplemented
Issue-Commit: ecf0c4efe25bcf25a3f7aae9bca0eda53779d0ad

# Implement kms Verify: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ecf0c4efe25bcf25a3f7aae9bca0eda53779d0ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Verify

## Description
`Verify` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/verify.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `Verify` call, so each one unblocks only when `Verify` works: `kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from`, `kms.kms-asymmetric-signature-verifies-locally-against-the-published-public-key`, `kms.sep-kms-key-scope-signature-from-one-key-fails-verify-under-another-rejected`, `kms.sign-message-type-raw-and-digest-agree-only-when-the-digest-matches`, `kms.sign-secp256k1-digest-cross-verify`, `kms.sign-verify-ecc`, `kms.sign-verify-rsa`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/verify.rs`. Replaying the corpus, the emulator refused the `Verify` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the Verify operation: operation Verify is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kms emulator
Call Verify and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
