Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.codeartifact-copy-package-versions-preserves-asset-bytes-and-digest codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish codeartifact.copy-from-upstream-without-include-flag-rejected codeartifact.copy-package-versions-preserves-revision-and-assets codeartifact.deleted-package-version-frees-the-version-number codeartifact.describe-absent-package-version-reports-the-package codeartifact.disposed-package-version-is-terminal codeartifact.e2e-publish-fetch-delete-package-version codeartifact.generic-asset-roundtrip-is-byte-exact codeartifact.get-package-version-asset-after-disposal-blocked codeartifact.list-package-versions-returns-newest-first codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository codeartifact.package-delete-cascades-to-versions codeartifact.package-origin-configuration-blocks-direct-publish codeartifact.package-origin-configuration-blocks-publish codeartifact.package-version-asset-binary-blob-byte-equality codeartifact.package-version-published-time-timestamp-precision-preserved codeartifact.package-version-status-lifecycle-publish-to-disposed codeartifact.package-version-status-progresses-to-archived codeartifact.package-version-status-transitions-enforced-forward-only codeartifact.publish-asset-sha256-mismatch-rejected codeartifact.publish-duplicate-immutable-version-rejected codeartifact.repository-policy-deny-scopes-to-the-named-action codeartifact.unfinished-version-is-incomplete-until-the-final-asset-closes-it-and-cannot-reopen codeartifact.update-package-version-status-invalid-target-rejected codeartifact.update-status-expected-status-mismatch-rejected codeartifact.update-status-to-disposed-rejected codeartifact.upstream-order-decides-resolved-version
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|PublishPackageVersion|NotImplemented
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codeartifact PublishPackageVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishPackageVersion

## Description
`PublishPackageVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/publish_package_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

28 recorded unit(s) fail on their `PublishPackageVersion` call, so each one unblocks only when `PublishPackageVersion` works: `codeartifact.codeartifact-copy-package-versions-preserves-asset-bytes-and-digest`, `codeartifact.codeartifact-package-origin-configuration-blocks-direct-publish`, `codeartifact.copy-from-upstream-without-include-flag-rejected`, `codeartifact.copy-package-versions-preserves-revision-and-assets`, `codeartifact.deleted-package-version-frees-the-version-number`, `codeartifact.describe-absent-package-version-reports-the-package`, `codeartifact.disposed-package-version-is-terminal`, `codeartifact.e2e-publish-fetch-delete-package-version`, `codeartifact.generic-asset-roundtrip-is-byte-exact`, `codeartifact.get-package-version-asset-after-disposal-blocked`, `codeartifact.list-package-versions-returns-newest-first`, `codeartifact.list-packages-origin-control-prefix-and-format-filters-partition-the-repository`, `codeartifact.package-delete-cascades-to-versions`, `codeartifact.package-origin-configuration-blocks-direct-publish`, `codeartifact.package-origin-configuration-blocks-publish`, `codeartifact.package-version-asset-binary-blob-byte-equality`, `codeartifact.package-version-published-time-timestamp-precision-preserved`, `codeartifact.package-version-status-lifecycle-publish-to-disposed`, `codeartifact.package-version-status-progresses-to-archived`, `codeartifact.package-version-status-transitions-enforced-forward-only` (+8 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/publish_package_version.rs`. Replaying the corpus, the emulator refused the `PublishPackageVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PublishPackageVersion operation: operation PublishPackageVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codeartifact emulator
Call PublishPackageVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
