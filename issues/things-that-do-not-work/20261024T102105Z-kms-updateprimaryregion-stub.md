Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.update-primary-region-non-multiregion-key-rejected kms.update-primary-region-on-non-multi-region-key-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.update-primary-region-non-multiregion-key-rejected|run#9|AssertionError|assert err["Code"] == "UnsupportedOperationException", (
Issue-Key: kms.update-primary-region-on-non-multi-region-key-rejected|run#8|AssertionError|assert code_ordinary == "UnsupportedOperationException", (
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# Implement kms UpdatePrimaryRegion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePrimaryRegion

## Description
`UpdatePrimaryRegion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kms-trentservice/src/ops/update_primary_region.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdatePrimaryRegion` call, so each one unblocks only when `UpdatePrimaryRegion` works: `kms.update-primary-region-non-multiregion-key-rejected`, `kms.update-primary-region-on-non-multi-region-key-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kms-trentservice/src/ops/update_primary_region.rs`. Replaying the corpus, the emulator refused the `UpdatePrimaryRegion` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_kms_update_primary_region_non_multiregion_key_rejected.py", line 42, in run
    assert err["Code"] == "UnsupportedOperationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected UpdatePrimaryRegion on a non-multi-Region key to fail with UnsupportedOperationException, got {'Code': 'NotImplemented', 'Message': 'operation UpdatePrimaryRegion is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the kms emulator
Call UpdatePrimaryRegion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
