Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.list-distributions-by-vpc-origin-id-index-consistency
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.list-distributions-by-vpc-origin-id-index-consistency|_launch_backing_instance#2|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found to launch the VPC-origin backing instance")
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# cloudfront: EC2 DescribeImages reports no Amazon-published Amazon Linux 2 AMI, so nothing can launch the instance a CloudFront VPC origin needs

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: a DescribeImages call that asks for the Amazon-published Amazon Linux 2 AMI catalog answers 200 with an EMPTY Images list, so any flow that has to launch a backing EC2 instance (here, the instance a CloudFront VPC origin is created over) cannot pick an AMI at all.
Failing check: cloudfront.list-distributions-by-vpc-origin-id-index-consistency.

AWS: DescribeImages "describes the specified images (AMIs, AKIs, and ARIs) available to you"; the images Amazon publishes are visible to every account under the `amazon` owner alias. Smithy (ec2): DescribeImages -> DescribeImagesResult$Images (ImageList); DescribeImagesRequest$Owners accepts "a combination of Amazon Web Services account IDs, self, amazon, aws-backup-vault, and aws-marketplace", and DescribeImagesRequest$Filters documents the `name`, `state` and `owner-alias` filters. In every commercial Region the published catalog contains Amazon Linux 2 x86_64 gp2 AMIs whose Name matches `amzn2-ami-hvm-*-x86_64-gp2` with State `available`, so a request with Owners=["amazon"] and Filters name=`amzn2-ami-hvm-*-x86_64-gp2` + state=`available` returns one or more Image entries, each carrying ImageId and CreationDate (the caller sorts on CreationDate and launches the newest). The exact AMI ids, names and dates are per-Region, per-release values and are NOT part of the requirement — the requirement is that at least one Amazon-owned, available Amazon Linux 2 x86_64 gp2 image is reported.

Emulator: the call is served (HTTP 200, no error) with `Images` empty, so the caller has nothing to launch from. The handler itself is implemented and does resolve the `amazon` owner alias and the `name`/`state` filters (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs), so the gap is in the published-image catalog the store is seeded with, not in filtering.

Why: a client that resolves its AMI the documented way — DescribeImages filtered by Owners=amazon plus a name pattern, as every AWS launch tutorial, CDK/CFN SSM-free pattern and provisioning script does — gets an empty list where AWS always returns images, and either aborts or must hard-code an AMI id that exists nowhere. Every behavioural flow that needs a running instance behind it (CloudFront VPC origins, ALB/NLB targets) is unreachable for that reason alone.

Fix: the EC2 published-image catalog this service seeds a fresh account with — `insert_published_images` / the published-image definitions in services/service-com-amazonaws-ec2-amazonec2/src/state.rs — must include Amazon-owned (ImageOwnerAlias `amazon`, Public true, State `available`) Amazon Linux 2 x86_64 gp2 images whose Name follows the published `amzn2-ami-hvm-<version>-x86_64-gp2` form, so the DescribeImages path in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs can report them. No change to the filter logic is needed.

Traces: cloudfront.list-distributions-by-vpc-origin-id-index-consistency — fails inside _launch_backing_instance (check line 139) immediately after the DescribeImages call at check line 139-ff.

## Evidence
Expected (AWS): ec2.describe_images(Owners=["amazon"], Filters=[{"Name":"name","Values":["amzn2-ami-hvm-*-x86_64-gp2"]},{"Name":"state","Values":["available"]}]) returns Images with at least one entry carrying ImageId and CreationDate.
Actual (emulator): HTTP 200 with Images == [], so the check raises `AssertionError: no Amazon Linux 2 AMI found to launch the VPC-origin backing instance` (check_cloudfront_list_distributions_by_vpc_origin_id_index_consistency.py line 139, reached from _create_vpc_origin line 171). No error code is returned — the divergence is an empty result set where AWS reports the published catalog.

## Steps to Reproduce
1. Point an EC2 client at the emulator.
2. Call DescribeImages with Owners=["amazon"] and Filters=[{Name:"name",Values:["amzn2-ami-hvm-*-x86_64-gp2"]},{Name:"state",Values:["available"]}].
3. Observe Images is empty; on AWS at least one Amazon-owned available AMI is returned.
4. Replay cloudfront.list-distributions-by-vpc-origin-id-index-consistency: it aborts in _launch_backing_instance before any CloudFront VPC-origin call is made.
