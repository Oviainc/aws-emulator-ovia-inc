Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.package-version-readme-unicode-round-trip
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.package-version-readme-unicode-round-trip|_poll_for_readme#14|AssertionError|raise AssertionError(f"readme was never populated: {last_exc}")
Issue-Commit: 8f24345936df6262f616e6a72be898a454a52720

# Implement codeartifact GetPackageVersionReadme: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8f24345936df6262f616e6a72be898a454a52720`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPackageVersionReadme

## Description
`GetPackageVersionReadme` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_package_version_readme.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetPackageVersionReadme` call, so each one unblocks only when `GetPackageVersionReadme` works: `codeartifact.package-version-readme-unicode-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/get_package_version_readme.rs`. Replaying the corpus, the emulator refused the `GetPackageVersionReadme` call: File "check_codeartifact_package_version_readme_unicode_round_trip.py", line 52, in run
    readme = _poll_for_readme(ca, domain, repo, package, version, timeout_s=90.0)
             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  File "check_codeartifact_package_version_readme_unicode_round_trip.py", line 162, in _poll_for_readme
    raise AssertionError(f"readme was never populated: {last_exc}")
AssertionError: readme was never populated: An error occurred (NotImplemented) when calling the GetPackageVersionReadme operation: operation GetPackageVersionReadme is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeartifact emulator
Call GetPackageVersionReadme and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
