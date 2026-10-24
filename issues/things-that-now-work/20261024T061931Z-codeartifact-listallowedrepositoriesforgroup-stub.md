Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.package-group-origin-restriction-inherits-from-parent
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.package-group-origin-restriction-inherits-from-parent|run#16|AssertionError|assert refusal == expected, (
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Implement codeartifact ListAllowedRepositoriesForGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAllowedRepositoriesForGroup

## Description
`ListAllowedRepositoriesForGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/list_allowed_repositories_for_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListAllowedRepositoriesForGroup` call, so each one unblocks only when `ListAllowedRepositoriesForGroup` works: `codeartifact.package-group-origin-restriction-inherits-from-parent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/list_allowed_repositories_for_group.rs`. Replaying the corpus, the emulator refused the `ListAllowedRepositoriesForGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codeartifact_package_group_origin_restriction_inherits_from_parent.py", line 101, in run
    assert refusal == expected, (
           ^^^^^^^^^^^^^^^^^^^
AssertionError: listing allowed repositories for an INHERIT restriction returned ('NotImplemented', 'operation ListAllowedRepositoriesForGroup is scaffolded but not implemented in this emulator'), expected ('ValidationException', 'OriginRestrictionMode is not ALLOW_SPECIFIC_REPOSITORIES for the given OriginControlType') rather than an empty list

## Steps to Reproduce
Boot the codeartifact emulator
Call ListAllowedRepositoriesForGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
