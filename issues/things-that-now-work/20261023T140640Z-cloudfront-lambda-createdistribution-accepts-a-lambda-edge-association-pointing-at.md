Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected
Issue-Key: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected|_error_of#22|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# CreateDistribution accepts a Lambda@Edge association pointing at an unpublished function version ($LATEST)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a cache behavior whose LambdaFunctionAssociations entry names an unqualified/$LATEST function ARN is accepted instead of refused.
AWS: Smithy: CreateDistribution models InvalidLambdaFunctionAssociation with httpError 400 (alongside InvalidFunctionAssociation and TooManyLambdaFunctionAssociations). Lambda@Edge requires a published, numbered function version — an association carrying the unqualified ARN or the $LATEST qualifier is rejected with a 400 before any distribution is created.
Emulator: the call returns a created distribution (Status InProgress, generated Id/ARN/DomainName) whose DefaultCacheBehavior carries the LambdaFunctionAssociations entry as submitted.
Why: a client gets a distribution that AWS would never create; the configuration error that AWS reports synchronously at create time is instead reported never, and the distribution is believed to have a working edge association.
Fix: LambdaFunctionAssociations validation in the CreateDistribution/UpdateDistribution handlers of the cloudfront crate (require a qualified, published version ARN and emit InvalidLambdaFunctionAssociation otherwise).
Traces: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected.

## Evidence
"AssertionError: expected the call to be rejected, but it succeeded: {'Id': 'E7IYCGPKTANSF3', 'ARN': 'arn:aws:cloudfront::000000000000:distribution/E7IYCGPKTANSF3', 'Status': 'InProgress', ... 'DefaultCacheBehavior': {... 'LambdaFunctionAssociations': {'Quantity': 1, 'Items': [{'LambdaFunctionARN': 'arn...'}]} ...}}" — the submitted association names the function without a published version qualifier.

## Steps to Reproduce
1. CreateFunction a Lambda function and do NOT publish a version.
2. CreateDistribution with DefaultCacheBehavior.LambdaFunctionAssociations={'Quantity':1,'Items':[{'EventType':'viewer-request','LambdaFunctionARN':'<unqualified or :$LATEST ARN>'}]} -> expect 400 InvalidLambdaFunctionAssociation and no distribution; observe 201 with the association stored.
