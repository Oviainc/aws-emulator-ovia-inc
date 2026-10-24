Issue-Source: trace-sweep-divergence
Issue-Trace: codeartifact-package-group-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codeartifact-package-group-error-paths codeartifact-package-group-hierarchy-and-association codeartifact-package-group-lifecycle-and-hierarchy codeartifact-package-group-origin-configuration
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact|CreatePackageGroup|2xx->2xx|$.packageGroup.arn
Issue-Key: codeartifact|CreatePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.EXTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|CreatePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.INTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|CreatePackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.PUBLISH.inheritedFrom.arn
Issue-Key: codeartifact|CreatePackageGroup|2xx->2xx|$.packageGroup.parent.arn
Issue-Key: codeartifact|GetAssociatedPackageGroup|2xx->2xx|$.packageGroup.arn
Issue-Key: codeartifact|GetAssociatedPackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.EXTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|GetAssociatedPackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.INTERNAL_UPSTREAM.inheritedFrom.arn
Issue-Key: codeartifact|GetAssociatedPackageGroup|2xx->2xx|$.packageGroup.originConfiguration.restrictions.PUBLISH.inheritedFrom.arn
Issue-Key: codeartifact|GetAssociatedPackageGroup|2xx->2xx|$.packageGroup.parent.arn
Issue-Related: 20261023T134651Z-codeartifact-update-operations-do-not-persist-attribute-changes.md
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CodeArtifact package-group ARNs omit percent-encoding of pattern characters

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePackageGroup

## Description
One cause, reached through four operations that all serialize a PackageGroupDescription.

Cause 1 of 1: the package-group ARN is built from the raw group pattern, without percent-encoding the characters that are not legal unencoded in an ARN resource path.
AWS: PackageGroupDescription.arn is an Arn (Smithy: PackageGroupDescription.arn -> Arn, @length 1..1011, @pattern ^\S+$) and the recorded ARNs percent-encode the pattern's wildcard and sigil characters in the resource segment: a group pattern ending in '/*' yields '...:package-group/<domain>/npm/<group>/%2a', the domain-root group yields '...:package-group/<domain>/%2a', and a pattern ending in 'thing$' yields '...:package-group/<domain>/generic/sim2real-probe/thing%24'. The same encoding appears in the nested PackageGroupReference ARNs: packageGroup.parent.arn and packageGroup.originConfiguration.restrictions.{PUBLISH,EXTERNAL_UPSTREAM,INTERNAL_UPSTREAM}.inheritedFrom.arn.
Emulator: emits the raw characters ('.../%2a' -> '.../*', 'thing%24' -> 'thing$') in arn, parent.arn and every inheritedFrom.arn.
Why: the ARN is an identifier clients carry into other APIs — IAM resource elements, tagging (TagResource/ListTagsForResource take a resourceArn), and ARN parsers. An unencoded '*' in the resource path is a wildcard in IAM policy matching, so a policy built from the emulator's ARN is broader than intended; and an ARN string comparison against one obtained from AWS never matches. Clients that round-trip the ARN back into CodeArtifact also get a different resource identity than AWS would.
Fix: the ARN construction used by the package-group handlers (CreatePackageGroup, UpdatePackageGroup, DescribePackageGroup, GetAssociatedPackageGroup, ListPackageGroups) — percent-encode the pattern when composing the ARN resource segment, and use the same helper for the parent and inheritedFrom references so the whole description is consistent.
Traces: codeartifact-package-group-error-paths#task#3 (CreatePackageGroup, 5-fixture cluster), codeartifact-package-group-hierarchy-and-association#task#5 (DescribePackageGroup, 4-fixture cluster), codeartifact-package-group-hierarchy-and-association#task#7 (GetAssociatedPackageGroup, including the '%24' case), codeartifact-package-group-lifecycle-and-hierarchy#task#8 (CreatePackageGroup of a child group, parent/inheritedFrom ARNs).

Note: codeartifact-package-group-lifecycle-and-hierarchy#task#6 shows this same ARN defect alongside UpdatePackageGroup not persisting contactInfo/description; that persistence fault is filed separately under the CodeArtifact update-operations issue.

## Evidence
codeartifact-package-group-error-paths#task#3, CreatePackageGroup, 200/200:
  $.packageGroup.arn: expected '...:package-group/<domain>/npm/<group>/%2a', got '...:package-group/<domain>/npm/<group>/*'
  $.packageGroup.parent.arn: expected '...:package-group/<domain>/%2a', got '...:package-group/<domain>/*'
  $.packageGroup.originConfiguration.restrictions.PUBLISH.inheritedFrom.arn / .EXTERNAL_UPSTREAM.inheritedFrom.arn / .INTERNAL_UPSTREAM.inheritedFrom.arn: same '%2a' vs '*' divergence

codeartifact-package-group-hierarchy-and-association#task#7, GetAssociatedPackageGroup, 200/200:
  $.packageGroup.arn: expected '...:package-group/<domain>/generic/sim2real-probe/thing%24', got '...:package-group/<domain>/generic/sim2real-probe/thing$'
  $.packageGroup.parent.arn: expected '.../generic/sim2real-probe/%2a', got '.../generic/sim2real-probe/*'

Same divergence on DescribePackageGroup (codeartifact-package-group-hierarchy-and-association#task#5) and on CreatePackageGroup of a child group (codeartifact-package-group-lifecycle-and-hierarchy#task#8). Domain and group names in the ARNs are fixture-generated; the requirement is the percent-encoding of '*' as %2a and '$' as %24 in the resource segment.

## Steps to Reproduce
1. Replay codeartifact-package-group-error-paths step task#3: POST /v1/package-group with packageGroup '/npm/<group>/*' -> read $.packageGroup.arn and the parent/inheritedFrom ARNs; '*' appears raw instead of '%2a'.
2. Replay codeartifact-package-group-hierarchy-and-association step task#5 (DescribePackageGroup) and step task#7 (GetAssociatedPackageGroup for a pattern ending in 'thing$') -> '$' appears raw instead of '%24'.
3. Replay codeartifact-package-group-lifecycle-and-hierarchy step task#8: create a child group and inspect packageGroup.parent.arn.
