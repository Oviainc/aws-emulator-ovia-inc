Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected|run#18|AssertionError|assert msg == unqualified_msg, f"expected message {unqualified_msg!r}, got {msg!r}"
Issue-Commit: 2882fa01447cddf34bfea2947358b39ec0b8885e

# cloudfront: the InvalidLambdaFunctionAssociation refusal message drops the trailing "ARN: <LambdaFunctionARN>" segment

> _Note: this issue was identified against the code base at commit `2882fa01447cddf34bfea2947358b39ec0b8885e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
One net-new root cause in the CloudFront distribution-config validator for Lambda@Edge associations.

Cause 1 of 1: CreateDistribution/UpdateDistribution correctly refuse a LambdaFunctionAssociations entry whose LambdaFunctionARN is not qualified by a published version, but the refusal message is truncated: it omits the final " ARN: <the LambdaFunctionARN from the request>" segment that AWS always appends. Failing check id: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected.

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json) models com.amazonaws.cloudfront#InvalidLambdaFunctionAssociation as a client error with httpError 400 and a single Message member, documented "The specified Lambda@Edge function association is invalid."; LambdaFunctionAssociation.LambdaFunctionARN is required and documented "You must specify the ARN of a function version; you can't specify an alias or $LATEST." The model does not fix the message text, so the message contract here is recorded behavior only. The recording shows AWS answering HTTP 400, error code InvalidLambdaFunctionAssociation, with the message built from TWO parts: the fixed text "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: " followed by the variable part — the exact LambdaFunctionARN string the request supplied for that association, echoed verbatim (an unqualified function ARN, or one qualified with $LATEST; account id, region and function name all come from the request, never from a constant).

Emulator: the emulator answers the correct status (400) and the correct code (InvalidLambdaFunctionAssociation), but the message is only the fixed prefix with the variable part and its " ARN: " separator missing: 'The function ARN must reference a specific function version. (The ARN must end with the version number.)'. The replay asserts message equality and fails with: AssertionError: expected message 'The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: arn:aws:lambda:us-east-1:<account>:function:<fn-name>', got 'The function ARN must reference a specific function version. (The ARN must end with the version number.)' (the ARN in that line is this run's generated function ARN — it fills the template, it is not part of it).

Why: a client cannot tell WHICH association in the config was rejected. A DistributionConfig may carry many LambdaFunctionAssociations across DefaultCacheBehavior and every CacheBehavior; the echoed ARN is the only thing in the response identifying the offending entry. Deployment tooling and operators that parse or log this message to point at the bad behavior get an unattributable error, and any test or client asserting on AWS's message text (as these checks do) breaks against the emulator.

Fix: the Lambda@Edge association validation inside the CloudFront distribution-config handling (the shared DistributionConfig validation used by CreateDistribution and UpdateDistribution in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/), where the InvalidLambdaFunctionAssociation error is constructed. The message must be assembled as the fixed sentence plus " ARN: " plus the request's LambdaFunctionARN value for the rejected association, rather than the fixed sentence alone. Note the same defect surfaces under the cloudfront__lambda service token (filed separately, since an issue carries one service); and note that the resolved issue 20261026T092638Z-cloudfront-three-error-contract-divergences-entitynotfound-messages-append.md concerned messages WRONGLY appending an identifier — this is the opposite direction and is not covered by it. No open cloudfront issue carries this replay key.

Traces: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected (assertion at check line 319, against the expectation built at lines 6101-6104 of the replay report).

## Evidence
Check cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected, verdict mismatch.

Request: UpdateDistribution on an existing distribution, with DefaultCacheBehavior.LambdaFunctionAssociations = {Quantity: 1, Items: [{LambdaFunctionARN: <unqualified function ARN>, EventType: "viewer-request", IncludeBody: false}]}, IfMatch = the current ETag. A published version :1 of that function exists.

Expected (AWS): HTTP 400, Error.Code = InvalidLambdaFunctionAssociation, Error.Message = "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: " + the request's LambdaFunctionARN.

Actual (emulator): HTTP 400, Error.Code = InvalidLambdaFunctionAssociation (both correct), Error.Message = "The function ARN must reference a specific function version. (The ARN must end with the version number.)" — the " ARN: <LambdaFunctionARN>" tail is absent.

Replay output:
AssertionError: expected message 'The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: arn:aws:lambda:us-east-1:000000000000:function:sim2real-cfedge-le-d451ea7913d7', got 'The function ARN must reference a specific function version. (The ARN must end with the version number.)'

Fixed text vs variable parts of the required message: fixed = "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: "; variable = the LambdaFunctionARN taken from the request (region, account id, function name, and the absent/"$LATEST" qualifier are all the request's values — the ARN above is this run's generated name and must not be hard-coded).

## Steps to Reproduce
1. Create an IAM role and a Lambda function in us-east-1; wait for it to be Active and PublishVersion it (version "1" exists, so the only defect under test is the unqualified ARN).
2. Create an S3 bucket with one object and a CloudFront distribution over it with an OAC and a default cache behavior that has NO Lambda associations.
3. GetDistributionConfig(Id) to obtain the config and its ETag.
4. Set DefaultCacheBehavior.LambdaFunctionAssociations = {"Quantity": 1, "Items": [{"LambdaFunctionARN": <the UNQUALIFIED function ARN, i.e. with no :<version> suffix>, "EventType": "viewer-request", "IncludeBody": false}]}.
5. Call UpdateDistribution(Id, IfMatch=<that ETag>, DistributionConfig=<the modified config>).
6. Observe the ClientError: status 400 and code InvalidLambdaFunctionAssociation are correct, but Error.Message stops after "...(The ARN must end with the version number.)" instead of continuing " ARN: <the ARN supplied in step 4>".
7. Re-replay cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected to confirm.
