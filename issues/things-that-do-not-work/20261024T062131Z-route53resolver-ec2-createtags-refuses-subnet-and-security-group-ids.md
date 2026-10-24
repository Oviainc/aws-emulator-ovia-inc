Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53resolver.e2e-resolver-rule-forwarding-lifecycle-flow
Issue-Shape-Id: com.amazonaws.route53resolver#Route53Resolver
Issue-Key: route53resolver|CreateTags|InvalidID
Issue-Commit: df1261813c70037074c7525ba4fddd96df97f3b7

# EC2 CreateTags refuses subnet and security-group ids the emulator itself minted (InvalidID), so only instances and VPCs can be tagged

> _Note: this issue was identified against the code base at commit `df1261813c70037074c7525ba4fddd96df97f3b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
One root cause. It was found through a route53resolver behavioral check (hence this issue's service token, taken from the check id), but the defect is in the EC2 crate's CreateTags handler.

Cause 1 of 1: CreateTags resolves only instances and VPCs, and answers InvalidID for every other resource id — including subnet and security-group ids CreateSubnet/CreateSecurityGroup in this same emulator just returned.
AWS: CreateTags "adds or overwrites only the specified tags for the specified Amazon EC2 resource or resources" and accepts any taggable EC2 resource id, subnets and security groups among them (the request takes Resources, a list of resource ids, with no type restriction in the model: Smithy CreateTags.Resources, target ResourceIdList, documented "The IDs of the resources, separated by spaces" / "Up to 1000 resource IDs"). Tagging an id the account holds returns HTTP 200 with an empty body. The InvalidID error AWS returns is for a MALFORMED id, and InvalidSubnetID.NotFound-style errors are for ids that do not exist — neither applies to a well-formed subnet id the service itself issued.
Emulator: the handler walks store.instances then store.vpcs and, for anything else, returns Scaffolded::invalid_id(resource_id) — HTTP 400, error code InvalidID, message example "The ID 'subnet-00000000000000020' is not valid" — with the code comment "Instances and VPCs are the only taggable resources this crate stores". The failing check creates a VPC, then subnets in it, then tags each subnet (and later a security group) and is refused on the subnet id its own CreateSubnet returned, aborting the whole resolver-rule lifecycle before any Resolver call is made.
Why: a well-formed, existing resource id is reported as syntactically invalid, so any client or test fixture that tags its network resources — essentially every real provisioning script, and every Terraform/CloudFormation-like flow that tags what it creates — fails against the emulator with an error AWS would never return, and the error code points the caller at a non-existent id-format bug. It also blocks unrelated Resolver coverage that merely tags its scaffolding.
Fix: EC2's CreateTags handler (and the matching DeleteTags/DescribeTags paths) must resolve a resource id against every taggable resource collection the EC2 state holds — subnets and security groups at minimum, beside instances and VPCs — tag it on success, and reserve InvalidID for ids that are not well-formed, using the per-type not-found error for a well-formed id that names nothing.
Traces: route53resolver.e2e-resolver-rule-forwarding-lifecycle-flow. Code: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_tags.rs:247-293 (the instances/vpcs-only resolution loop and the Scaffolded::invalid_id fallback).

## Evidence
route53resolver.e2e-resolver-rule-forwarding-lifecycle-flow — botocore ClientError: An error occurred (InvalidID) when calling the CreateTags operation: The ID 'subnet-00000000000000020' is not valid. The id was returned moments earlier by this emulator's own ec2.create_subnet(VpcId=..., CidrBlock='10.91.0.0/24', ...) call, and the very next statement in the fixture is ec2.create_tags(Resources=[subnet_id], Tags=[...]). The same fixture goes on to tag a security group created by this emulator, which the same code path would refuse. Expected: HTTP 200 and the tags applied (AWS tags subnets and security groups). Actual: HTTP 400, code InvalidID. Source: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_tags.rs resolves only store.instances and store.vpcs and falls through to Scaffolded::invalid_id for anything else.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. ec2.create_vpc(CidrBlock='10.91.0.0/16') -> vpc-*; ec2.create_subnet(VpcId=<that vpc>, CidrBlock='10.91.0.0/24', AvailabilityZone=<any available AZ>) -> subnet-*.
3. ec2.create_tags(Resources=[<that subnet id>], Tags=[{'Key':'Name','Value':'x'}]) — the emulator answers HTTP 400 InvalidID "The ID '<subnet id>' is not valid"; AWS returns 200.
4. Same for a security group created by ec2.create_security_group in that VPC.
5. Replaying behavioral check route53resolver.e2e-resolver-rule-forwarding-lifecycle-flow reproduces it end to end: the check aborts at step 2's tagging and never reaches its Resolver assertions.
