Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters
Issue-Key: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters|run#10|AssertionError|assert status == 200, f"expected 200 for {bucket1}/alpha.txt, got {status} body={body!r}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__iam__s3: an S3 proxy integration with renamed path parameters is refused 403 execute-api:Invoke for an anonymous caller on a NONE-authorized method

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: a request to a method whose authorizationType is NONE is refused by the execute-api IAM authorizer instead of reaching the AWS-service (S3) proxy integration.
AWS: an AWS-service proxy integration to S3 whose requestParameters rename path parameters (e.g. integration.request.path.bucket from method.request.path.folder and integration.request.path.key from method.request.path.item) forwards the request to S3 under the integration's credentials role and answers the object with HTTP 200. A method with authorizationType NONE admits an unsigned caller; no execute-api:Invoke identity check applies.
Emulator: the stage answers HTTP 403 with {"Message":"User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:<region>:<account>:<apiId>/<stage>/GET/<bucket>/<key>"} — the IAM denial — so the renamed path parameters are never applied and S3 is never called.
Why: an unauthenticated (authorizationType NONE) REST API cannot be used at all, and the S3 proxy integration's path-parameter renaming — the thing under test — is unreachable. The caller is told it lacks a permission that AWS never requires for this method.
Fix: the execute-api invoke path's authorization stage must skip the execute-api:Invoke identity check when the matched method's authorizationType is NONE, so the AWS-service proxy integration (and its requestParameters path mapping) runs.
Traces: apigateway__iam__s3.s3-proxy-integration-renamed-path-parameters

## Evidence
"AssertionError: expected 200 for <bucket>/alpha.txt, got 403 body=b'{\"Message\":\"User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:us-west-2:000000000000:<apiId>/test/GET/<bucket>/alpha.txt\"}'". Expected: 200 with the S3 object's bytes. The bucket name, api id and account in the message are request/state values; the defect is that the denial happens at all.

## Steps to Reproduce
1. Create an S3 bucket with an object, and an IAM role API Gateway can assume with s3:GetObject on it.
2. Create a REST API with GET /{folder}/{item}, authorizationType NONE, and PutIntegration type AWS to S3 with credentials=<role> and requestParameters mapping integration.request.path.bucket<-method.request.path.folder and integration.request.path.key<-method.request.path.item.
3. Deploy and GET the stage URL unsigned.
Expected: 200 with the object. Actual: 403 execute-api:Invoke denial for "anonymous".
