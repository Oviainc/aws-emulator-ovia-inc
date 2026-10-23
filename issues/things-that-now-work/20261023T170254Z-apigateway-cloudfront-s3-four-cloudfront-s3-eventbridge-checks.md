Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive
Issue-Key: apigateway__cloudfront__s3|TagResource|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__cloudfront__s3: four CloudFront/S3-EventBridge checks abort because TagResource answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource is a scaffold stub, so four cross-service checks that tag a resource during setup never reach the behaviour they exist to verify.
AWS: TagResource is modeled on this surface as a normal tagging operation that returns an empty success body (HTTP 200/204 depending on the service) and whose modeled failures are client 4xx errors (BadRequestException/NotFoundException class for an unknown resource ARN, UnauthorizedException/TooManyRequestsException otherwise). Real AWS accepts a tag write against an existing resource and the subsequent read reflects it.
Emulator: every such call answers 501 with error code NotImplemented ('operation TagResource is scaffolded but not implemented in this emulator'), aborting the check at setup.
Why: any client or IaC flow that tags a CloudFront/API Gateway/S3-EventBridge resource as part of provisioning fails with an unmodeled 501 instead of succeeding (or failing with a modeled 4xx). Everything downstream of the tag — origin-protocol negative behaviour, the EventBridge rule-target permission negative, the wrong-bucket-ARN-pattern negative, and the S3 ObjectCreated to CloudFront invalidation positive — is therefore untested and unusable.
Fix: implement the TagResource handler for the service reached in these flows (the `src/ops/tag_resource.rs` scaffold of the service that owns the tagged ARN) so it persists tags into resource state and answers the modeled empty success.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative, apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative, apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative, apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive.

## Evidence
Expected: TagResource succeeds (empty modeled success body) so the check can proceed.
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator (report lines 38819, 41308, 41718, 42127). HTTP status 501, error code NotImplemented — neither the modeled success nor any modeled client error.

## Steps to Reproduce
1. Stand up the emulator and run any of the four listed checks; each performs a TagResource call during setup.
2. Equivalent manual repro: create the resource the flow uses, then call tag-resource with a valid ARN and one tag.
3. Expect a modeled empty success; observe HTTP 501 with code NotImplemented.
Re-replay the four check ids in trace_ids.
