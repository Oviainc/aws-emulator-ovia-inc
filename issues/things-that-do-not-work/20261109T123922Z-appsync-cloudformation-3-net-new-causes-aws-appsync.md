Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudformation.deploy-appsync-apicache-cfn-invalid-ttl-rejected appsync__cloudformation.deploy-appsync-apicache-cfn-round-trip-and-delete appsync__cloudformation.deploy-appsync-source-api-association-cfn-ref-and-attributes appsync__cloudformation.deploy-appsync-source-api-association-invalid-merge-type-rejected appsync__cloudformation.deploy-appsync-type-cfn-added-and-removed-in-place appsync__cloudformation.sam-graphqlapi-missing-auth-rejected appsync__cloudformation.sam-graphqlapi-round-trip-and-removed-on-delete
Issue-Key: appsync__cloudformation|CreateStack|ValidationError
Issue-Key: appsync__cloudformation.sam-graphqlapi-missing-auth-rejected|run#7|AssertionError|assert status is None, f"the refused change set must not be executed; stack ended {status!r}"
Issue-Key: appsync__cloudformation.sam-graphqlapi-round-trip-and-removed-on-delete|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# appsync__cloudformation: 3 net-new causes — AWS::AppSync::{ApiCache,SourceApiAssociation,Type} are unregistered resource types, a refused SAM change set is executed anyway, and stack Outputs carry no OutputValue

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three independent causes behind the 7 failing appsync__cloudformation checks.

Cause 1 of 3: AWS::AppSync::ApiCache, AWS::AppSync::SourceApiAssociation and AWS::AppSync::Type are missing from the CloudFormation template resource-type registry.
AWS: recorded behavior only — CreateStack accepts these types, provisions them through the AppSync control plane and reaches CREATE_COMPLETE; an ApiCache with an invalid TTL or a SourceApiAssociation with an out-of-enum merge type instead rolls the stack back with AppSync's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AppSync::ApiCache]` / `[AWS::AppSync::SourceApiAssociation]` / `[AWS::AppSync::Type]`. No cache, source-API association or schema type is created, so none of the five round-trip and refusal legs is reached.
Why: AppSync caching, merged-API associations and template-declared schema types are completely undeployable by template.
Fix: register the three types in the CloudFormation template resource-type registry and add a resource handler per type over the appsync crate (including the ApiCache TTL range and the SourceApiAssociation merge-type enum on create).
Traces: appsync__cloudformation.deploy-appsync-apicache-cfn-invalid-ttl-rejected, appsync__cloudformation.deploy-appsync-apicache-cfn-round-trip-and-delete, appsync__cloudformation.deploy-appsync-source-api-association-cfn-ref-and-attributes, appsync__cloudformation.deploy-appsync-source-api-association-invalid-merge-type-rejected, appsync__cloudformation.deploy-appsync-type-cfn-added-and-removed-in-place

Cause 2 of 3: a change set that CloudFormation refused is executed anyway, so an invalid SAM GraphQLApi reaches CREATE_COMPLETE.
AWS: recorded behavior only — a SAM AWS::Serverless::GraphQLApi with no auth configuration produces a change set that cannot be executed; the stack does not reach CREATE_COMPLETE.
Emulator: the refused change set is still executed and the stack settles 'CREATE_COMPLETE'.
Why: an invalid template is deployed. A client relying on CloudFormation to gate a refused change set gets a stack it believes is valid, and the authoring error is never surfaced.
Fix: the ExecuteChangeSet gate in the change-set state machine — a change set in a non-executable status must not drive a stack transition.
Traces: appsync__cloudformation.sam-graphqlapi-missing-auth-rejected

Cause 3 of 3: DescribeStacks returns Outputs entries with no OutputValue.
AWS: recorded behavior only — each entry of a CREATE_COMPLETE stack's Outputs carries OutputKey AND the resolved OutputValue.
Emulator: the entry has no OutputValue key at all, so reading it raises KeyError: 'OutputValue'.
Why: every consumer of stack outputs (cdk, CLI `--query 'Stacks[0].Outputs'`, cross-stack wiring) gets nothing to read and cannot resolve the value the stack published. Related open issues report the same shape elsewhere (20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261109T113512Z-cloudformation-ec2-network-firewall-describestacks-returns-a-create.md); neither covers this check's replay key.
Fix: the Outputs serializer in the DescribeStacks handler, and the output-resolution step that must populate it once the owning resources exist.
Traces: appsync__cloudformation.sam-graphqlapi-round-trip-and-removed-on-delete

## Evidence
Cause 1 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppSync::ApiCache] / [AWS::AppSync::SourceApiAssociation] / [AWS::AppSync::Type]. Expected: 200 + StackId, CREATE_COMPLETE (ROLLBACK_COMPLETE with AppSync's refusal for the invalid-TTL and invalid-merge-type legs).

Cause 2 — AssertionError: the refused change set must not be executed; stack ended 'CREATE_COMPLETE'. Expected: the change set stays unexecuted and the stack never reaches CREATE_COMPLETE.

Cause 3 — KeyError: 'OutputValue' raised while reading DescribeStacks()['Stacks'][0]['Outputs'][*]. Expected: every Outputs entry carries both OutputKey and OutputValue.

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::AppSync::ApiCache, AWS::AppSync::SourceApiAssociation or AWS::AppSync::Type; observe 400 ValidationError "Unrecognized resource types".
2. Cause 2: deploy a SAM AWS::Serverless::GraphQLApi with no auth configuration via a change set; observe the refused change set being executed and the stack reaching CREATE_COMPLETE.
3. Cause 3: deploy a SAM GraphQLApi stack with an Outputs block and call DescribeStacks; observe Outputs entries without OutputValue.
4. Re-replay all 7 checks in trace_ids.
