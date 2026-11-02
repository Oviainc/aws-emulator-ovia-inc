Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action
Issue-Key: cloudformation__servicecatalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action|run#10|AssertionError|assert topic_tags.get(_MANAGED_TAG_KEY) == "true", topic_tags
Issue-Commit: 41a5a58882085f034e9d1217c58dbee38ea55080

# CloudFormation provisions a stack's AWS::SNS::Topic but applies neither the stack's own Tags nor the aws:cloudformation:* tags to it (empty tag set)

> _Note: this issue was identified against the code base at commit `41a5a58882085f034e9d1217c58dbee38ea55080`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — a CloudFormation stack now really creates its declared AWS::SNS::Topic in SNS (sns:ListTagsForResource for the Ref'd ARN answers 200 instead of 404), but CloudFormation never tags the resource it created: the topic comes back with an EMPTY tag set, carrying neither the stack-level `Tags` supplied on CreateStack nor any of the `aws:cloudformation:*` system tags.

Failing check id: cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action. The check id's leading token is the composite suite token `cloudformation__service-catalog-appregistry`; the defect is attributed to `cloudformation` because the failing side effect is CloudFormation's own provisioning/tagging of the resources a stack declares (servicecatalog-appregistry and sns are never at fault here — appregistry is not even reached).

AWS: CreateStack accepts `Tags` (Smithy cloudformation: CreateStack.Tags -> Tags, list, @length 0..50) and CloudFormation propagates those stack-level tags to every resource it creates whose type supports tagging, in addition to the `aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` tags it adds itself. For an AWS::SNS::Topic the Ref value is the topic ARN, and reading that ARN back with SNS's own API therefore reports those tags (Smithy sns: ListTagsForResource.ResourceArn required AmazonResourceName @length 1..1011, output member `Tags` -> TagList; tagging a topic is sns:TagResource, input ResourceArn + required Tags, errors AuthorizationErrorException 403 / InvalidParameterException 400 / ResourceNotFoundException 404 / TagLimitExceededException 400 / StaleTagException 400 / TagPolicyException 400 / ConcurrentAccessException 400). The `aws:cloudformation:*` key names are AWS-documented behavior for stack-created resources — recorded behavior plus documentation, not a modeled member.

Emulator: after a stack whose only resource is `ProbeTopic: AWS::SNS::Topic` reaches CREATE_COMPLETE and publishes `Outputs.TopicArn: !Ref ProbeTopic`, `sns:ListTagsForResource(ResourceArn=<TopicArn>)` returns HTTP 200 with `Tags: []` — the flattened mapping the check builds is `{}`. The stack was created with `Tags=[{sim2real:managed,true},{sim2real:prefix,sim2real-xsvc}]`, so at minimum `sim2real:managed=true`, `aws:cloudformation:stack-name=<stack name>` and `aws:cloudformation:logical-id=ProbeTopic` are expected and all three are absent. The check aborts on this positive control, so the behavior actually under test — servicecatalog-appregistry SyncResource(resourceType=CFN_STACK) reporting actionTaken NO_ACTION with resourceArn equal to the stack id and NO manufactured applicationArn, idempotently on a second call, and SyncResource(resourceType=RESOURCE_TAG_VALUE) being refused with ValidationException (400) — is never exercised.

Why: a client that keys off CloudFormation's tagging sees nothing to key off. Cost allocation by stack tag, tag-based access control (aws:ResourceTag conditions), stack-tag-driven inventory, and AppRegistry's whole `awsApplication` propagation model all read the tags CloudFormation is supposed to have written onto the provisioned resource. Because the tag set is empty rather than wrong, an absence assertion like this check's cannot distinguish "the association was correctly NOT created" from "this emulator never propagates tags at all", so every downstream claim about tag propagation across the CFN/AppRegistry/SNS boundary is unfalsifiable. It also means the inverse is untestable: no caller can confirm a stack-created resource is attributable to its stack.

Fix: the CloudFormation stack-resource provisioner — the per-type create path in the cloudformation crate's provisioning layer (`crate::state::provisioning` / `crate::state::provisioner`, which calls the owning service's own create API in-process) must, after a successful create, apply to the minted physical resource the stack's stored `Tags` plus the `aws:cloudformation:stack-name` / `aws:cloudformation:stack-id` / `aws:cloudformation:logical-id` tags, using the owning service's tagging operation (for AWS::SNS::Topic that is sns:TagResource on the topic ARN, or the `Tags` member of CreateTopic), and must keep them in step on stack update. The stack's stored tag list must also be readable back on DescribeStacks (the check reads Stack$Tags on the same stack).

