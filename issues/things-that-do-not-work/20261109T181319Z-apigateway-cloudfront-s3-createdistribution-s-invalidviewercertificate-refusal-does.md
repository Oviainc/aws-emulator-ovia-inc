Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative
Issue-Key: apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative|run#17|AssertionError|assert error == (
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# apigateway__cloudfront__s3: CreateDistribution's InvalidViewerCertificate refusal does not use AWS's message for an alias with only the default certificate

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: CreateDistribution refuses an alias-with-default-certificate configuration with the right error code but an emulator-authored sentence.

AWS: CloudFront refuses CreateDistribution when DistributionConfig.Aliases is non-empty and ViewerCertificate names only CloudFrontDefaultCertificate=true. Smithy: CloudFront error shape InvalidViewerCertificate (client error, HTTP 400, doc "A viewer certificate specified is not valid."). The runtime sentence AWS returns for this class is fixed text with no variable parts: "To add an alternate domain name (CNAME) to a CloudFront distribution, you must attach a trusted certificate that validates your authorization to use the domain name. For more details, see: https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/CNAMEs.html#alternate-domain-names-requirements" (recorded behavior; the model documents only the class, not the sentence).

Emulator: the refusal carries code InvalidViewerCertificate (correct) but the message "This distribution states the alternate domain names [<alias>] and no viewer certificate that covers them: a distribution with an alternate domain name has to name one of ViewerCertificate.ACMCertificateArn or ViewerCertificate.IAMCertificateId, since CloudFront's default certificate validates no domain name of yours". Only the bracketed alias list is request-derived; the rest is emulator-authored prose that AWS never emits.

Why: a client (and the AWS CLI, which prints the message verbatim) that greps or asserts on AWS's documented sentence — or a runbook that points the operator at the CNAMEs.html requirements link — sees neither. The code matches, so error-code-only handling survives; anything reading the message diverges.

Fix: the CreateDistribution handler's viewer-certificate/alias validation in the CloudFront service crate must emit AWS's fixed sentence for the InvalidViewerCertificate class instead of its own description of the rule.

Traces: apigateway__cloudfront__s3.cli-apigw-cloudfront-custom-domain-missing-cert-negative (the check asserts the (code, message) pair; the code matched and the message did not).

## Evidence
Expected (AWS): 400 InvalidViewerCertificate, message exactly "To add an alternate domain name (CNAME) to a CloudFront distribution, you must attach a trusted certificate that validates your authorization to use the domain name. For more details, see: https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/CNAMEs.html#alternate-domain-names-requirements".
Actual (emulator): InvalidViewerCertificate with "This distribution states the alternate domain names [sim2real-cfcert-e3c6ac956a87.com] and no viewer certificate that covers them: a distribution with an alternate domain name has to name one of ViewerCertificate.ACMCertificateArn or ViewerCertificate.IAMCertificateId, since CloudFront's default certificate validates no domain name of yours" (the alias in brackets is the request's Aliases.Items value for that run; the surrounding prose is fixed emulator text).
Check failure: AssertionError: unexpected rejection: ('InvalidViewerCertificate', "This distribution states the alternate domain names [...] ...").

## Steps to Reproduce
1. CreateRestApi to get an execute-api origin domain (any custom origin works).
2. aws cloudfront create-distribution with DistributionConfig containing Aliases={Quantity:1, Items:["<unique-label>.com"]}, ViewerCertificate={CloudFrontDefaultCertificate:true}, one CustomOriginConfig origin and a matching DefaultCacheBehavior.
3. Observe the CLI's non-zero exit and read the error code and message: the code is InvalidViewerCertificate, the message is the emulator's own sentence rather than AWS's CNAME-requirements sentence.
