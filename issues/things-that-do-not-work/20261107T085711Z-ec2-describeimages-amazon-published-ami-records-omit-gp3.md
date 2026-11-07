Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-public-ami-parameter-resolves-to-a-describable-amazon-image
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-public-ami-parameter-resolves-to-a-describable-amazon-image
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item.blockDeviceMapping.item.ebs.iops
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item.blockDeviceMapping.item.ebs.throughput
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item.deprecationTime
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item.publicSsmParameterName
Issue-Commit: 9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83

# EC2 DescribeImages Amazon-published AMI records omit gp3 EBS iops/throughput and deprecationTime and report publicSsmParameterName wrongly

> _Note: this issue was identified against the code base at commit `9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
2 CAUSES, both in the Amazon-published AMI record DescribeImages serves for an image reached through a public SSM AMI alias.

Note on related open issues: 20261104T051153Z-cloudfront-ec2-describeimages-reports-no-amazon-published-amazon.md and 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md concern DescribeImages returning NO Amazon-published image at all; here an image IS described, and the divergence is in the described record's content. The replay reports this cluster as unowned, so it is filed net-new.

Cause 1 of 2: the described image's EBS root block device and deprecation metadata are incomplete.
AWS: 200 where an Amazon-published AMI's blockDeviceMapping entry for the root device reports its EBS volume fully - including ebs.iops and ebs.throughput for a gp3 root volume (Smithy: com.amazonaws.ec2#EbsBlockDevice members Iops, xmlName 'iops', and Throughput, xmlName 'throughput'; the model documents gp3 as defaulting to 3,000 IOPS with throughput valid from 125 MiB/s, which is what the recording shows) - and reports deprecationTime for the image (Smithy: com.amazonaws.ec2#Image member DeprecationTime, xmlName 'deprecationTime', 'The date and time to deprecate the AMI, in UTC, in the following format: YYYY-MM-DDTHH:MM:SSZ'). Amazon-published AMIs always carry a deprecation time. The requirement is that these members are PRESENT and consistent with the volume type and the image's publication; the literal numbers and timestamp of any one recording are environment data (the corpus itself asserts deprecationTime only as shaped).
Emulator: 200 with $.DescribeImagesResponse.imagesSet.item.blockDeviceMapping.item.ebs.iops missing, ...ebs.throughput missing and ...item.deprecationTime missing.
Why: a client that sizes or re-creates a volume from the AMI's block device mapping (RunInstances overriding the root volume, CreateImage/RegisterImage round-trips, Terraform/CDK plans that diff gp3 iops and throughput) reads absent values where AWS gives numbers and either mis-provisions or fails its own validation; AMI-lifecycle tooling that filters or alarms on deprecationTime sees every Amazon image as never-deprecating.
Fix: the EC2 image catalog/state record for Amazon-published AMIs and the DescribeImages serializer that projects its block device mappings - populate EbsBlockDevice.Iops/Throughput for gp3 root volumes and Image.DeprecationTime.
Traces: ssm-public-ami-parameter-resolves-to-a-describable-amazon-image#task#2, #task#4.

Cause 2 of 2: publicSsmParameterName is serialized with a leading forward slash and names an alias other than the one that resolved to the image.
AWS: 200 with imagesSet.item.publicSsmParameterName naming the public Systems Manager parameter that resolves to THIS AMI, under the 'aws/service/' namespace and WITHOUT a leading slash - Smithy: com.amazonaws.ec2#Image member PublicSsmParameterName, xmlName 'publicSsmParameterName', documented as 'The name of the public Systems Manager parameter that resolves to this AMI, under the aws/service/ namespace.' The recording shows the un-slashed form, e.g. 'aws/service/ami-amazon-linux-latest/<alias>', for the alias the image was resolved from. Fixed text: the 'aws/service/...' namespace prefix with no leading '/'; variable part: the alias path segment, which must be the alias whose parameter value is this image id.
Emulator: 200 with publicSsmParameterName = '/aws/service/ami-amazon-linux-latest/<some-other-alias>' - a leading '/' that AWS never emits, and an alias that is not the one the request resolved through.
Why: tooling that round-trips the two surfaces - take the AMI's publicSsmParameterName and read it back with ssm:GetParameter, or compare it to the resolve:ssm: reference it launched from - builds a name AWS would never produce ('/aws/service/...'), so the lookup misses, and the mismatched alias makes the emulator claim the image came from a different published family than it did.
Fix: the DescribeImages serializer/projection for Image.PublicSsmParameterName (strip the leading slash, i.e. emit the aws/service-relative name) and the EC2<->SSM public-alias mapping that chooses which alias is reported for a resolved image.
Traces: ssm-public-ami-parameter-resolves-to-a-describable-amazon-image#task#2, #task#4.

## Evidence
Replay of simparity-wire-aws-v1. Cluster service=ec2|DescribeImages|status=200->200, representative ssm-public-ami-parameter-resolves-to-a-describable-amazon-image#task#2, also affecting #task#4 (affected_count 2), verdict mismatch, no owning issue and no blocking stub.

Expected 200 / actual 200, with these field verdicts:
  $.DescribeImagesResponse.imagesSet.item.blockDeviceMapping.item.ebs.iops: missing (expected '3000', got None)
  $.DescribeImagesResponse.imagesSet.item.blockDeviceMapping.item.ebs.throughput: missing (expected '125', got None)
  $.DescribeImagesResponse.imagesSet.item.deprecationTime: missing (expected '<shaped-not-asserted>', got None)
  $.DescribeImagesResponse.imagesSet.item.publicSsmParameterName: value_mismatch (expected 'aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64', got '/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-x86_64')

The '3000'/'125' values are the gp3 defaults the model documents rather than per-run data, but the requirement asserted here is the presence of the members for a gp3 root volume; deprecationTime is asserted as shaped only. For publicSsmParameterName the asserted divergence is the leading '/' (AWS emits the name under the 'aws/service/' namespace with no leading slash) plus the alias not matching the parameter the image was resolved through.

Model consulted: services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json (DescribeImages, Image, EbsBlockDevice).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Fixture ssm-public-ami-parameter-resolves-to-a-describable-amazon-image, step task#2 (and again at task#4): resolve a public /aws/service/ami-amazon-linux-latest AMI alias parameter, then call EC2 DescribeImages for the resolved image id.
4. In the response's imagesSet.item: observe no ebs.iops and no ebs.throughput on the gp3 root block device mapping, no deprecationTime on the image, and publicSsmParameterName emitted as '/aws/service/ami-amazon-linux-latest/...' (leading slash) naming a different alias than the one resolved - AWS returns iops/throughput for the gp3 volume, a deprecationTime, and the un-slashed 'aws/service/ami-amazon-linux-latest/<resolved-alias>'.
5. Equivalent by hand: aws ec2 describe-images --image-ids <id resolved from the public parameter> against the emulator endpoint.
