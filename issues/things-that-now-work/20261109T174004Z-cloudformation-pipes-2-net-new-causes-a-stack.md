Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__pipes.deploy-cloudformation-pipe-missing-target-property-rejected cloudformation__pipes.deploy-cloudformation-pipe-ref-and-getatt-match-the-provisioned-resource
Issue-Key: cloudformation__pipes.deploy-cloudformation-pipe-missing-target-property-rejected|run#9|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__pipes.deploy-cloudformation-pipe-ref-and-getatt-match-the-provisioned-resource|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# cloudformation__pipes: 2 net-new causes — a stack whose AWS::Pipes::Pipe omits the required Target reaches CREATE_COMPLETE, and DescribeStacks Outputs carry no OutputValue

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct defects in the CloudFormation emulator's handling of AWS::Pipes::Pipe stacks. Both are net-new: no open issue lists either replay key.

Cause 1 of 2: a resource whose required property is absent is deployed instead of failing schema validation and rolling the stack back.
AWS: AWS::Pipes::Pipe declares Target as a required property (the Smithy model for Pipes marks CreatePipe input members Name, Source, Target and RoleArn `required`; the CFN resource schema mirrors that). CloudFormation runs resource-schema (PROPERTY_VALIDATION) validation before provisioning: the stack fails with the row (LogicalResourceId=ThePipe, ValidationType=PROPERTY_VALIDATION, Status=FAILED, Action=FAIL, Reason "Required property [Target] not found", at the resource's own path /Resources/<logical-id>/Properties). The fixed text of that reason is "Required property [" + the missing member name + "] not found"; the member name and the /Resources/<logical-id>/Properties path come from the request's template. The stack then ends ROLLBACK_COMPLETE and no pipe exists — DescribePipe for the template's Name answers NotFoundException "Pipe <name> does not exist." (the name is the request's).
Emulator: the same template reaches terminal status CREATE_COMPLETE. The missing required property is never diagnosed and the stack is reported as a successful deployment.
Why: a client's negative deployment test passes when it should fail, and infrastructure that AWS would refuse is reported as live. A template that is invalid in production deploys clean against the emulator, so the defect is only discovered in AWS. Worse, downstream steps then describe a pipe the stack claims to have created.
Fix: the CloudFormation stack-create path's pre-deployment resource-property validation (required-member enforcement against the resource type's schema) and the rollback path that must take the stack to ROLLBACK_COMPLETE and record the PROPERTY_VALIDATION row.
Traces: cloudformation__pipes.deploy-cloudformation-pipe-missing-target-property-rejected (mismatch: "a stack whose Pipe omits the required Target property ended 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE").

Cause 2 of 2: DescribeStacks returns Output entries without OutputValue.
AWS: every entry of Stack.Outputs returned by DescribeStacks for a CREATE_COMPLETE stack carries both OutputKey and OutputValue, with the Value resolved — {"Ref": "ThePipe"} resolves to the pipe's primary identifier (its Name) and {"Fn::GetAtt": ["ThePipe", "Arn"]} / ["ThePipe", "CurrentState"] resolve to the values DescribePipe reports for the same pipe.
Emulator: the stack reaches a terminal state but the Outputs list it returns contains entries with no OutputValue member at all — reading o["OutputValue"] raises KeyError: 'OutputValue'. Nothing about the Ref/GetAtt values can be observed.
Why: any client that reads a stack output — the normal way a deployed resource's identifier is handed to the next step — crashes or sees an absent value, so a deploy-then-use workflow cannot work at all. Ref/GetAtt fidelity for AWS::Pipes::Pipe is also unverifiable.
Fix: the CloudFormation DescribeStacks serializer for Stack.Outputs, and the intrinsic-function resolution (Ref / Fn::GetAtt, including the Pipes resource's Arn and CurrentState attributes) that must populate each OutputValue at provision time.
Traces: cloudformation__pipes.deploy-cloudformation-pipe-ref-and-getatt-match-the-provisioned-resource (error: KeyError: 'OutputValue' while building {o["OutputKey"]: o["OutputValue"]} from Stacks[0].Outputs).

## Evidence
Check deploy-cloudformation-pipe-missing-target-property-rejected: expected terminal stack status ROLLBACK_COMPLETE plus a ThePipe PROPERTY_VALIDATION/FAILED/FAIL row reading "Required property [Target] not found" at /Resources/ThePipe/Properties, and DescribePipe NotFoundException "Pipe <name> does not exist."; got terminal status 'CREATE_COMPLETE'.

Check deploy-cloudformation-pipe-ref-and-getatt-match-the-provisioned-resource: expected DescribeStacks Stacks[0].Outputs to contain PipeRef (= pipe Name), PipeArn, PipeCurrentState, PipeRoleArn, SourceQueueArn, TargetQueueArn each with an OutputValue; got entries lacking the OutputValue member (KeyError: 'OutputValue').

## Steps to Reproduce
1. CreateStack with a template containing AWS::SQS::Queue x2, AWS::IAM::Role and an AWS::Pipes::Pipe whose Properties omit Target (Capabilities=[CAPABILITY_IAM]); poll DescribeStacks(StackName=<StackId>) to a terminal status. Emulator: CREATE_COMPLETE; AWS: ROLLBACK_COMPLETE with the "Required property [Target] not found" PROPERTY_VALIDATION row, and DescribePipe for the template's pipe Name raises NotFoundException.
2. CreateStack with the same template including Target and Outputs {PipeRef: {Ref: ThePipe}, PipeArn: {Fn::GetAtt: [ThePipe, Arn]}, PipeCurrentState: {Fn::GetAtt: [ThePipe, CurrentState]}}; after CREATE_COMPLETE call DescribeStacks and read Stacks[0]["Outputs"]. Each entry is missing OutputValue.
