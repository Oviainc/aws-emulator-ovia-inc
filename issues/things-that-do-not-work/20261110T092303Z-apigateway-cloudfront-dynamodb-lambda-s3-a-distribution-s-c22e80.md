Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb
Issue-Key: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected|_poll_until#34|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb|_poll_until#40|AssertionError|raise AssertionError(
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# apigateway__cloudfront__dynamodb__lambda__s3: a distribution's edge answers 200 at / but not the S3 index object's bytes — DefaultRootObject is never fetched, so the SPA shell is never served

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront viewer GET "/" at a distribution edge (S3 origin)

## Description
Cause 1 of 1: a viewer GET of "/" through a Deployed distribution whose DefaultRootObject names the SPA's index.html is answered 200, but the body is not that object.
AWS: Smithy: DistributionConfig.DefaultRootObject — "When a viewer requests the root URL for your distribution, the default root object is the object that you want CloudFront to request from your origin... A default root object avoids exposing the contents of your distribution." With the bucket policy granting s3:GetObject to Principal {"Service": "cloudfront.amazonaws.com"} under Condition StringEquals AWS:SourceArn = the distribution's ARN, GET / must return HTTP 200 whose body is the index object's bytes, byte for byte.
Emulator: GET / is answered 200 but the body is never equal to the seeded index document, for the whole 180 s poll — the viewer path "/" is not resolved to DefaultRootObject before the origin request is built, so the origin answers with something else (in the sibling cloudfront__s3 suite the same defect returns S3's ListObjects XML at /).
Why: the positive control of every SPA-behind-CloudFront workflow fails, so neither the shell nor the API-behind-the-same-distribution contract can be reached; and a client relying on the default root object receives content it never asked for instead of its index page.
Fix: the CloudFront edge request handler that maps a viewer path onto an origin path — substitute DefaultRootObject for a viewer path of "/" before signing and forwarding the origin request.
Traces: apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-api-without-lambda-permission-rejected, apigateway__cloudfront__dynamodb__lambda__s3.e2e-spa-distribution-serves-shell-and-api-writes-dynamodb.

Not covered by the open 20261109T220008Z-apigateway-cloudfront-dynamodb-lambda-s3-a-distribution-s.md, which describes a DomainName the emulator does not serve; here the edge is reached and answers 200 with the wrong body.

## Evidence
Expected: GET / returns 200 and body == the seeded index.html.
Actual: 'AssertionError: timed out waiting for the shell at / after 180s (last: <Response [200]>)' — the edge answers 200 throughout but never with the index object's bytes (the predicate asserted is `r.content == INDEX_HTML`). The same defect in the cloudfront__s3 suite shows what is served instead: an S3 '<ListBucketResult ...>' document.

## Steps to Reproduce
Replay the behavioral suite and re-run the two checks in trace_ids. By hand: create a bucket, put the SPA's index.html at the bucket root, create an origin access control, create a distribution with that bucket's REST endpoint as the origin and DefaultRootObject=index.html, grant s3:GetObject to cloudfront.amazonaws.com with Condition AWS:SourceArn = the distribution ARN, wait for Status=Deployed, then GET / at the distribution domain and compare the body with the uploaded index.html.