Traces: cloudformation__service-catalog-appregistry.appregistry-sync-unassociated-cfn-stack-is-no-action. Related open issue: `20261023T182128Z-cloudformation-never-provisions-aws-sns-topic-into-sns.md` carries this same check id but describes the EARLIER failure mode (ListTagsForResource answering 404 ResourceNotFound because the topic was never created at all, replay key `...|ListTagsForResource|ResourceNotFound`); the topic now exists and the remaining divergence is the tagging, which that issue's key does not cover, so it is filed here and the two should be closed together. Provisioner entry points to inspect: services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs (module doc, lines 1-120, describes the create call per resource type and says nothing about tagging) and services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs (lines 15-20, computes the physical id and Outputs).

## Evidence
Expected (real AWS): after cloudformation:CreateStack(StackName=<name>, Capabilities=[], Tags=[{Key: sim2real:managed, Value: true}, {Key: sim2real:prefix, Value: sim2real-xsvc}], TemplateBody declaring `ProbeTopic: AWS::SNS::Topic` with `Outputs.TopicArn: !Ref ProbeTopic`) reaches CREATE_COMPLETE, sns:ListTagsForResource(ResourceArn=<TopicArn output>) returns HTTP 200 whose Tags include:
  sim2real:managed = true                      (the stack's own tag, propagated)
  aws:cloudformation:stack-name = <stack name>
  aws:cloudformation:logical-id = ProbeTopic
  aws:cloudformation:stack-id   = <stack id>

Actual (emulator): sns:ListTagsForResource(ResourceArn=<TopicArn output>) returns HTTP 200 with an empty Tags list, so the flattened mapping is {}:

  File "check_cloudformation__service_catalog_appregistry_appregistry_sync_unassociated_cfn_stack_is_no_action.py", line 72, in run
    assert topic_tags.get(_MANAGED_TAG_KEY) == "true", topic_tags
  AssertionError: {}

The stack reported CREATE_COMPLETE and did publish the TopicArn Output, and SNS resolves that ARN (no 404), so the topic exists and is simply untagged. Because the check fails on this positive control, the assertions that follow were never evaluated: that the topic and the stack both lack an `awsApplication` tag, that servicecatalog-appregistry:SyncResource(resourceType=CFN_STACK, resource=<stack name>) answers actionTaken=NO_ACTION with resourceArn == the stack id and no applicationArn (twice, idempotently), and that SyncResource(resourceType=RESOURCE_TAG_VALUE, resource=<TopicArn>) is refused with ValidationException (HTTP 400).

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation:CreateStack with StackName=<name>, Capabilities=[], Parameters=[{TopicName,<topic>}], Tags=[{Key: sim2real:managed, Value: true}, {Key: sim2real:prefix, Value: sim2real-xsvc}] and template body:
     Resources:
       ProbeTopic:
         Type: AWS::SNS::Topic
         Properties: { TopicName: !Ref TopicName }
     Outputs:
       TopicArn: { Value: !Ref ProbeTopic }
3. Poll cloudformation:DescribeStacks until StackStatus == CREATE_COMPLETE; read the TopicArn Output.
4. Call sns:ListTagsForResource(ResourceArn=<TopicArn>) — observe HTTP 200 with Tags == [] instead of a tag set containing sim2real:managed=true, aws:cloudformation:stack-name=<name> and aws:cloudformation:logical-id=ProbeTopic.
5. Also read cloudformation:DescribeStacks(StackName=<name>).Stacks[0].Tags and confirm the stack's own sim2real:managed=true tag is reported there.
6. (Blocked by step 4) servicecatalog-appregistry:SyncResource(resourceType=CFN_STACK, resource=<name>) must answer actionTaken=NO_ACTION with resourceArn == the stack id and no applicationArn; SyncResource(resourceType=RESOURCE_TAG_VALUE, resource=<TopicArn>) must be refused with ValidationException (400).
