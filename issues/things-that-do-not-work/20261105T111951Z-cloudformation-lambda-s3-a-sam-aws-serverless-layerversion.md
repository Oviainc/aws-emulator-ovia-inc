Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack
Issue-Key: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack|_invoke#28|AssertionError|assert "FunctionError" not in out, f"{fn} raised: {body[:300]!r}"
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudformation__lambda__s3: a SAM AWS::Serverless::LayerVersion does not serve its function and does not outlive the stack

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishLayerVersion

## Description
Cause 1 of 1 — a LayerVersion declared by a SAM template is not usable by the function that references it, and is not retained after the stack is deleted.
AWS: AWS::Serverless::LayerVersion transforms to AWS::Lambda::LayerVersion; the layer is published, attached to the referencing function, and its content is importable by the handler at /opt. Because AWS::Lambda::LayerVersion carries DeletionPolicy: Retain by default under the SAM transform, deleting the stack leaves the layer version in place and GetLayerVersion still resolves it.
Emulator: recorded behavior only — the check fails in its body where it asserts the function is served by the layer and that the layer survives stack deletion.
Why: a SAM application that ships shared code in a layer deploys but cannot run, and layer versions other stacks still reference disappear with the stack.
Fix: the SAM transform for AWS::Serverless::LayerVersion, the layer-attachment path at invoke time, and the DeletionPolicy handling for AWS::Lambda::LayerVersion in the CloudFormation resource provider.
Traces: cloudformation__lambda__s3.sam-layerversion-serves-function-and-outlives-stack.

## Evidence
Error: File "check_cloudformation__lambda__s3_sam_layerversion_serves_function_and_outlives_stack.py", line 70, in run. Expected: the handler imports the layer's module successfully, and after DeleteStack GetLayerVersion for that layer version still succeeds.

## Steps to Reproduce
CreateStack (with CAPABILITY_AUTO_EXPAND) from a SAM template declaring AWS::Serverless::LayerVersion plus an AWS::Serverless::Function with Layers: [!Ref TheLayer]; Invoke the function and import the layer module; DeleteStack and call GetLayerVersion for the published version.
