Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.package-group-allowed-repository-update-without-mode-rejected codeartifact.package-group-origin-configuration-inherited-by-subgroups
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.package-group-allowed-repository-update-without-mode-rejected|run#14|AssertionError|assert refusal == expected, (
Issue-Key: codeartifact|UpdatePackageGroupOriginConfiguration|NotImplemented
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Implement codeartifact UpdatePackageGroupOriginConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePackageGroupOriginConfiguration

## Description
`UpdatePackageGroupOriginConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/update_package_group_origin_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdatePackageGroupOriginConfiguration` call, so each one unblocks only when `UpdatePackageGroupOriginConfiguration` works: `codeartifact.package-group-allowed-repository-update-without-mode-rejected`, `codeartifact.package-group-origin-configuration-inherited-by-subgroups`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/update_package_group_origin_configuration.rs`. Replaying the corpus, the emulator refused the `UpdatePackageGroupOriginConfiguration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codeartifact_package_group_allowed_repository_update_without_mode_rejected.py", line 86, in run
    assert refusal == expected, (
           ^^^^^^^^^^^^^^^^^^^
AssertionError: adding an allowed repository while setting the mode to INHERIT returned ('NotImplemented', 'operation UpdatePackageGroupOriginConfiguration is scaffolded but not implemented in this emulator'), expected ('ValidationException', 'Resulting mode is not ALLOW_SPECIFIC_REPOSITORIES for [EXTERNAL_UPSTREAM]. Cannot add or remove allowed-repositories.'): the message names the RESULTING mode, which is what makes this a cross-field rule rather than an enum check

## Steps to Reproduce
Boot the codeartifact emulator
Call UpdatePackageGroupOriginConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
