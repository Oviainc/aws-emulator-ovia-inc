Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2-instance-connect.send-ssh-public-key-nonexistent-instance-not-found-rejected
Issue-Shape-Id: com.amazonaws.ec2instanceconnect#AWSEC2InstanceConnectService
Issue-Key: ec2-instance-connect.send-ssh-public-key-nonexistent-instance-not-found-rejected|_latest_al2_ami#4|AssertionError|raise AssertionError("no Amazon Linux 2 AMI found in this region")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2-instance-connect: EC2 DescribeImages reports no Amazon-published Amazon Linux 2 AMI, so no instance can be launched to exercise SendSSHPublicKey

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
Cause 1 of 1: a DescribeImages call that asks for the Amazon-published Amazon Linux 2 catalog (Owners=["amazon"], Filters name=`amzn2-ami-hvm-*-x86_64-gp2` + state=`available`) is served HTTP 200 with an EMPTY `Images` list. Every behavioural flow that must first launch a backing EC2 instance therefore cannot pick an AMI, and the EC2 Instance Connect data-plane contract that flow exists to check (SendSSHPublicKey succeeding against a running instance, and answering EC2InstanceNotFoundException "Instance not found." for a well-formed but unknown instance id) is never reached. Failing check: ec2-instance-connect.send-ssh-public-key-nonexistent-instance-not-found-rejected.

AWS: DescribeImages describes the images (AMIs, AKIs, ARIs) available to the caller, which always includes the images Amazon publishes under the `amazon` owner alias. Smithy (com.amazonaws.ec2#AmazonEC2): DescribeImages -> input DescribeImagesRequest with members `Owners` (OwnerStringList, wire name `Owner`) and `Filters` (FilterList, wire name `Filter`); output DescribeImagesResult$Images (ImageList, wire name `imagesSet`); the operation models NO errors, so the only AWS-correct answer here is a 200 whose Images list reports the matching published images. In every commercial Region the published catalog contains Amazon-owned Amazon Linux 2 x86_64 gp2 images whose Name matches `amzn2-ami-hvm-<version>-x86_64-gp2` with State `available`, each carrying ImageId and CreationDate (callers sort on CreationDate and launch the newest). The concrete AMI ids, names and dates are per-Region, per-release values and are NOT the requirement; the requirement is that at least one Amazon-owned, available Amazon Linux 2 x86_64 gp2 image is reported for this request.

Emulator: the request is accepted and answered 200 with `Images == []` — no error code, no error message, simply an empty result set where AWS reports the published catalog. The DescribeImages handler itself resolves the `amazon` owner alias and the `name`/`state` filters, so the gap is in the published-image catalog a fresh account's EC2 state is seeded with, not in the filter logic.

Why: a client that resolves its AMI the documented way — DescribeImages with Owners=amazon plus a name pattern, as every AWS launch tutorial and provisioning script does — gets nothing back and either aborts or must hard-code an AMI id that exists nowhere in the emulator. Downstream, nothing that needs a running instance (EC2 Instance Connect pushes, ALB/NLB targets, serial-console flows) can be driven at all, so those contracts are silently unverifiable rather than merely wrong.

Fix: the EC2 published-image seeding of the ec2 service state (the published-image definitions/insertion in services/service-com-amazonaws-ec2-amazonec2/src/state.rs, consumed by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs) must include Amazon-owned images (ImageOwnerAlias `amazon`, Public true, State `available`, Architecture x86_64, RootDeviceType ebs/gp2) whose Name follows the published `amzn2-ami-hvm-<version>-x86_64-gp2` form. No change to the filter path is needed.

Related already-open issue, named here because it does not cover this check: 20261104T051153Z-cloudfront-ec2-describeimages-reports-no-amazon-published-amazon.md describes the same empty-catalog behaviour reached from a CloudFront VPC-origin flow; this filing is grounded in the ec2-instance-connect check, which the suite did not mark as owned by it.

Traces: ec2-instance-connect.send-ssh-public-key-nonexistent-instance-not-found-rejected — aborts in `_latest_al2_ami` (check line 80), called from `run` line 24, before any ec2-instance-connect call is made.

## Evidence
Expected (AWS): ec2.describe_images(Owners=["amazon"], Filters=[{"Name":"name","Values":["amzn2-ami-hvm-*-x86_64-gp2"]},{"Name":"state","Values":["available"]}]) -> HTTP 200 with Images containing at least one Amazon-owned, available Amazon Linux 2 x86_64 gp2 entry, each with ImageId and CreationDate.

Actual (emulator): HTTP 200 with Images == [] (no error code, no error message). The check consequently raises:
  File "check_ec2_instance_connect_send_ssh_public_key_nonexistent_instance_not_found_rejected.py", line 24, in run
    ami_id = _latest_al2_ami(ec2)
  File "...", line 80, in _latest_al2_ami
    raise AssertionError("no Amazon Linux 2 AMI found in this region")
  AssertionError: no Amazon Linux 2 AMI found in this region

Because of this the intended assertions were never exercised: SendSSHPublicKey against a running instance returning Success=true, and SendSSHPublicKey for a well-formed unknown instance id being refused with error code `EC2InstanceNotFoundException` and the message `Instance not found.` (that message is entirely fixed text — it carries no request- or state-derived values).

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it (fresh account/region state).
2. Call DescribeImages with Owners=["amazon"] and Filters=[{Name:"name",Values:["amzn2-ami-hvm-*-x86_64-gp2"]},{Name:"state",Values:["available"]}].
3. Observe HTTP 200 with an empty Images list; on AWS at least one Amazon-owned available Amazon Linux 2 AMI is reported.
4. Replay ec2-instance-connect.send-ssh-public-key-nonexistent-instance-not-found-rejected: it aborts in _latest_al2_ami with "no Amazon Linux 2 AMI found in this region" before any SendSSHPublicKey call is issued.
