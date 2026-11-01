Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive|_poll_until#5|Timeout|raise AssertionError(
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative|_poll_http_get#19|Timeout|raise AssertionError(f"polling {url} never satisfied the predicate within {timeout_s}s; last={status}: {body!r}")
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative|run#18|Timeout|lam.add_permission(
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative|run#21|Timeout|result = _wait_for_effect(
Issue-Commit: 446e5d40b4aa72232e72e50ee85b856a1859ebd6

# apigateway__cloudfront__s3: three more edge/eventing root causes — the CloudFront edge serves nothing for an API-Gateway or OAC-protected S3 origin, and the S3→EventBridge→Lambda→CreateInvalidation pipeline never delivers

> _Note: this issue was identified against the code base at commit `446e5d40b4aa72232e72e50ee85b856a1859ebd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge GET (distribution DomainName) / CreateInvalidation

## Description
This issue bundles 3 net-new root causes in the apigateway__cloudfront__s3 behavioral service. Each cause below is a separate defect that must be closed before this issue is submitted. None of these failing checks was marked ALREADY FILED or OWNED by the replay report. Open issues that look adjacent but do NOT carry these checks' replay keys: 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md, 20261024T043741Z-apigateway-cloudfront-s3-five-more-cloudfront-api-gateway.md, 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md, 20261024T182555Z-apigateway-cloudfront-s3-a-deployed-rest-api-s.md — they are named here only so a fixer can see the family; they do not cover these checks.

=== Cause 1 of 3: a CFN-deployed REST API behind a CloudFront custom origin answers on neither plane ===
AWS: recorded behavior only — CloudFront's edge data plane and the API Gateway REST stage invoke endpoint are not described by the Smithy model. Real AWS: after the stack reaches CREATE_COMPLETE and GetDistribution reports Distribution.Status == "Deployed" (Smithy: GetDistribution, GET /2020-05-31/distribution/{Id}, output Distribution + ETag), a GET of the stage invoke URL returns 200 with the MOCK integration's modelled body, and a GET of the distribution DomainName is proxied to that custom origin and returns the same 200 and the same body. Both are plain HTTP 200 responses carrying the origin's entity body; no error code is involved.
Emulator: neither plane ever answers. The check polls the direct invoke URL for a 200 (its own budget 60 s) and then the distribution domain for a 200 (budget 240 s) and obtains no 200 from either; the check is cut off at the suite's 60 s per-check cap with no successful response recorded. The asserted body (a JSON object whose "message" member is the integration's response template value) is never produced.
Why: a client that deploys a REST API behind CloudFront and then calls it — the standard "API behind a CDN" topology — gets nothing back. Both the direct stage URL and the CDN domain are dead, so neither the API nor the distribution can be exercised end to end, and no CDN-fronted API integration can be developed against the emulator.
Fix: the CloudFront edge data-plane request path (the listener that serves a distribution's DomainName and performs the origin fetch for a CustomOriginConfig origin) and the API Gateway REST stage invoke data plane that it fronts.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive

=== Cause 2 of 3: an OAC-protected private S3 origin produces neither the 403 AccessDenied nor the post-grant 200 at the edge ===
AWS: recorded behavior only for the edge plane. Real AWS: a distribution whose origin carries an OriginAccessControlId over a private bucket with NO bucket-policy statement granting the cloudfront.amazonaws.com service principal answers the viewer with HTTP 403 and relays S3's error document, whose fixed <Code> text is AccessDenied; the object bytes must not appear in that body. After PutBucketPolicy adds the OAC grant (Principal cloudfront.amazonaws.com, Condition AWS:SourceArn = the distribution ARN), the identical GET answers 200 with the object bytes byte-for-byte. The 403/AccessDenied pair is fixed text; the bucket name, key, distribution ARN and object bytes are values taken from the request and from state.
Emulator: no response of any kind is obtained at the distribution domain. The 403-poll (45 s) never sees a 403, so the AccessDenied assertion is never reached, the positive-control 200 after the policy grant is never reached either, and the check is cut off at the suite's 60 s cap. The emulator therefore neither denies nor serves — the permission boundary is unobservable rather than wrong.
Why: a client relying on AWS's OAC contract cannot tell a correctly locked-down origin from a broken one. The two outcomes AWS distinguishes — 403 AccessDenied with no content leak, and 200 with the exact bytes once the bucket policy grants the OAC — collapse into "no answer", so neither the security guarantee nor the fix can be validated.
Fix: the CloudFront edge data-plane request path for S3OriginConfig origins, including OAC request signing and the propagation of the origin's S3 403/AccessDenied error document to the viewer.
Traces: apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative

=== Cause 3 of 3: the S3 ObjectCreated → EventBridge → Lambda → CreateInvalidation pipeline never produces an invalidation ===
AWS: a bucket with NotificationConfiguration.EventBridgeConfiguration emits an "Object Created" event (source aws.s3, detail-type "Object Created", envelope resources = [arn:aws:s3:::<bucket>], detail.object.key = the written key). An ENABLED rule whose pattern matches, with a Lambda target that carries a lambda:AddPermission grant for Principal events.amazonaws.com scoped to the rule ARN, invokes the function; the function's CreateInvalidation call (Smithy: CreateInvalidation, POST /2020-05-31/distribution/{DistributionId}/invalidation, HTTP 201, output Invalidation; errors AccessDenied 403, BatchTooLarge 413, NoSuchDistribution 404, InvalidArgument 400) then makes ListInvalidations return an entry whose InvalidationBatch.Paths.Items contains "/<key>" and whose Status is InProgress or Completed. Fixed parts: the detail-type and source strings and the Status enum values; variable parts: bucket name, object key, distribution id, rule ARN.
Emulator: no invalidation ever appears for any object written to the bucket. In both checks the bounded negative window (no invalidation must appear for the mis-permissioned target / mismatched-ARN pattern) passes only because nothing is ever delivered, and the run is cut off at the suite's 60 s cap before the positive control — i.e. the leg that must produce the effect (grant the events.amazonaws.com invoke permission, or correct the rule pattern to the real bucket ARN, then see an invalidation for the new key) never produces it inside the budget. The delivery leg is inert, so the permission boundary and the pattern-scoping the checks exist to prove cannot be distinguished from an always-inert pipeline. The sibling family for this same pipeline is described in 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md and 20261023T140640Z-cloudfront-events-3-of-the-4-causes-remain-open.md, which do not carry these two checks.
Why: a client wiring cache invalidation off S3 uploads gets no invalidation at all, and — worse — a deliberately broken wiring (missing invoke permission, pattern scoped to the wrong bucket ARN) is indistinguishable from a correct one, so the emulator silently validates misconfigured infrastructure.
Fix: the EventBridge rule-target delivery path for Lambda targets (and its evaluation of the target's resource-based policy), plus S3's EventBridge notification emission for Object Created, so that the invoked function's CreateInvalidation lands in the distribution's invalidation state.
Traces: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative, apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative

## Evidence
Suite: simparity-behavioral-aws-v1, service apigateway__cloudfront__s3 — 1/60 passed (2%), 59 failed; 55 of those are already filed/owned, these 4 are not.

Cause 1 — [mismatch] apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive
  Expected: direct REST-stage invoke URL -> 200 with the MOCK integration body; then GET https://<distribution-domain>/ -> 200 with the identical body.
  Actual: no 200 from either URL; "Error: Check timed out after 60.0s" (the check's own polls allow 60 s direct + 240 s via CloudFront).

Cause 2 — [mismatch] apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative
  Expected: GET https://<distribution-domain>/<key> -> 403 whose body contains the fixed code AccessDenied and NOT the object bytes; after PutBucketPolicy grants the OAC principal, the same GET -> 200 with the exact object bytes.
  Actual: no response obtained at the edge domain at all; "Error: Check timed out after 60.0s".

Cause 3 — [mismatch] apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative and [mismatch] apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative
  Expected: after granting lambda:InvokeFunction to Principal events.amazonaws.com (resp. after PutRule corrects the pattern's resources to arn:aws:s3:::<bucket>), a PutObject of a new key yields a CloudFront invalidation whose InvalidationBatch.Paths.Items contains "/<key>" and whose Status is InProgress or Completed.
  Actual: no invalidation is ever listed for any key; both checks end "Error: Check timed out after 60.0s" having never observed the pipeline deliver.

## Steps to Reproduce
Cause 1: deploy a CloudFormation stack containing AWS::ApiGateway::RestApi + Method (MOCK integration returning a JSON body) + Deployment + Stage "prod" + AWS::CloudFront::Distribution with a CustomOriginConfig pointing at the stage's host. Wait for CREATE_COMPLETE and for GetDistribution Status == Deployed. GET the stage invoke URL (expect 200 + the integration body) and GET https://<DistributionDomainName>/ (expect the same 200 + body).

Cause 2: create a private bucket (public access fully blocked) and PutObject a key; CreateOriginAccessControl; CreateDistribution with an S3OriginConfig origin carrying that OriginAccessControlId and the managed CachingDisabled cache policy; do NOT add a bucket policy. Wait for Status == Deployed, then GET https://<DomainName>/<key> — expect 403 with AccessDenied in the body and no object bytes. Then PutBucketPolicy granting cloudfront.amazonaws.com with AWS:SourceArn = the distribution ARN and repeat the same GET — expect 200 and the exact object bytes.

Cause 3: create a bucket with NotificationConfiguration {EventBridgeConfiguration:{}}; create a distribution; create an IAM role trusted by lambda.amazonaws.com allowing cloudfront:CreateInvalidation; create a Lambda that calls CreateInvalidation for detail.object.key; PutRule with pattern {"source":["aws.s3"],"detail-type":["Object Created"],"resources":["arn:aws:s3:::<bucket>"]} State=ENABLED; AddPermission on the function for Principal events.amazonaws.com SourceArn=<rule arn>; PutTargets with the function ARN. PutObject a key and poll ListInvalidations for an entry whose Paths.Items contains "/<key>". Repeat with the invoke permission withheld, and with the rule pattern's resources scoped to a different bucket ARN, to confirm the negative and then the corrected positive control.
