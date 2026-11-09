Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__datasync.deploy-cfn-datasync-location-object-storage-round-trip-and-delete cloudformation__datasync.deploy-cfn-datasync-location-efs-round-trip-and-delete cloudformation__datasync.deploy-cfn-datasync-s3-locations-and-task-round-trip-and-delete cloudformation__datasync.deploy-cfn-datasync-task-same-source-and-destination-location-rejected
Issue-Key: cloudformation__datasync|CreateStack|ValidationError
Issue-Key: cloudformation__datasync.deploy-cfn-datasync-location-efs-round-trip-and-delete|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__datasync.deploy-cfn-datasync-s3-locations-and-task-round-trip-and-delete|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__datasync.deploy-cfn-datasync-task-same-source-and-destination-location-rejected|run#13|AssertionError|assert good_task_arns == [good_outputs["TaskArn"]], (
Issue-Commit: 00353ccf65811f65ad7f73dbcefa75ea121211d3

# cloudformation__datasync: 3 net-new causes — AWS::DataSync::LocationObjectStorage is an unrecognized resource type, stack Outputs built from Fn::GetAtt/Fn::Sub carry no OutputValue, and a CREATE_COMPLETE AWS::DataSync::Task is never registered in DataSync

> _Note: this issue was identified against the code base at commit `00353ccf65811f65ad7f73dbcefa75ea121211d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three independent CloudFormation defects surface when DataSync resources are deployed from a template. Each must be closed before this issue is submitted.

Cause 1 of 3: CreateStack refuses AWS::DataSync::LocationObjectStorage as an unrecognized resource type.
AWS: CloudFormation registers the resource type AWS::DataSync::LocationObjectStorage (TemplateReference aws-resource-datasync-locationobjectstorage). A CreateStack whose template declares it is accepted — Smithy: CreateStack -> CreateStackOutput.StackId — and the stack proceeds to CREATE_COMPLETE, the resource's Ref being the DataSync location ARN. A template-format ValidationError for an unrecognized type is reserved for types CloudFormation genuinely does not know; the fixed text is "Template format error: Unrecognized resource types: [...]" with the bracketed list being the offending types from the request, and it must not name a supported type.
Emulator: CreateStack is refused with error code ValidationError (HTTP 400) and message "Template format error: Unrecognized resource types: [AWS::DataSync::LocationObjectStorage]" — fixed text correct, but the variable type list names a type AWS supports.
Why: no object-storage DataSync location can ever be provisioned from a template; the entire stack is rejected, so every other resource in it is lost too and infrastructure-as-code deployments of DataSync fail at the first call.
Fix: the CloudFormation resource-type registry and the provisioning path for the AWS::DataSync::* types (template validation in the CreateStack handler, plus the resource provider that must call DataSync CreateLocationObjectStorage and record the physical id).
Traces: cloudformation__datasync.deploy-cfn-datasync-location-object-storage-round-trip-and-delete.

Cause 2 of 3: DescribeStacks returns a CREATE_COMPLETE stack's Outputs entries without OutputValue when the output's Value uses an intrinsic other than Ref.
AWS: Smithy: Stack.Outputs -> list of Output{OutputKey, OutputValue, Description, ExportName}. For a stack in CREATE_COMPLETE every declared output has been resolved, so each Output carries both OutputKey and OutputValue; the value is the resolved result of the template's Value expression — including Fn::GetAtt (e.g. a file system's Arn, a DataSync location's LocationUri, a task's Status) and Fn::Sub (e.g. an ARN assembled from AWS::Partition / AWS::Region / AWS::AccountId and a resource reference).
Emulator: DescribeStacks returns Outputs rows with OutputKey present but OutputValue absent for every output whose Value is an Fn::GetAtt or Fn::Sub expression; a client reading {o["OutputKey"]: o["OutputValue"]} raises KeyError: 'OutputValue'. Outputs whose Value is a plain Ref do resolve (the same suite's task check reads Ref outputs successfully), which isolates the gap to the non-Ref intrinsics. Hypothesis: intrinsic resolution for Outputs is only implemented for Ref, and an unresolved expression is emitted as an Output row with no value instead of resolving or failing the stack.
Why: a client cannot read any stack output derived from GetAtt/Sub — the documented way to hand a resource's ARN, URI or status to the caller — so every downstream step keyed off an output breaks even though the stack reports CREATE_COMPLETE. The response is also not shape-valid for a resolved stack.
Fix: Outputs resolution in the CloudFormation stack-provisioning state (Fn::GetAtt attribute support per resource type, Fn::Sub substitution including pseudo-parameters) and the DescribeStacks serializer, which must never emit an Output lacking OutputValue. Similar open issues exist under other service headings (20261109T113512Z-cloudformation-ec2-network-firewall-describestacks-returns-a-create.md, 20261109T115509Z-cloudformation-ec2-memorydb-describestacks-returns-a-create-complete.md, 20261109T132032Z-cloudformation-efs-3-net-new-causes-aws.md); none carries these checks.
Traces: cloudformation__datasync.deploy-cfn-datasync-location-efs-round-trip-and-delete, cloudformation__datasync.deploy-cfn-datasync-s3-locations-and-task-round-trip-and-delete.

Cause 3 of 3: an AWS::DataSync::Task in a CREATE_COMPLETE stack does not exist in DataSync.
AWS: CloudFormation provisions AWS::DataSync::Task by creating a real DataSync task (TemplateReference aws-resource-datasync-task); the resource's Ref is the task ARN, and DataSync's own ListTasks then lists exactly that task with Name equal to the template's Name property and TaskArn equal to the Ref, while DescribeTask returns it.
Emulator: after the control stack reaches CREATE_COMPLETE and its TaskArn output is read, DataSync ListTasks holds no entry whose Name is the template's task name — "expected ListTasks to hold exactly the control task Ref under its name, got 0 match(es) []". Hypothesis: the CFN resource provider synthesizes a physical id for the task without calling DataSync CreateTask, so the task is invisible to the DataSync control plane (alternatively ListTasks omits Name for CFN-created tasks).
Why: a client that deploys a DataSync task by template and then drives it through the DataSync API (ListTasks/DescribeTask/StartTaskExecution) finds nothing, so the stack reports success for a resource that does not exist; the negative half of the same scenario (a self-referential task that must roll the stack back) also cannot be observed.
Fix: the CloudFormation resource provider for AWS::DataSync::Task — provision through DataSync's own CreateTask and store the returned ARN as the physical resource id — and the shared DataSync state it writes to.
Traces: cloudformation__datasync.deploy-cfn-datasync-task-same-source-and-destination-location-rejected.

## Evidence
Cause 1 — expected: CreateStack accepts the template and returns a StackId; stack reaches CREATE_COMPLETE with the LocationObjectStorage resource's Ref a DataSync location ARN. Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DataSync::LocationObjectStorage]

Cause 2 — expected: every row of Stack.Outputs carries OutputKey and OutputValue (outputs declared as {"Fn::GetAtt": ["Fs", "Arn"]}, {"Fn::GetAtt": ["SrcLoc", "LocationUri"]}, {"Fn::GetAtt": ["Task", "Status"]}, {"Fn::Sub": "arn:${AWS::Partition}:ec2:${AWS::Region}:${AWS::AccountId}:subnet/${Subnet}"}). Actual: KeyError: 'OutputValue' raised at `return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}` after the stack reported CREATE_COMPLETE; Ref-valued outputs in the sibling check resolve, so only the non-Ref intrinsics lack a value.

Cause 3 — expected: ListTasks (paginated) holds exactly one task whose Name is the template's task name and whose TaskArn equals the stack's TaskArn output. Actual: AssertionError: expected ListTasks to hold exactly the control task Ref under its name, got 0 match(es) []

## Steps to Reproduce
1. CreateStack with a template declaring Resources.Loc = {"Type": "AWS::DataSync::LocationObjectStorage", ...} -> observe the 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DataSync::LocationObjectStorage]" (Cause 1).
2. CreateStack with a template whose Outputs include an Fn::GetAtt value (e.g. {"Value": {"Fn::GetAtt": ["Fs", "Arn"]}}) or an Fn::Sub value, wait for CREATE_COMPLETE, then DescribeStacks(StackName=<StackId>) and read Stacks[0].Outputs -> rows have OutputKey but no OutputValue (Cause 2).
3. CreateStack with two AWS::DataSync::LocationS3 resources and an AWS::DataSync::Task whose SourceLocationArn/DestinationLocationArn Ref them, wait for CREATE_COMPLETE, read the TaskArn output, then call DataSync ListTasks -> no task is listed under the template's Name (Cause 3).
