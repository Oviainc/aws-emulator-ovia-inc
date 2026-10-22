Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.delete-host-key-not-found-error transfer.host-key-added-timestamp-precision-preserved transfer.host-key-lifecycle-import-describe-delete transfer.hostkey-import-describe-delete-404 transfer.import-hostkey-duplicate-resource-exists-rejected transfer.imported-host-key-active-without-downtime-window
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ImportHostKey|NotImplemented
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Implement transfer ImportHostKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportHostKey

## Description
`ImportHostKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-transfer-transferservice/src/ops/import_host_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `ImportHostKey` call, so each one unblocks only when `ImportHostKey` works: `transfer.delete-host-key-not-found-error`, `transfer.host-key-added-timestamp-precision-preserved`, `transfer.host-key-lifecycle-import-describe-delete`, `transfer.hostkey-import-describe-delete-404`, `transfer.import-hostkey-duplicate-resource-exists-rejected`, `transfer.imported-host-key-active-without-downtime-window`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-transfer-transferservice/src/ops/import_host_key.rs`. Replaying the corpus, the emulator refused the `ImportHostKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportHostKey operation: operation ImportHostKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the transfer emulator
Call ImportHostKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
