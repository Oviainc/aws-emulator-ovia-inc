Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mwaa-serverless.deploy-mwaaserverless-workflow-invalid-rolearn-pattern-rejected cloudformation__mwaa-serverless.deploy-mwaaserverless-workflow-round-trip-and-delete
Issue-Key: cloudformation__iam__mwaa-serverless__s3|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation: AWS::MWAAServerless::Workflow is not a registered resource type — CreateStack refuses every template that declares one with "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE net-new root cause in the CloudFormation emulator, grounded in two failing behavioural checks. Both checks die on the very first CreateStack, so nothing past template parsing was exercised by either.

=== Cause 1 of 1: the CloudFormation resource-type registry does not carry AWS::MWAAServerless::Workflow, so no IaC-provisioned MWAA Serverless workflow is reachable at all ===

AWS: recorded behavior only (CloudFormation's resource-type registry is not described by the CloudFormation Smithy model; the target service's shapes are modelled at services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json, sdkId "MWAA Serverless"). AWS accepts a template whose Resources map declares a resource of Type `AWS::MWAAServerless::Workflow` with Properties Name, Description, DefinitionS3Location {Bucket, ObjectKey}, RoleArn and Tags (a string->string map). CreateStack returns 200 with a StackId and the stack reaches CREATE_COMPLETE. The deployed resource is observable through the mwaa-serverless API: ListWorkflows reports exactly one workflow under the templated Name; `Ref` on the logical id yields that workflow's WorkflowArn (the modelled WorkflowArn pattern ^arn:aws(-...)?:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>$); `Fn::GetAtt` exposes the read-only attributes WorkflowVersion and WorkflowStatus, whose values equal GetWorkflow's WorkflowVersion and WorkflowStatus (READY); GetWorkflow echoes the templated Name, Description, RoleArn and DefinitionS3Location; ListTagsForResource reports the resource's own Tags plus the stack-level Tags (and the aws:cloudformation:* reserved keys). DeleteStack deletes the workflow, after which GetWorkflow on the Ref'd ARN is refused with Code `ResourceNotFoundException` and Message `Resource not found` (fixed text, no variable part).
AWS also applies the resource type's own property schema BEFORE provisioning: a RoleArn that fails the type's ARN pattern fails early property validation, the stack settles ROLLBACK_COMPLETE, and DescribeEvents reports a VALIDATION_ERROR OperationEvent with ValidationName `PROPERTY_VALIDATION`, ValidationStatus `FAILED`, ValidationFailureMode `FAIL`, ValidationPath `/Resources/<LogicalId>/Properties/RoleArn` and ValidationStatusReason of the fixed form `Property value [<the supplied value>] does not match pattern: <the schema's RoleArn regex>` — the bracketed value and the regex are the variable parts, taken from the request and from the type's schema; no workflow of that Name is ever created.

Emulator: CreateStack is refused outright with HTTP 400, Code `ValidationError`, Message `Template format error: Unrecognized resource types: [AWS::MWAAServerless::Workflow]`. The fixed text here is `Template format error: Unrecognized resource types: [` ... `]` and the bracketed list is the set of declared types the registry did not resolve — i.e. the emulator's own refusal wording is well-formed, the defect is that this type is absent from the registry it consults. Because the refusal happens at template parse, neither the happy path (Ref/GetAtt/read-back/tag merge/stack-delete propagation) nor the negative path (property-pattern validation, ROLLBACK_COMPLETE, the VALIDATION_ERROR event row) exists at all.

Why: a client relying on AWS's behavior cannot provision MWAA Serverless workflows as infrastructure-as-code against the emulator. Every CloudFormation/CDK/SAM template that contains an AWS::MWAAServerless::Workflow fails at CreateStack with a template-format error, so the entire IaC path for the service is unusable; worse, the failure mode is indistinguishable from a genuinely malformed template, so a user debugging it looks for a typo in a correct template. The mwaa-serverless data plane itself answers (its other checks pass), so only the CloudFormation binding is missing.

Fix: the CloudFormation emulator's resource-type registry / resource-provider dispatch — register `AWS::MWAAServerless::Workflow` with its property schema (Name, Description, DefinitionS3Location{Bucket,ObjectKey}, RoleArn with the IAM-role ARN pattern, Tags map), its primary identifier (Ref -> WorkflowArn) and its read-only attributes (WorkflowVersion, WorkflowStatus), and wire Create/Read/Update/Delete onto the mwaa-serverless state's CreateWorkflow/GetWorkflow/UpdateWorkflow/DeleteWorkflow, propagating stack-level Tags onto the workflow. The property-schema check must run in the early-validation phase that emits the VALIDATION_ERROR OperationEvent and rolls the stack back without creating the workflow. No open issue covers this type: 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md is the same CLASS of gap for AWS::SSM::MaintenanceWindowTarget and does not name this one.

