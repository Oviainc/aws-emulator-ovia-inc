Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codepipeline.deploy-codepipeline-custom-action-type-round-trip-and-delete cloudformation__codepipeline.deploy-codepipeline-pipeline-round-trip-and-delete cloudformation__codepipeline.deploy-codepipeline-pipeline-unassumable-role-rolled-back cloudformation__codepipeline.deploy-codepipeline-webhook-round-trip-and-delete
Issue-Key: cloudformation__codepipeline.deploy-codepipeline-custom-action-type-round-trip-and-delete|run#14|AssertionError|assert ref_value == want_ref, (
Issue-Key: cloudformation__codepipeline.deploy-codepipeline-pipeline-round-trip-and-delete|stack_outputs#11|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__codepipeline.deploy-codepipeline-pipeline-unassumable-role-rolled-back|run#16|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__codepipeline.deploy-codepipeline-webhook-round-trip-and-delete|run#18|AssertionError|assert found is not None, f"ListWebhooks has no entry named {webhook_name!r}"
Issue-Commit: 4cb124a92bc472448f3b420460157b630c09e49d

# cloudformation__codepipeline: 4 net-new causes — CustomActionType Ref is a generated physical id, pipeline stack Outputs carry no OutputValue, an unassumable pipeline role reaches CREATE_COMPLETE, and AWS::CodePipeline::Webhook is stub-provisioned

