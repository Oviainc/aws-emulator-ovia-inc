Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__synthetics.cfn-synthetics-canary-properties-and-getatt-read-back-then-resources-removed-on-delete cloudformation__synthetics.cfn-synthetics-group-duplicate-name-rejected-leaving-the-original cloudformation__synthetics.cfn-synthetics-group-tags-and-getatt-read-back-then-not-found-after-delete
Issue-Key: cloudformation__synthetics.cfn-synthetics-canary-properties-and-getatt-read-back-then-resources-removed-on-delete|stack_outputs#12|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__synthetics|GetGroup|ResourceNotFoundException
Issue-Key: cloudformation__synthetics|ListGroups|NotImplemented
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# cloudformation+synthetics: 3 net-new root causes — DescribeStacks Outputs rows carry no OutputValue, an AWS::Synthetics::Group stack reaches CREATE_COMPLETE with no group at Synthetics, and ListGroups is a 501 stub

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three net-new root causes on the CloudFormation⇄Synthetics deploy path, grounded in all 3 failing checks the suite lists under `### cloudformation__synthetics` (0/3 passed). None of these three checks was marked ALREADY FILED or OWNED in this replay. The service token is the suite heading for these cross-service checks; causes 1-2 live in the CloudFormation stack engine, cause 3 in the synthetics ListGroups handler.

