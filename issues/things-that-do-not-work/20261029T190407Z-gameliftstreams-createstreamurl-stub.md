Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.create-stream-url-unassociated-application-rejected gameliftstreams.stream-url-lifecycle-create-then-revoke
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams.create-stream-url-unassociated-application-rejected|run#7|AssertionError|assert err.get("Code") == "ValidationException", (
Issue-Key: gameliftstreams|CreateStreamUrl|NotImplemented
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Implement gameliftstreams CreateStreamUrl: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStreamUrl

## Description
`CreateStreamUrl` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/create_stream_url.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateStreamUrl` call, so each one unblocks only when `CreateStreamUrl` works: `gameliftstreams.create-stream-url-unassociated-application-rejected`, `gameliftstreams.stream-url-lifecycle-create-then-revoke`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/create_stream_url.rs`. Replaying the corpus, the emulator refused the `CreateStreamUrl` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_gameliftstreams_create_stream_url_unassociated_application_rejected.py", line 38, in run
    assert err.get("Code") == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for a CreateStreamUrl against an unassociated application, got {'Message': 'operation CreateStreamUrl is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the gameliftstreams emulator
Call CreateStreamUrl and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
