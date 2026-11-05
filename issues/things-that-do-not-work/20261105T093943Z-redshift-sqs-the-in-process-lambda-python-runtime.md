Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative
Issue-Key: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative|run#13|AssertionError|assert "AccessDenied" in error_message and "SendMessage" in error_message, (
Issue-Commit: 57f1a4a9658e7cdb3a8b0e89a3cbf841357a62a9

# redshift__sqs: the in-process Lambda python runtime provides no AWS SDK, so the under-permissioned SQS relay dies with Runtime.ImportModuleError instead of an sqs:SendMessage AccessDenied

> _Note: this issue was identified against the code base at commit `57f1a4a9658e7cdb3a8b0e89a3cbf841357a62a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, grounded in the single surviving failing redshift__sqs check. (Every other redshift__sqs failure in this run is already carried by an open issue: 20261029T193114Z-redshift-sqs-3-net-new-causes-eventbridge-sqs.md, 20261103T130707Z-redshift-sqs-eventbridge-dead-letters-a-delivery-its.md, 20261104T045922Z-redshift-sqs-no-redshift-data-statement-status-change.md, 20261029T082153Z-redshift-sqs-a-data-api-statement-submitted-with.md — none of them covers this import failure.)

Cause 1 of 1: the emulator's managed Python Lambda runtime does not make the AWS SDK for Python (boto3/botocore) importable, so the CloudFormation-deployed EventBridge->Lambda->SQS relay function never reaches its handler body, and the IAM refusal the check exists to observe (sqs:SendMessage AccessDenied) is never produced.

AWS: AWS Lambda's managed Python runtimes ship boto3 and botocore preinstalled on the runtime import path, so a deployment package that bundles neither can still `import boto3` and build a client. Smithy (lambda): Invoke, POST /2015-03-31/functions/{FunctionName}/invocations, input InvocationRequest{FunctionName @httpLabel @required, InvocationType @httpHeader X-Amz-Invocation-Type, Payload @httpPayload}, output InvocationResponse{StatusCode @httpResponseCode (200), FunctionError @httpHeader X-Amz-Function-Error, LogResult, Payload @httpPayload, ExecutedVersion}. An unhandled function/init failure is NOT one of Invoke's 43 modeled errors; it is reported in band as HTTP 200 plus FunctionError. Recorded behavior: the relay handler is `import boto3, json, os` then `boto3.client("sqs").send_message(QueueUrl=os.environ["QUEUE_URL"], MessageBody=json.dumps(event))`. On AWS, invoking the relay whose execution role carries NO sqs:SendMessage grant answers 200 with FunctionError="Unhandled" and a Payload whose errorMessage is the boto3 ClientError text for a denied SendMessage — i.e. an AccessDenied-class message naming the SendMessage operation (fixed text of the form `An error occurred (AccessDenied) when calling the SendMessage operation: Access to the resource <queue arn> is denied.`, with the error code, the operation name and the queue ARN taken from the request and the caller's state), and errorType the Python exception class the SDK raises. The correctly permitted sibling relay in the same stack answers 200 with no FunctionError.

Emulator: Invoke answers 200 with FunctionError="Unhandled" but the Payload is the Python runtime's init-failure document: {"errorMessage": "Unable to import module 'index': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation uuid>", "stackTrace": []}. In that message the fixed text is `Unable to import module '<module>': No module named '<module-name>'`; `index` is the module half of the function's configured Handler and `boto3` is the module the handler asked for — both come from the function's state and its code, and requestId is a per-run value and no part of the requirement. The errorType `Runtime.ImportModuleError` is the Python runtime's init-error class. So the package IS executed in process, but the interpreter's environment is bare: no boto3, no botocore. Consequently no SendMessage is ever attempted, no IAM decision is ever made, and the check cannot distinguish "denied" from "never tried".

Why: calling another AWS service from a Lambda function is the core Lambda pattern, and on the managed Python runtimes the SDK is a runtime-provided dependency nobody vendors into the archive. A client relying on AWS's behavior sees its handler's own result (or its own SDK's AccessDenied) where this emulator returns a runtime import error. Worse, this silently converts a NEGATIVE authorization check into a false pass surface: the under-permissioned relay does fail, but for the wrong reason, and the positive control in the same stack cannot deliver either — so neither "sqs:SendMessage is enforced for a Lambda execution role" nor "a correctly permitted relay delivers exactly one message" is observable at all. This is the same runtime gap already filed for other service pairs (e.g. 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md, 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md); this issue records it for the redshift__sqs pairing, whose check is not carried by any of those.

Fix: the Lambda Invoke execution path's runtime environment — the lambda crate's Invoke handler / in-process execution component that builds the interpreter for a `python3.*` function. The interpreter it starts must have boto3 and botocore importable as runtime-provided modules, in addition to whatever the deployment package contains, as the managed runtime does. Hypothesis (neither the model nor the recording states it, but the check asserts on the effect): the runtime-provided SDK must also be pointed at the emulator's own endpoint and given the function's execution-role credentials through the standard Lambda runtime environment variables, or the import will succeed and every client call will then be directed off-box and/or authorized as the wrong principal — in which case the sqs:SendMessage denial still will not be observed.

Traces: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative.

## Evidence
Failing check: redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative [mismatch]

Expected (AWS): lambda Invoke of the under-permissioned relay -> HTTP 200, X-Amz-Function-Error: Unhandled, Payload = the boto3 ClientError document whose errorMessage names the AccessDenied error code and the SendMessage operation, e.g. errorMessage containing "AccessDenied" and "SendMessage" (fixed text `An error occurred (<code>) when calling the SendMessage operation: <service message>`; the code, operation name and queue ARN come from the request and the caller's authorization state). The correctly permitted sibling relay then delivers exactly one message to QueueGood for a matching PutEvents, and QueueBad receives none.

Actual (emulator): lambda Invoke -> HTTP 200 with X-Amz-Function-Error: Unhandled and Payload:
  {"errorMessage": "Unable to import module 'index': No module named 'boto3'",
   "errorType": "Runtime.ImportModuleError",
   "requestId": "<uuid, per invocation>",
   "stackTrace": []}

The suite surfaces it as:
  File "check_redshift__sqs_cfn_deploy_redshift_sqs_missing_permission_negative.py", line 43, in run
    assert "AccessDenied" in error_message and "SendMessage" in error_message
  AssertionError: expected an sqs:SendMessage AccessDenied, got {'errorMessage': "Unable to import module 'index': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '...', 'stackTrace': []}

Message anatomy of the emulator's divergent payload: fixed text `Unable to import module '<handler module>': No module named '<module>'`; `index` is taken from the function's Handler (`index.handler`), `boto3` is the module the handler imported; requestId is a per-run value and is not part of the requirement. Because the import fails, the function never calls SQS, so the suite's downstream assertions (QueueBad receives nothing for a reason, QueueGood receives exactly one message) are never reached on their intended path.

## Steps to Reproduce
1. CreateStack a template containing: two AWS::SQS::Queue resources (QueueGood, QueueBad); two AWS::IAM::Role resources trusting lambda.amazonaws.com, one granting sqs:SendMessage on QueueGood and one granting NO sqs:SendMessage; two AWS::Lambda::Function resources (Runtime python3.x, Handler index.handler) whose inline code is `import boto3, json, os` and a handler doing `boto3.client("sqs").send_message(QueueUrl=os.environ["QUEUE_URL"], MessageBody=json.dumps(event))`; and two AWS::Events::Rule resources whose EventPattern matches source "sim2real.redshift", detail-type "AWS API Call via CloudTrail" and detail.requestParameters.clusterIdentifier equal to the respective cluster id, targeting the matching function. Export FunctionGoodName, FunctionBadName, QueueGoodUrl, QueueBadUrl as stack Outputs.
2. lambda:Invoke FunctionBadName with InvocationType=RequestResponse and a Redshift/CloudTrail-shaped event payload (detail.eventSource redshift.amazonaws.com, eventName ResumeCluster, requestParameters.clusterIdentifier = the bad cluster id).
3. Read FunctionError and the response Payload. AWS: FunctionError=Unhandled and an errorMessage naming AccessDenied and the SendMessage operation. Emulator: FunctionError=Unhandled and errorMessage "Unable to import module 'index': No module named 'boto3'" with errorType Runtime.ImportModuleError — the SDK import failed before any SQS call.
4. events:PutEvents a matching entry for the bad cluster and poll QueueBadUrl: no message arrives, but for the wrong reason (the relay never ran its body).
5. events:PutEvents a matching entry for the good cluster and poll QueueGoodUrl: AWS delivers exactly one message; the emulator delivers none, because the permitted relay fails at the same import.
6. Replay the behavioral suite and run redshift__sqs.cfn-deploy-redshift-sqs-missing-permission-negative.
