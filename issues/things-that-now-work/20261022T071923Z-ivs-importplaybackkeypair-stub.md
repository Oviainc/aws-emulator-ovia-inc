Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.import-playback-key-pair-duplicate-conflict-rejected ivs.import-playback-key-pair-duplicate-rejected ivs.import-playback-key-pair-fingerprint-consistency ivs.import-playback-key-pair-round-trip ivs.list-playback-key-pairs-reflects-imported ivs.playback-key-pair-fingerprint-format
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ImportPlaybackKeyPair|NotImplemented
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Implement ivs ImportPlaybackKeyPair: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportPlaybackKeyPair

## Description
`ImportPlaybackKeyPair` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/import_playback_key_pair.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `ImportPlaybackKeyPair` call, so each one unblocks only when `ImportPlaybackKeyPair` works: `ivs.import-playback-key-pair-duplicate-conflict-rejected`, `ivs.import-playback-key-pair-duplicate-rejected`, `ivs.import-playback-key-pair-fingerprint-consistency`, `ivs.import-playback-key-pair-round-trip`, `ivs.list-playback-key-pairs-reflects-imported`, `ivs.playback-key-pair-fingerprint-format`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/import_playback_key_pair.rs`. Replaying the corpus, the emulator refused the `ImportPlaybackKeyPair` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportPlaybackKeyPair operation: operation ImportPlaybackKeyPair is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ivs emulator
Call ImportPlaybackKeyPair and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution
`ImportPlaybackKeyPair` is implemented: it decodes the PEM `publicKeyMaterial`,
refuses anything that is not a DER `SubjectPublicKeyInfo` with the modelled
`ValidationException` (400), fingerprints the key as the MD5 of its DER in
colon-separated hex, refuses an already-imported key with the modelled
`ConflictException` (409), and persists the pair under a minted
`arn:aws:ivs:<region>:<account>:playback-key/<id>`. `GetPlaybackKeyPair` was
implemented in the same change so the import reads back.

Still blocked elsewhere: the unit `ivs.list-playback-key-pairs-reflects-imported`
now gets past the import and fails on `ListPlaybackKeyPairs`, which is still a
scaffold 501 and is owned by
`issues/things-that-do-not-work/20261022T071923Z-ivs-listplaybackkeypairs-stub.md`.
