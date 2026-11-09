Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__cloudformation__lambda.lambda-function-code-zipfile-writeonly-update-hidden-from-cloudcontrol
Issue-Key: cloudcontrol__cloudformation__lambda|GetResource|ResourceNotFoundException
Issue-Commit: bc4720188099afd2bb4f421d1e83533814225f44

# Cloud Control GetResource reports AWS::Lambda::Function NotFound after a write-only ZipFile update

> _Note: this issue was identified against the code base at commit `bc4720188099afd2bb4f421d1e83533814225f44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: GetResource for an existing AWS::Lambda::Function fails with a handler NotFound once the function's Code.ZipFile (a write-only property) has been updated.

AWS: recorded behavior only for the handler text; the Cloud Control Smithy model declares GetResource's errors (ResourceNotFoundException among them) but not the handler message. AWS's Cloud Control resource handler for AWS::Lambda::Function reads the live function back and returns a ResourceDescription whose Properties omit the write-only Code.ZipFile — it does not report the resource as absent. GetResource returns 200 with the resource's identifier and its readable properties; ResourceNotFoundException (404) with HandlerErrorCode NotFound is reserved for an identifier that names no function.

Emulator: answers `ResourceNotFoundException` — `AWS::Lambda::Function Handler returned status FAILED: Resource of type 'AWS::Lambda::Function' with identifier '<function-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)` — for a function that exists and is invocable. The fixed text of that message is AWS's NotFound template; the defect is that it is produced at all for a live resource. (Function name and request token are per-run values.)

Why: Cloud Control is the uniform read path for IaC tooling. A client that updates a function's code and then reads it back to confirm drift/state is told the function no longer exists, so it will re-create or roll back a healthy resource.

Fix: the Cloud Control AWS::Lambda::Function read handler (and the Lambda state it reads) — resolve the function by its primary identifier independently of the write-only Code member, and project the readable properties instead of failing the handler.

Traces: cloudcontrol__cloudformation__lambda.lambda-function-code-zipfile-writeonly-update-hidden-from-cloudcontrol.

## Evidence
Check cloudcontrol__cloudformation__lambda.lambda-function-code-zipfile-writeonly-update-hidden-from-cloudcontrol:
botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetResource operation: AWS::Lambda::Function Handler returned status FAILED: Resource of type 'AWS::Lambda::Function' with identifier '<fn-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)
Expected: GetResource 200 with a ResourceDescription for the live function whose Properties omit the write-only Code.ZipFile.

## Steps to Reproduce
1. Create an AWS::Lambda::Function (CloudFormation or Cloud Control) with inline Code.ZipFile.
2. Update the function's Code.ZipFile.
3. Call cloudcontrol GetResource TypeName='AWS::Lambda::Function', Identifier=<function name>.
4. Observe ResourceNotFoundException / HandlerErrorCode NotFound instead of a 200 ResourceDescription.
