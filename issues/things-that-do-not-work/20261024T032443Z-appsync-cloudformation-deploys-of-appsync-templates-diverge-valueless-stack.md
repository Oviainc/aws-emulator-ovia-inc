Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.deploy-api-key-cfn-invalid-key-rejected appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected appsync.deploy-data-source-resolver-cfn-executes-query appsync.deploy-graphql-api-cfn-behaves-live-query appsync.deploy-graphql-api-cfn-schema-active appsync__cognito-idp__dynamodb.deploy-appsync-cognito-groups-field-authorization-positive appsync__cloudwatch__cognito-idp__dynamodb__logs.cdk-authenticated-graphql-app-workflow appsync.deploy-appsync-3-cfn-association-to-non-merged-api-rejected appsync.deploy-appsync-3-cfn-merged-api-auto-merge-serves-source-update appsync.deploy-appsync-3-cfn-merged-api-manual-merge-withholds-update-until-merge appsync.deploy-appsync-3-cfn-merged-endpoint-source-api-key-rejected appsync.deploy-appsync-3-cfn-merged-role-without-source-graphql-denied appsync.deploy-channel-namespace-cfn-event-api-functional appsync.deploy-cfn-invalid-schema-stack-rolls-back-rejected
Issue-Key: appsync.deploy-api-key-cfn-invalid-key-rejected|_deploy_stack#2|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Key: appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected|_outputs#16|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Key: appsync.deploy-data-source-resolver-cfn-executes-query|_deploy_stack#6|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in outputs_raw}
Issue-Key: appsync.deploy-graphql-api-cfn-behaves-live-query|_deploy_stack#3|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in desc.get("Outputs", [])}
Issue-Key: appsync.deploy-graphql-api-cfn-schema-active|_deploy_stack#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in outputs_raw}
Issue-Key: appsync__cognito-idp__dynamodb.deploy-appsync-cognito-groups-field-authorization-positive|_deploy_cfn#12|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: appsync__cloudwatch__cognito-idp__dynamodb__logs.cdk-authenticated-graphql-app-workflow|hard#5|AssertionError|raise AssertionError(self._summary(name, _one_line(exc))) from exc
Issue-Key: appsync|CreateStack|ValidationError
Issue-Key: appsync.deploy-cfn-invalid-schema-stack-rolls-back-rejected|run#9|AssertionError|raise AssertionError("expected the invalid-schema stack creation to fail, but it succeeded")
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# CloudFormation deploys of AppSync templates diverge: valueless stack Outputs, unrecognized AppSync resource types, invalid SDL not rolled back

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Three distinct root causes in the CloudFormation path exercised by the AppSync deploy checks. All three are in the emulator's CloudFormation engine (template resource-type registry, output resolution, resource-failure/rollback), reached through AppSync templates. 3 causes in this issue; all three must be closed.

