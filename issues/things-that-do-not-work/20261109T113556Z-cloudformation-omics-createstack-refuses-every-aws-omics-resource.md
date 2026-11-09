Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__omics.deploy-reference-and-sequence-store-via-cloudformation-reaches-create-complete cloudformation__omics.deploy-run-cache-via-cloudformation-reaches-create-complete cloudformation__omics.deploy-sequence-store-duplicate-name-rejected cloudformation__omics.deploy-workflow-and-workflow-version-via-cloudformation-reaches-create-complete
Issue-Key: cloudformation__omics|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__omics: CreateStack refuses every AWS::Omics::* resource type as "Unrecognized resource types" — 4 net-new causes, no HealthOmics resource is deployable by IaC

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
The emulator's CloudFormation template-type registry knows no AWS::Omics::* resource type, so CreateStack rejects the template at validation time and no HealthOmics resource can be provisioned through CloudFormation/CDK at all. Four net-new causes, one per resource family a check deploys. They are listed separately because each needs its own resource provider (create/read/update/delete plus its own Ref and GetAtt attribute set); registering the type names alone closes none of them.

No open issue covers these. The closest, 20261107T075601Z-cloudformation-ec2-ssm-...-aws-ssm-maintenancewindowtarget-unrecognised..., is the same shape of defect for AWS::SSM::MaintenanceWindowTarget and does not cover AWS::Omics. 20261029T184954Z-omics-4-net-new-root-causes-startrun-refuses.md owns omics.deploy-run-group-via-cloudformation (AWS::Omics::RunGroup), which is therefore NOT part of this issue.

Cause 1 of 4: AWS::Omics::ReferenceStore and AWS::Omics::SequenceStore are unrecognized.
AWS: CloudFormation accepts a template declaring Type: AWS::Omics::ReferenceStore and Type: AWS::Omics::SequenceStore with a Name property, reaches CREATE_COMPLETE, and provisions real HealthOmics stores. Ref yields the store id (Smithy omics CreateSequenceStore.output.id — SequenceStoreId, @length 10-36, @pattern ^[0-9]+$; CreateReferenceStore.output.id likewise); GetAtt Arn yields output.arn (@pattern ^arn:.+$); for the sequence store GetAtt S3Uri and S3AccessPointArn equal CreateSequenceStore.output.s3Access.s3Uri / .s3AccessPointArn, and GetS3AccessPolicy over that access point ARN answers storeId = the store id and storeType = SEQUENCE_STORE. The stores are then visible through ListReferenceStores/ListSequenceStores filtered by name and through GetReferenceStore/GetSequenceStore, and DeleteStack really deletes them so those reads answer 404 ResourceNotFoundException.
Emulator: the very first call fails — CreateStack returns HTTP 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::Omics::ReferenceStore, AWS::Omics::SequenceStore]". Fixed text: "Template format error: Unrecognized resource types: [" + the comma-separated list of type names taken from the request's TemplateBody + "]"; the type names are the only variable part. Nothing is provisioned, so Ref/GetAtt, the access-point policy and the delete semantics are all unobservable.
Why: a client that deploys HealthOmics storage with CloudFormation or CDK cannot create a stack at all; the refusal is reported as a template-format defect, so the client blames its own template rather than a missing provider.
Fix: the CloudFormation resource-type registry/provider set — register AWS::Omics::ReferenceStore and AWS::Omics::SequenceStore and back them with providers calling the omics CreateReferenceStore/CreateSequenceStore, DeleteReferenceStore/DeleteSequenceStore state, exposing Ref = id and GetAtt Arn/S3Uri/S3AccessPointArn.
Traces: cloudformation__omics.deploy-reference-and-sequence-store-via-cloudformation-reaches-create-complete.

Cause 2 of 4: AWS::Omics::RunCache is unrecognized.
AWS: a template declaring Type: AWS::Omics::RunCache with Name, CacheS3Location and CacheBehavior reaches CREATE_COMPLETE. Ref yields the run cache ARN and GetAtt Id / Status yield CreateRunCache.output.id / .status (Smithy omics CreateRunCache: cacheS3Location required, @pattern ^s3://([a-z0-9][a-z0-9-.]{1,61}[a-z0-9])(/(.{0,1024}))?$; output arn/id/status; HTTP 201). The cache is then listed by ListRunCaches and read by GetRunCache with cacheBehavior as declared and cacheS3Uri being one minted folder directly under the declared CacheS3Location prefix; DeleteStack removes it so GetRunCache answers ResourceNotFoundException "Run Cache <id> was not found" (fixed text "Run Cache " + the requested id + " was not found").
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Omics::RunCache]".
Why: run caching cannot be declared as infrastructure; a workflow estate that depends on a stack-managed cache cannot be stood up.
Fix: the CloudFormation resource-type registry/provider set — register AWS::Omics::RunCache with a provider over the omics run-cache state, Ref = arn, GetAtt Id/Status.
Traces: cloudformation__omics.deploy-run-cache-via-cloudformation-reaches-create-complete.

