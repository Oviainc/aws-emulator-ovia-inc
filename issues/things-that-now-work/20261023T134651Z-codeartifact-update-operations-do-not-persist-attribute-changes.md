Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-domain-repository-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-domain-repository-lifecycle codeartifact-repository-lifecycle-and-conflicts codeartifact-package-group-lifecycle-and-hierarchy codeartifact-generic-package-publish-and-reads codeartifact-package-version-status-transitions codeartifact-repository-upstream-configuration
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|DescribeRepository|2xx->2xx|$.repository.description
Issue-Key: codeartifact|ListRepositoriesInDomain|2xx->2xx|$.repositories[arn="arn
Issue-Key: codeartifact|DeleteRepository|2xx->2xx|$.repository.description
Issue-Key: codeartifact|ListRepositoriesInDomain|2xx->2xx|$.repositories[*].description
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.arn
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.contactInfo
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.description
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.EXTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.INTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.PUBLISH.inheritedFrom.arn
Issue-Key: codeartifact|DescribePackageGroup|2xx->2xx|$.packageGroup.parent.arn
Issue-Key: codeartifact|DescribePackage|2xx->2xx|$.package.originConfiguration.restrictions.upstream
Issue-Key: codeartifact|ListPackages|2xx->2xx|$.packages[*].originConfiguration.restrictions.upstream
Issue-Key: codeartifact|DescribePackageVersion|2xx->2xx|$.packageVersion.displayName
Issue-Key: codeartifact|DescribePackageVersion|2xx->2xx|$.packageVersion.licenses
Issue-Key: codeartifact|DescribePackageVersion|2xx->2xx|$.packageVersion.status
Issue-Key: codeartifact|DescribeRepository|2xx->2xx|$.repository.upstreams[*].repositoryName
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact update operations do not persist attribute changes

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRepository

## Description
Five causes, all the same shape of defect: a mutating CodeArtifact operation is answered 200 but the mutation is not committed to state, so every later read returns the pre-update value. All five must be closed before submitting.

Cause 1 of 5: UpdateRepository does not apply `description`.
AWS: UpdateRepository takes an optional description (Smithy: UpdateRepository.description -> Description, @length 0..1000, @pattern ^\P{C}*$; http PUT /v1/repository code 200, output repository -> RepositoryDescription) and the new value is durable: DescribeRepository, ListRepositoriesInDomain and the DeleteRepository echo all report the updated description.
Emulator: subsequent reads return the description supplied at CreateRepository. The update is accepted and dropped.
Why: a client that renames/retags a repository's description and then reads it back sees its write silently lost — the classic read-your-write violation. Reconcilers re-issue the update forever; UIs show stale metadata.
Fix: the UpdateRepository handler/state — persist description onto the stored RepositoryDescription instead of only echoing it.
Traces: codeartifact-domain-repository-lifecycle#task#5 (DescribeRepository), codeartifact-domain-repository-lifecycle#task#7 (ListRepositoriesInDomain), codeartifact-domain-repository-lifecycle#teardown#8 (DeleteRepository echo), codeartifact-repository-lifecycle-and-conflicts#task#5 (ListRepositoriesInDomain).

Cause 2 of 5: UpdatePackageGroup does not apply `contactInfo` or `description`.
AWS: Smithy: UpdatePackageGroup.contactInfo -> PackageGroupContactInfo (@length 0..1000) and .description -> Description, http PUT /v1/package-group code 200, output packageGroup -> PackageGroupDescription whose contactInfo/description members carry the current values.
Emulator: DescribePackageGroup after a successful UpdatePackageGroup returns the values given at CreatePackageGroup.
Why: package-group ownership metadata (who to contact, what the group is for) cannot be maintained; any workflow that updates then verifies fails.
Fix: the UpdatePackageGroup handler/state — commit contactInfo and description to the stored package group.
Traces: codeartifact-package-group-lifecycle-and-hierarchy#task#6.

Cause 3 of 5: PutPackageOriginConfiguration does not apply the origin restrictions.
AWS: PackageOriginRestrictions has required members publish and upstream (Smithy: PackageOriginRestrictions.publish -> AllowPublish, .upstream -> AllowUpstream, both required enums). Setting upstream to BLOCK is durable and is reported by DescribePackage and by ListPackages' PackageSummary.originConfiguration.
Emulator: both reads still report restrictions.upstream = 'ALLOW' after the restriction was set to BLOCK.
Why: this is a security-relevant control — it is what prevents a package name from being resolved from an external/upstream source (dependency-substitution defence). The emulator reports the repository as unprotected after the client successfully applied protection, so a client cannot verify the control, and any test of it passes/fails against the wrong state.
Fix: the PutPackageOriginConfiguration handler/state, plus the originConfiguration projection used by DescribePackage and ListPackages.
Traces: codeartifact-generic-package-publish-and-reads#task#5 (DescribePackage), codeartifact-generic-package-publish-and-reads#task#6 (ListPackages).

Cause 4 of 5: UpdatePackageVersionsStatus does not apply the new status.
AWS: PackageVersionStatus is an enum with values Published, Unfinished, Unlisted, Archived, Disposed, Deleted (Smithy: PackageVersionStatus), and DescribePackageVersion reports the version's current status (output packageVersion -> PackageVersionDescription, required).
Emulator: after a version is moved to Archived, DescribePackageVersion still reports 'Published'.
Why: status is how CodeArtifact retires a version — consumers decide resolvability from it. A client that archives a version and then reads it believes the retirement did not happen; conversely, logic gated on 'Published' keeps treating a retired version as live.
Fix: the UpdatePackageVersionsStatus handler/state and the status field served by DescribePackageVersion/ListPackageVersions.
Traces: codeartifact-package-version-status-transitions#task#8 (this cluster also shows the missing displayName/licenses members, which are a separate, separately-filed serialization defect).

Cause 5 of 5: UpdateRepository does not preserve the submitted order of `upstreams`.
AWS: upstreams is a list (Smithy: UpdateRepository.upstreams -> UpstreamRepositoryList) and the recording shows DescribeRepository returning the upstreams in exactly the order submitted — for CodeArtifact that order is the resolution priority.
Emulator: returns the two upstreams in the opposite (apparently creation/sorted) order.
Why: upstream order decides which upstream a package is fetched from first. Reordering it changes resolution semantics, so a client that reprioritises upstreams gets the old priority while being told the call succeeded.
Fix: the UpdateRepository handler/state — store upstreams as an ordered sequence and serialize them in submission order.
Traces: codeartifact-repository-upstream-configuration#task#9.

## Evidence
Cause 1 — codeartifact-domain-repository-lifecycle#task#5, DescribeRepository, 200/200:
  $.repository.description: expected 'updated repo description', got 'initial repo description'
  also #task#7 (ListRepositoriesInDomain, same values), #teardown#8 (DeleteRepository echo), and codeartifact-repository-lifecycle-and-conflicts#task#5 ($.repositories[0].description expected 'sim2real probe repository, updated', got 'sim2real probe repository').

Cause 2 — codeartifact-package-group-lifecycle-and-hierarchy#task#6, DescribePackageGroup, 200/200:
  $.packageGroup.contactInfo: expected 'new-team@example.com', got 'team-widgets@example.com'
  $.packageGroup.description: expected 'updated widget npm packages', got 'widget scoped npm packages'

Cause 3 — codeartifact-generic-package-publish-and-reads#task#5, DescribePackage, 200/200:
  $.package.originConfiguration.restrictions.upstream: expected 'BLOCK', got 'ALLOW'
  #task#6, ListPackages: $.packages[0].originConfiguration.restrictions.upstream: expected 'BLOCK', got 'ALLOW'

Cause 4 — codeartifact-package-version-status-transitions#task#8, DescribePackageVersion, 200/200:
  $.packageVersion.status: expected 'Archived', got 'Published'

Cause 5 — codeartifact-repository-upstream-configuration#task#9, DescribeRepository, 200/200:
  $.repository.upstreams[0].repositoryName: expected 'sim2real-probe-ca-upb10-...', got 'sim2real-probe-ca-upa10-...'
  $.repository.upstreams[1].repositoryName: expected 'sim2real-probe-ca-upa10-...', got 'sim2real-probe-ca-upb10-...'
  (the names are fixture-generated; the requirement is that the returned order equals the submitted order)

## Steps to Reproduce
1. Replay codeartifact-domain-repository-lifecycle: create a repository with description A, PUT /v1/repository with description B, then step task#5 (DescribeRepository), task#7 (ListRepositoriesInDomain) and teardown#8 (DeleteRepository) -> all report A.
2. Replay codeartifact-package-group-lifecycle-and-hierarchy step task#6: UpdatePackageGroup with new contactInfo/description, then DescribePackageGroup -> old values.
3. Replay codeartifact-generic-package-publish-and-reads steps task#5/task#6: PutPackageOriginConfiguration with restrictions.upstream=BLOCK, then DescribePackage/ListPackages -> ALLOW.
4. Replay codeartifact-package-version-status-transitions step task#8: UpdatePackageVersionsStatus to Archived, then DescribePackageVersion -> Published.
5. Replay codeartifact-repository-upstream-configuration step task#9: UpdateRepository with upstreams [B, A], then DescribeRepository -> [A, B].
