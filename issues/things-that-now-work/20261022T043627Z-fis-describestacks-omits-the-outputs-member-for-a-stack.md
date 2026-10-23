Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.deploy-experiment-template-cloudformation-then-start fis.deploy-experiment-template-via-cloudformation-starts-experiment
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis.deploy-experiment-template-cloudformation-then-start|run#12|KeyError|outputs = cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]
Issue-Key: fis.deploy-experiment-template-via-cloudformation-starts-experiment|run#18|KeyError|o["OutputValue"] for o in stack["Outputs"] if o["OutputKey"] == "TemplateId"
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# DescribeStacks omits the Outputs member for a stack whose template declares Outputs, breaking every CloudFormation-deployed FIS workflow

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause, surfaced by two FIS deploy checks that provision an AWS::FIS::ExperimentTemplate through CloudFormation and then read the stack's declared output.

Cause 1 of 1: a CREATE_COMPLETE stack whose template declares an Outputs section comes back from DescribeStacks with no Outputs member at all.
AWS: DescribeStacks returns, for each stack, the resolved Outputs list — one entry per output declared in the template, each with OutputKey and OutputValue. The checks deploy a template declaring `Outputs: TemplateId: {Value: !Ref FisTemplate}`, so a CREATE_COMPLETE stack must report Outputs containing OutputKey 'TemplateId' whose OutputValue is the created experiment template's id (the value of !Ref on an AWS::FIS::ExperimentTemplate). Recorded behavior only for the exact Outputs payload; the requirement is that the member is present and carries one entry per declared output, resolved.
Emulator: DescribeStacks(StackName=<stack>) returns a Stacks[0] dict with NO 'Outputs' key — indexing it raises KeyError: 'Outputs' in both checks. Output resolution for the stack's template is absent (and with it the !Ref resolution for the FIS experiment-template resource).
Why: the output of a stack is the only supported way for a client to learn the physical ids of resources CloudFormation created. With Outputs missing, every deploy-then-use workflow breaks at the first read after CREATE_COMPLETE: the client cannot discover the experiment template id it just deployed and cannot start the experiment, even though the stack reports success.
Fix: the DescribeStacks handler/serializer and the stack state that should record resolved template outputs at create time (including !Ref resolution for AWS::FIS::ExperimentTemplate).
Traces: fis.deploy-experiment-template-cloudformation-then-start, fis.deploy-experiment-template-via-cloudformation-starts-experiment

Note on attribution: both failing checks carry the `fis` service token (per the suite's check ids), so this issue is filed under `fis`; the defective response, however, is CloudFormation's DescribeStacks, and the fix belongs in the emulated CloudFormation service.

## Evidence
Check fis.deploy-experiment-template-cloudformation-then-start: `outputs = cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["Outputs"]` -> KeyError: 'Outputs'.
Check fis.deploy-experiment-template-via-cloudformation-starts-experiment: `o["OutputValue"] for o in stack["Outputs"] if o["OutputKey"] == "TemplateId"` -> KeyError: 'Outputs'.
Stack template deployed by both checks declares:
  Outputs:
    TemplateId:
      Value: !Ref FisTemplate
with Resources.FisTemplate of Type AWS::FIS::ExperimentTemplate.
Expected: DescribeStacks returns Stacks[0].Outputs == [{'OutputKey': 'TemplateId', 'OutputValue': '<experiment template id>'}] once the stack is CREATE_COMPLETE. Actual: the Outputs member is absent from the response entirely.

## Steps to Reproduce
1. Boot the emulator; create an IAM role trusting fis.amazonaws.com.
2. With a botocore cloudformation client, create_stack a template containing an AWS::FIS::ExperimentTemplate resource (RoleArn parameter, StopConditions [{Source: none}], Targets {}, Actions {waitAction: {ActionId: aws:fis:wait, Parameters: {duration: PT1M}}}) and an Outputs section `TemplateId: {Value: !Ref FisTemplate}`.
3. Wait for the stack to reach CREATE_COMPLETE.
4. Call describe_stacks(StackName=<stack>) and read Stacks[0]['Outputs'] — the key is missing (KeyError) instead of listing OutputKey 'TemplateId'.
Equivalently, re-replay the two checks in trace_ids.
