Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: 51acfe3f2ac52bf94dbabe00a764942f56e02eae

# cloudfront: the unqualified-Lambda@Edge-ARN rejection check never completes — the distribution's edge domain answers nothing, so the post-rejection positive control exhausts the whole budget

> _Note: this issue was identified against the code base at commit `51acfe3f2ac52bf94dbabe00a764942f56e02eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistribution

## Description
This issue carries ONE net-new root cause in the cloudfront service.

Cause 1 of 1: after UpdateDistribution refuses an unqualified Lambda@Edge ARN, the check's positive control — a plain GET through the SAME distribution's DomainName, which must still serve the untouched origin object — never produces a response, so the check is cut off before it can confirm either the refusal's effect-absence or that the distribution still serves.

AWS: Smithy for the control-plane half — UpdateDistribution (PUT /2020-05-31/distribution/{Id}/config) with modelled error InvalidLambdaFunctionAssociation (errorKind client, httpError 400, doc "The specified Lambda@Edge function association is invalid."); LambdaFunctionAssociation.LambdaFunctionARN is @required and documented "You must specify the ARN of a function version; you can't specify an alias or $LATEST." The expected refusal is HTTP 400, code InvalidLambdaFunctionAssociation, message "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: <lambdaFunctionArn>" — everything up to "ARN: " is FIXED text, <lambdaFunctionArn> is the unqualified ARN taken verbatim from the request. The data-plane half is recorded behavior only (CloudFront's edge is not described by the control-plane Smithy model): what the recording requires is that after the rejected update the config still carries LambdaFunctionAssociations.Quantity == 0, the config ETag is UNCHANGED, the distribution reaches Status "Deployed", and a GET of https://<DomainName>/index.txt answers HTTP 200 with the origin object's exact bytes.

Emulator: the check is killed at the 60 s harness cap with no further detail ("Check timed out after 60.0s"), so none of the four post-refusal requirements above is ever observed. Hypothesis: the stall is the final edge GET — that step polls the distribution's DomainName for up to 240 s, which alone exceeds the cap, and the absence of any response from a distribution's edge domain is the same absence already recorded for other cloudfront checks in 20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md (that issue carries only cloudfront.server-side-field-level-encryption-enforced-on-request and cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused, not this check's key, so it does not cover this one); the related 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md likewise does not carry it. What is certain from the replay is only that the operation sequence never completes.

Why: this check is the negative/positive pair that tells a client an invalid Lambda@Edge association was rejected WITHOUT collateral damage — the config unchanged, the ETag unmoved, and the distribution still serving. Against this emulator none of that is confirmable: a client cannot distinguish "the rejection was clean and the edge still serves" from "the rejection corrupted the config" or "the distribution serves nothing", and any CI step that waits on the distribution's own domain hangs until its timeout rather than failing or passing.

Fix: the cloudfront crate's edge/data-plane request path (the distribution DomainName must resolve on the emulator's advertised address and answer requests by routing them to the matching cache behavior's origin), plus the UpdateDistribution handler's Lambda@Edge association validation and its rollback — the refusal must leave the stored DistributionConfig and its ETag byte-identical.

Traces: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected.

## Evidence
Cause 1 — cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected [mismatch]:
  Error: Check timed out after 60.0s

The check, in order: CreateRole trusting lambda.amazonaws.com + edgelambda.amazonaws.com; CreateFunction (nodejs20.x, us-east-1) and wait function_active_v2; PublishVersion (asserts Version == "1" and FunctionArn == "<functionArn>:1"); create an S3 bucket holding index.txt = b"sim2real lambda@edge negative - untouched origin object"; CreateOriginAccessControl; CreateDistribution over the OAC-locked bucket with the managed CachingDisabled policy 4135ea2d-6df8-44a3-9df3-4b5a84be39ad and NO associations; PutBucketPolicy granting cloudfront.amazonaws.com with AWS:SourceArn = the distribution ARN.
Then the assertions that never run to completion:
  - UpdateDistribution with DefaultCacheBehavior.LambdaFunctionAssociations = {Quantity: 1, Items: [{LambdaFunctionARN: <UNQUALIFIED functionArn>, EventType: "viewer-request", IncludeBody: false}]} must be refused with code "InvalidLambdaFunctionAssociation" and message "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: <functionArn>".
  - GetDistributionConfig afterwards must show LambdaFunctionAssociations.Quantity == 0 and the SAME ETag as before the rejected update.
  - GetDistribution must reach Status == "Deployed".
  - GET https://<DomainName>/index.txt must answer 200 with exactly those origin bytes (the check polls this for up to 240 s and tags a failure DATAPLANE_UNIMPLEMENTED).
Actual: the check is terminated at the 60 s cap, so no edge response and no post-refusal read is obtained. The generated bucket name, function name, distribution id and domain name differ per run and are not requirements; the 400/InvalidLambdaFunctionAssociation/message template, the unchanged-config-and-ETag effect, Status "Deployed" and the 200-with-origin-bytes are.

## Steps to Reproduce
1. IAM CreateRole trusting both lambda.amazonaws.com and edgelambda.amazonaws.com; Lambda CreateFunction in us-east-1 (nodejs20.x, handler returns event.Records[0].cf.request); wait until Active; PublishVersion (expect Version "1").
2. S3 CreateBucket and PutObject index.txt with a known body; CloudFront CreateOriginAccessControl; CreateDistribution with one S3 origin using that OAC, DefaultCacheBehavior.CachePolicyId = 4135ea2d-6df8-44a3-9df3-4b5a84be39ad and FunctionAssociations.Quantity = 0; PutBucketPolicy allowing cloudfront.amazonaws.com with AWS:SourceArn = the distribution ARN.
3. GetDistributionConfig, record the ETag. UpdateDistribution with that IfMatch, adding DefaultCacheBehavior.LambdaFunctionAssociations = {Quantity: 1, Items: [{LambdaFunctionARN: "<functionArn>" (UNQUALIFIED — no ":1"), EventType: "viewer-request", IncludeBody: false}]}.
4. Expect HTTP 400, Error.Code "InvalidLambdaFunctionAssociation", Error.Message "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: <functionArn>".
5. GetDistributionConfig again: expect LambdaFunctionAssociations.Quantity == 0 and the ETag unchanged from step 3.
6. Poll GetDistribution until Status == "Deployed", then GET https://<DomainName>/index.txt: expect HTTP 200 and exactly the body written in step 2.
7. Observe: the sequence never completes — step 6's edge GET gets no response from the distribution's domain and the run is cut off at the harness cap, leaving steps 4-6 unverified.

Re-replay cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected to confirm.
