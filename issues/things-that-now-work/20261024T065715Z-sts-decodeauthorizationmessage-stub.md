Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.decode-authorization-message-rejects-realistic-fake-blob
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.decode-authorization-message-rejects-realistic-fake-blob|run#8|AssertionError|assert refusal == ("InvalidAuthorizationMessageException", expected), (refusal, expected)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement sts DecodeAuthorizationMessage: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DecodeAuthorizationMessage

## Description
`DecodeAuthorizationMessage` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/decode_authorization_message.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DecodeAuthorizationMessage` call, so each one unblocks only when `DecodeAuthorizationMessage` works: `sts.decode-authorization-message-rejects-realistic-fake-blob`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/decode_authorization_message.rs`. Replaying the corpus, the emulator refused the `DecodeAuthorizationMessage` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sts_decode_authorization_message_rejects_realistic_fake_blob.py", line 48, in run
    assert refusal == ("InvalidAuthorizationMessageException", expected), (refusal, expected)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: (('NotImplemented', 'operation DecodeAuthorizationMessage is scaffolded but not implemented in this emulator'), 'Message is not valid: [-54, -14, -56, -5, 49, 2, 68, -26, -120, -43, -85, 33, 67, 108, 89, 49, 8, -103, -62, -105, 45, -49, 84, 1, 90, -19, 78, -12, -123, 90, 114, -34, 16, 77, 20, -20, -110, 82, 100, 66, 107, -127, 7, -119, -89, -19, 28, -15, 114, 15, 83, -110, -48, 39, -53, 20, 5, -104, -39, -51, 53, 29, -123, -15, 39, -96, -53, 52, -7, 48, 109, 107, 64, 117, -74, -117, -54, 37, 52, -16, -100, -59, 2, -89, 86, -20, 103, -122, -124, -90, 123, 59, 121, -44, 6, 0, 102, -104, -128, 53, -33, 79, -65, -115, -116, 78, 116, -89, 12, -50, -10, 41, -65, 104, -117, 13, 126, 70, 30, 76, 28, -12, -97, 10, 40, 123, -28, -113, -64, -57, -78, 64, 126, -29, 109, 90, 69, 59, 78, 6, -95, -116, -20, -117, 4, -54, -19, 71, 13, -89, -61, -93, -60, 92, 116, 40, -101, -86, 54, 95, 22, -99, -82, -27, -64, 42, -103, -117, -10, 2, 69, 77, 24, -104, -103, 107, -119, -81, 52, -127, -3, 7, -105, 56, -96, 6, -33, -92, 45, -72, 109, 107, -11, -97, -112, 46, 127, 112, 65, -24, 90, 26, -91, 52, 74, -7, -119, 35, -61, 123, 20, -35, 95, 30, 9, 68, 18, 22, -3, -21, -28, 45, 25, 23, -54, -51, -66, -76, 111, -98, 64, -122, 117, -51, -48, 66, 97, 76, 110, -101, 94, -107, 101, 0, 59, -96, -79, 14, 95, 2, -38, -107, -34, 36, 56, -40, 112, -14, -63, -104, -55, -82, -102, 64, -112, -3, 34, 6, 96, -122, 79, 114, -115, -35, -44, -87, 73, -41, 100, -25, 9, -89, -53, 105, 42, 8, 52, 35, 58, -76, -58, -91, 35, 36, 51, -118, 80, 101, -35, 108,

## Steps to Reproduce
Boot the sts emulator
Call DecodeAuthorizationMessage and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
