Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.codeartifact-concurrent-publish-of-one-version-lets-exactly-one-content-win codeartifact.unfinished-version-is-incomplete-until-the-final-asset-closes-it-and-cannot-reopen
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|ListPackageVersionAssets|NotImplemented
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# Implement codeartifact ListPackageVersionAssets: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPackageVersionAssets

## Description
`ListPackageVersionAssets` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/list_package_version_assets.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListPackageVersionAssets` call, so each one unblocks only when `ListPackageVersionAssets` works: `codeartifact.codeartifact-concurrent-publish-of-one-version-lets-exactly-one-content-win`, `codeartifact.unfinished-version-is-incomplete-until-the-final-asset-closes-it-and-cannot-reopen`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/list_package_version_assets.rs`. Replaying the corpus, the emulator refused the `ListPackageVersionAssets` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPackageVersionAssets operation: operation ListPackageVersionAssets is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeartifact emulator
Call ListPackageVersionAssets and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