=== Cause 1 of 3: DescribeStacks returns Outputs rows with OutputKey but no OutputValue ===
AWS: Smithy com.amazonaws.cloudformation#Output declares members OutputKey, OutputValue, Description, ExportName. For a stack that declares `Outputs` and has reached CREATE_COMPLETE, DescribeStacks returns one row per declared output carrying BOTH the key and the RESOLVED value: a `Ref` to a resource resolves to that resource's physical id (for AWS::Synthetics::Canary, the canary name), and `Fn::GetAtt` resolves to the named attribute (Canary.Id, Canary.State, Role.Arn). An output whose value cannot be resolved does not get an empty/absent value member — the stack create fails instead.
Emulator: the stack reaches CREATE_COMPLETE (the check's status assertion passes), but DescribeStacks(StackName=<stack id>) returns Outputs rows that have no `OutputValue` member at all: reading `o["OutputValue"]` over `Stacks[0].Outputs` raises `KeyError: 'OutputValue'`. Observed for a template whose outputs are `{"Ref": "Canary"}`, `{"Fn::GetAtt": ["Canary","Id"]}`, `{"Fn::GetAtt": ["Canary","State"]}`, `{"Fn::GetAtt": ["Role","Arn"]}`.
Why: handing a deployed resource's name/id/attribute to downstream code through stack outputs is the normal CloudFormation contract. A client gets a key with no value: a dict access raises KeyError, a typed SDK sees an absent optional field, and nothing distinguishes "unresolvable output" from "empty string". The stack also reports success while its advertised interface is unusable.
Fix: the CloudFormation stack engine's output resolution and the DescribeStacks serializer — each declared Output must be resolved (Ref / Fn::GetAtt, including attributes of AWS::Synthetics::Canary) and emitted as `Output.OutputValue` alongside `OutputKey`.
Traces: cloudformation__synthetics.cfn-synthetics-canary-properties-and-getatt-read-back-then-resources-removed-on-delete.

=== Cause 2 of 3: a stack containing AWS::Synthetics::Group reaches CREATE_COMPLETE, but Synthetics holds no such group ===
AWS: a stack in CREATE_COMPLETE has really provisioned its resources, so an independent read of the resource through its own service succeeds. Smithy synthetics#GetGroup (GET /group/{GroupIdentifier}, 200) returns `Group{Id, Name, Arn, CreatedTime, Tags}` for a group that exists; its modeled errors are ResourceNotFoundException (404), ValidationException (400), ConflictException (409), InternalServerException (500), and the 404 means the group does not exist. A CloudFormation-created AWS::Synthetics::Group is therefore readable at GetGroup(GroupIdentifier=<declared Name>) immediately after CREATE_COMPLETE, and `Ref`/`GetAtt Id` agree with that read.
Emulator: CreateStack of a template whose only resource is `AWS::Synthetics::Group` with `Properties.Name = <name>` reaches CREATE_COMPLETE and even resolves its outputs, but the follow-up `synthetics.GetGroup(GroupIdentifier=<name>)` is refused with HTTP 404 `ResourceNotFoundException`, message `Group does not exist: <name>` (fixed text `Group does not exist: `, variable part = the GroupIdentifier taken from the request — one run recorded `sim2real-behav-dupgrp-698950cc762e`, which is that run's generated name, not part of the requirement). The CloudFormation resource provider for AWS::Synthetics::Group evidently creates nothing in the synthetics group store.
Why: deploy-then-use is broken for this resource type. Any client that creates a group by template and then drives it through the Synthetics API (GetGroup, AssociateResource, tag reads) sees a resource CloudFormation claims exists but the owning service denies, and the name-conflict behaviour of a second stack under the same name cannot arise at all because no name is ever taken.
Fix: the CloudFormation resource provider for AWS::Synthetics::Group — create/update/delete must go through the same synthetics group state that GetGroup/ListGroups/DeleteGroup read, so a CREATE_COMPLETE stack leaves a group readable there and a DELETE_COMPLETE stack leaves it not-found.
Traces: cloudformation__synthetics.cfn-synthetics-group-duplicate-name-rejected-leaving-the-original.

=== Cause 3 of 3: synthetics ListGroups is an unimplemented scaffold stub and answers 501 NotImplemented ===
AWS: Smithy synthetics#ListGroups — POST /groups, success 200, output `{Groups: GroupSummaryList, NextToken}`, paginated (inputToken/outputToken NextToken, pageSize MaxResults 1..20); modeled errors are ValidationException (400) and InternalServerException (500) only. There is no condition under which AWS answers 501 or an error code `NotImplemented`.
Emulator: ListGroups is refused with HTTP 501 and error code `NotImplemented`, message `operation ListGroups is scaffolded but not implemented in this emulator`. services/service-com-amazonaws-synthetics-synthetics/src/ops/list_groups.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(..)` from `ListGroups::handle`. This replay did NOT mark this cluster ALREADY FILED, and the only open stub issue for this service is 20261029T184613Z-synthetics-updatecanary-stub.md, which names UpdateCanary — nothing covers ListGroups.
Why: enumerating groups is how a client discovers CloudFormation-created groups and verifies a group was deleted with its stack; an unmodeled 501/NotImplemented is not retried or handled by any SDK error path and aborts the whole flow.
Fix: implement `ListGroups::handle` in services/service-com-amazonaws-synthetics-synthetics/src/ops/list_groups.rs over the shared group state (returning group summaries, with NextToken paging bounded by MaxResults) and drop the SCAFFOLDED marker.
Traces: cloudformation__synthetics.cfn-synthetics-group-tags-and-getatt-read-back-then-not-found-after-delete.

## Evidence
Suite simparity-behavioral-aws-v1: 15/27 passed. Per-service: cloudformation__synthetics 0/3 (0%), 3 failed.

Cause 1 — [error] cloudformation__synthetics.cfn-synthetics-canary-properties-and-getatt-read-back-then-resources-removed-on-delete:
  stack reached CREATE_COMPLETE (assert at the status check passed), then
  `outputs = stack_outputs(cfn, stack_id)` →
  `return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` →
  `KeyError: 'OutputValue'`.
  Expected: Outputs == {CanaryName: <canary name from Ref>, CanaryId: <GetAtt Canary.Id>, CanaryState: "READY", RoleArn: <GetAtt Role.Arn>}.

Cause 2 — [error] cloudformation__synthetics.cfn-synthetics-group-duplicate-name-rejected-leaving-the-original:
  stack A CREATE_COMPLETE and its Outputs (Ref Group / GetAtt Group.Id) readable, then
  `before = syn.get_group(GroupIdentifier=group_name)["Group"]` →
  `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetGroup operation: Group does not exist: sim2real-behav-dupgrp-698950cc762e`.
  Expected: HTTP 200 with Group{Name=<declared Name>, Id, Arn, CreatedTime, Tags=[{sim2real-group: <name>}]}, Id equal to the stack's `GetAtt Group.Id` output.

Cause 3 — [error] cloudformation__synthetics.cfn-synthetics-group-tags-and-getatt-read-back-then-not-found-after-delete:
  `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListGroups operation: operation ListGroups is scaffolded but not implemented in this emulator` (HTTP 501).
  Expected: HTTP 200 `{"Groups": [...], "NextToken": ...}`.
  Source: services/service-com-amazonaws-synthetics-synthetics/src/ops/list_groups.rs lines 3-12 — `//! @op ListGroups` / `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`.

Model citations: com.amazonaws.cloudformation#Output members {OutputKey, OutputValue, Description, ExportName}; com.amazonaws.synthetics#GetGroup GET /group/{GroupIdentifier} → 200 Group, errors ResourceNotFoundException 404 / ValidationException 400 / ConflictException 409 / InternalServerException 500; com.amazonaws.synthetics#ListGroups POST /groups → 200 {Groups, NextToken}, errors ValidationException 400 / InternalServerException 500.

## Steps to Reproduce
Cause 1:
1. cloudformation.CreateStack with resources Bucket (AWS::S3::Bucket), Role (AWS::IAM::Role) and Canary (AWS::Synthetics::Canary with Name, Code{Handler,Script}, ArtifactS3Location={"Fn::Sub":"s3://${Bucket}"}, ExecutionRoleArn={"Fn::GetAtt":["Role","Arn"]}, Schedule{Expression:"rate(0 minute)"}, RuntimeVersion syn-nodejs-puppeteer-9.0, StartCanaryAfterCreation=false, ProvisionedResourceCleanup=AUTOMATIC) and Outputs {CanaryName: Ref Canary, CanaryId: GetAtt Canary.Id, CanaryState: GetAtt Canary.State, RoleArn: GetAtt Role.Arn}, Capabilities=[CAPABILITY_NAMED_IAM].
2. Poll DescribeStacks(StackName=<StackId>) until CREATE_COMPLETE.
3. Read Stacks[0].Outputs: every row has OutputKey but no OutputValue. Expected: each row carries the resolved OutputValue.

Cause 2:
1. cloudformation.CreateStack with a single resource Group (AWS::Synthetics::Group, Properties {Name: <group-name>, Tags:[{Key:sim2real-group,Value:<group-name>}]}) and Outputs {GroupName: Ref Group, GroupId: GetAtt Group.Id}.
2. Poll DescribeStacks until CREATE_COMPLETE.
3. synthetics.GetGroup(GroupIdentifier=<group-name>) → 404 ResourceNotFoundException `Group does not exist: <group-name>`. Expected 200 with the group CloudFormation just created (Id matching the GetAtt output).
4. (Blocked follow-on) a second stack with the same template must end ROLLBACK_COMPLETE with a VALIDATION_ERROR row (Group, NAME_CONFLICT_VALIDATION, FAILED, FAIL, "Resource of type 'AWS::Synthetics::Group' with identifier '<name>' already exists.", /Resources/Group) and leave stack A's group byte-identical.

Cause 3:
1. synthetics.ListGroups() → HTTP 501 `NotImplemented` ("operation ListGroups is scaffolded but not implemented in this emulator"). Expected HTTP 200 with a Groups list (empty list when no groups exist).
