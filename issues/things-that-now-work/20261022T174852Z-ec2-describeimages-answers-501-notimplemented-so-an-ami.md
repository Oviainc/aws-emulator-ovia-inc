Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__pricing.ecprice-max-results-with-instance-ids-rejected
Issue-Key: ec2__pricing|DescribeImages|NotImplemented
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# EC2 DescribeImages answers 501 NotImplemented, so an AMI cannot be resolved by name and the EC2+Pricing running-cost-estimate check never reaches its DescribeInstances parameter-combination assertion

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeImages

## Description
=== Cause 1 of 1: DescribeImages is an unimplemented scaffold stub ===
AWS: Smithy (com.amazonaws.ec2#DescribeImages, ec2Query protocol): DescribeImages returns Images[] for the owners/filters supplied, each with ImageId, Name, State and CreationDate; the standard way a portable test or bootstrap script picks an AMI is DescribeImages(Owners=['amazon'], Filters=[name=<pattern>, state=available]) and then the newest CreationDate. AWS answers HTTP 200 with the matching image set (possibly empty), never a 501.
Emulator: DescribeImages answers HTTP 501 with error code NotImplemented and the message 'operation DescribeImages is scaffolded but not implemented in this emulator'.
Why: no client can resolve an AMI id by name against the emulator, and AMI ids are per-Region and per-publication so they cannot be hard-coded. Every workflow that launches an instance (RunInstances needs an ImageId) is therefore blocked before it starts — here it blocks a cross-service running-cost-estimate check whose real target, the EC2 DescribeInstances rule that InstanceIds may not be combined with MaxResults (AWS: InvalidParameterCombination, message 'The parameter instancesSet cannot be used with the parameter maxResults'), was never exercised and remains ungraded along with the MaxResults tag-sweep and the InstanceIds lookup.
Fix: implement the DescribeImages handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_images.rs) over seeded AMI state, honouring Owners and the name/state filters.
Traces: ec2__pricing.ecprice-max-results-with-instance-ids-rejected (suite report: 'ClientError: An error occurred (NotImplemented) when calling the DescribeImages operation'). The suite did NOT mark this failure ALREADY FILED or OWNED. Related but non-covering open issues: 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md mentions an 'EC2 DescribeImages stub' among four FIS-service divergences and is filed against fis, not ec2, and carries no ec2 replay key; 20261022T002237Z-ec2-createvpc-stub.md covers CreateVpc, which blocks the sibling check ec2__pricing.ecprice-running-filter-excludes-stopped-instance-from-estimate (that one IS marked ALREADY FILED and is not re-filed here).

## Evidence
Check ec2__pricing.ecprice-max-results-with-instance-ids-rejected, [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeImages operation: operation DescribeImages is scaffolded but not implemented in this emulator
The failing call is the check's AMI resolution step:
  ec2.describe_images(Owners=['amazon'], Filters=[{'Name':'name','Values':['al2023-ami-minimal-*-x86_64']},{'Name':'state','Values':['available']}])
Expected (AWS): HTTP 200 with a non-empty Images list whose newest CreationDate yields an ImageId usable in RunInstances. Actual: HTTP 501, code NotImplemented.
Consequently un-exercised in this run (still ungraded, not separate defects): RunInstances/DescribeInstances by InstanceIds, DescribeInstances(Filters=tag+state, MaxResults=5), and the terminal assertion that DescribeInstances(InstanceIds=[...], MaxResults=5) is refused with code InvalidParameterCombination.

## Steps to Reproduce
1. Boot the emulator.
2. aws ec2 describe-images --owners amazon --filters Name=name,Values='al2023-ami-minimal-*-x86_64' Name=state,Values=available
3. Observe HTTP 501 / NotImplemented instead of an Images list. Any subsequent RunInstances is impossible because no ImageId can be resolved.
Or replay ec2__pricing.ecprice-max-results-with-instance-ids-rejected.
