Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override ec2-delete-launch-template-versions-partial-failure ec2-ibp-traj-launch-template-from-failed-ami-poll ec2-launch-template-versioning
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->2xx|$.CreateLaunchTemplateVersionResponse.launchTemplateVersion.createdBy
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item[*].createdBy
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item[*].defaultVersion
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item[*].launchTemplateData.networkInterfaceSet.item.associatePublicIpAddress
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item[*].launchTemplateData.networkInterfaceSet.item.groupSet
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item[*].versionNumber
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item
Issue-Key: ec2|CreateLaunchTemplateVersion|2xx->2xx|$.CreateLaunchTemplateVersionResponse.launchTemplateVersion.launchTemplateId
Issue-Commit: b3b51bdc06920507036cd31ba091b78449e90c00

# EC2 launch-template version reads misattribute the creator, order versions newest-first, and keep a deleted version

> _Note: this issue was identified against the code base at commit `b3b51bdc06920507036cd31ba091b78449e90c00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLaunchTemplateVersions

## Description
Three net-new root causes, grounded in six wire-replay divergence clusters, all on the launch-template version record the emulator serves from CreateLaunchTemplateVersion and DescribeLaunchTemplateVersions. Every cluster replayed with HTTP 200 on both sides: the status and the envelope are right, the version record's contents are not. Each cause must be closed before the six clusters pass.

Cause 1 of 3: the version's createdBy names the account root instead of the principal that made the call.
AWS: Smithy com.amazonaws.ec2#LaunchTemplateVersion$CreatedBy (xmlName createdBy, ec2QueryName CreatedBy, doc "The principal that created the version"). Recorded behavior: createdBy is the ARN of the IAM principal whose credentials signed the request - the recordings show an IAM-user ARN of the form arn:aws:iam::<account-id>:user/<user-name> (recorded as arn:aws:iam::000000000000:user/emulator). The account id and the user name are per-environment values taken from the signing credential; the requirement is the shape 'the ARN of the calling principal', not any one recorded string.
Emulator: every version, on both the CreateLaunchTemplateVersion response and the DescribeLaunchTemplateVersions projection, reports createdBy = 'arn:aws:iam::000000000000:root' - a constant account-root ARN that ignores the request's credential.
Why: a client cannot tell who created a launch-template version. Change attribution, audit and "who bumped this template" tooling reads root for every version, and a client that filters or asserts on its own principal ARN (as CI harnesses and drift detectors do) never matches.
Fix: the launch-template version builder/serializer behind CreateLaunchTemplateVersion and DescribeLaunchTemplateVersions - createdBy must be derived from the authenticated principal of the request instead of a hard-coded root ARN.
Traces: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch#task#11 (same cluster also covers ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override#task#14 and #task#16), ec2-delete-launch-template-versions-partial-failure#task#4, ec2-ibp-traj-launch-template-from-failed-ami-poll#task#9. On that last step the second reported key ($...launchTemplateId expected '{{lt_id}}') is an unresolved fixture placeholder, not an emulator value; only the createdBy half of that cluster is this defect.

Cause 2 of 3: DescribeLaunchTemplateVersions returns launchTemplateVersionSet newest-version-first instead of ascending by version number.
AWS: Smithy com.amazonaws.ec2#LaunchTemplateVersion$VersionNumber (xmlName versionNumber) and $DefaultVersion (xmlName defaultVersion, doc "Indicates whether the version is the default version"). Recorded behavior: with no Versions filter, launchTemplateVersionSet.item is ordered by ascending versionNumber - item[0] is version 1, item[1] is version 2 - so each item's defaultVersion flag and each item's launchTemplateData line up positionally with the version that was created first.
Emulator: the set is emitted in descending order (item[0] = version 2, item[1] = version 1). The reversal is the whole divergence: item[0].versionNumber expected '1', got '2'; item[0].defaultVersion expected 'true', got 'false'; item[1].defaultVersion expected 'false', got 'true'; and the launchTemplateData members reported missing on each item (item[0].launchTemplateData.networkInterfaceSet.item.groupSet, item[1].launchTemplateData.networkInterfaceSet.item.associatePublicIpAddress - Smithy LaunchTemplateInstanceNetworkInterfaceSpecification$Groups xmlName groupSet and $AssociatePublicIpAddress xmlName associatePublicIpAddress) are present on the other item, i.e. each version's own data is correct but sits at the wrong index.
Why: every client that reads "the current/first version" positionally - the console's version picker, Auto Scaling and CloudFormation template-version resolution, and any script that takes LaunchTemplateVersions[0] - picks the wrong version and therefore the wrong instance type, security groups and public-IP setting.
Fix: the DescribeLaunchTemplateVersions handler's result ordering (sort the resolved versions by ascending versionNumber before serializing), not the per-version projection.
Traces: ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override#task#15, ec2-launch-template-versioning#task#5.

Cause 3 of 3: after a partial-failure DeleteLaunchTemplateVersions the deleted version is still returned, so the template reads back with every version it ever had.
AWS: recorded behavior only (EC2 is an ec2Query service and DescribeLaunchTemplateVersions/DeleteLaunchTemplateVersions carry no modeled errors). Recorded behavior: a DeleteLaunchTemplateVersions call that names one live non-default version plus one version that does not exist succeeds per item - the live version is removed, the unknown one is reported as unsuccessful - and the following DescribeLaunchTemplateVersions returns ONLY the surviving version, a single launchTemplateVersionSet.item element carrying versionNumber 1 with defaultVersion 'true'.
Emulator: the same read returns two items (versionNumber '2' then '1'), i.e. the deletion never took effect - the replay reports a type_mismatch because AWS returns one item where the emulator returns a two-element list, and both items also carry the root createdBy of cause 1. This is the observable state consequence of DeleteLaunchTemplateVersions not applying; the open issues 20261108T203125Z-ec2-launch-template-version-operations-default-version-delete.md (cause 1) and the scaffold stub for delete_launch_template_versions describe the refusal semantics of that operation but neither carries this read's replay key, and neither asserts the post-delete read.
Why: version pruning is unobservable - a client that deletes a version and re-reads the template still sees it, so quota-driven cleanup loops forever and "is this version gone?" checks never succeed.
Fix: the DeleteLaunchTemplateVersions handler's state mutation (remove the named non-default versions from the template's version state and report the unknown ones per item), which the DescribeLaunchTemplateVersions read then reflects.
Traces: ec2-delete-launch-template-versions-partial-failure#task#6.

## Evidence
Cluster 1 - fixture ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch, step task#11, CreateLaunchTemplateVersion, status 200 -> 200 (affected_count 3, also ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override#task#14 and #task#16):
  $.CreateLaunchTemplateVersionResponse.launchTemplateVersion.createdBy: expected 'arn:aws:iam::000000000000:user/emulator' (the calling principal's ARN), got 'arn:aws:iam::000000000000:root'.

Cluster 2 - fixture ec2-delete-launch-template-versions-partial-failure, step task#4, DescribeLaunchTemplateVersions, status 200 -> 200:
  $...launchTemplateVersionSet.item[0].createdBy and item[1].createdBy: expected 'arn:aws:iam::000000000000:user/emulator', got 'arn:aws:iam::000000000000:root'.

Cluster 3 - fixture ec2-ibp-traj-launch-template-from-failed-ami-poll, step task#9, CreateLaunchTemplateVersion, status 200 -> 200:
  $.CreateLaunchTemplateVersionResponse.launchTemplateVersion.createdBy: expected 'arn:aws:iam::000000000000:user/emulator', got 'arn:aws:iam::000000000000:root'. (The co-reported launchTemplateId key compares against the unresolved placeholder '{{lt_id}}' and is not an emulator value.)

Cluster 4 - fixture ec2-asgssh-traj-template-public-ip-flag-shape-and-version-override, step task#15, DescribeLaunchTemplateVersions, status 200 -> 200:
  item[0].versionNumber: expected '1', got '2'; item[1].versionNumber: expected '2', got '1'.
  item[0].defaultVersion: expected 'true', got 'false'; item[1].defaultVersion: expected 'false', got 'true'.
  item[0].launchTemplateData.networkInterfaceSet.item.groupSet: expected {'groupId': '<the sg id the version was created with>'}, got None.
  item[1].launchTemplateData.networkInterfaceSet.item.associatePublicIpAddress: expected 'true', got None.
  (All eight keys are the ascending-vs-descending ordering of the set; the per-version data itself matches at the other index.)

Cluster 5 - fixture ec2-launch-template-versioning, step task#5, DescribeLaunchTemplateVersions, status 200 -> 200:
  item[0].defaultVersion: expected 'true', got 'false'; item[1].defaultVersion: expected 'false', got 'true'.

Cluster 6 - fixture ec2-delete-launch-template-versions-partial-failure, step task#6, DescribeLaunchTemplateVersions, status 200 -> 200:
  $...launchTemplateVersionSet.item: type_mismatch - expected a single element {versionNumber '1', defaultVersion 'true', launchTemplateData {instanceType t3.micro}, createdBy <calling principal>}, got a two-element list [{versionNumber '2', defaultVersion 'false', instanceType t3.small, createdBy root}, {versionNumber '1', defaultVersion 'true', instanceType t3.micro, createdBy root}] - version 2 was never deleted.

## Steps to Reproduce
1. ec2 CreateLaunchTemplate(LaunchTemplateName=NAME, LaunchTemplateData={InstanceType: t3.micro, NetworkInterfaces: [{DeviceIndex: 0, Groups: [SG]}]}) -> LT.
2. ec2 CreateLaunchTemplateVersion(LaunchTemplateId=LT, SourceVersion="1", LaunchTemplateData={InstanceType: t3.small, NetworkInterfaces: [{DeviceIndex: 0, AssociatePublicIpAddress: true}]}).
   Observed: launchTemplateVersion.createdBy = arn:aws:iam::<account>:root; AWS reports the ARN of the signing principal (Cause 1).
3. ec2 DescribeLaunchTemplateVersions(LaunchTemplateId=LT) with no Versions filter.
   Observed: launchTemplateVersionSet.item[0] is version 2 and item[1] is version 1; AWS returns them ascending, so defaultVersion and each version's networkInterfaceSet (groupSet on v1, associatePublicIpAddress on v2) line up with the opposite index (Cause 2). Every item's createdBy is the root ARN (Cause 1).
4. ec2 DeleteLaunchTemplateVersions(LaunchTemplateId=LT, Versions=["2", "99"]) - one live non-default version plus one that never existed.
5. ec2 DescribeLaunchTemplateVersions(LaunchTemplateId=LT).
   Observed: both version 2 and version 1 are still returned; AWS returns only version 1 (Cause 3).
