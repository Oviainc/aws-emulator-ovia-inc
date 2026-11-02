Issue-Source: trace-sweep-divergence
Issue-Trace: asglf-disabled-kms-key-launch-cancelled
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asglf-disabled-kms-key-launch-cancelled asglf-lt-contents-refusals autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->4xx|$.DescribeLaunchTemplateVersionsResponse
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->4xx|status
Issue-Key: ec2|CreateLaunchTemplate|4xx->2xx|$.Response
Issue-Key: ec2|CreateLaunchTemplate|4xx->2xx|status
Issue-Key: ec2|DeleteLaunchTemplate|4xx->2xx|status
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000002b"].dhcpOptionsSource
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000002d"].dhcpOptionsSource
Issue-Commit: a4ef059b31cd49fc5529baf331c68065b4514ad5

# EC2 launch template read, validation and subnet projection diverge

> _Note: this issue was identified against the code base at commit `a4ef059b31cd49fc5529baf331c68065b4514ad5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLaunchTemplateVersions

## Description
FOUR net-new causes on the EC2 (ec2Query) launch-template and subnet surface. They share one fault: EC2's launch-template read/validation path and the subnet projection do not match AWS - a valid read is refused, an invalid create is accepted, the matching delete refusal is therefore absent, and a described subnet is missing a member AWS returns. A fixer must close ALL FOUR before submitting.

Cause 1 of 4 - DescribeLaunchTemplateVersions on an existing template is refused. AWS: HTTP 200 with DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet containing one item per version, each carrying launchTemplateId, launchTemplateName, versionNumber, defaultVersion, createTime, createdBy and the full launchTemplateData as stored (here a blockDeviceMappingSet whose ebs block keeps deleteOnTermination, encrypted, kmsKeyId, volumeSize and volumeType, plus imageId and instanceType). Smithy: DescribeLaunchTemplateVersions input accepts LaunchTemplateId or LaunchTemplateName with Versions/MinVersion/MaxVersion/Filters, output DescribeLaunchTemplateVersionsResult.LaunchTemplateVersions (xmlName launchTemplateVersionSet); the operation models NO client errors, so a resolvable template must answer 200. Emulator: HTTP 400 with no DescribeLaunchTemplateVersionsResponse element at all. Why: the launch-template version contents are how a client verifies what a group or a RunInstances call will actually launch (including the KMS key for the root volume); a 400 makes that unverifiable and breaks every dependent step. Hypothesis: the request form this fixture uses (a template whose version data carries an ebs kmsKeyId) is not handled by the handler's version lookup. Fix: the DescribeLaunchTemplateVersions handler plus the launch-template version projection in EC2 state. Traces: asglf-disabled-kms-key-launch-cancelled#task#12.

Cause 2 of 4 - CreateLaunchTemplate accepts a malformed ImageId. AWS: HTTP 400 with the ec2Query error envelope Response.Errors.Error.Code 'InvalidAMIID.Malformed' and Message "The image ID '<image-id>' is not valid. The expected format is ami-xxxxxxxx or ami-xxxxxxxxxxxxxxxxx." - the two sentences are fixed text and the only variable part is the offending ImageId quoted from the request; the RequestID element is per-run. Smithy: CreateLaunchTemplate requires LaunchTemplateName and LaunchTemplateData (RequestLaunchTemplateData, whose ImageId has no pattern constraint in the model), and models no operation-level errors, so the AMI-id syntax refusal is recorded behavior only. Emulator: HTTP 200 with a normal CreateLaunchTemplate result and no Response error envelope - the template is created around an unusable image id. Why: the worse direction - every later launch from that template fails for a reason the client was never told at create time, where AWS rejects the create outright. Fix: CreateLaunchTemplate input validation must check LaunchTemplateData.ImageId syntax and emit the InvalidAMIID.Malformed envelope. Traces: asglf-lt-contents-refusals#task#13.

Cause 3 of 4 - DeleteLaunchTemplate succeeds where AWS refuses. AWS: HTTP 400 for the delete this fixture issues during teardown (the template the preceding malformed-contents create never produced). Smithy: DeleteLaunchTemplate takes LaunchTemplateId or LaunchTemplateName and models no operation-level errors, so the refusal is the ec2Query error envelope (recorded behavior only for the code/message). Emulator: HTTP 200 - it deletes a template that should not exist. Why: a cleanup path that distinguishes 'deleted' from 'was never there' records the wrong outcome, and the emulator's launch-template namespace is left holding entries AWS never created. Hypothesis: this is the downstream face of Cause 2 - because the malformed create was accepted, the template exists and the delete succeeds; re-replay this binding after Cause 2 is closed and, if the delete still answers 200 for an absent template, fix the DeleteLaunchTemplate handler's lookup. Fix: CreateLaunchTemplate validation (Cause 2) and then the DeleteLaunchTemplate handler's not-found refusal. Traces: asglf-lt-contents-refusals#teardown#28.

Cause 4 of 4 - DescribeSubnets omits dhcpOptionsSource. AWS: every item in DescribeSubnetsResponse.subnetSet carries dhcpOptionsSource, whose value states where the subnet's DHCP options come from ('vpc' when they are inherited from the VPC). Recorded behavior only: the on-disk Smithy model's shape Subnet (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) does NOT declare this member, so the model is behind the wire here; the recording is the authority. Emulator: the member is absent (got None) on every described subnet. Why: a client that reads a subnet's DHCP-options source to decide whether it must set subnet-level options reads null and cannot tell inherited from overridden. Fix: add the member to the Subnet projection in the DescribeSubnets serializer (and to subnet state), defaulting to the VPC-inherited value. Traces: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200). Verbatim from the clusters:
1) DescribeLaunchTemplateVersions asglf-disabled-kms-key-launch-cancelled#task#12 - 'status: expected 200, got 400'; '$.DescribeLaunchTemplateVersionsResponse: missing (expected {requestId: <per-run>, launchTemplateVersionSet: {item: {createTime: <per-run>, createdBy: arn:aws:iam::<account>:user/emulator, defaultVersion: true, launchTemplateData: {blockDeviceMappingSet: {item: {deviceName: /dev/xvda, ebs: {deleteOnTermination: true, encrypted: true, kmsKeyId: <key arn>, volumeSize: 8, volumeType: gp3}}}, imageId: <ami>, instanceType: t3.micro}, launchTemplateId: <lt id>, launchTemplateName: <name>, operator: {managed: false}, versionNumber: 1}}}, got None)'. Ids, arns, names and timestamps are per-run.
2) CreateLaunchTemplate asglf-lt-contents-refusals#task#13 - 'status: expected 400, got 200'; '$.Response: missing (expected {Errors: {Error: {Code: InvalidAMIID.Malformed, Message: "The image ID \'ami-0123456789abcdef0\' is not valid. The expected format is ami-xxxxxxxx or ami-xxxxxxxxxxxxxxxxx."}}, RequestID: <per-run>}, got None)'.
3) DeleteLaunchTemplate asglf-lt-contents-refusals#teardown#28 - 'status: expected 400, got 200'.
4) DescribeSubnets autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13 - 200 -> 200; '$.DescribeSubnetsResponse.subnetSet.item[subnetId="..."].dhcpOptionsSource: missing (expected \'vpc\', got None)' for both described subnets (the subnet ids are per-run).
Smithy grounding (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): CreateLaunchTemplate requires LaunchTemplateName + LaunchTemplateData and models no errors; DescribeLaunchTemplateVersions output LaunchTemplateVersions (xmlName launchTemplateVersionSet), no modeled errors; DeleteLaunchTemplate takes LaunchTemplateId/LaunchTemplateName, no modeled errors; shape Subnet does not declare dhcpOptionsSource (recorded behavior only).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Cause 1: replay asglf-disabled-kms-key-launch-cancelled to task#12 - Action=DescribeLaunchTemplateVersions for the template created in setup (its version data carries an ebs kmsKeyId) and expect 200 with launchTemplateVersionSet, not 400.
3. Cause 2: replay asglf-lt-contents-refusals task#13 - Action=CreateLaunchTemplate with LaunchTemplateData.ImageId='ami-0123456789abcdef0' and expect 400 with Errors.Error.Code InvalidAMIID.Malformed and AWS's two-sentence message quoting the image id.
4. Cause 3: same fixture teardown#28 - Action=DeleteLaunchTemplate for that template and expect 400 (re-check after Cause 2 is fixed, since the create should no longer have produced it).
5. Cause 4: replay autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets task#13 - Action=DescribeSubnets and expect every subnetSet item to carry dhcpOptionsSource ('vpc' for VPC-inherited options).
6. Re-replay all four bindings after the fix.