=== Cause 1 of 3: DescribeStacks returns Outputs entries with no OutputValue ===
AWS: Smithy (cloudformation) `Output` structure has members `OutputKey` and `OutputValue` ("The value associated with the output"). For a stack that reports StackStatus=CREATE_COMPLETE, DescribeStacks returns each template Output with both members populated, with intrinsic functions (`Fn::GetAtt` on e.g. AWS::AppSync::GraphQLApi `GraphQLUrl`/`ApiId`, `Ref` on AWS::Cognito::UserPool, etc.) already resolved to concrete values.
Emulator: the stack reaches CREATE_COMPLETE (the checks' deploy helpers assert on the status first and passed it), but the `Outputs` list it returns contains entries that carry `OutputKey` and NO `OutputValue`, so every client that builds `{o["OutputKey"]: o["OutputValue"]}` dies with `KeyError: 'OutputValue'`. The CDK workflow check reports the same thing one layer up: `WORKFLOW FAILED at deploy: 'OutputValue'`.
Why: a client cannot retrieve ANY deployed endpoint/id from a stack, so every downstream data-plane step (query the GraphQL URL, authenticate against the user pool, read the table) is unreachable. A stack that says CREATE_COMPLETE but hands back valueless outputs is worse than a failure: the caller cannot tell the deploy did not really produce resolvable attributes.
Fix: the CloudFormation DescribeStacks output serializer plus the output/intrinsic-resolution step of stack creation — an Output must not be emitted unless its value resolved, and a resource attribute that cannot be resolved must fail the stack rather than produce a half-output.
Traces: appsync.deploy-api-key-cfn-invalid-key-rejected, appsync.deploy-appsync-2-cfn-update-replace-rollback-rejected, appsync.deploy-data-source-resolver-cfn-executes-query, appsync.deploy-graphql-api-cfn-behaves-live-query, appsync.deploy-graphql-api-cfn-schema-active, appsync__cognito-idp__dynamodb.deploy-appsync-cognito-groups-field-authorization-positive, appsync__cloudwatch__cognito-idp__dynamodb__logs.cdk-authenticated-graphql-app-workflow.

=== Cause 2 of 3: CreateStack rejects supported AWS::AppSync::* resource types as unrecognized ===
AWS: recorded behavior only (the CFN resource registry is not part of the Smithy model). Real CloudFormation accepts templates containing `AWS::AppSync::SourceApiAssociation`, `AWS::AppSync::Api` and `AWS::AppSync::ChannelNamespace` and provisions them; the checks' whole point is the behavior AFTER they are provisioned (merged-API auto/manual merge, Event API channel namespaces).
Emulator: CreateStack fails the template outright with HTTP 400 `ValidationError`, message of the recorded form `Template format error: Unrecognized resource types: [AWS::AppSync::SourceApiAssociation]` (and `[AWS::AppSync::Api, AWS::AppSync::ChannelNamespace]` for the Event API template). The error code/status are the requirement; the message text is quoted as an example.
Why: any infrastructure-as-code deploy that includes a merged-API association or an Event API is impossible against the emulator; the resource types are simply absent from the template registry even though the underlying AppSync control-plane concepts exist.
Fix: the CloudFormation template resource-type registry and the per-type provisioning handlers for these three AWS::AppSync::* types.
Traces: appsync.deploy-appsync-3-cfn-association-to-non-merged-api-rejected, appsync.deploy-appsync-3-cfn-merged-api-auto-merge-serves-source-update, appsync.deploy-appsync-3-cfn-merged-api-manual-merge-withholds-update-until-merge, appsync.deploy-appsync-3-cfn-merged-endpoint-source-api-key-rejected, appsync.deploy-appsync-3-cfn-merged-role-without-source-graphql-denied, appsync.deploy-channel-namespace-cfn-event-api-functional.

=== Cause 3 of 3: a stack whose AWS::AppSync::GraphQLSchema definition is invalid SDL reaches CREATE_COMPLETE ===
AWS: recorded behavior only. Schema creation for an AWS::AppSync::GraphQLSchema whose `Definition` is not valid GraphQL SDL fails, the failing resource fails the stack, and CloudFormation rolls the stack back to StackStatus=ROLLBACK_COMPLETE, leaving no usable GraphqlApi behind (a subsequent GetGraphqlApi on the rolled-back api id answers NotFoundException).
Emulator: the stack with the definition `this is not a valid graphql schema at all !!!` reaches CREATE_COMPLETE — the create waiter succeeds instead of erroring. No SDL validation happens and no rollback is triggered.
Why: this is an overly-permissive boundary. A broken schema deploys "successfully", so a client believes it has a working API and only discovers at query time that the schema is garbage; CI gates built on stack status never fire.
Fix: the AWS::AppSync::GraphQLSchema provisioning handler must validate the SDL and report resource-create failure, and the CloudFormation stack engine must then roll the stack back (ROLLBACK_COMPLETE) and delete the already-created AppSync resources.
Traces: appsync.deploy-cfn-invalid-schema-stack-rolls-back-rejected.

## Evidence
Cause 1 — expected: DescribeStacks on a CREATE_COMPLETE stack returns Outputs entries with both OutputKey and OutputValue. Actual: `outputs = {o["OutputKey"]: o["OutputValue"] for o in outputs_raw}` -> `KeyError: 'OutputValue'` (appsync.deploy-graphql-api-cfn-schema-active line 80; identical in 5 sibling checks; the CDK workflow check reports `AssertionError: LEGS 0/10 PASS | deploy=FAIL / WORKFLOW FAILED at deploy: 'OutputValue'`).

Cause 2 — expected: CreateStack accepts the template and provisions the AppSync resources. Actual: `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppSync::SourceApiAssociation]` (5 checks) and `... Unrecognized resource types: [AWS::AppSync::Api, AWS::AppSync::ChannelNamespace]` (appsync.deploy-channel-namespace-cfn-event-api-functional).

Cause 3 — expected: the create waiter fails and DescribeStacks reports ROLLBACK_COMPLETE. Actual: `AssertionError: expected the invalid-schema stack creation to fail, but it succeeded` (check line 34) — the stack reached CREATE_COMPLETE with an invalid SDL definition.

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::AppSync::GraphQLApi + AWS::AppSync::GraphQLSchema and an Output `{"ApiUrl": {"Value": {"Fn::GetAtt": ["GraphQLApi", "GraphQLUrl"]}}}`; wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs — entries come back without OutputValue.
2. Cause 2: CreateStack with a template containing a resource of type AWS::AppSync::SourceApiAssociation (or AWS::AppSync::Api / AWS::AppSync::ChannelNamespace) — CreateStack returns 400 ValidationError 'Unrecognized resource types'.
3. Cause 3: CreateStack with AWS::AppSync::GraphQLApi + AWS::AppSync::GraphQLSchema whose Definition is 'this is not a valid graphql schema at all !!!' — the stack reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
