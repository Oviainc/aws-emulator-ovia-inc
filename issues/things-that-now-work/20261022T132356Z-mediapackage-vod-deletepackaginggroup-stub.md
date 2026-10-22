Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: mediapackage-vod|DeletePackagingGroup|NotImplemented
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# Implement mediapackage-vod DeletePackagingGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePackagingGroup

## Description
`DeletePackagingGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mediapackagevod-mediapackagevod/src/ops/delete_packaging_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeletePackagingGroup` call, so each one unblocks only when `DeletePackagingGroup` works: `mediapackage-vod.delete-packaging-group-with-dependent-configs-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mediapackagevod-mediapackagevod/src/ops/delete_packaging_group.rs`. Replaying the corpus, the emulator refused the `DeletePackagingGroup` call: File "check_mediapackage_vod_delete_packaging_group_with_dependent_configs_rejected.py", line 33, in run
    _assert_rejected(
    ~~~~~~~~~~~~~~~~^
        lambda: mpv.delete_packaging_group(Id=group_id),
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    ...<2 lines>...
        "DeletePackagingGroup while a dependent PackagingConfiguration still references the group",
        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    )
    ^
  File "check_mediapackage_vod_delete_packaging_group_with_dependent_configs_rejected.py", line 61, in _assert_rejected
    assert code == expected_code, f"{what}: expected {expected_code!r}, got {code!r}"
           ^^^^^^^^^^^^^^^^^^^^^
AssertionError: DeletePackagingGroup while a dependent PackagingConfiguration still references the group: expected 'UnprocessableEntityException', got 'NotImplemented'

## Steps to Reproduce
Boot the mediapackage-vod emulator
Call DeletePackagingGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
