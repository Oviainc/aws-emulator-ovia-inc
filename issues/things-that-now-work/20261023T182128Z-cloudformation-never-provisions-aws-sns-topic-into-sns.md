Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action
Issue-Key: cloudformation__servicecatalog-appregistry|ListTagsForResource|ResourceNotFound
Issue-Commit: 3dc196f76d19116b5d570e1dc438e2b93d0e0d47

# CloudFormation never provisions AWS::SNS::Topic into SNS: a CREATE_COMPLETE stack's Ref'd topic ARN is unknown to SNS and carries no stack tags

> _Note: this issue was identified against the code base at commit `3dc196f76d19116b5d570e1dc438e2b93d0e0d47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — CloudFormation reports a stack CREATE_COMPLETE and publishes an Output for an AWS::SNS::Topic it never actually created in SNS, so the topic ARN the stack hands back is unknown to SNS and none of CloudFormation's stack tags or aws:cloudformation:* tags propagate to it.

AWS: a CreateStack whose template declares `AWS::SNS::Topic` reaches CREATE_COMPLETE only once the topic EXISTS in SNS and is addressable by the physical id the stack publishes (for AWS::SNS::Topic the Ref value is the topic ARN). CloudFormation then applies to that topic the stack-level `Tags` supplied on CreateStack plus the `aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` tags, so `sns:ListTagsForResource` on the Ref'd ARN returns those tags. Smithy (SNS): ListTagsForResource.ResourceArn is a required AmazonResourceName (@length 1..1011) and returns `Tags`; `ResourceNotFoundException` (awsQueryError code `ResourceNotFound`, HTTP 404) is modelled for a resource that does not exist — so answering it here is a correct serialization of a genuinely absent topic, i.e. the divergence is the provisioning, not the error path. The AppRegistry side of the same check is modelled too (Smithy servicecatalog-appregistry: SyncResource POST /sync/{resourceType}/{resource}, output members applicationArn/resourceArn/actionTaken, errors ValidationException 400 / ResourceNotFoundException 404), but it is never reached.

Emulator: the stack created from a template whose only resource is an `AWS::SNS::Topic` (with an `Outputs.TopicArn: !Ref ProbeTopic`) converges to CREATE_COMPLETE and DescribeStacks yields a TopicArn Output, yet `sns:ListTagsForResource` with that ARN fails with HTTP 404 `ResourceNotFound` (recorded message example: "Topic does not exist"). Nothing was created in SNS, so neither the stack's own `Tags` (here `sim2real:managed=true`) nor the `aws:cloudformation:stack-name` / `aws:cloudformation:logical-id` tags exist on any topic. The check aborts on this positive control before it can exercise AppRegistry SyncResource at all, so the whole cross-service association probe (actionTaken NO_ACTION, resourceArn == stack id, no manufactured applicationArn, RESOURCE_TAG_VALUE rejected with ValidationException) is unexercised.

Why: every infrastructure-as-code path through SNS is broken. A client deploys a stack, observes success, takes the published topic ARN, and then every SNS call against it — Publish, Subscribe, SetTopicAttributes, ListTagsForResource — 404s. Because the failure is reported as a legitimate "absent resource", callers cannot distinguish a provisioning bug from a deliberate deletion, and anything that keys off CloudFormation's `aws:cloudformation:*` tags (cost allocation, AppRegistry's `awsApplication` propagation model, tag-based access control) silently sees nothing to key off.

Fix: the CloudFormation stack-resource provisioner — the per-resource-type handler path in the cloudformation crate's template provisioning/state layer must CREATE the topic in the SNS service that owns the type, record the physical id (ARN) SNS minted, apply the stack-level tags plus the aws:cloudformation:* tags to it, and populate the stack Outputs from that id; a resource type with no such handler must drive the stack to its modelled failure state naming the type it could not provision instead of CREATE_COMPLETE over a resource that does not exist. Related open issue: `20261023T000000Z-cloudformation-stacks-report-create-complete-but-provision-no-resources.md` describes the same provisioner gap for S3/IAM/CodePipeline types and cites `services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs::provision`; it does not carry this check's replay key or the AWS::SNS::Topic type, so this issue is filed separately and should be closed together with it.

Traces: cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action (the check id carries a composite `cloudformation__service-catalog-appregistry` token; the defect is attributed to cloudformation because the failing side effect is the stack's provisioning of AWS::SNS::Topic).

## Evidence
Expected: after CreateStack(TemplateBody declaring AWS::SNS::Topic 'ProbeTopic', Tags=[{sim2real:managed,true},{sim2real:prefix,sim2real-xsvc}], Outputs.TopicArn=!Ref ProbeTopic) reaches CREATE_COMPLETE, sns:ListTagsForResource(ResourceArn=<TopicArn output>) returns HTTP 200 with Tags containing sim2real:managed=true, aws:cloudformation:stack-name=<stack name> and aws:cloudformation:logical-id=ProbeTopic.

Actual: sns:ListTagsForResource(ResourceArn=<TopicArn output>) raises HTTP 404 ResourceNotFoundException, Error.Code=ResourceNotFound (recorded message example: "Topic does not exist"):

  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFound) when calling the ListTagsForResource operation: Topic does not exist

The stack itself reported CREATE_COMPLETE and did publish a TopicArn Output, so CloudFormation claims a resource SNS has no record of. Because the check fails on this positive control, the downstream AppRegistry assertions (SyncResource CFN_STACK -> actionTaken=NO_ACTION with resourceArn equal to the stack id and no applicationArn; SyncResource RESOURCE_TAG_VALUE -> ValidationException) were never evaluated.

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation:CreateStack with StackName=<name>, Tags=[{Key:sim2real:managed,Value:true}], Capabilities=[], and a template whose only resource is:
     Resources:
       ProbeTopic:
         Type: AWS::SNS::Topic
         Properties: { TopicName: !Ref TopicName }
     Outputs:
       TopicArn: { Value: !Ref ProbeTopic }
3. Poll cloudformation:DescribeStacks until StackStatus == CREATE_COMPLETE and read the TopicArn Output.
4. Call sns:ListTagsForResource(ResourceArn=<TopicArn>) — observe HTTP 404 Error.Code=ResourceNotFound instead of a 200 whose Tags include sim2real:managed=true, aws:cloudformation:stack-name and aws:cloudformation:logical-id=ProbeTopic.
5. (Blocked by step 4) servicecatalog-appregistry:SyncResource(resourceType=CFN_STACK, resource=<stack name>) and SyncResource(resourceType=RESOURCE_TAG_VALUE, resource=<TopicArn>).
