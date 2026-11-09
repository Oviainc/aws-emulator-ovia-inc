Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dax.deploy-dax-parameter-group-round-trip-and-delete cloudformation__dax.deploy-dax-parameter-group-invalid-ttl-value-rejected-and-rolled-back cloudformation__dax.deploy-dax-subnet-group-round-trip-and-delete
Issue-Key: cloudformation__dax.deploy-dax-parameter-group-round-trip-and-delete|run#12|AssertionError|assert len(groups) == 1, f"DescribeParameterGroups lists {groups!r} for {name!r}"
Issue-Key: cloudformation__dax.deploy-dax-parameter-group-invalid-ttl-value-rejected-and-rolled-back|run#10|AssertionError|assert groups == [(good_name, _DESCRIPTION)], f"control parameter group reads {groups!r}"
Issue-Key: cloudformation__dax.deploy-dax-subnet-group-round-trip-and-delete|run#13|AssertionError|assert len(groups) == 1, f"DescribeSubnetGroups lists {groups!r} for {name!r}"
Issue-Commit: dd81faee0e8912cda86320bb65fd8f2654901268

# cloudformation__dax: 2 net-new causes — AWS::DAX::ParameterGroup and AWS::DAX::SubnetGroup are stub-provisioned (stack reaches CREATE_COMPLETE but nothing exists in DAX)

