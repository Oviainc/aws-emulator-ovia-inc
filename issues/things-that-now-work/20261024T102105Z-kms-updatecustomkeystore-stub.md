Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.update-custom-key-store-requires-valid-configuration
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.update-custom-key-store-requires-valid-configuration|run#4|AssertionError|assert err["Code"] == "CustomKeyStoreNotFoundException", err
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms UpdateCustomKeyStore: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCustomKeyStore

## Description
`UpdateCustomKeyStore` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/update_custom_key_store.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateCustomKeyStore` call, so each one unblocks only when `UpdateCustomKeyStore` works: `kms.update-custom-key-store-requires-valid-configuration`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/update_custom_key_store.rs`. Replaying the corpus, the emulator refused the `UpdateCustomKeyStore` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kms_update_custom_key_store_requires_valid_configuration.py", line 44, in run
    assert err["Code"] == "CustomKeyStoreNotFoundException", err
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'Code': 'NotImplemented', 'Message': 'operation UpdateCustomKeyStore is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the kms emulator
Call UpdateCustomKeyStore and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
