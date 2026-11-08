Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: b173a8e674d7ba975def7d68c06d3de96ae43616

# apigateway__cloudfront__s3: a CloudFormation-deployed REST API behind a CloudFront custom origin never serves the API's MOCK response, so the stack's data plane never converges

> _Note: this issue was identified against the code base at commit `b173a8e674d7ba975def7d68c06d3de96ae43616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: a CFN stack wiring AWS::ApiGateway::RestApi/Method/Deployment/Stage behind an AWS::CloudFront::Distribution custom origin deploys, but neither the stage's own invoke URL nor the distribution's edge domain ever answers the method's integration response, so the check never completes.
AWS: after the stack reaches CREATE_COMPLETE, the REST stage's invoke URL answers HTTP 200 with the MOCK integration's response template body ({"message": "hello-from-apigw"} for the recorded template), and a GET of the distribution's DistributionDomainName with that stage as a custom origin proxies the same 200 and the same body once the distribution is Deployed. Recorded behavior only: the Smithy models (apigateway Method/Integration/IntegrationResponse, cloudfront DistributionConfig) describe the configuration surface and say nothing about edge or stage serving; the status and body above are the recorded responses.
Emulator: no response is ever observed on either hop. The check polls the direct invoke URL (up to 60 s) and then the distribution domain (up to 240 s) and is killed by its own budget: "Check timed out after 180.0s". Nothing is served, so the positive control (the API answering directly) cannot be separated from the CloudFront hop.
Why: a client that deploys an API behind CloudFront by template gets a stack that reports success and an endpoint that answers nothing; every request either hangs or fails, and no CDN-fronted API can be exercised end to end.
Fix: the API Gateway stage data plane (the stage invoke-URL router and the MOCK integration/IntegrationResponse evaluator) and the CloudFront custom-origin edge handler that proxies it. Related open issues cover neighbouring CloudFront/API Gateway symptoms but do not carry this replay key: 20261107T185637Z-apigateway-cloudfront-s3-4-net-new-causes-the-execute-api-invoke.md and 20261060951Z-cloudfront-a-deployed-distribution-s-edge-domain-serves.md.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive.

## Evidence
Expected (AWS): GET <stage invoke URL> -> 200 with body {"message": "hello-from-apigw"} (the MOCK IntegrationResponse's ResponseTemplate); GET https://<DistributionDomainName>/ -> the same 200 and the same body. The distribution domain name, API id and stack name in the recording are per-run values.
Actual (emulator): neither URL ever satisfies status == 200 within the polling windows (60 s direct, 240 s via CloudFront); the check is terminated with "Check timed out after 180.0s" and no response is recorded at all.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::ApiGateway::RestApi, an AWS::ApiGateway::Method on the root resource with a MOCK Integration whose IntegrationResponse 200 ResponseTemplate is {"message": "hello-from-apigw"}, an AWS::ApiGateway::Deployment, an AWS::ApiGateway::Stage named prod, and an AWS::CloudFront::Distribution whose only origin is that stage as a custom origin; output DistributionDomainName and ApiId.
2. Wait for CREATE_COMPLETE.
3. GET the stage invoke URL for ApiId/prod. Observed: never 200; expected 200 with the MOCK body.
4. GET https://<DistributionDomainName>/. Observed: never 200; expected the same 200 and body proxied from the origin.
