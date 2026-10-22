Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.origin-endpoint-container-type-immutable-rejected mediapackagev2.update-origin-endpoint-manifest-change-persists mediapackagev2.update-origin-endpoint-startover-window-persists
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.origin-endpoint-container-type-immutable-rejected|run#14|AssertionError|assert err.response["Error"]["Code"] == "ValidationException", err.response
Issue-Key: mediapackagev2|UpdateOriginEndpoint|NotImplemented
Issue-Commit: b8bbb064bfede5b5be6782e6b85c8258ff782b33

# Implement mediapackagev2 UpdateOriginEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b8bbb064bfede5b5be6782e6b85c8258ff782b33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOriginEndpoint

## Description
`UpdateOriginEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/update_origin_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdateOriginEndpoint` call, so each one unblocks only when `UpdateOriginEndpoint` works: `mediapackagev2.origin-endpoint-container-type-immutable-rejected`, `mediapackagev2.update-origin-endpoint-manifest-change-persists`, `mediapackagev2.update-origin-endpoint-startover-window-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/update_origin_endpoint.rs`. Replaying the corpus, the emulator refused the `UpdateOriginEndpoint` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_mediapackagev2_origin_endpoint_container_type_immutable_rejected.py", line 67, in run
    assert err.response["Error"]["Code"] == "ValidationException", err.response
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: {'Error': {'Message': 'operation UpdateOriginEndpoint is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}, 'ResponseMetadata': {'HTTPStatusCode': 501, 'HTTPHeaders': {'content-type': 'application/json', 'x-amzn-errortype': 'NotImplemented', 'content-length': '121', 'date': 'Thu, 22 Oct 2026 02:26:18 GMT'}, 'RetryAttempts': 0}}

## Steps to Reproduce
Boot the mediapackagev2 emulator
Call UpdateOriginEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
