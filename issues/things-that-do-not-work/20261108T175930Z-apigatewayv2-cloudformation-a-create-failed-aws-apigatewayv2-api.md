Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-bodys3location-missing-object-fails-stack
Issue-Key: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-bodys3location-missing-object-fails-stack|run#21|AssertionError|assert parsed is not None, (
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# apigatewayv2__cloudformation: a CREATE_FAILED AWS::ApiGatewayV2::Api carries an empty ResourceStatusReason, so API Gateway V2's BodyS3Location refusal is lost (1 cause)

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
Cause 1 of 1: a stack whose AWS::ApiGatewayV2::Api declares a BodyS3Location naming an object that does not exist correctly fails that resource, but the resource's ResourceStatusReason is empty, so API Gateway V2's own refusal never reaches CloudFormation's resource view.
Failing check ids for this cause: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-bodys3location-missing-object-fails-stack.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): StackResource.ResourceStatusReason is an optional member documented "Success/failure message associated with the resource"; the ApiGatewayV2 side is ImportApi's BadRequestException (client, HTTP 400). Recorded behavior: when the resource provider for AWS::ApiGatewayV2::Api cannot read the declared BodyS3Location, DescribeStackResources reports that resource CREATE_FAILED with a ResourceStatusReason built from CloudFormation's fixed handler wrapper around API Gateway V2's verbatim refusal — `Resource handler returned message: "` + the service's message + `" (RequestToken: <token>, HandlerErrorCode: <code>)`. For this failure class the wrapped message is `Unable to parse BodyS3Location (Service: null; Status Code: 0; Error Code: null; Request ID: null; Proxy: null) (Service: null; Status Code: 404; Error Code: BadRequestException; Request ID: null; Proxy: null)` and the handler error code is `GeneralServiceException`. Everything in that sentence is fixed text except the RequestToken, which is a per-run value.

Emulator: the control plane is right up to the reason — the stack reaches CREATE_FAILED (created with OnFailure=DO_NOTHING), DescribeStackResources lists exactly one S3Api row and it is CREATE_FAILED — but `ResourceStatusReason` is the empty string, so the wrapper, API Gateway V2's message and the HandlerErrorCode are all absent.

Why: the reason is the only place the owning service's rejection survives into CloudFormation's view, so a client cannot tell an unreadable/missing OpenAPI document in S3 from a permissions failure, a malformed body, or any other provisioning failure. Every "deploy, read the failure, fix the template" workflow over BodyS3Location-imported APIs is blind, and the failure cannot be classified from the HandlerErrorCode either.

Fix: failure-reason propagation in the AWS::ApiGatewayV2::Api resource provider and the CloudFormation resource-record writer behind DescribeStackResources (services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stack_resources.rs and the provisioner that stores the per-resource status) — carry the provider's error message verbatim inside CloudFormation's `Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: ...)` wrapper instead of an empty string.

Note: the same empty/absent-reason shape shows up in the plain `cloudformation` section for an SSM-refused resource; that one is filed separately under service `cloudformation` because it is grounded in a different check and a different provider. No open issue carries this check's replay key.

Traces: apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-bodys3location-missing-object-fails-stack.

## Evidence
Check apigatewayv2__cloudformation.deploy-apigatewayv2-cfn-bodys3location-missing-object-fails-stack:
`AssertionError: the S3Api failure reason is not the expected forwarded BodyS3Location refusal: ''`

Expected (AWS): after CreateStack(OnFailure='DO_NOTHING') on a template whose AWS::ApiGatewayV2::Api has Properties.BodyS3Location={Bucket:<existing bucket>, Key:'never-uploaded.json'}, the stack is CREATE_FAILED and DescribeStackResources' S3Api row carries ResourceStatusReason matching
`Resource handler returned message: "Unable to parse BodyS3Location (Service: null; Status Code: 0; Error Code: null; Request ID: null; Proxy: null) (Service: null; Status Code: 404; Error Code: BadRequestException; Request ID: null; Proxy: null)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException)`
Actual (emulator): the stack is CREATE_FAILED and the S3Api row IS CREATE_FAILED (both correct), but ResourceStatusReason is `''`.

Message anatomy: the only variable part of AWS's reason is the RequestToken uuid; the wrapper text, API Gateway V2's `Unable to parse BodyS3Location ...` sentence and `HandlerErrorCode: GeneralServiceException` are fixed for this failure class.

The positive control in the same check (same bucket, a key that DOES exist -> CREATE_COMPLETE, GetRoutes == ['GET /ok'], and the imported HTTP_PROXY route serving the check's own backend token) is never reached because the run stops at the assertion above.

## Steps to Reproduce
1. s3 CreateBucket <bucket> (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. cloudformation CreateStack(OnFailure='DO_NOTHING') on a template with S3Api: AWS::ApiGatewayV2::Api {BodyS3Location: {Bucket: <bucket>, Key: 'never-uploaded.json'}} and S3ApiStage: AWS::ApiGatewayV2::Stage {ApiId: !Ref S3Api, StageName: '$default', AutoDeploy: true}.
3. Poll DescribeStacks to a terminal create state -> CREATE_FAILED (already correct).
4. DescribeStackResources(StackName) and read the S3Api row's ResourceStatusReason.
   Expected: CloudFormation's handler wrapper carrying API Gateway V2's 'Unable to parse BodyS3Location ...' refusal and HandlerErrorCode GeneralServiceException.
   Observed: '' (empty).
5. Control: s3 PutObject the same bucket at 'good.json' with a valid OpenAPI document and deploy the same template against that key -> CREATE_COMPLETE and GetRoutes == ['GET /ok'].
6. Or replay the check id in trace_ids.
