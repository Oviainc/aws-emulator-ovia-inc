Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__osis.deploy-osis-pipeline-minunits-above-maxunits-rejected
Issue-Key: cloudformation__osis|CreateStack|ValidationError
Issue-Commit: f1d315614d2491eea3b7c6e4236ba64fe2dea133

# cloudformation__osis: CreateStack refuses AWS::OSIS::Pipeline as an unrecognized resource type, so no OpenSearch Ingestion pipeline can be deployed or rejected by a template

> _Note: this issue was identified against the code base at commit `f1d315614d2491eea3b7c6e4236ba64fe2dea133`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause.

Cause 1 of 1: the CloudFormation emulator's resource-type registry does not contain `AWS::OSIS::Pipeline`, so CreateStack rejects the whole template up front at template-format validation instead of creating a stack and invoking the OSIS resource handler.

AWS: Smithy (cloudformation, awsQuery, CreateStack POST /) models `ValidationError` as the client-side refusal for a template CloudFormation cannot accept, and `AWS::OSIS::Pipeline` IS a registered CloudFormation resource type in real AWS (properties PipelineName, MinUnits, MaxUnits, PipelineConfigurationBody, VpcOptions, LogPublishingOptions, Tags; Ref returns the pipeline ARN). The OSIS side is modelled too: com.amazonaws.osis#CreatePipeline / GetPipeline with Pipeline.MinUnits and Pipeline.MaxUnits (Integer, default 0) and Pipeline.Status (PipelineStatus enum), and ResourceNotFoundException for an absent pipeline. Recorded behavior for the deploy path: a template declaring an `AWS::OSIS::Pipeline` with MinUnits greater than MaxUnits is accepted by CreateStack (a StackId is returned) and no early-validation row is produced (DescribeEvents reports no VALIDATION_ERROR OperationEvents for the stack); the OSIS resource handler then refuses the create, the stack ends `ROLLBACK_COMPLETE`, and the `Pipeline` logical resource carries exactly one CREATE_FAILED event whose ResourceStatusReason has the registry-handler form `Resource handler returned message: "<message>" (RequestToken: <token>, HandlerErrorCode: <code>)` with message `minUnits cannot be greater than maxUnits (Service: Osis, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` and code `InvalidRequest`. Fixed text of that message: `minUnits cannot be greater than maxUnits (Service: Osis, Status Code: 400, Request ID: ` ... `) (SDK Attempt Count: ` ... `)`; the Request ID and SDK Attempt Count are per-run values, not part of the requirement. No pipeline survives the rollback: ListPipelines does not list the name the template asked for, and GetPipeline for it is refused 404 `ResourceNotFoundException` with the fixed message `The pipeline could not be found`. The positive control (same template with MinUnits equal to MaxUnits) reaches `CREATE_COMPLETE`, `Ref` on the Pipeline resource yields the pipeline ARN that ListPipelines reports, GetPipeline shows (MinUnits, MaxUnits, Status) = (1, 1, "ACTIVE") with VpcEndpoints whose VpcId/SubnetIds/SecurityGroupIds equal the stack's provisioned VPC, subnet and security group, and DeleteStack ends `DELETE_COMPLETE` with the pipeline gone from ListPipelines and GetPipeline back to ResourceNotFoundException.

