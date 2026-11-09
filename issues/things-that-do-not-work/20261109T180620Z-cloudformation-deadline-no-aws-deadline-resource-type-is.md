Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__deadline.deploy-deadline-farm-queue-storageprofile-round-trip-and-delete cloudformation__deadline.deploy-deadline-fleet-queuefleetassociation-worker-round-trip-and-delete cloudformation__deadline.deploy-deadline-limit-maxcount-zero-rejected-and-rolled-back
Issue-Key: cloudformation__deadline|CreateStack|ValidationError
Issue-Commit: dd81faee0e8912cda86320bb65fd8f2654901268

# cloudformation__deadline: no AWS::Deadline::* resource type is registered — CreateStack refuses every Deadline Cloud template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `dd81faee0e8912cda86320bb65fd8f2654901268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
The suite lists these checks under the composite heading `### cloudformation__deadline`; the diverging call is CloudFormation's CreateStack, so the issue is filed against `cloudformation`. Both `cloudformation` and `deadline` are stood up (nothing is pending in the bootstrap status).

Cause 1 of 1: CloudFormation's template resource-type registry contains no AWS::Deadline::* type, so CreateStack refuses the template outright instead of provisioning the resources.

AWS: Real CloudFormation supports the AWS Deadline Cloud resource types and provisions them through the Deadline control plane — AWS::Deadline::Farm, AWS::Deadline::Queue, AWS::Deadline::StorageProfile, AWS::Deadline::Fleet, AWS::Deadline::QueueFleetAssociation, AWS::Deadline::Worker and AWS::Deadline::Limit are all accepted by CreateStack, which answers 200 with a StackId and drives the stack to CREATE_COMPLETE (or to CREATE_FAILED/ROLLBACK_COMPLETE on a resource-level refusal). Each handler maps onto a modelled Deadline operation that this emulator already serves the API for: Smithy (deadline) CreateFarm POST /2023-10-12/farms -> farmId `^farm-[0-9a-f]{32}$`; CreateLimit POST /2023-10-12/farms/{farmId}/limits with `maxCount` @range(min:-1, max:2147483647) and ValidationException (400) outside it; CreateWorker POST /2023-10-12/farms/{farmId}/fleets/{fleetId}/workers -> workerId `^worker-[0-9a-f]{32}$`. CloudFormation's "Template format error: Unrecognized resource types: [...]" ValidationError is reserved for types CloudFormation genuinely does not know; AWS does know these. Recorded behavior for the templates in these checks is CREATE_COMPLETE with Ref/Fn::GetAtt resolving (farm Ref = the farm ARN, GetAtt FarmId = the farm id, storage-profile Ref = `<FarmId>|<StorageProfileId>`, fleet/worker Ref = their ARNs), the resources readable through Deadline (ListFarms/GetFarm, ListQueues/GetQueue, GetStorageProfile, ListFleets/GetFleet, ListWorkers, GetLimit), template Tags propagated and readable via ListTagsForResource, a child-dropping UpdateStack deleting just those children, DeleteStack removing the rest, and — for a Limit whose MaxCount is 0 — a resource-level CREATE_FAILED whose reason is the Deadline refusal wrapped by the resource handler, followed by ROLLBACK_COMPLETE.

Emulator: CreateStack is refused before any resource is touched. HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [<types>]`. The fixed text is `Template format error: Unrecognized resource types: ` and the bracketed list is the set of types taken from the submitted template, in template order. Observed per check: `[AWS::Deadline::Farm, AWS::Deadline::Queue, AWS::Deadline::StorageProfile]`, `[AWS::Deadline::Farm, AWS::Deadline::Queue, AWS::Deadline::Fleet, AWS::Deadline::QueueFleetAssociation, AWS::Deadline::Worker]`, and `[AWS::Deadline::Farm, AWS::Deadline::Limit]` (the AWS::EC2::*, AWS::IAM::Role types in the same templates are accepted, so the gap is specific to the Deadline namespace). No stack is created, so DescribeStacks, DescribeStackEvents and the Deadline read-backs are never reached.

Why: a client cannot deploy any Deadline Cloud infrastructure as code. Every CDK app, SAM template or raw CloudFormation stack that declares a farm, queue, storage profile, fleet, queue-fleet association, worker or limit fails at the very first API call with a template-format error, so neither the happy-path deploy nor the resource-level rejection/rollback path can be exercised at all. Because the refusal is a synchronous ValidationError on CreateStack rather than a stack-level failure, a client that handles CREATE_FAILED/ROLLBACK_COMPLETE correctly still cannot proceed, and a client that relies on the Deadline control plane being reachable through CloudFormation sees it as entirely absent.

Fix: CloudFormation's template resource-type registry and its resource handlers. Register the AWS::Deadline::* types listed above and implement their create/update/delete handlers against the Deadline service's existing operations, including the primary-identifier/Ref and Fn::GetAtt mappings (Arn, FarmId, QueueId, StorageProfileId, FleetId, Status, WorkerId, LimitId), Tags propagation, and the wrapping of a Deadline ValidationException into the resource's CREATE_FAILED reason so a bad MaxCount rolls the stack back instead of aborting the request.

Traces: the three failing checks listed under `### cloudformation__deadline` (see trace_ids). The two `cloudformation__deadline__ec2` checks, which declare AWS::Deadline::LicenseEndpoint and AWS::Deadline::MeteredProduct, fail earlier on EC2 DescribeVpcEndpointServices and are already OWNED by 20261109T114319Z-ec2-iam-imagebuilder-describevpcendpointservices-refuses-com-amazonaws-region.md; they are expected to surface the same registry gap once that is fixed, so LicenseEndpoint and MeteredProduct should be registered too. Several open issues report the same class of registry gap for other namespaces (e.g. 20261109T143515Z-cloudformation-logs-cloudformation-rejects-six-modelled-aws-logs-resource-types.md, 20261109T145937Z-cloudformation-entityresolution-no-aws-entityresolution-resource-type-is.md); none of them covers AWS::Deadline::*.

