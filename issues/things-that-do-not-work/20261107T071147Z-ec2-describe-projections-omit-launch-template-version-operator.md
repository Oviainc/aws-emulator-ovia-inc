Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-asghc-asg-keeps-the-latest-version-literal-while-the-template-moves
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-asghc-asg-keeps-the-latest-version-literal-while-the-template-moves autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item.defaultVersion
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item.launchTemplateData.instanceType
Issue-Key: ec2|DescribeLaunchTemplateVersions|2xx->2xx|$.DescribeLaunchTemplateVersionsResponse.launchTemplateVersionSet.item.versionNumber
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000003d"].dhcpOptionsSource
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000003f"].dhcpOptionsSource
Issue-Related: 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md
Issue-Commit: 5e1f0b507a128238d75b745ce1d1dcae4dc5a977

# EC2 describe projections omit launch-template version operator and subnet dhcpOptionsSource members

> _Note: this issue was identified against the code base at commit `5e1f0b507a128238d75b745ce1d1dcae4dc5a977`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLaunchTemplateVersions

## Description
Two net-new root causes with one shared fault: EC2 read projections drop members AWS always serves, and the launch-template version read also reports the wrong version identity. Each cause must be closed separately.

Cause 1 of 2 - DescribeLaunchTemplateVersions omits operator and misreports the version's number and default flag.
AWS: Smithy: LaunchTemplateVersion members VersionNumber (xmlName versionNumber), DefaultVersion (xmlName defaultVersion), Operator (xmlName operator, OperatorResponse) and LaunchTemplateData (xmlName launchTemplateData). When a second version has been created and the template's default is still version 1, the $Latest read serves versionNumber 2, defaultVersion false, the data of that version, and operator with managed 'false'.
Emulator: the response omits the operator element entirely (got None) and serves versionNumber '1' with defaultVersion 'true' and the version-1 launchTemplateData (instanceType of version 1 rather than the newer version's), i.e. the newest version is invisible and the only version present is reported as the default.
Why: a client resolving $Latest to decide what an ASG will launch reads the stale version and its stale instance type, and the omitted operator member deserializes to an empty optional where AWS always sends a value. The version/default half of this is downstream of CreateLaunchTemplateVersion not being served (its stub issue is filed separately), but the operator omission is purely the serializer.
Fix: the DescribeLaunchTemplateVersions serializer/projection (emit Operator) and the launch-template version store's $Latest/default-version resolution. Related open issue 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md carries a different cluster of this family and does not cover this step.
Traces: autoscaling-asghc-asg-keeps-the-latest-version-literal-while-the-template-moves#task#9.

Cause 2 of 2 - DescribeSubnets omits dhcpOptionsSource on every subnet.
AWS: recorded behavior only - the member is not in the checked-out Smithy Subnet shape, but the recorded DescribeSubnets response carries dhcpOptionsSource with the value 'vpc' for each subnet in subnetSet.item (the source of the subnet's DHCP options; 'vpc' when they are inherited from the VPC).
Emulator: the member is absent from every subnet item (got None), while the rest of the subnet projection matches.
Why: a client that reads which DHCP option set applies to a subnet gets nothing, so DHCP-inheritance logic sees an unset value for every subnet.
Fix: the DescribeSubnets serializer/subnet projection (and the subnet state that must record the DHCP options source). An open issue with the same title-level symptom exists - 20261107T035051Z-ec2-describesubnets-omits-the-dhcpoptionssource-member-on-every.md - but it does not carry this fixture step's replay key, so this cause is filed here and must be re-replayed against this step.
Traces: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13.

## Evidence
Cause 1 (autoscaling-asghc-asg-keeps-the-latest-version-literal-while-the-template-moves#task#9, DescribeLaunchTemplateVersions, 200 vs 200): launchTemplateVersionSet.item.operator expected {'managed': 'false'}, got None; item.versionNumber expected '2', got '1'; item.defaultVersion expected 'false', got 'true'; item.launchTemplateData.instanceType expected 't3.small', got 't3.micro'.
Cause 2 (autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13, DescribeSubnets, 200 vs 200): subnetSet.item[subnetId=...].dhcpOptionsSource expected 'vpc', got None - for both subnets in the response (the subnet ids themselves are per-run values).

## Steps to Reproduce
Replay the simparity-wire-aws-v1 corpus and inspect autoscaling-asghc-asg-keeps-the-latest-version-literal-while-the-template-moves#task#9 (DescribeLaunchTemplateVersions for $Latest after a second version was created, with the default still version 1) and autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13 (DescribeSubnets over the probe's private subnets).