Cause 3 of 4: AWS::Omics::Workflow and AWS::Omics::WorkflowVersion are unrecognized.
AWS: a template declaring Type: AWS::Omics::Workflow (Name, Engine, DefinitionUri, Main) and Type: AWS::Omics::WorkflowVersion (WorkflowId: !Ref of the workflow, VersionName, Engine, DefinitionUri, Main) reaches CREATE_COMPLETE. Ref of the workflow yields CreateWorkflow.output.id (@pattern ^[0-9]+$), GetAtt Arn yields output.arn; Ref of the version yields CreateWorkflowVersion.output.arn and GetAtt Status / Uuid yield output.status / output.uuid (@pattern ^[0-9a-fA-F]{8}-...-[0-9a-fA-F]{12}$). GetWorkflow/GetWorkflowVersion read back the declared name, engine and main. An UpdateStack that removes only the version reaches UPDATE_COMPLETE and deletes only the version — GetWorkflowVersion then answers ResourceNotFoundException "Workflow <workflowId> with version name <versionName> not found" while GetWorkflow still succeeds; DeleteStack then deletes the workflow and GetWorkflow answers ResourceNotFoundException "Workflow <workflowId> not found". (Those messages' fixed text is "Workflow ", " with version name ", " not found"; the id and version name come from the request.) The omics model itself carries the CloudFormation mapping intent: CreateWorkflow.definitionZip and CreateWorkflowVersion.definitionZip are marked cfnExcludeProperty.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Omics::Workflow, AWS::Omics::WorkflowVersion]".
Why: no HealthOmics workflow or workflow version can be managed as code, and the version-scoped delete-on-update semantics (version removed, workflow retained) are unreachable.
Fix: the CloudFormation resource-type registry/provider set — register AWS::Omics::Workflow and AWS::Omics::WorkflowVersion with providers over the omics workflow state, honouring the inter-resource dependency from the version's WorkflowId Ref and deleting only the version on its removal from the template.
Traces: cloudformation__omics.deploy-workflow-and-workflow-version-via-cloudformation-reaches-create-complete.

Cause 4 of 4: the duplicate-name refusal of a stack-declared AWS::Omics::SequenceStore is unreachable, and with it the registry-handler failure reason.
AWS: the positive control (a template with a Name no store holds) reaches CREATE_COMPLETE and its Ref is the one store ListSequenceStores lists under that name. The same template with a Name an existing sequence store already holds reaches ROLLBACK_COMPLETE, and the resource's CREATE_FAILED StackEvent carries a registry-handler ResourceStatusReason whose message is "Name must be unique. (Service: Omics, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" with HandlerErrorCode AlreadyExists — fixed text "Name must be unique." plus the standard service/status/request-id envelope, whose request id and attempt count are per-run values and not part of the requirement. The rejected create leaves the pre-existing store the only one of that name.
Emulator: CreateStack answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Omics::SequenceStore]" for both the control and the duplicate template, so neither the rollback status, nor the handler error code, nor the "Name must be unique." reason is ever produced.
Why: an IaC caller gets no deployable control path and, once the type exists, must still get AWS's AlreadyExists/"Name must be unique." handler reason rather than a generic failure; a name collision must roll the stack back and leave existing state untouched.
Fix: the CloudFormation resource-type registry/provider set plus the AWS::Omics::SequenceStore provider's error mapping — surface the omics create refusal as a registry-handler reason with HandlerErrorCode AlreadyExists and roll the stack back.
Traces: cloudformation__omics.deploy-sequence-store-duplicate-name-rejected.

## Evidence
All four checks fail on their first CloudFormation call.

Expected (AWS): CreateStack accepts the template (200, StackId returned) and the stack converges to CREATE_COMPLETE (or, for the duplicate-name case, ROLLBACK_COMPLETE with a resource CREATE_FAILED reason of "Name must be unique. (Service: Omics, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" and HandlerErrorCode AlreadyExists).

Actual (emulator), verbatim from the replay:
- deploy-reference-and-sequence-store-via-cloudformation-reaches-create-complete: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Omics::ReferenceStore, AWS::Omics::SequenceStore]
- deploy-run-cache-via-cloudformation-reaches-create-complete: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Omics::RunCache]
- deploy-sequence-store-duplicate-name-rejected: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Omics::SequenceStore]
- deploy-workflow-and-workflow-version-via-cloudformation-reaches-create-complete: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Omics::Workflow, AWS::Omics::WorkflowVersion]

The HTTP status (400) and error code (ValidationError) are AWS's for a genuinely unknown type; the divergence is that these types are NOT unknown to AWS. In the message, "Template format error: Unrecognized resource types: [...]" is fixed text and the bracketed type list is taken from the request's TemplateBody.

Model support (Smithy, service-com-amazonaws-omics-omics): CreateReferenceStore -> id/arn; CreateSequenceStore -> id/arn/s3Access{s3Uri,s3AccessPointArn}; CreateRunCache(cacheS3Location required) -> arn/id/status, HTTP 201; CreateWorkflow -> id/arn/status/uuid with definitionZip marked cfnExcludeProperty (a CloudFormation-mapping trait); CreateWorkflowVersion -> arn/workflowId/versionName/status/uuid.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cloudformation.create_stack(StackName="s1", TemplateBody=yaml with
     Resources:
       Store:
         Type: AWS::Omics::SequenceStore
         Properties: {Name: seq-1}
     Outputs: {SeqStoreId: {Value: !Ref Store}})
   -> observed: ClientError 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Omics::SequenceStore]".
   -> expected: 200 with a StackId; DescribeStacks reaches CREATE_COMPLETE and Outputs.SeqStoreId equals the id GetSequenceStore/ListSequenceStores report for seq-1.
3. Repeat with Type: AWS::Omics::ReferenceStore (+ GetAtt Arn), AWS::Omics::RunCache (Name/CacheS3Location/CacheBehavior, Ref = arn, GetAtt Id/Status), and AWS::Omics::Workflow + AWS::Omics::WorkflowVersion (WorkflowId: !Ref of the workflow) — each is refused the same way.
4. For the collision path: omics.create_sequence_store(name="dup-1"), then create a stack declaring AWS::Omics::SequenceStore with Name: dup-1 -> expected ROLLBACK_COMPLETE with the resource CREATE_FAILED reason "Name must be unique. (Service: Omics, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" / HandlerErrorCode AlreadyExists, and the pre-existing store left as the only store of that name; observed the same CreateStack ValidationError instead.
5. Replay: the four check ids in trace_ids.
