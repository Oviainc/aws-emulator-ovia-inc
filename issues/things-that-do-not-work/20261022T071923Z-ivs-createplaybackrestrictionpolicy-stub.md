Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.create-playback-restriction-policy-invalid-country-rejected ivs.create-playback-restriction-policy-round-trip ivs.delete-playback-restriction-policy-attached-conflict-rejected ivs.delete-playback-restriction-policy-attached-rejected ivs.playback-restriction-policy-update-persists ivs.update-playback-restriction-policy-fields-persisted ivs.update-playback-restriction-policy-persists-change
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs.create-playback-restriction-policy-invalid-country-rejected|run#4|AssertionError|assert error["Code"] == "ValidationException", (
Issue-Key: ivs|CreatePlaybackRestrictionPolicy|NotImplemented
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Implement ivs CreatePlaybackRestrictionPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePlaybackRestrictionPolicy

## Description
`CreatePlaybackRestrictionPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_playback_restriction_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreatePlaybackRestrictionPolicy` call, so each one unblocks only when `CreatePlaybackRestrictionPolicy` works: `ivs.create-playback-restriction-policy-invalid-country-rejected`, `ivs.create-playback-restriction-policy-round-trip`, `ivs.delete-playback-restriction-policy-attached-conflict-rejected`, `ivs.delete-playback-restriction-policy-attached-rejected`, `ivs.playback-restriction-policy-update-persists`, `ivs.update-playback-restriction-policy-fields-persisted`, `ivs.update-playback-restriction-policy-persists-change`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_playback_restriction_policy.rs`. Replaying the corpus, the emulator refused the `CreatePlaybackRestrictionPolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ivs_create_playback_restriction_policy_invalid_country_rejected.py", line 25, in run
    assert error["Code"] == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for allowedCountries=['ZZ'], got {'Message': 'operation CreatePlaybackRestrictionPolicy is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ivs emulator
Call CreatePlaybackRestrictionPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
