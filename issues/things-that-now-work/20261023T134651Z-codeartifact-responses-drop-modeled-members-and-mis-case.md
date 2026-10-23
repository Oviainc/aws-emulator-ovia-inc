Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-generic-package-publish-and-reads
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-generic-package-publish-and-reads codeartifact-resource-not-found-envelopes codeartifact-external-connection-lifecycle codeartifact-external-connection-workflow
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|AssociateExternalConnection|2xx->2xx|$.repository.externalConnections[*].status
Issue-Key: codeartifact|DescribeRepository|2xx->2xx|$.repository.externalConnections[*].status
Issue-Related: 20261023T134651Z-codeartifact-update-operations-do-not-persist-attribute-changes.md
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact responses drop modeled members and mis-case the external-connection status

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePackageVersion

## Description
Two causes in the success-path serializers. Both must be closed before submitting.

Cause 1 of 2: PackageVersionDescription omits `displayName` and `licenses`.
AWS: DescribePackageVersion returns a required PackageVersionDescription (Smithy: DescribePackageVersion output packageVersion -> PackageVersionDescription, required) whose members include displayName -> String255 ('The name of the package that is displayed... depends on the package version's format') and licenses -> LicenseInfoList ('Information about licenses associated with the package version'). The recordings show displayName populated with the package's display name and licenses present as an empty list for a generic package with no license metadata.
Emulator: both members are absent from the response body.
Why: displayName is what clients render for a version (and is format-dependent, so it cannot be reconstructed by the caller); an absent licenses member is not the same as an empty one — SDK consumers that iterate licenses get None/null instead of an empty collection and crash or report 'unknown licensing' where AWS reports 'no licenses'.
Fix: the PackageVersionDescription serializer (and the state it reads) used by DescribePackageVersion/ListPackageVersions — populate displayName from the format-specific display rule and always emit licenses, empty list included.
Traces: codeartifact-generic-package-publish-and-reads#task#4 (2-fixture cluster, also codeartifact-resource-not-found-envelopes#task#8); the same two members are missing in codeartifact-package-version-status-transitions#task#8, whose status divergence is filed under the CodeArtifact update-operations issue.

Cause 2 of 2: externalConnections[].status is serialized as 'Available' instead of the recorded 'AVAILABLE'.
AWS: recorded behavior only — the live responses for AssociateExternalConnection and DescribeRepository return repository.externalConnections[].status = 'AVAILABLE'. Note the on-disk model disagrees with the wire here: Smithy ExternalConnectionStatus lists the single value 'Available', which is exactly what the emulator emits; the recording is what clients actually observe on the wire, so the wire value governs and the model entry should be treated as stale for this member.
Emulator: emits 'Available' in both operations.
Why: this is an enum value, not free text — a client comparing status against the value it sees from AWS ('AVAILABLE') never matches, so 'is the external connection ready?' is permanently false against the emulator.
Fix: the ExternalConnectionStatus serialization in the repository-description serializer (shared by AssociateExternalConnection, DisassociateExternalConnection and DescribeRepository).
Traces: codeartifact-external-connection-lifecycle#task#3 (AssociateExternalConnection, 2-fixture cluster), codeartifact-external-connection-lifecycle#task#4 (DescribeRepository, 2-fixture cluster).

## Evidence
Cause 1 — codeartifact-generic-package-publish-and-reads#task#4, DescribePackageVersion, 200/200:
  $.packageVersion.displayName: missing (expected 'sim2real-probe-pkg-fbfc55f2', got None)
  $.packageVersion.licenses: missing (expected [], got None)
  (same two members missing in codeartifact-package-version-status-transitions#task#8)

Cause 2 — codeartifact-external-connection-lifecycle#task#3, AssociateExternalConnection, 200/200:
  $.repository.externalConnections[0].status: expected 'AVAILABLE', got 'Available'
  codeartifact-external-connection-lifecycle#task#4, DescribeRepository, 200/200: same divergence

## Steps to Reproduce
1. Replay codeartifact-generic-package-publish-and-reads step task#4: publish a generic package version, then GET /v1/package/version -> the response has no displayName and no licenses member.
2. Replay codeartifact-external-connection-lifecycle step task#3 (POST /v1/repository/external-connection) and step task#4 (GET /v1/repository) -> externalConnections[0].status is 'Available' where the recording has 'AVAILABLE'.
