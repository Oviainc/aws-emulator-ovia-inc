Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.delete-stream-while-update-in-progress-resource-in-use firehose.e2e-update-destination-stale-version-rejected firehose.server-side-enforces-single-active-role-after-update-destination firehose.update-destination-buffering-hints-numeric-precision-preserved firehose.update-destination-change-type-rejected firehose.update-destination-immutable-delivery-stream-type firehose.update-destination-invalid-destination-id firehose.update-destination-rejected-while-stream-creating firehose.update-destination-stale-version-conflict firehose.update-destination-stale-version-id-rejected firehose.update-destination-unknown-destination-id firehose.update-destination-version-increment
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.delete-stream-while-update-in-progress-resource-in-use|run#10|AssertionError|assert update_code == "ResourceInUseException", (update_code, update_message)
Issue-Key: firehose|UpdateDestination|NotImplemented
Issue-Key: firehose.update-destination-change-type-rejected|run#17|AssertionError|assert code == "InvalidArgumentException", f"expected InvalidArgumentException, got {code!r}: {message!r}"
Issue-Key: firehose.update-destination-immutable-delivery-stream-type|run#11|AssertionError|assert err["Code"] == "InvalidArgumentException", f"expected InvalidArgumentException, got {err!r}"
Issue-Key: firehose.update-destination-rejected-while-stream-creating|run#12|AssertionError|assert code == "ResourceInUseException", (
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Implement firehose UpdateDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDestination

## Description
`UpdateDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/update_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `UpdateDestination` call, so each one unblocks only when `UpdateDestination` works: `firehose.delete-stream-while-update-in-progress-resource-in-use`, `firehose.e2e-update-destination-stale-version-rejected`, `firehose.server-side-enforces-single-active-role-after-update-destination`, `firehose.update-destination-buffering-hints-numeric-precision-preserved`, `firehose.update-destination-change-type-rejected`, `firehose.update-destination-immutable-delivery-stream-type`, `firehose.update-destination-invalid-destination-id`, `firehose.update-destination-rejected-while-stream-creating`, `firehose.update-destination-stale-version-conflict`, `firehose.update-destination-stale-version-id-rejected`, `firehose.update-destination-unknown-destination-id`, `firehose.update-destination-version-increment`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/update_destination.rs`. Replaying the corpus, the emulator refused the `UpdateDestination` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_firehose_delete_stream_while_update_in_progress_resource_in_use.py", line 44, in run
    assert update_code == "ResourceInUseException", (update_code, update_message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation UpdateDestination is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the firehose emulator
Call UpdateDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