> _Note: this issue was identified against the code base at commit `dd81faee0e8912cda86320bb65fd8f2654901268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
CloudFormation accepts, provisions and settles CREATE_COMPLETE for both modelled DAX resource types, and hands back a correct Ref, but neither resource is ever created in DAX: the service's own read APIs report an empty account. Two causes, one per resource type's handler.

Cause 1 of 2: AWS::DAX::ParameterGroup is stub-provisioned.
AWS: the AWS::DAX::ParameterGroup resource handler calls DAX CreateParameterGroup (Smithy: CreateParameterGroup input ParameterGroupName required, Description optional; output ParameterGroup) and then applies the template's ParameterNameValues via UpdateParameterGroup, since CreateParameterGroup's input models no parameter values. After CREATE_COMPLETE the group is a real DAX resource: DescribeParameterGroups (Smithy: output ParameterGroups, paginated via NextToken) lists exactly one ParameterGroup whose ParameterGroupName equals the template's ParameterGroupName (which is also the resource's Ref/PhysicalResourceId) and whose Description equals the template's Description, and DescribeParameters(ParameterGroupName=<that name>) reports each name from ParameterNameValues with ParameterValue equal to the declared value and Source "user" (as opposed to the catalog default's "system"). On stack delete the handler calls DeleteParameterGroup, after which DescribeParameterGroups(ParameterGroupNames=[<name>]) raises ParameterGroupNotFoundFault (Smithy: httpError 404, awsQueryError code ParameterGroupNotFound).
Emulator: the stack reaches CREATE_COMPLETE and Outputs carry the right Ref (the declared ParameterGroupName), but DAX DescribeParameterGroups, paged in full, lists NO group with that name — the replay reads `DescribeParameterGroups lists [] for 'sim2real-daxpg-<suffix>'` and, in the negative-control check, `control parameter group reads []`. The resource handler therefore never reaches DAX state at all; the ParameterNameValues/Source="user" and the post-delete ParameterGroupNotFoundFault assertions are unreachable behind it. (Note: DAX UpdateParameterGroup is an unimplemented stub, open issue 20261105T204401Z-dax-updateparametergroup-stub.md; that stub is a prerequisite for the ParameterNameValues half of this handler but does not explain the group's total absence, since CreateParameterGroup alone would have made it listable. Open issue 20261105T235326Z-dax-parameter-group-reads-diverge-not-found-message.md covers the direct-DAX read divergences and likewise does not cover CFN provisioning.)
Why: any template or CDK app that declares a DAX parameter group deploys "successfully" and then every consumer that resolves the group through DAX — DescribeParameterGroups, DescribeParameters, a cluster created with ParameterGroupName — behaves as if the group does not exist. The deploy gives no signal, so the failure surfaces far away from its cause, and the rejection path cannot be exercised either (the negative-control check can never compare the rolled-back stack's handler reason because its positive control already diverges).
Fix: the CloudFormation AWS::DAX::ParameterGroup resource handler (create/update/delete) — it must drive the real DAX CreateParameterGroup / UpdateParameterGroup / DeleteParameterGroup against DAX state instead of recording a synthetic physical id, so DAX's own reads see the group and the delete removes it.
Traces: cloudformation__dax.deploy-dax-parameter-group-round-trip-and-delete (assert at check line 189/472: `DescribeParameterGroups lists [] for 'sim2real-daxpg-f1849294a563'`), cloudformation__dax.deploy-dax-parameter-group-invalid-ttl-value-rejected-and-rolled-back (assert at check line 221/245: `control parameter group reads []`).

Cause 2 of 2: AWS::DAX::SubnetGroup is stub-provisioned.
AWS: the AWS::DAX::SubnetGroup handler calls DAX CreateSubnetGroup (Smithy: SubnetGroupName and SubnetIds required, Description optional; output SubnetGroup) with the subnet ids the template's SubnetIds resolve to. After CREATE_COMPLETE, DescribeSubnetGroups (Smithy: output SubnetGroups, paginated) lists exactly one SubnetGroup whose SubnetGroupName equals the declared SubnetGroupName (the resource's Ref), whose Description equals the declared Description, whose VpcId is the VPC the member subnets belong to, and whose Subnets are exactly the declared subnets, each row carrying SubnetIdentifier and that subnet's SubnetAvailabilityZone. On stack delete the group is removed and DescribeSubnetGroups(SubnetGroupNames=[<name>]) raises SubnetGroupNotFoundFault (Smithy: httpError 404) with message "Subnet group <name> not found." where the fixed text is "Subnet group " / " not found." and <name> is the requested group name from the request.
Emulator: the stack reaches CREATE_COMPLETE, the VPC/subnet resources resolve and Outputs carry the right SubnetGroupRef, but DAX DescribeSubnetGroups, paged in full, lists NO group with that name — the replay reads `DescribeSubnetGroups lists [] for 'sim2real-daxsg-<suffix>'`. The VpcId/Subnets representation and the post-delete SubnetGroupNotFoundFault are unreachable behind this. (Open issue 20261029T193620Z-dax-createsubnetgroup-refuses-every-client-created-subnet-id.md covers direct DAX CreateSubnetGroup refusing client-created subnet ids; it does not cover this, because here CloudFormation never surfaces any DAX refusal at all — the stack succeeds and the group is simply absent.)
Why: a client that provisions its DAX subnet group through CloudFormation — the documented way to do it — gets a green stack and then cannot create a cluster in it or read it back; nothing in the deploy reports the gap.
Fix: the CloudFormation AWS::DAX::SubnetGroup resource handler (create/update/delete) — it must drive DAX CreateSubnetGroup/DeleteSubnetGroup with the resolved SubnetIds so DAX's own reads report the group with its VpcId and member subnets, and the delete makes it not-found.
Traces: cloudformation__dax.deploy-dax-subnet-group-round-trip-and-delete (assert at check line 207/730: `DescribeSubnetGroups lists [] for 'sim2real-daxsg-ddcc3ad47630'`).

## Evidence
Suite: simparity-behavioral-aws-v1, service heading `### cloudformation__dax` — 0/3 passed (3 failed), all three verdict `mismatch`.

Cause 1 — AWS::DAX::ParameterGroup:
  check deploy-dax-parameter-group-round-trip-and-delete
    Expected: after CREATE_COMPLETE of a stack with one AWS::DAX::ParameterGroup{ParameterGroupName:<name>, Description:"sim2real dax parameter group", ParameterNameValues:{record-ttl-millis:"70000", query-ttl-millis:"80000"}}, paging DescribeParameterGroups yields exactly one group named <name> with that Description, and DescribeParameters(ParameterGroupName=<name>) yields record-ttl-millis=("70000","user") and query-ttl-millis=("80000","user"); after DeleteStack, DescribeParameterGroups(ParameterGroupNames=[<name>]) raises ParameterGroupNotFoundFault / "Parameter Group not found."
    Actual:   StackStatus CREATE_COMPLETE and Outputs.ParamGroupRef == <name> (both correct), then
              AssertionError: DescribeParameterGroups lists [] for 'sim2real-daxpg-f1849294a563'
  check deploy-dax-parameter-group-invalid-ttl-value-rejected-and-rolled-back
    Expected: the positive-control stack (record-ttl-millis "1000") reaches CREATE_COMPLETE and its group reads back as [(<name>, "sim2real negative-control parameter group")]
    Actual:   AssertionError: control parameter group reads []
              (the negative control — ROLLBACK_COMPLETE with handler reason ('value:-5 is outside of range:0- (Service: Dax, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest') — is never reached and is NOT claimed here)

