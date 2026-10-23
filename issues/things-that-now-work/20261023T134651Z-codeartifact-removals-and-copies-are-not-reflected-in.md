Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-domain-lifecycle-and-tagging
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-domain-lifecycle-and-tagging codeartifact-tag-isolation-across-domains codeartifact-repository-tag-lifecycle codeartifact-package-group-hierarchy-and-association codeartifact-package-group-lifecycle-and-hierarchy codeartifact-package-origin-configuration-and-version-delete codeartifact-copy-package-versions-between-repositories
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|ListTagsForResource|2xx->2xx|$.tags[*]
Issue-Key: codeartifact|DescribePackageGroup|4xx->2xx|$.message
Issue-Key: codeartifact|DescribePackageGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|DescribePackageGroup|4xx->2xx|status
Issue-Key: codeartifact|DescribePackage|4xx->2xx|$.message
Issue-Key: codeartifact|DescribePackage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: codeartifact|DescribePackage|4xx->2xx|status
Issue-Key: codeartifact|DescribePackageVersion|2xx->4xx|$.packageVersion
Issue-Key: codeartifact|DescribePackageVersion|2xx->4xx|status
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact removals and copies are not reflected in later reads

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
Four causes: three removals that are accepted but not committed, and one copy that is accepted but materializes nothing. All four must be closed before submitting.

Cause 1 of 4: UntagResource does not remove tags.
AWS: UntagResource removes the named tag keys from the resource and ListTagsForResource then returns the remaining set (recorded behavior only for the post-state; the recordings show an empty `tags` list after every tag added by the fixture is untagged, for both a domain and a repository resource ARN).
Emulator: ListTagsForResource still returns the tags that were untagged, with a 200 on the UntagResource call itself.
Why: tags drive cost allocation and tag-based access control. A client that removes a tag is told it succeeded while the tag remains readable, so untagging can never be verified and tag-based policy state is wrong.
Fix: the UntagResource handler/state (tag store keyed by resource ARN) — delete the requested keys so ListTagsForResource reflects them.
Traces: codeartifact-domain-lifecycle-and-tagging#task#7 (domain, 2-fixture cluster), codeartifact-repository-tag-lifecycle#task#7 (repository, two residual tags).

Cause 2 of 4: DeletePackageGroup leaves the group readable.
AWS: after a successful DeletePackageGroup, DescribePackageGroup for that pattern is refused with ResourceNotFoundException, HTTP 404 (Smithy: DescribePackageGroup errors -> ResourceNotFoundException httpError 404; recorded x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/').
Emulator: answers HTTP 200 with the deleted package group's description — the delete did not remove it from state.
Why: the group's origin-control restrictions keep applying from the client's point of view, and a 'deleted?' check that relies on the 404 never fires, so delete/recreate flows behave as conflicts rather than fresh creates.
Fix: the DeletePackageGroup handler/state, and the lookup DescribePackageGroup performs.
Traces: codeartifact-package-group-hierarchy-and-association#task#13.

Cause 3 of 4: deleting the last package version leaves the package readable.
AWS: once a package has no versions left, DescribePackage for it is refused with HTTP 404 (recorded response: 404 with message 'Package not found in repository. Package = {format=generic, name=<pkg>}'; the errortype was not asserted by the recording).
Emulator: answers HTTP 200 with the package description after its versions were deleted.
Why: a client cannot tell whether a package still exists in a repository; cleanup verification and 'recreate from scratch' flows see a phantom package, and listing/pruning logic loops.
Fix: the DeletePackageVersions handler/state must prune the package record when its last version is removed, and the DescribePackage lookup must 404 for a package with no versions.
Traces: codeartifact-package-origin-configuration-and-version-delete#task#11.

Cause 4 of 4: CopyPackageVersions does not materialize the versions in the destination repository.
AWS: CopyPackageVersions copies the named versions into the destination repository, after which DescribePackageVersion against the DESTINATION returns HTTP 200 with the full PackageVersionDescription (format, namespace, packageName, version, status 'Published', revision, publishedTime, origin.originType INTERNAL with origin.domainEntryPoint.repositoryName naming the SOURCE repository) — Smithy: DescribePackageVersion output packageVersion -> PackageVersionDescription, required.
Emulator: the copy call is answered, but DescribePackageVersion on the destination is refused with HTTP 404 and no `packageVersion` member; nothing was written to the destination.
Why: the whole promotion pattern CodeArtifact exists for (copy a validated version from a build repo into a release repo, then resolve it there) is unusable. A client's promotion step reports success and the artifact is not there.
Fix: the CopyPackageVersions handler/state — insert the copied versions (and their assets/origin provenance) into the destination repository's package records.
Traces: codeartifact-copy-package-versions-between-repositories#task#6.

## Evidence
Cause 1 — codeartifact-domain-lifecycle-and-tagging#task#7, ListTagsForResource, 200/200:
  $.tags[0]: extra (expected None, got {'key': 'sim2real', 'value': 'domain-lifecycle-and-tagging'})
  codeartifact-repository-tag-lifecycle#task#7: $.tags[0] extra {'key': 'Purpose', 'value': 'probe'}, $.tags[1] extra {'key': 'Stage', 'value': 'dev'}

Cause 2 — codeartifact-package-group-hierarchy-and-association#task#13, DescribePackageGroup:
  status: expected 404, got 200
  $headers.x-amzn-errortype: expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.codeartifact.controlplane/', got None
  $.message: expected a not-found message naming the group pattern, got None (message text is an example; the 404 + ResourceNotFoundException classification is the requirement)

Cause 3 — codeartifact-package-origin-configuration-and-version-delete#task#11, DescribePackage:
  status: expected 404, got 200
  $.message: expected 'Package not found in repository. Package = {format=generic, name=<pkg>}' (example), got None

Cause 4 — codeartifact-copy-package-versions-between-repositories#task#6, DescribePackageVersion:
  status: expected 200, got 404
  $.packageVersion: expected a full description (format 'generic', status 'Published', licenses [], origin.originType 'INTERNAL', origin.domainEntryPoint.repositoryName = source repo), got None

## Steps to Reproduce
1. Replay codeartifact-domain-lifecycle-and-tagging step task#7 (and codeartifact-repository-tag-lifecycle step task#7): TagResource, UntagResource the same keys, then ListTagsForResource -> the removed tags are still returned.
2. Replay codeartifact-package-group-hierarchy-and-association step task#13: DeletePackageGroup, then DescribePackageGroup for the same pattern -> 200 instead of 404 ResourceNotFoundException.
3. Replay codeartifact-package-origin-configuration-and-version-delete step task#11: DeletePackageVersions for the only version, then DescribePackage -> 200 instead of 404.
4. Replay codeartifact-copy-package-versions-between-repositories step task#6: CopyPackageVersions from source to destination, then DescribePackageVersion against the destination -> 404 instead of 200 with the copied version.