Emulator: CreateStack is refused immediately with HTTP 400, error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::OSIS::Pipeline]`. No stack is created, so none of the behavior above (handler refusal, rollback, events, Ref, provisioning, delete) can be observed at all.

Why: a client cannot deploy OpenSearch Ingestion from infrastructure-as-code against the emulator — neither the rejection path nor the happy path. The template is valid in real AWS, so the emulator turns a successful deploy (or a modelled handler rejection with a precise reason the client parses) into a permanent template-format error at submit time; every CDK/CloudFormation test that provisions a pipeline fails for a reason that has nothing to do with the behavior it is testing.

Fix: the CloudFormation emulator's resource-type registry plus the AWS::OSIS::Pipeline resource provider — register the type so templates declaring it pass template-format validation, and wire its create/read/delete to the osis service state so the handler returns the OSIS refusal (surfaced as the `Resource handler returned message: ... HandlerErrorCode: InvalidRequest` CREATE_FAILED reason and ROLLBACK_COMPLETE) for MinUnits > MaxUnits, and otherwise provisions a pipeline visible to osis ListPipelines/GetPipeline with Ref = its ARN and VpcEndpoints reflecting the template's VpcOptions.

Traces: cloudformation__osis.deploy-osis-pipeline-minunits-above-maxunits-rejected.

Related open issues: a family of `CreateStack refuses AWS::<service>::* as unrecognized resource type` issues is already filed for other services (e.g. 20261109T113214Z-cloudformation-mediatailor-..., 20261109T113428Z-cloudformation-mwaa-...); none of them covers AWS::OSIS::Pipeline, and no open osis issue mentions CloudFormation.

## Evidence
run_behavioral_suite (suite simparity-behavioral-aws-v1): 15/23 passed, bootstrap_signal false — the emulator is up and answering, and 15 osis checks pass, so this is not a bootstrap gap.

[error] cloudformation__osis.deploy-osis-pipeline-minunits-above-maxunits-rejected — the very first call of the check, create_stack(StackName=<unique>, TemplateBody=<template>, Tags=[...], Capabilities=["CAPABILITY_IAM"]), raises:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::OSIS::Pipeline]

Expected instead (from the check's own assertions): CreateStack returns a StackId; wait_stack -> 'ROLLBACK_COMPLETE'; validation_rows(cf, stack_id) == [] (no VALIDATION_ERROR OperationEvents, i.e. refused by the handler and not by early validation); failed_reason(events, 'Pipeline') matches `Resource handler returned message: "<message>" (RequestToken: <token>, HandlerErrorCode: <code>)` with (masked message, code) == ('minUnits cannot be greater than maxUnits (Service: Osis, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest'); osis.list_pipelines shows no entry for the pipeline name; osis.get_pipeline(PipelineName=<name>) raises ('ResourceNotFoundException', 'The pipeline could not be found').

The template declares AWS::EC2::VPC, AWS::EC2::Subnet, AWS::EC2::SecurityGroup, AWS::S3::Bucket and AWS::IAM::Role alongside the AWS::OSIS::Pipeline; only AWS::OSIS::Pipeline is named in the emulator's 'Unrecognized resource types' list, so the rest of the registry is present and OSIS is the single missing type.

Model grounding: com.amazonaws.osis#Pipeline carries MinUnits/MaxUnits (Integer, default 0), Status (PipelineStatus enum), VpcEndpoints (VpcEndpointsList) and IngestEndpointUrls, i.e. everything the AWS::OSIS::Pipeline provider must read back; cloudformation#CreateStack models ValidationError as its client refusal.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay cloudformation__osis.deploy-osis-pipeline-minunits-above-maxunits-rejected (run_behavioral_suite).
2. Or by hand: cloudformation CreateStack with a JSON template whose Resources contain a `{"Type": "AWS::OSIS::Pipeline", "Properties": {"PipelineName": <name>, "MinUnits": 5, "MaxUnits": 1, "PipelineConfigurationBody": <Data Prepper YAML with an http source and an s3 sink>, "VpcOptions": {"SubnetIds": [...], "SecurityGroupIds": [...]}, "Tags": [...]}}` plus the VPC/Subnet/SecurityGroup/Bucket/Role it references, and Capabilities=["CAPABILITY_IAM"].
3. Observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::OSIS::Pipeline]' instead of a StackId.
4. After the fix, the same call must return a StackId, the stack must settle ROLLBACK_COMPLETE with no VALIDATION_ERROR event and a single Pipeline CREATE_FAILED row reading `Resource handler returned message: "minUnits cannot be greater than maxUnits (Service: Osis, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`, osis ListPipelines must not list the name and GetPipeline must refuse 404 ResourceNotFoundException 'The pipeline could not be found'.
5. Then repeat with MinUnits == MaxUnits == 1: the stack must reach CREATE_COMPLETE, Ref on the Pipeline resource must equal the ARN ListPipelines reports, GetPipeline must show (1, 1, 'ACTIVE') and VpcEndpoints matching the stack's VpcId/SubnetId/SgId, and DeleteStack must reach DELETE_COMPLETE with the pipeline gone.
6. Re-replay cloudformation__osis.deploy-osis-pipeline-minunits-above-maxunits-rejected to confirm.
