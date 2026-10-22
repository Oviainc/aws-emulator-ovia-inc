Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.get-channel-policy-not-found-when-never-attached
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.get-channel-policy-not-found-when-never-attached|run#11|AssertionError|assert err["Code"] == "ResourceNotFoundException", (
Issue-Commit: b8bbb064bfede5b5be6782e6b85c8258ff782b33

# Implement mediapackagev2 GetChannelPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b8bbb064bfede5b5be6782e6b85c8258ff782b33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetChannelPolicy

## Description
`GetChannelPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/get_channel_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetChannelPolicy` call, so each one unblocks only when `GetChannelPolicy` works: `mediapackagev2.get-channel-policy-not-found-when-never-attached`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/get_channel_policy.rs`. Replaying the corpus, the emulator refused the `GetChannelPolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_mediapackagev2_get_channel_policy_not_found_when_never_attached.py", line 48, in run
    assert err["Code"] == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException on a never-attached policy, got {'Message': 'operation GetChannelPolicy is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the mediapackagev2 emulator
Call GetChannelPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
