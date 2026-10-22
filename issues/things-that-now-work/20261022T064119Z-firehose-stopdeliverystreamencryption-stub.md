Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.stop-encryption-on-unencrypted-stream
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.stop-encryption-on-unencrypted-stream|run#8|AssertionError|assert err.get("Code") == "InvalidArgumentException", (
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Implement firehose StopDeliveryStreamEncryption: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeliveryStreamEncryption

## Description
`StopDeliveryStreamEncryption` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/stop_delivery_stream_encryption.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StopDeliveryStreamEncryption` call, so each one unblocks only when `StopDeliveryStreamEncryption` works: `firehose.stop-encryption-on-unencrypted-stream`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/stop_delivery_stream_encryption.rs`. Replaying the corpus, the emulator refused the `StopDeliveryStreamEncryption` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_firehose_stop_encryption_on_unencrypted_stream.py", line 258, in run
    assert err.get("Code") == "InvalidArgumentException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidArgumentException stopping encryption on an unencrypted stream, got {'Message': 'operation StopDeliveryStreamEncryption is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the firehose emulator
Call StopDeliveryStreamEncryption and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