Cause 2 — AWS::DAX::SubnetGroup:
  check deploy-dax-subnet-group-round-trip-and-delete
    Expected: after CREATE_COMPLETE of a stack with AWS::EC2::VPC + two AWS::EC2::Subnet + AWS::DAX::SubnetGroup{SubnetGroupName:<name>, Description:"sim2real dax subnet group", SubnetIds:[Ref SubnetA, Ref SubnetB]}, paging DescribeSubnetGroups yields exactly one group named <name> with that Description, VpcId == the stack's VPC id, and Subnets == {(subnetA, azA), (subnetB, azB)}; after DeleteStack, DescribeSubnetGroups(SubnetGroupNames=[<name>]) raises SubnetGroupNotFoundFault / "Subnet group <name> not found."
    Actual:   StackStatus CREATE_COMPLETE and Outputs.SubnetGroupRef == <name> (both correct), then
              AssertionError: DescribeSubnetGroups lists [] for 'sim2real-daxsg-ddcc3ad47630'

In all three cases CloudFormation reports success and a correct Ref while DAX state is empty, which is why these cluster as one family of stub-provisioned DAX resource handlers (one handler per resource type). The run-specific group names, stack ids, request ids and timestamps above are illustrative values, not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and point botocore at it with cloudformation, dax and ec2 clients.
2. Cause 1 (ParameterGroup):
   a. CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"ParamGroup":{"Type":"AWS::DAX::ParameterGroup","Properties":{"ParameterGroupName":"<pg>","Description":"sim2real dax parameter group","ParameterNameValues":{"record-ttl-millis":"70000","query-ttl-millis":"80000"}}}},"Outputs":{"ParamGroupRef":{"Value":{"Ref":"ParamGroup"}}}}.
   b. Poll DescribeStacks(StackName=<StackId>) until terminal — observe CREATE_COMPLETE and Outputs.ParamGroupRef == "<pg>".
   c. Page dax.DescribeParameterGroups with no filter and look for ParameterGroupName == "<pg>" — the list is empty (expected: one group with Description "sim2real dax parameter group").
   d. dax.DescribeParameters(ParameterGroupName="<pg>") — expected record-ttl-millis/query-ttl-millis with the declared values and Source "user".
   e. DeleteStack, wait DELETE_COMPLETE, then dax.DescribeParameterGroups(ParameterGroupNames=["<pg>"]) — expected ParameterGroupNotFoundFault / "Parameter Group not found."
3. Cause 2 (SubnetGroup):
   a. CreateStack with an AWS::EC2::VPC (CidrBlock 10.211.0.0/16), two AWS::EC2::Subnet in different AZs, and AWS::DAX::SubnetGroup{SubnetGroupName:"<sg>",Description:"sim2real dax subnet group",SubnetIds:[{"Ref":"SubnetA"},{"Ref":"SubnetB"}]}, with Outputs for the group Ref, both subnet ids, both AZs and the VpcId.
   b. Wait for CREATE_COMPLETE — observe Outputs.SubnetGroupRef == "<sg>".
   c. Page dax.DescribeSubnetGroups with no filter and look for SubnetGroupName == "<sg>" — the list is empty (expected: one group with that Description, the stack's VpcId, and Subnets == the two declared subnet ids with their AZs).
   d. DeleteStack, wait DELETE_COMPLETE, then dax.DescribeSubnetGroups(SubnetGroupNames=["<sg>"]) — expected SubnetGroupNotFoundFault / "Subnet group <sg> not found."
4. Replay to confirm: the three checks in trace_ids.
