Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete
Issue-Key: cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 4107fd5c25009167eba32ef42f98137c54de7c7a

# cloudformation__mq: stack Outputs for AWS::AmazonMQ::Configuration come back with no OutputValue, so Ref/Fn::GetAtt on the configuration resolve to nothing

> _Note: this issue was identified against the code base at commit `4107fd5c25009167eba32ef42f98137c54de7c7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
ONE net-new root cause, grounded in one failing behavioral check.

Cause 1 of 1: a stack that declares an AWS::AmazonMQ::Configuration resource reaches CREATE_COMPLETE, but DescribeStacks returns its template Outputs as entries that carry OutputKey and NO OutputValue, i.e. the intrinsics Ref / Fn::GetAtt against that resource resolve to nothing and the stack is serialized anyway. Failing check: cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete.

AWS: Smithy com.amazonaws.cloudformation#Output declares members OutputKey ("The key associated with the output") and OutputValue ("The value associated with the output"); DescribeStacks returns Stacks[].Outputs as a list of those. On a stack in CREATE_COMPLETE every declared template Output appears with BOTH members populated: OutputValue is the resolved string. For AWS::AmazonMQ::Configuration the documented return values are Ref -> the configuration's Id, Fn::GetAtt Arn -> the configuration ARN, Fn::GetAtt Revision -> the latest revision number, Fn::GetAtt Id -> the configuration id. AWS never serves an Output whose value is absent: a GetAtt that names an attribute the resource type does not expose is refused at submit time (400 ValidationError from CreateStack/ValidateTemplate), and a resource that fails to provision drives the stack to CREATE_FAILED/ROLLBACK_COMPLETE instead of CREATE_COMPLETE. The exact host/id/revision values are environment values; the requirement is that each declared Output is present with a resolved OutputValue, consistent with what MQ's own DescribeConfiguration then reports for the same configuration (Id matching Ref, Arn matching GetAtt Arn, LatestRevision.Revision matching GetAtt Revision).
Emulator: the stack is reported CREATE_COMPLETE, and DescribeStacks(StackName=<stack id>) returns Outputs entries for CfgId, CfgArn and CfgRevision with no OutputValue member at all. Replay: KeyError: 'OutputValue' raised while building {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])} immediately after the stack reached CREATE_COMPLETE, so the check never gets to compare the outputs with MQ's DescribeConfiguration / DescribeConfigurationRevision.
Why: a client reads a provisioned resource's identity out of stack outputs (and through exports/nested-stack parameters built on the same Ref/GetAtt evaluation). With the member missing, every SDK and every template consumer gets None/null for an id and ARN the stack claims to have created successfully — the deployment reports success while handing back nothing to connect to, and a stack-driven pipeline that passes the configuration id to a broker fails one step later with an empty identifier. A failure that AWS surfaces loudly (ValidationError at submit, or CREATE_FAILED) is instead reported as a complete stack.
Fix: CloudFormation's stack-output evaluation and the AWS::AmazonMQ::Configuration resource provider in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation/src — the intrinsic-function/Ref/GetAtt resolution used when the stack's Outputs are computed, and the attribute set that resource type publishes: Id/Arn/Revision). An output value that cannot be resolved must make the stack fail (or be refused at CreateStack with ValidationError); the DescribeStacks serializer must never emit an Output entry without OutputValue.
Traces: cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete (the check's other assertions — ListConfigurations/DescribeConfiguration agreement, the base64 XML round-trip, tag propagation and the NotFoundException after the stack delete — are unreached and unverified behind this).

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 12/28 passed; cloudformation__mq 0/3 (0%), 3 failed.

cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete [error]:
  File "check_cloudformation__mq_deploy_amazonmq_configuration_round_trip_and_delete.py", line 214, in run
    outputs = stack_outputs(cfn, stack_id)
  File "...", line 113, in stack_outputs
    return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'

The stack had already passed `assert status == "CREATE_COMPLETE"`, so the emulator declared the AWS::AmazonMQ::Configuration resource provisioned. The template's Outputs are CfgId = {"Ref": "Cfg"}, CfgArn = {"Fn::GetAtt": ["Cfg", "Arn"]}, CfgRevision = {"Fn::GetAtt": ["Cfg", "Revision"]} — expected: three outputs each with a resolved OutputValue (the configuration id, its ARN, its latest revision number) that MQ's DescribeConfiguration then confirms. Actual: Output entries present but with no OutputValue member.

Smithy reference: services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json, shape com.amazonaws.cloudformation#Output {OutputKey, OutputValue, Description, ExportName}.

Not covered by an existing open issue: 20261027T122319Z-lambda-mq-a-cloudformation-stack-declaring-aws-amazonmq.md is about an AWS::AmazonMQ::Broker + Lambda ESM stack never reaching CREATE_COMPLETE (a different resource type and a different symptom — here the stack DOES complete), and 20261109T125120Z-cloudformation-ssm-quicksetup-createstack-refuses-aws-ssmquicksetup-configurationmanager.md is about CreateStack refusing an unrecognized resource type.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from the checkout, and replay the behavioral suite (simparity-behavioral-aws-v1), check cloudformation__mq.deploy-amazonmq-configuration-round-trip-and-delete.
2. CreateStack with a template declaring one resource Cfg of type AWS::AmazonMQ::Configuration (Name=<unique>, EngineType=ACTIVEMQ, EngineVersion=5.18, Data=<base64 of <broker xmlns="http://activemq.apache.org/schema/core" schedulePeriodForDestinationPurge="7"></broker>>, one Tag) and Outputs CfgId={"Ref":"Cfg"}, CfgArn={"Fn::GetAtt":["Cfg","Arn"]}, CfgRevision={"Fn::GetAtt":["Cfg","Revision"]}.
3. Poll DescribeStacks(StackName=<StackId>) until the status is terminal; it reports CREATE_COMPLETE.
4. Read Stacks[0].Outputs. Expect three entries, each with OutputKey AND a resolved OutputValue (configuration id, ARN, latest revision number). The emulator returns entries with no OutputValue, so any {o["OutputKey"]: o["OutputValue"]} read raises KeyError.
5. Cross-check with mq.ListConfigurations/DescribeConfiguration/DescribeConfigurationRevision: the configuration's Id, Arn and LatestRevision.Revision are what the three outputs must carry.
