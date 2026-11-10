Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative|_poll_get#18|Timeout|time.sleep(interval_s)
Issue-Commit: 95197e97fcff53dcc824c7c5147d63782e206b9f

# apigateway__cloudfront__s3: a CloudFront distribution over an API Gateway REST API custom origin never answers at the edge, so OriginProtocolPolicy https-only/http-only has no observable effect

> _Note: this issue was identified against the code base at commit `95197e97fcff53dcc824c7c5147d63782e206b9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution (CloudFront edge custom-origin fetch)

## Description
One root cause (Cause 1 of 1). This check is not marked ALREADY FILED or OWNED in this replay. The open issue 20261110T153718Z-apigateway-cloudfront-s3-2-net-new-causes-a.md describes the same behaviour family (its Cause 1 names this scenario) but its replay keys are bound to a different failing frame, so this failure survives unowned and is filed here rather than assumed covered; a fixer should treat the two as one defect if they close together.

Cause 1 of 1: a Deployed distribution with two cache behaviors over the same API Gateway REST API — one through an https-only custom origin, one through an http-only custom origin — produces neither the API's response on the https leg nor an origin-connect failure on the http leg; the edge answers nothing either way.
AWS: recorded behavior only — OriginProtocolPolicy's effect is not a modelled wire rule. A custom origin whose OriginProtocolPolicy is "https-only" connects to the execute-api domain over TLS, so the edge relays the integration's response: HTTP 200 with the Lambda-proxy JSON body (the check requires the body's origin_label member to equal the label it configured). A custom origin whose policy is "http-only" connects over plaintext HTTP, which API Gateway never serves; the origin request therefore never yields an HTTP response and CloudFront relays its own 502 (Bad Gateway) or 504 (Gateway Timeout) to the viewer, with no origin bytes in the body.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. Its positive control polls the https-only path for up to 240 s at 10 s intervals for a 200 whose JSON carries the configured origin_label, and the negative leg polls the http-only path for up to 90 s at 5 s intervals for a status in (502, 504); neither predicate is ever satisfied inside the run budget, so no status, header or body is ever compared.
Why: a client cannot put CloudFront in front of an API Gateway REST API at all — the most common edge-plus-API topology serves nothing. And the one behaviour OriginProtocolPolicy exists to express (a downgraded origin connection failing closed instead of silently working) is unobservable, so a misconfigured origin is indistinguishable from a correct one and an edge that is simply dead is indistinguishable from an edge correctly enforcing the protocol.
Fix: the CloudFront edge data plane's custom-origin request path — it must serve the distribution's published DomainName, honour each origin's OriginProtocolPolicy when connecting (TLS for https-only, plaintext for http-only), relay the origin's response for the https-only behavior, and surface a refused or stalled plaintext origin connection to the viewer as 502/504 with no origin body.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative.

## Evidence
Replay of suite simparity-behavioral-aws-v1, heading apigateway__cloudfront__s3 (34/60, 26 failed).

[mismatch] apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative
  Error: Check timed out after 60.0s

The check creates an IAM role and a Lambda echo function, builds a Lambda-proxy REST API and deploys a stage, then CreateDistribution with two custom origins on the same execute-api domain and OriginPath "/<stage>" — origin "ok-origin" with OriginProtocolPolicy https-only as the default behavior, origin "broken-origin" with http-only bound to cache-behavior path pattern "broken/*" — and waits for the distribution to report Deployed.
Expected (positive control, same distribution and instant): GET <edge>/probe?cb=<n> answers HTTP 200 and json.loads(body)["origin_label"] equals the configured label. Expected (negative leg): GET <edge>/broken/probe?cb=<n> answers 502 or 504 and the body contains no "origin_label".
Actual: neither predicate is ever met; the harness kills the check at its 60 s budget before any comparison. Distribution ids, edge domains, function/role/API names and the cache-buster query values are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Create an IAM role for Lambda, create a Python echo function returning {statusCode: 200, headers: {Content-Type: application/json}, body: json.dumps({origin_label: $ORIGIN_LABEL, ...})}, and build a REST API with an AWS_PROXY ANY /{proxy+} integration to it; deploy a stage.
3. cloudfront.CreateDistribution with two CustomOriginConfig origins on <api-id>.execute-api.<region>.amazonaws.com with OriginPath "/<stage>": one with OriginProtocolPolicy "https-only" (default cache behavior) and one with "http-only" bound to path pattern "broken/*"; wait until the distribution reports Deployed.
4. GET https://<distribution domain>/probe?cb=<unique>: AWS answers 200 with the integration's JSON (origin_label present); the emulator never answers.
5. GET https://<distribution domain>/broken/probe?cb=<unique>: AWS answers 502 or 504 with no origin bytes; the emulator never answers.
6. Re-replay the check id in trace_ids to confirm the fix.
