Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative
Issue-Key: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative|run#12|AssertionError|assert bad_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# cloudformation__lambda__s3__sqs: 2 net-new root causes — stack Outputs are returned without OutputValue, and a Lambda Code.S3Key that names no object deploys CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct net-new root causes in this service; both must be closed before this issue is submitted.

Cause 1 of 2: DescribeStacks returns Outputs entries that carry no OutputValue.
AWS: Smithy: cloudformation#Output has members OutputKey, OutputValue, Description and ExportName; for a CREATE_COMPLETE stack every declared Output is returned with both OutputKey and its resolved OutputValue (the resolved Ref/GetAtt/Sub string). A client can therefore build {OutputKey: OutputValue} for the whole Outputs list.
Emulator: at least one Outputs element has OutputKey but no OutputValue, so building that map raises KeyError: 'OutputValue'. The stack reports a terminal CREATE_COMPLETE while its outputs are unusable.
Why: stack outputs are the documented hand-off between a deploy and everything that consumes it (queue URLs, function names, bucket names). An output with no value makes every downstream call impossible and makes the stack unusable even though it reports success.
Fix: the CloudFormation DescribeStacks serializer and the stack-output resolution in the stack provisioner — resolve each Output's Value and emit OutputValue.
The open issue 20261029T065950Z-cloudformation-dynamodb-s3-describestacks-returns-stack-outputs-entries.md reports the same defect under a different service token and does not carry this check's replay key.
Traces: cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive (KeyError: 'OutputValue' while reading describe_stacks()["Stacks"][0]["Outputs"]).

Cause 2 of 2: an AWS::Lambda::Function whose Code.S3Key names no object is created instead of failing the stack.
AWS: recorded behavior only for the provider wording; Smithy grounding is lambda#CreateFunction's InvalidParameterValueException for an unreadable code location. A stack whose Code is {S3Bucket: <real bucket>, S3Key: "nonexistent/code.zip"} fails the LambdaFunction resource with exactly one CREATE_FAILED ResourceStatusReason of the fixed form `Resource handler returned message: "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The specified key does not exist. (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` — the fixed text is everything but the two request ids and the attempt count, which are generated per call. The stack then ends ROLLBACK_COMPLETE and the function is absent: GetFunction answers 404 ResourceNotFoundException with message "Function not found: arn:aws:lambda:<region>:<account>:function:<fn>". The identical stack with a real key reaches CREATE_COMPLETE (positive control, which passed).
Emulator: the stack with the nonexistent key reaches CREATE_COMPLETE. The code object is never fetched, so no NoSuchKey failure reason exists, nothing rolls back, and a function with no code is created.
Why: a typo'd or not-yet-uploaded code key deploys green in the emulator and fails in AWS. Worse, the resulting function exists with no deployment package, so later reads and invocations are testing a resource AWS would never have created.
Fix: the CloudFormation AWS::Lambda::Function resource handler (and the Lambda CreateFunction code-location validation it calls) — GetObject the S3Bucket/S3Key before creating the function and surface the NoSuchKey failure as the resource's CREATE_FAILED reason.
Traces: cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative (AssertionError: Expected the stack to fail CREATE and roll back for a non-existent S3 key, got: CREATE_COMPLETE).

## Evidence
Cause 1 — expected: every element of DescribeStacks Stacks[0].Outputs has OutputKey and OutputValue, so {o["OutputKey"]: o["OutputValue"]} is buildable. Actual: KeyError: 'OutputValue' at check_cloudformation__lambda__s3__sqs_cfn_custom_resource_lambda_invocation_positive.py line 159.
Cause 2 — expected: StackStatus ROLLBACK_COMPLETE; exactly one LambdaFunction CREATE_FAILED ResourceStatusReason matching `Resource handler returned message: "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The specified key does not exist. (Service: Lambda, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`; and GetFunction 404 ResourceNotFoundException "Function not found: arn:aws:lambda:<region>:<account>:function:<fn>". Actual: StackStatus CREATE_COMPLETE for the missing-key stack.

## Steps to Reproduce
1) Replay cloudformation__lambda__s3__sqs.cfn-custom-resource-lambda-invocation-positive: CreateStack for the custom-resource template, wait for a terminal status, then DescribeStacks and read Stacks[0].Outputs.
2) Replay cloudformation__lambda__s3__sqs.cfn-invalid-lambda-code-s3-key-negative: create a bucket, put a real zip at code/good.zip, CreateStack(Parameters CodeBucket/CodeKey=code/good.zip, Capabilities=[CAPABILITY_IAM]) — expect CREATE_COMPLETE — then CreateStack the same template with CodeKey=nonexistent/code.zip and poll DescribeStacks plus DescribeStackEvents for the LambdaFunction logical id.
