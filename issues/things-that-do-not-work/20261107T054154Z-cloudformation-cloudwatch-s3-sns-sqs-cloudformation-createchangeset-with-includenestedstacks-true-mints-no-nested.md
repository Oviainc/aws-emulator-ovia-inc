Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes
Issue-Key: cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes|run#24|AssertionError|assert len(nested_ids) == 1, (
Issue-Commit: dcaafa445ca563c5947bd284ba4541134d1f4f9e

# cloudformation: CreateChangeSet with IncludeNestedStacks=True mints no nested change set — the child stack's ResourceChange carries no ChangeSetId, so nothing inside the child can be previewed

> _Note: this issue was identified against the code base at commit `dcaafa445ca563c5947bd284ba4541134d1f4f9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: a parent UPDATE change set created with IncludeNestedStacks=True does not produce a change set for the nested stack, and the parent's change for the nested AWS::CloudFormation::Stack resource omits ResourceChange.ChangeSetId.

AWS: Smithy: CreateChangeSet.IncludeNestedStacks — "Creates a change set for the all nested stacks specified in the template. The default behavior of this action is set to False. To include nested sets in a change set, specify True." Smithy: ResourceChange.ChangeSetId (target ChangeSetId, @length min 1, @pattern ^arn:[-a-zA-Z0-9:/]*$) — "The change set ID of the nested change set." So when a parent change set created with IncludeNestedStacks=True changes a child AWS::CloudFormation::Stack resource, DescribeChangeSet on the parent returns, for that resource, a ResourceChange with ResourceType AWS::CloudFormation::Stack AND a ChangeSetId naming the child's own change set; DescribeChangeSet on that id reports Status CREATE_COMPLETE and Changes naming the child's own modified resources (here LogicalResourceId ChildQueue, Action Modify for a VisibilityTimeout 30 -> 45 property change, no replacement).

Emulator: the parent change set does reach CREATE_COMPLETE and does list one change for the child stack resource (Action Modify, ResourceType AWS::CloudFormation::Stack, Scope ['Properties'], Details on the TemplateURL property), but that ResourceChange has NO ChangeSetId member at all, and consequently no nested change set exists to describe. The check's filter over the parent's Changes for a Stack-typed change carrying a ChangeSetId yields zero entries.

Why: a client that previews an update before executing it (console review, `aws cloudformation deploy --no-execute-changeset`, cdk diff, any change-set gate in CI) sees only "the child stack's TemplateURL changes" and can never learn WHICH resources inside the child change or whether any of them would be replaced — the entire point of IncludeNestedStacks. Code that walks parent Changes and calls DescribeChangeSet on each ResourceChange.ChangeSetId finds nothing to walk, so a destructive child-resource replacement passes a review gate unnoticed.

Fix: the CreateChangeSet handler's nested-stack expansion for IncludeNestedStacks=True (it must create a child change set per changed nested AWS::CloudFormation::Stack resource and record its id in the change-set state), plus the DescribeChangeSet serializer for ResourceChange.ChangeSetId.

Traces: cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes.

## Evidence
Check cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes [mismatch]:

Expected (AWS): exactly one entry in the parent change set's Changes with ResourceChange.ResourceType == 'AWS::CloudFormation::Stack' and a non-empty ResourceChange.ChangeSetId; DescribeChangeSet on that id -> Status CREATE_COMPLETE, Changes == [ChildQueue], Action 'Modify'.

Actual (emulator): zero such entries. AssertionError: "IncludeNestedStacks=True must surface exactly one nested ChangeSetId; the parent's changes were [{'Type': 'Resource', 'ResourceChange': {'Action': 'Modify', 'LogicalResourceId': 'Child', 'PhysicalResourceId': 'arn:aws:cloudformation:<region>:<account>:stack/<parent>-Child-<id>/<uuid>', 'ResourceType': 'AWS::CloudFormation::Stack', 'Replacement': 'False', 'Scope': ['Properties'], 'Details': [{'Target': {'Attribute': 'Properties', 'Name': 'TemplateURL', 'RequiresRecreation': 'Never'}, 'Evaluation': 'Static', 'ChangeSource': 'ParameterReference', 'CausingEntity': 'ChildTemplateUrl'}, {'Target': {'Attribute': 'Properties', 'Name': 'TemplateURL', 'RequiresRecreation': 'Never'}, 'Evaluation': 'Dynamic', 'ChangeSource': 'DirectModification'}]}}]" — note the absent ChangeSetId member (the stack arn/uuid values above are per-run and are not part of the requirement).

Related open issues that do NOT cover this (they do not carry this replay key and describe other nested-stack defects): 20261102T093047Z-cloudformation-sns-sqs-6-net-new-root-causes.md, 20261103T090000Z-cloudformation-generated-template-limit-wording-nested-stacks-collision-reason-cdk-deploys.md.

## Steps to Reproduce
1. git_clone the repo, then run_behavioral_suite and re-replay cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes.
2. Upload two child templates to S3 (child-v1 with SQS VisibilityTimeout 30, child-v2 with 45). CreateStack a parent whose only resource is an AWS::CloudFormation::Stack 'Child' with TemplateURL from a parameter; wait for CREATE_COMPLETE.
3. CreateChangeSet on the parent with ChangeSetType=UPDATE, UsePreviousTemplate=True, IncludeNestedStacks=True, and the ChildTemplateUrl parameter pointed at child-v2; wait for CREATE_COMPLETE.
4. DescribeChangeSet on the parent and inspect the AWS::CloudFormation::Stack change: ResourceChange.ChangeSetId is absent, so there is no nested change set to DescribeChangeSet.