> _Note: this issue was identified against the code base at commit `4cb124a92bc472448f3b420460157b630c09e49d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four independent defects in the CloudFormation resource handlers for the AWS::CodePipeline::* resource family. Each is a separate root cause and must be closed separately.

=== Cause 1 of 4: AWS::CodePipeline::CustomActionType Ref resolves to a generated physical name instead of the composite primary identifier ===
AWS: the CloudFormation resource reference for AWS::CodePipeline::CustomActionType states that Ref returns the resource's composite primary identifier, the declared `Category|Provider|Version` triple (for Category=Build, Provider=<provider>, Version=1 this is `Build|<provider>|1`). Smithy (codepipeline): ActionTypeId carries category/owner/provider/version, and ListActionTypes(actionOwnerFilter="Custom") keys custom action types by exactly that triple, so Ref and the listed id agree.
Emulator: the stack reaches CREATE_COMPLETE and the Output declared as {"Ref": "ActionType"} resolves to a CloudFormation-style generated physical name of the form `<stack-name>-<logical-id>-<random-suffix>` instead of the `Category|Provider|Version` string.
Why: a template that passes {"Ref": "ActionType"} into another resource's ActionTypeId, into an Export, or into a cross-stack Fn::ImportValue receives a string that names no action type, so the wiring silently points at nothing and CodePipeline later refuses the action type as unknown.
Fix: the AWS::CodePipeline::CustomActionType resource handler's physical-resource-id / Ref assignment in the CloudFormation resource registry (it must build the identifier from the declared Category, Provider and Version rather than minting a name).
Traces: cloudformation__codepipeline.deploy-codepipeline-custom-action-type-round-trip-and-delete

=== Cause 2 of 4: DescribeStacks Outputs entries of a CodePipeline pipeline stack come back with no OutputValue member ===
AWS: Smithy (cloudformation) Output is {OutputKey, OutputValue, Description, ExportName}; for a stack in CREATE_COMPLETE every Output declared in the template is returned with its resolved OutputValue. The stack here declares PipelineRef ({"Ref": "Pipeline"}), PipelineArn (Fn::GetAtt Pipeline.Arn), PipelineVersion (Fn::GetAtt Pipeline.Version) and RoleArn (Fn::GetAtt PipelineRole.Arn).
Emulator: DescribeStacks returns Outputs rows that carry OutputKey but omit OutputValue entirely, so reading o["OutputValue"] raises KeyError: 'OutputValue'. The Ref/GetAtt values for the pipeline and its role are therefore never published.
Why: every client that reads stack outputs — the CLI's `describe-stacks --query Stacks[0].Outputs`, CDK, and any stack that consumes the value through Fn::ImportValue — gets an output with no value, so the pipeline's name, ARN and version cannot be discovered from the stack that created them.
Note: the same OutputValue omission is already filed for other resource families (20261109T143402Z-cloudformation-mq-..., 20261109T144630Z-cloudformation-elasticbeanstalk-..., 20261109T154607Z-cloudformation-signer-...). Those issues do not cover the CodePipeline resource handlers; this cause is net-new here. The sibling webhook stack in cause 4 DOES resolve its Outputs, so the omission is specific to the Pipeline/Role attribute resolution rather than global.
Fix: the Ref/Fn::GetAtt attribute resolution of the AWS::CodePipeline::Pipeline (and AWS::IAM::Role) resource handlers, and the DescribeStacks Outputs serializer that drops a member with no resolved value.
Traces: cloudformation__codepipeline.deploy-codepipeline-pipeline-round-trip-and-delete

=== Cause 3 of 4: a pipeline whose RoleArn names a role CodePipeline cannot assume reaches CREATE_COMPLETE instead of failing the resource and rolling the stack back ===
AWS: CodePipeline's CreatePipeline validates that it can assume the declared service role; a role whose trust policy allows a different service principal (here lambda.amazonaws.com) is refused. CloudFormation then records a CREATE_FAILED event on the Pipeline resource whose ResourceStatusReason is the registry-handler form `Resource handler returned message: "CodePipeline is not authorized to perform AssumeRole on role arn:<partition>:iam::<account>:role/<role-name> (Service: CodePipeline, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)`, the stack ends ROLLBACK_COMPLETE with a ROLLBACK_COMPLETE event for the stack itself, and GetPipeline on the declared name answers PipelineNotFoundException with `Account '<account>' does not have a pipeline with name '<pipeline-name>'`. Fixed text of that reason: "CodePipeline is not authorized to perform AssumeRole on role ", "(Service: CodePipeline, Status Code: 400, Request ID: ", "(SDK Attempt Count: ", and HandlerErrorCode GeneralServiceException. Variable parts: the role ARN's partition, account and role name (taken from the request's RoleArn) and the per-run request id / request token / attempt count.
Emulator: the stack ends CREATE_COMPLETE. No CREATE_FAILED event is recorded for the Pipeline resource, no rollback happens, and the pipeline is left created with a role CodePipeline cannot assume.
Why: this is an overly-permissive accept. A deploy AWS rejects outright appears to succeed, so a broken trust policy ships undetected and the pipeline fails only later at execution time, with no CloudFormation signal pointing at the role.
Fix: the AWS::CodePipeline::Pipeline resource handler's create path must route through CodePipeline's CreatePipeline role-assumability validation, surface the refusal as the handler failure reason above, and let CloudFormation's rollback path run.
Traces: cloudformation__codepipeline.deploy-codepipeline-pipeline-unassumable-role-rolled-back

=== Cause 4 of 4: AWS::CodePipeline::Webhook is stub-provisioned — the stack completes but no webhook exists in CodePipeline ===
AWS: Smithy (codepipeline) ListWebhooks returns webhooks: list<ListWebhookItem>, each with a required definition (WebhookDefinition: name, targetPipeline, targetAction, filters, authentication, authenticationConfiguration), a required url and an arn. A webhook declared as AWS::CodePipeline::Webhook is a real PutWebhook, so after CREATE_COMPLETE ListWebhooks lists an item whose definition.name is the declared Name, with the declared targetPipeline, targetAction, filters and IP authentication; deleting the stack removes that item.
Emulator: the stack reaches CREATE_COMPLETE and Ref / Fn::GetAtt Id both resolve, but paginating ListWebhooks returns no entry whose definition.name equals the template's Name — the webhook was never created in CodePipeline state.
Why: the deployed webhook does not exist. Nothing can trigger the target pipeline through it, DeleteWebhook/ListWebhooks-based management sees nothing to manage, and the stack reports a resource it never provisioned.
Fix: the AWS::CodePipeline::Webhook resource handler must create the webhook in CodePipeline's state (the PutWebhook path) on create and remove it on delete, rather than only minting an id.
Traces: cloudformation__codepipeline.deploy-codepipeline-webhook-round-trip-and-delete

Related but not covering: open issue 20261023T080113Z-codebuild-codepipeline-ecr-cloudformation-stack-reports-completion-but-provisions-no-codepipeline.md reports a CFN stack provisioning no pipeline under a different service heading; none of the four causes above is marked owned by it.

## Evidence
Cause 1 — cloudformation__codepipeline.deploy-codepipeline-custom-action-type-round-trip-and-delete [mismatch]:
  AssertionError: Ref resolved to 'sim2real-cpcat-stack-95e60564c26b-ActionType-18D6D96B394F', expected the composite primary identifier 'Build|Sim2realCAT93109cae1987|1'
  Expected: Outputs['ActionTypeRef'] == "<Category>|<Provider>|<Version>" and equal to the ListActionTypes(actionOwnerFilter='Custom') entry's category|provider|version.
  Actual: a generated <stack>-<logical-id>-<suffix> physical name.

Cause 2 — cloudformation__codepipeline.deploy-codepipeline-pipeline-round-trip-and-delete [error]:
  File "check_...pipeline_round_trip_and_delete.py", line 117, in stack_outputs
    return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'
  Expected: DescribeStacks on the CREATE_COMPLETE stack returns Outputs PipelineRef/PipelineArn/PipelineVersion/RoleArn each with a resolved OutputValue.
  Actual: Outputs rows have no OutputValue member.

Cause 3 — cloudformation__codepipeline.deploy-codepipeline-pipeline-unassumable-role-rolled-back [mismatch]:
  AssertionError: stack 'sim2real-cpneg-stack-944acadeabac' ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE
  Expected: Pipeline CREATE_FAILED with reason 'Resource handler returned message: "CodePipeline is not authorized to perform AssumeRole on role arn:<partition>:iam::<account>:role/<role> (Service: CodePipeline, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: GeneralServiceException)', stack ROLLBACK_COMPLETE, and GetPipeline -> PipelineNotFoundException "Account '<account>' does not have a pipeline with name '<pipeline>'".
  Actual: CREATE_COMPLETE, no CREATE_FAILED event, pipeline created.

Cause 4 — cloudformation__codepipeline.deploy-codepipeline-webhook-round-trip-and-delete [mismatch]:
  AssertionError: ListWebhooks has no entry named 'sim2real-cpwh-59c11959fa8a'
  Expected: after CREATE_COMPLETE, ListWebhooks lists an item with definition.name == the template Name, definition.targetPipeline == the declared pipeline, definition.targetAction == the declared action, the declared filters and authentication 'IP'.
  Actual: no such item on any page (Ref and GetAtt Id did resolve).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: CreateStack with a template declaring a single AWS::CodePipeline::CustomActionType (Category=Build, Provider=<unique>, Version=1, InputArtifactDetails {MinimumCount:1,MaximumCount:1}, OutputArtifactDetails {MinimumCount:0,MaximumCount:1}) and Output {"ActionTypeRef": {"Value": {"Ref": "ActionType"}}}. Wait for CREATE_COMPLETE, then DescribeStacks and read Outputs['ActionTypeRef']; compare with the Category|Provider|Version triple and with the ListActionTypes(actionOwnerFilter='Custom') entry.
3. Cause 2: CreateStack with a template declaring an AWS::CodePipeline::Pipeline plus its IAM role and Outputs PipelineRef (Ref Pipeline), PipelineArn (GetAtt Pipeline.Arn), PipelineVersion (GetAtt Pipeline.Version), RoleArn (GetAtt PipelineRole.Arn). Wait for CREATE_COMPLETE, call DescribeStacks and read OutputValue on each Outputs row.
4. Cause 3: CreateRole with a trust policy whose Principal.Service is lambda.amazonaws.com, attach an artifact-bucket policy, then CreateStack with an AWS::CodePipeline::Pipeline whose RoleArn is that role. Poll DescribeStacks until a terminal status; inspect DescribeStackEvents for the Pipeline resource and call GetPipeline on the declared pipeline name.
5. Cause 4: create a prerequisite pipeline, then CreateStack with an AWS::CodePipeline::Webhook (Authentication=IP, AuthenticationConfiguration, filters, TargetPipeline/TargetAction of that pipeline, RegisterWithThirdParty=false). Wait for CREATE_COMPLETE, then paginate ListWebhooks and look for definition.name == the declared Name; delete the stack and re-list.
