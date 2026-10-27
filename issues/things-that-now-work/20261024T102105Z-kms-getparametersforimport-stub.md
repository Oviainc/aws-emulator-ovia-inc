Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.e2e-external-key-import-and-expire kms.e2e-import-external-key-material-envelope-flow kms.e2e-import-external-key-material-then-delete-blocks-decrypt kms.external-origin-pending-import-lifecycle kms.generate-mac-equals-a-locally-computed-hmac-over-the-imported-key-material kms.import-key-material-base64-blob-round-trip kms.import-key-material-binary-blob-integrity kms.import-key-material-lifecycle-pending-to-imported-to-deleted kms.import-key-material-state-transitions kms.import-key-material-token-single-use-rejected kms.import-key-material-valid-to-timestamp-preserved kms.import-key-material-validto-auto-expiry kms.import-token-is-bound-to-the-wrapping-key-pair-it-was-issued-with kms.import-token-ttl-window-enforced kms.imported-key-material-expiration-reverts-key-state kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from kms.key-material-expiration-model-auto-expires kms.kms-imported-aes-key-material-wrapped-locally-decrypts-what-kms-encrypts kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|GetParametersForImport|NotImplemented
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms GetParametersForImport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParametersForImport

## Description
`GetParametersForImport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/get_parameters_for_import.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `GetParametersForImport` call, so each one unblocks only when `GetParametersForImport` works: `kms.e2e-external-key-import-and-expire`, `kms.e2e-import-external-key-material-envelope-flow`, `kms.e2e-import-external-key-material-then-delete-blocks-decrypt`, `kms.external-origin-pending-import-lifecycle`, `kms.generate-mac-equals-a-locally-computed-hmac-over-the-imported-key-material`, `kms.import-key-material-base64-blob-round-trip`, `kms.import-key-material-binary-blob-integrity`, `kms.import-key-material-lifecycle-pending-to-imported-to-deleted`, `kms.import-key-material-state-transitions`, `kms.import-key-material-token-single-use-rejected`, `kms.import-key-material-valid-to-timestamp-preserved`, `kms.import-key-material-validto-auto-expiry`, `kms.import-token-is-bound-to-the-wrapping-key-pair-it-was-issued-with`, `kms.import-token-ttl-window-enforced`, `kms.imported-key-material-expiration-reverts-key-state`, `kms.imported-rsa-private-key-signs-identically-to-the-local-key-it-came-from`, `kms.key-material-expiration-model-auto-expires`, `kms.kms-imported-aes-key-material-wrapped-locally-decrypts-what-kms-encrypts`, `kms.sep-kms-key-scope-identical-imported-material-does-not-make-keys-interchangeable-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/get_parameters_for_import.rs`. Replaying the corpus, the emulator refused the `GetParametersForImport` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetParametersForImport operation: operation GetParametersForImport is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kms emulator
Call GetParametersForImport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
