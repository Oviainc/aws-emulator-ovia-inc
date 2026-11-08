Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-copy-package-versions-between-repositories
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-copy-package-versions-between-repositories codeartifact-package-version-status-transitions codeartifact-write-ops-on-missing-resources
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|CopyPackageVersions|2xx->2xx|$.failedVersions.9.9.9.errorMessage
Issue-Key: codeartifact|UpdatePackageVersionsStatus|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Key: codeartifact|DisposePackageVersions|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Key: codeartifact|CopyPackageVersions|2xx->2xx|$.failedVersions.1.0.0.errorMessage
Issue-Commit: d7142f0d93a1083aeee635c6d7e6fd3141132351

# CodeArtifact batch package-version operations report a failedVersions errorMessage AWS does not

> _Note: this issue was identified against the code base at commit `d7142f0d93a1083aeee635c6d7e6fd3141132351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopyPackageVersions

## Description
The three batch package-version operations that answer 200 with a per-version outcome map (CopyPackageVersions, UpdatePackageVersionsStatus, DisposePackageVersions) serialize a `failedVersions` entry whose `errorMessage` member disagrees with the recorded AWS response for every per-version failure. Four distinct causes, one per operation/fixture below; all four are the SAME defect in three sibling handlers, so each handler has to be fixed for this issue to close.

Cause 1 of 4: CopyPackageVersions, a version the source repository does not hold.
AWS: Smithy `PackageVersionError` has exactly two OPTIONAL members, `errorCode` (`PackageVersionErrorCode`: ALREADY_EXISTS | MISMATCHED_REVISION | MISMATCHED_STATUS | NOT_ALLOWED | NOT_FOUND | SKIPPED) and `errorMessage` (`ErrorMessage`, "The error message associated with the error."); `CopyPackageVersions` http 200, output member `failedVersions` targets `PackageVersionErrorMap` (key `PackageVersion`, value `PackageVersionError`). The model does not state what text AWS puts in `errorMessage`, so this is recorded behavior only: in the recording the failing version is reported under `failedVersions` keyed by the version string, and the comparator resolves NO value at `$.failedVersions.9.9.9.errorMessage` on the expected side (the map key `9.9.9` contains dots, which is why the diff path reads that way).
Emulator: the handler always fills `errorMessage` with a sentence of its own making — `version_error()` constructs `PackageVersionError { error_code: Some(..), error_message: Some(..) }` unconditionally, with per-code texts such as "Package version {package} {version} is at revision {held}, not the specified revision {pinned}.", "Package version {package} {version} already exists in repository {repository}." and "Copying into package {package} is blocked by its package origin configuration in repository {repository}." — so the 200 body carries an `errorMessage` whose content is not AWS's.
Why: a client that surfaces or matches on `failedVersions[version].errorMessage` (log lines, retry decisions keyed on the sentence, assertions in deployment tooling) reads emulator-authored prose that no real CodeArtifact response contains; code written against the emulator therefore mis-renders or mis-branches against AWS.
Fix: the per-version error construction in the batch handlers' `version_error()` helper / their serializer — the `errorMessage` of a `failedVersions` entry must reproduce AWS's text for that `PackageVersionErrorCode` (per the recordings, no `errorMessage` value at all for these entries), while `errorCode` keeps carrying the outcome. Hypothesis: AWS reports these per-version failures by `errorCode` only and leaves `errorMessage` unset, and the emulator's added sentence is the whole divergence.
Traces: codeartifact-copy-package-versions-between-repositories#task#8 (version `9.9.9`, errorCode NOT_FOUND path). Emulator code: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/copy_package_versions.rs lines 436-448 and 709-714.

Cause 2 of 4: UpdatePackageVersionsStatus, a version that is absent or not in the expected status.
AWS: Smithy `UpdatePackageVersionsStatus` http 200, output `failedVersions` -> `PackageVersionErrorMap`; same two-member `PackageVersionError`. Recorded behavior only: the expected document carries no value at `$.failedVersions.1.0.0.errorMessage`.
Emulator: emits its own `errorMessage` sentence in the entry.
Why: same as cause 1 — the per-version failure report of a status update does not match AWS.
Fix: the `version_error()` helper of services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/update_package_versions_status.rs.
Traces: codeartifact-package-version-status-transitions#task#6, codeartifact-write-ops-on-missing-resources#task#8.

Cause 3 of 4: DisposePackageVersions on a missing version.
AWS: Smithy `DisposePackageVersions` http 200, output `failedVersions` -> `PackageVersionErrorMap`. Recorded behavior only: no value at `$.failedVersions.1.0.0.errorMessage` on the expected side.
Emulator: emits its own `errorMessage` sentence in the entry.
Why: same as cause 1, on the dispose path.
Fix: the `version_error()` helper of services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/dispose_package_versions.rs.
Traces: codeartifact-write-ops-on-missing-resources#task#7.

Cause 4 of 4: CopyPackageVersions against resources that do not hold the version (the write-ops-on-missing-resources path).
AWS: as cause 1; recorded behavior only, no value at `$.failedVersions.1.0.0.errorMessage`.
Emulator: emits its own `errorMessage` sentence in the entry.
Why: same as cause 1; listed separately because it is a different fixture step and must be re-replayed on its own.
Fix: same component as cause 1.
Traces: codeartifact-write-ops-on-missing-resources#task#9.

No open issue covers these: the replay marked none of the four as owned, and the existing codeartifact issues concern domain/policy/package-group resource fields (20261029T203338Z-codeartifact-domain-policy-and-package-group-resource-fields.md), list-read fields (20261106T203113Z-codeartifact-list-reads-project-resource-fields-that-diverge.md) and the DescribePackage not-found sentence (20261103T055241Z), none of which touch `failedVersions`.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (status 200 on both sides in all four causes; only the failedVersions entry diverges):

1) CopyPackageVersions — fixture codeartifact-copy-package-versions-between-repositories, step task#8:
   $.failedVersions.9.9.9.errorMessage: value_mismatch (expected None, got None)
   divergence_key: service=codeartifact|CopyPackageVersions|status=200->200|keys=$.failedVersions.9.9.9.errorMessage

2) UpdatePackageVersionsStatus — fixture codeartifact-package-version-status-transitions, step task#6 (also codeartifact-write-ops-on-missing-resources#task#8):
   $.failedVersions.1.0.0.errorMessage: value_mismatch (expected None, got None)

3) DisposePackageVersions — fixture codeartifact-write-ops-on-missing-resources, step task#7:
   $.failedVersions.1.0.0.errorMessage: value_mismatch (expected None, got None)

4) CopyPackageVersions — fixture codeartifact-write-ops-on-missing-resources, step task#9:
   $.failedVersions.1.0.0.errorMessage: value_mismatch (expected None, got None)

Expected (AWS recording): the failing version appears under failedVersions with its errorCode and NO errorMessage value at that path.
Actual (emulator): the entry carries an emulator-authored errorMessage; see version_error() in copy_package_versions.rs (error_message: Some(..) always).

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Replay fixture codeartifact-copy-package-versions-between-repositories step task#8: copy a package version the source repository does not hold, and read $.failedVersions.<version> of the 200 response — it carries an errorMessage the recorded AWS response does not.
3. Repeat with codeartifact-package-version-status-transitions#task#6 (UpdatePackageVersionsStatus), codeartifact-write-ops-on-missing-resources#task#7 (DisposePackageVersions) and codeartifact-write-ops-on-missing-resources#task#9 (CopyPackageVersions).
4. Compare each failedVersions entry field by field against the recorded response.