Traces: cloudformation__mwaa-serverless.deploy-mwaaserverless-workflow-round-trip-and-delete (happy path), cloudformation__mwaa-serverless.deploy-mwaaserverless-workflow-invalid-rolearn-pattern-rejected (negative path; its positive control stack, the same template with a well-formed RoleArn, is refused identically).

## Evidence
Suite: simparity-behavioral-aws-v1. Service heading `cloudformation__iam__mwaa-serverless__s3`: 0/2 (0%), 2 failed (both verdict `error`). Neither failure was marked ALREADY FILED or OWNED by an open issue.

Both checks, identical failure at the first CreateStack:
  expected HTTP 200 with a StackId, then the stack reaching CREATE_COMPLETE
  actual   botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MWAAServerless::Workflow]

Template submitted by deploy-mwaaserverless-workflow-round-trip-and-delete (per-run names/ARNs elided — they are generated values, not part of the requirement):
  {"AWSTemplateFormatVersion": "2010-09-09",
   "Resources": {"Workflow": {"Type": "AWS::MWAAServerless::Workflow",
     "Properties": {"Name": <name>, "Description": <description>,
                    "DefinitionS3Location": {"Bucket": <bucket>, "ObjectKey": "dags/sim2real.yaml"},
                    "RoleArn": <role arn>, "Tags": {"sim2real-workflow": <name>}}}},
   "Outputs": {"WorkflowRef": {"Value": {"Ref": "Workflow"}},
               "WorkflowVersionAtt": {"Value": {"Fn::GetAtt": ["Workflow", "WorkflowVersion"]}},
               "WorkflowStatusAtt": {"Value": {"Fn::GetAtt": ["Workflow", "WorkflowStatus"]}}}}

Assertions that were never reached (round-trip check): Ref == the single ListWorkflows entry's WorkflowArn; GetAtt WorkflowVersion == GetWorkflow WorkflowVersion; GetAtt WorkflowStatus == GetWorkflow WorkflowStatus == "READY"; GetWorkflow Name/Description/RoleArn/DefinitionS3Location equal to the templated values; ListTagsForResource (aws:-prefixed keys removed) == {"sim2real": <stack name>, "sim2real-workflow": <workflow name>}; after DeleteStack, GetWorkflow refused with ("ResourceNotFoundException", "Resource not found").

Assertions that were never reached (negative check): with RoleArn = "not-a-valid-role-arn", the stack settles ROLLBACK_COMPLETE and the sorted VALIDATION_ERROR rows of DescribeEvents equal exactly
  [("Workflow", "PROPERTY_VALIDATION", "FAILED", "FAIL",
    "Property value [not-a-valid-role-arn] does not match pattern: ^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_-]{1,512})*?/[a-zA-Z0-9+=,.@_-]{1,64}$",
    "/Resources/Workflow/Properties/RoleArn")]
and ListWorkflows contains no workflow under the rejected Name. The bracketed value is the request's RoleArn; the regex is the resource type's own schema pattern.

## Steps to Reproduce
Setup (both checks do this themselves): create an S3 bucket, PutObject an Airflow YAML DAG at `dags/sim2real.yaml`, and create an IAM role whose trust policy names `airflow-serverless.amazonaws.com` with read access to that object and log-write access to /aws/mwaa-serverless/*.

1. CreateStack with TemplateBody declaring one resource of Type `AWS::MWAAServerless::Workflow` with Properties {Name, Description, DefinitionS3Location:{Bucket,ObjectKey}, RoleArn, Tags:{"sim2real-workflow":<name>}} and Outputs for {"Ref":"Workflow"}, {"Fn::GetAtt":["Workflow","WorkflowVersion"]} and {"Fn::GetAtt":["Workflow","WorkflowStatus"]}. Expect 200 + a StackId; the emulator currently answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::MWAAServerless::Workflow]".
2. Poll DescribeStacks(StackName=<StackId>) to CREATE_COMPLETE. Read the Outputs: ListWorkflows must contain exactly one workflow of that Name and its WorkflowArn must equal the Ref; GetWorkflow(WorkflowArn=Ref) must report WorkflowStatus "READY" and a WorkflowVersion equal to the GetAtt output, and echo the templated Name, Description, RoleArn and DefinitionS3Location; ListTagsForResource(ResourceArn=Ref) must carry both the stack-level tag and the resource's own Tags entry.
3. DeleteStack and wait DELETE_COMPLETE; GetWorkflow(WorkflowArn=Ref) must then be refused with Code "ResourceNotFoundException" and Message "Resource not found".
4. Negative path: submit the same template with RoleArn = "not-a-valid-role-arn" and nothing else changed. Expect the stack to settle ROLLBACK_COMPLETE, DescribeEvents to carry exactly one VALIDATION_ERROR OperationEvent with ValidationName PROPERTY_VALIDATION / ValidationStatus FAILED / ValidationFailureMode FAIL / ValidationPath "/Resources/Workflow/Properties/RoleArn" and a ValidationStatusReason of the form "Property value [<supplied RoleArn>] does not match pattern: <the type's RoleArn regex>", and ListWorkflows to contain no workflow under the rejected Name.