## Evidence
Check cloudformation__deadline.deploy-deadline-farm-queue-storageprofile-round-trip-and-delete
  Template Resources: Farm (AWS::Deadline::Farm, DisplayName+Tags), Queue (AWS::Deadline::Queue, FarmId from GetAtt), StorageProfile (AWS::Deadline::StorageProfile, OsFamily LINUX).
  Expected: CreateStack -> 200 + StackId; stack reaches CREATE_COMPLETE; Outputs FarmId/FarmArn/QueueId/QueueArn/StorageProfileId/StorageProfileRef resolve; ListFarms/GetFarm, ListQueues/GetQueue, GetStorageProfile read the declared DisplayName/OsFamily; ListTagsForResource returns {sim2real-check: <run value>}; child-dropping UpdateStack -> UPDATE_COMPLETE with the queue and storage profile gone (ResourceNotFoundException) and the farm alive; DeleteStack -> DELETE_COMPLETE and GetFarm then ResourceNotFoundException.
  Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Deadline::Farm, AWS::Deadline::Queue, AWS::Deadline::StorageProfile]

Check cloudformation__deadline.deploy-deadline-fleet-queuefleetassociation-worker-round-trip-and-delete
  Template Resources: Farm, Queue, FleetRole (AWS::IAM::Role), Fleet (AWS::Deadline::Fleet, customerManaged NO_SCALING, MaxWorkerCount 1, MinWorkerCount 0), QFA (AWS::Deadline::QueueFleetAssociation), Worker (AWS::Deadline::Worker, HostProperties.HostName).
  Expected: CreateStack(Capabilities=[CAPABILITY_IAM]) accepted; CREATE_COMPLETE; GetFleet reports status ACTIVE, maxWorkerCount 1, minWorkerCount 0, the declared roleArn and workerCapabilities; ListWorkers lists the GetAtt WorkerId; Ref of the fleet/worker equals their ARNs.
  Actual: ClientError (ValidationError) on CreateStack: Template format error: Unrecognized resource types: [AWS::Deadline::Farm, AWS::Deadline::Queue, AWS::Deadline::Fleet, AWS::Deadline::QueueFleetAssociation, AWS::Deadline::Worker]

Check cloudformation__deadline.deploy-deadline-limit-maxcount-zero-rejected-and-rolled-back
  Template Resources: Farm, Limit (AWS::Deadline::Limit, AmountRequirementName "amount.sim2real_slots", MaxCount 0).
  Expected: CreateStack accepted; the stack reaches ROLLBACK_COMPLETE; exactly one Farm CREATE_COMPLETE event whose PhysicalResourceId is a farm ARN; the Limit CREATE_FAILED reason is the handler-wrapped Deadline refusal with HandlerErrorCode InvalidRequest and message "The maxCount <value> is not -1 or an integer between 1 and 2147483647 (Service: Deadline, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" — fixed text apart from the submitted maxCount value and the per-run request id/attempt count, consistent with Smithy CreateLimit.maxCount @range(min:-1, max:2147483647) / ValidationException 400; the rollback deletes the farm (GetFarm then ResourceNotFoundException); and the identical stack with MaxCount 5 reaches CREATE_COMPLETE with GetLimit reporting maxCount 5.
  Actual: ClientError (ValidationError) on CreateStack: Template format error: Unrecognized resource types: [AWS::Deadline::Farm, AWS::Deadline::Limit]

In all three cases the non-Deadline types in the same templates (AWS::EC2::*, AWS::IAM::Role) are not listed as unrecognized, and no stack row exists afterwards.

## Steps to Reproduce
1. Point a botocore/boto3 client at the emulator's cloudformation and deadline endpoints.
2. CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Farm":{"Type":"AWS::Deadline::Farm","Properties":{"DisplayName":"<name>"}}},"Outputs":{"FarmId":{"Value":{"Fn::GetAtt":["Farm","FarmId"]}}}}.
3. Observe HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Deadline::Farm]" instead of a 200 with a StackId.
4. Repeat with AWS::Deadline::Queue, ::StorageProfile, ::Fleet, ::QueueFleetAssociation, ::Worker and ::Limit (alone or alongside AWS::EC2::VPC / AWS::IAM::Role): only the Deadline types appear in the bracketed list.
5. Repeat with a Farm + Limit template whose MaxCount is 0: AWS accepts the stack and rolls it back with the Limit CREATE_FAILED carrying Deadline's maxCount refusal; the emulator never creates the stack.
6. Replay the three checks named in trace_ids to confirm.
