Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__globalaccelerator.cfn-globalaccelerator-listener-invalid-port-range-rejected-and-rolled-back cloudformation__globalaccelerator.deploy-globalaccelerator-accelerator-listener-endpointgroup-lifecycle cloudformation__globalaccelerator.deploy-globalaccelerator-cross-account-attachment-round-trip-and-delete cloudformation__globalaccelerator.deploy-globalaccelerator-cross-account-attachment-invalid-name-rejected
Issue-Key: cloudformation__globalaccelerator|ListListeners|AcceleratorNotFoundException
Issue-Key: cloudformation__globalaccelerator.deploy-globalaccelerator-accelerator-listener-endpointgroup-lifecycle|stack_outputs#8|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__globalaccelerator|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__globalaccelerator: 3 net-new causes — accelerator reported CREATE_COMPLETE is absent from Global Accelerator, Outputs omit OutputValue, and AWS::GlobalAccelerator::CrossAccountAttachment is unregistered

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct root causes. Each is numbered with its own failing check ids; all three must be closed before this issue is done.

Cause 1 of 3: an accelerator a stack reports CREATE_COMPLETE does not exist in Global Accelerator.
AWS: once the stack completes, ListListeners(AcceleratorArn=<the accelerator the stack created>) returns that accelerator's listeners; an ARN Global Accelerator does not know is refused with `AcceleratorNotFoundException` and the message `Accelerator <id> not found.`
Emulator: ListListeners answers `AcceleratorNotFoundException` / `Accelerator <name> not found.` for the accelerator the stack just minted — the name in the message comes from the request, the rest is fixed text.
Why: a client that deploys an accelerator with CloudFormation and then adds or reads listeners through the Global Accelerator API is told the accelerator does not exist.
Fix: the AWS::GlobalAccelerator::Accelerator resource handler must create the accelerator in Global Accelerator state.
Traces: cloudformation__globalaccelerator.cfn-globalaccelerator-listener-invalid-port-range-rejected-and-rolled-back.

Cause 2 of 3: DescribeStacks returns Outputs entries with no OutputValue.
AWS: Smithy: cloudformation#Output carries OutputKey, OutputValue, Description and ExportName; a resolved output always carries OutputValue.
Emulator: the Output entries carry OutputKey only, so reading output["OutputValue"] raises KeyError: 'OutputValue'.
Why: the accelerator/listener/endpoint-group ids the stack publishes cannot be read back, so nothing downstream can be driven.
Fix: the DescribeStacks serializer / stack-output resolution in the CloudFormation state.
Traces: cloudformation__globalaccelerator.deploy-globalaccelerator-accelerator-listener-endpointgroup-lifecycle.

Cause 3 of 3: AWS::GlobalAccelerator::CrossAccountAttachment is not a recognized resource type.
AWS: it is a public CloudFormation resource type, so CreateStack accepts a template declaring it and provisions the attachment; an invalid Name then fails the resource and rolls the stack back.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [AWS::GlobalAccelerator::CrossAccountAttachment]`.
Why: cross-account attachments cannot be deployed through CloudFormation at all.
Fix: register AWS::GlobalAccelerator::CrossAccountAttachment in the CloudFormation resource-type registry and add its resource handler.
Traces: cloudformation__globalaccelerator.deploy-globalaccelerator-cross-account-attachment-round-trip-and-delete, .deploy-globalaccelerator-cross-account-attachment-invalid-name-rejected.

## Evidence
Cause 1: botocore.errorfactory.AcceleratorNotFoundException: An error occurred (AcceleratorNotFoundException) when calling the ListListeners operation: Accelerator sim2real-ga-good-<id> not found. Expected the listeners of the accelerator the stack created.
Cause 2: KeyError: 'OutputValue' while reading DescribeStacks Stacks[0].Outputs.
Cause 3: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::GlobalAccelerator::CrossAccountAttachment].

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: deploy a stack with AWS::GlobalAccelerator::Accelerator and call ListListeners with the accelerator the stack reports.
3. Cause 2: read DescribeStacks(...)['Stacks'][0]['Outputs'][0]['OutputValue'] for that stack.
4. Cause 3: CreateStack with a template declaring AWS::GlobalAccelerator::CrossAccountAttachment.
