Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected
Issue-Key: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected|run#24|AssertionError|assert latest == want_latest, (
Issue-Commit: 2882fa01447cddf34bfea2947358b39ec0b8885e

# cloudfront__lambda: the InvalidLambdaFunctionAssociation refusal for a $LATEST Lambda@Edge ARN omits the trailing "ARN: <LambdaFunctionARN>" segment

> _Note: this issue was identified against the code base at commit `2882fa01447cddf34bfea2947358b39ec0b8885e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
One net-new root cause, observed through the cloudfront__lambda cross-service suite.

Cause 1 of 1: a distribution cache behavior that associates a Lambda@Edge function by an ARN qualified with $LATEST (i.e. not a published version) is refused with the right status and code, but the refusal message drops the final " ARN: <the LambdaFunctionARN from the request>" segment AWS appends. Failing check id: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json) models com.amazonaws.cloudfront#InvalidLambdaFunctionAssociation as a client error with httpError 400 and a single Message member; LambdaFunctionAssociation.LambdaFunctionARN is required and documented "You must specify the ARN of a function version; you can't specify an alias or $LATEST." The model does not fix the message text, so this message is recorded behavior only. The recording shows AWS answering HTTP 400, code InvalidLambdaFunctionAssociation, message = the fixed text "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: " followed by the variable part, the LambdaFunctionARN string exactly as the request supplied it — here the function ARN with the ":$LATEST" qualifier, whose region, account id and function name all come from the request.

Emulator: status 400 and code InvalidLambdaFunctionAssociation are correct, but the message is 'The function ARN must reference a specific function version. (The ARN must end with the version number.)' with the " ARN: <LambdaFunctionARN>" tail absent. Replay output: AssertionError: a viewer-request association naming arn:aws:lambda:us-east-1:<account>:function:<fn-name>:$LATEST must be refused with (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: arn:aws:lambda:us-east-1:<account>:function:<fn-name>:$LATEST'), got (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.)').

Why: the echoed ARN is the only part of the response that identifies which association in a multi-behavior DistributionConfig was rejected, and specifically that the caller pointed at $LATEST rather than a published version. Without it, Lambda@Edge deployment tooling cannot attribute the failure to a cache behavior or to the unpublished qualifier, and clients asserting AWS's message text diverge.

Fix: the Lambda@Edge association validation in the CloudFront distribution-config handling (the DistributionConfig validation shared by CreateDistribution and UpdateDistribution under services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/), at the point where InvalidLambdaFunctionAssociation is constructed: append " ARN: " plus the request's LambdaFunctionARN value for the rejected association. This is the same root cause as the companion issue filed for service token `cloudfront` from check cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected; they are filed separately only because an issue carries a single service token. The open issue 20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md covers the absent edge DATA plane for this service pair and does not carry this control-plane message key.

Traces: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected (assertion at check line 281).

## Evidence
Check cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected, verdict mismatch.

Request: UpdateDistribution with a cache behavior whose LambdaFunctionAssociations names the edge function by <function ARN>:$LATEST for EventType viewer-request.

Expected (AWS): (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: ' + the request's LambdaFunctionARN).

Actual (emulator): (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.)').

Replay output:
AssertionError: a viewer-request association naming arn:aws:lambda:us-east-1:000000000000:function:sim2real-xsvc-edge-fn-ddcfd5ed4bf8:$LATEST must be refused with (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: arn:aws:lambda:us-east-1:000000000000:function:sim2real-xsvc-edge-fn-ddcfd5ed4bf8:$LATEST'), got (400, 'InvalidLambdaFunctionAssociation', 'The function ARN must reference a specific function version. (The ARN must end with the version number.)')

Fixed text vs variable parts: fixed = "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: "; variable = the LambdaFunctionARN echoed from the request. The function name, account id and region in the line above are this run's generated values and are not part of the requirement.

## Steps to Reproduce
1. In us-east-1, create an IAM role and a Lambda@Edge-eligible function (no environment variables); wait for it to be Active.
2. Create a CloudFront distribution with a cache behavior using the managed CachingDisabled policy and no Lambda associations.
3. GetDistributionConfig(Id) for the config and ETag.
4. Add to the cache behavior: LambdaFunctionAssociations = {"Quantity": 1, "Items": [{"LambdaFunctionARN": "<function ARN>:$LATEST", "EventType": "viewer-request", "IncludeBody": false}]}.
5. Call UpdateDistribution(Id, IfMatch=<ETag>, DistributionConfig=<modified config>).
6. Observe (status, code, message): status 400 and code InvalidLambdaFunctionAssociation match AWS, but the message lacks the trailing " ARN: <function ARN>:$LATEST".
7. Re-replay cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected to confirm.
