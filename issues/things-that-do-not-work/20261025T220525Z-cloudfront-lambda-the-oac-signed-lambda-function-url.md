Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|_poll_until#36|Timeout|time.sleep(interval_s)
Issue-Commit: 1a78f0cc1d943875c45c11a36243ccaf94b7f40f

# cloudfront__lambda: the OAC-signed Lambda function URL origin path never completes — no response is ever obtained through the distribution's edge domain and the check exhausts the 60 s cap

> _Note: this issue was identified against the code base at commit `1a78f0cc1d943875c45c11a36243ccaf94b7f40f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One net-new root cause, grounded in behavioral replay.

=== Cause 1 of 1: a distribution whose origin is a Lambda function URL fronted by an origin access control of OriginAccessControlOriginType "lambda" never answers at its own domain name, so neither the signed CDN path nor the unsigned-direct-call refusal is observable; the check makes no forward progress and is cut at the harness cap ===

AWS: CloudFront documents the OAC-signed Lambda function URL origin shape that this check exercises: an origin access control created with OriginAccessControlOriginType "lambda", SigningBehavior "always" and SigningProtocol "sigv4", attached to an origin whose DomainName is the host of a Lambda function URL with AuthType AWS_IAM, makes CloudFront sign each origin request with SigV4, so a viewer GET to the distribution's own DomainName is admitted and answers 200 with the function's body, and the function sees CloudFront's own x-amz-cf-id header on the origin request. The documented companion requirement is that the cache behavior use the AWS-managed origin request policy "Managed-AllViewerExceptHostHeader" (forwarding the viewer Host header would invalidate the signature CloudFront computes over the function URL's host), resolved BY NAME out of ListOriginRequestPolicies(Type="managed"), plus lambda:InvokeFunctionUrl and lambda:InvokeFunction resource grants to the cloudfront.amazonaws.com principal scoped to the distribution ARN. The same function URL called directly and unsigned is refused by Lambda with HTTP 403 and leaks none of the function's payload. Hypothesis: the exact 403 body shape and the precise set of CloudFront-added origin headers — this check is marked in-corpus as an unblessed draft with no recorded oracle, so only the status codes and the x-amz-cf-id header name are asserted and the body is left for a blessing pass. What is NOT a hypothesis, and is what this issue is about: AWS serves the distribution's own hostname at all.

Emulator: the check never completes — verdict [mismatch], "Check timed out after 60.0s", with no frame attributed by the harness. The run therefore never reaches any of its four beats (200 through the CDN with the run's marker and x-amz-cf-id; unsigned direct GET -> 403; no marker in the refusal body; CDN path still 200 afterwards). Everything the check does before those beats is emulator-side and bounded by polls far longer than the 60 s cap, so the hang is one of the emulator's: (a) the CDN poll at BEAT 1 (_poll_until on a plain GET to the distribution's DomainName, timeout_s=240, interval 5 s) never sees a 200 because the CloudFront edge data plane serves nothing — the already-filed sibling issues establish exactly this for every other origin type (20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, 20261025T054735Z-cloudfront-lambda-a-deployed-distribution-s-edge-never.md, 20261024T102105Z-cloudfront-kms-s3-a-cloudfront-distribution-with-an-oac-over-an.md, 20261024T182555Z-cloudfront-cloudwatch-no-edge-traffic-is-ever-served.md); (b) the managed origin-request-policy catalog walk (ListOriginRequestPolicies(Type="managed", MaxItems="100") paged on NextMarker) spins instead of terminating, which happens when the emulator returns a NextMarker that never advances — the catalog gap itself is already filed as 20261025T025628Z-cloudfront-lambda-cloudfront-listoriginrequestpolicies-type-managed-serves-an-empty-page.md, which carries THIS check id in its Issue-Trace-Ids but under a different replay key (KeyError at _managed_origin_request_policy_id#24), and the suite no longer attributes this check to that frame, so that issue does not cover the failure reported here; or (c) the Lambda pre-edge gates, i.e. CreateFunction answering InvalidParameterValueException forever (the check retries it for 180 s, treating it as un-propagated role) or GetFunctionConfiguration().State never reaching "Active" (another 180 s poll). All three candidates are the emulator's own surfaces, and all three exceed the 60 s cap on their own; the fixer must bisect them in that order.

Why: this is the modern serverless-behind-CloudFront topology, and a client relying on it gets nothing it can verify. The positive leg is unverifiable (no request ever reaches the customer's function through the distribution, so the origin's code never runs and CloudFront's signature is never produced), and the negative leg is worse: the whole point of OriginAccessControlOriginType "lambda" with an AWS_IAM function URL is that ONLY CloudFront can invoke the function. In this emulator nothing demonstrates that boundary — a configuration AWS treats as a security control cannot be shown to deny an unsigned direct call, so an operator who validates the topology against the emulator ships a function URL whose access posture was never exercised. Separately, a run that hangs rather than refusing costs every consumer a wall-clock stall instead of an error.

Fix: the CloudFront distribution data plane — the edge listener that resolves a distribution DomainName to its DistributionConfig, selects the cache behavior, applies the cache/origin-request policies, and performs the origin fetch, including the SigV4 signing path for an origin access control whose OriginAccessControlOriginType is "lambda" against a Lambda function URL origin (CustomOriginConfig, https-only), adding the CloudFront-generated origin headers (x-amz-cf-id); plus, as pre-edge gates on the same path, ListOriginRequestPolicies' managed-catalog pagination in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/list_origin_request_policies.rs (a NextMarker must either advance or be omitted), and the Lambda function-lifecycle surface the check waits on (CreateFunction acceptance of a freshly created execution role, and GetFunctionConfiguration State reaching "Active"). The Lambda function-URL AuthType AWS_IAM enforcement that must refuse the unsigned direct call belongs in the Lambda function-URL data plane, not in CloudFront.

Traces: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied.

## Evidence
Cause 1 — check cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied [mismatch]

  Error: Check timed out after 60.0s

(no frame is attributed by the harness for a cap exhaustion, so the hang point is reported as a timeout rather than an exception)

Expected (per the check's own four beats):
  1. A plain GET to the distribution's DomainName -> HTTP 200, body is JSON whose "marker" equals the per-run marker injected into the function's SIM2REAL_MARKER environment variable, and whose echoed "header_names" contains "x-amz-cf-id" (proof the invocation arrived through CloudFront and not from the test client).
  2. The SAME Lambda function URL called directly and unsigned -> HTTP 403 (AuthType is AWS_IAM; only the OAC signs).
  3. That refusal body contains none of the run's marker.
  4. A repeat GET through the distribution still answers 200 with the same marker.

Actual (emulator): none of the four beats is ever reached. The run is cut at the 60 s harness cap while still inside one of the emulator-side waits that precede them, all of which are individually longer than the cap:
  - BEAT 1's edge poll: _poll_until(cdn_probe, status_code == 200, timeout_s=240.0, interval_s=5.0) on f"{ctx.endpoint_url(domain)}/" — the hostname polled is the emulator's own;
  - _managed_origin_request_policy_id(cf, "Managed-AllViewerExceptHostHeader"): an unbounded `while True` paging ListOriginRequestPolicies(Type="managed", MaxItems="100") that only terminates when a page omits NextMarker (raising AssertionError) or when the named policy is found;
  - _poll_until(create_function, r is not None, timeout_s=180.0, interval_s=10.0), which retries CreateFunction for 180 s while it answers InvalidParameterValueException;
  - _poll_until(get_function_configuration().State == "Active", timeout_s=180.0, interval_s=5.0).

What is known to have worked previously on this same check (from the issue that carries its id under a different replay key, 20261025T025628Z-...-listoriginrequestpolicies-type-managed-serves-an-empty-page.md): CreateOriginAccessControl with OriginAccessControlOriginType="lambda" and the GetOriginAccessControl read-back both succeeded, and the run then faulted with KeyError: 'Items' at _managed_origin_request_policy_id. The suite no longer reports that frame for this check, so the failure recorded here is a different terminal state and is not covered by that issue's replay key.

Related open issues that describe the absent CloudFront edge for OTHER origin types and do not carry this check's replay key: 20261025T054735Z-cloudfront-lambda-a-deployed-distribution-s-edge-never.md (AllowedMethods/CachedMethods at the edge over a Lambda function URL origin), 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, 20261024T102105Z-cloudfront-kms-s3-a-cloudfront-distribution-with-an-oac-over-an.md, 20261024T182555Z-cloudfront-cloudwatch-no-edge-traffic-is-ever-served.md. This check is the only one in the suite that grades an OAC of OriginAccessControlOriginType "lambda" signing a function URL origin and the paired unsigned-direct-call refusal.

## Steps to Reproduce
1. Create an IAM role assumable by lambda.amazonaws.com, then Lambda CreateFunction (runtime python3.12, handler index.handler) with a per-run marker in the SIM2REAL_MARKER environment variable, retrying only InvalidParameterValueException; then poll GetFunctionConfiguration until State == "Active".
   Expect: CreateFunction accepted within seconds of the role existing, and State reaching "Active". Note whether either poll is where the run stalls.
2. CreateFunctionUrlConfig(FunctionName=..., AuthType="AWS_IAM") and take the URL's host as the origin domain.
3. CreateOriginAccessControl with OriginAccessControlOriginType="lambda", SigningBehavior="always", SigningProtocol="sigv4"; read it back with GetOriginAccessControl and confirm the stored type is "lambda".
4. Resolve the managed origin request policy by NAME: page ListOriginRequestPolicies(Type="managed", MaxItems="100") following NextMarker, looking for OriginRequestPolicyConfig.Name == "Managed-AllViewerExceptHostHeader".
   Expect (AWS): the policy is present in every account. Confirm the emulator's paging TERMINATES — a NextMarker that repeats makes this loop spin forever, which alone exhausts the harness cap.
5. CreateDistribution with one CustomOriginConfig origin (https-only, TLSv1.2) whose DomainName is the function URL host and whose OriginAccessControlId is the OAC from step 3, and a DefaultCacheBehavior using the managed CachingDisabled cache policy plus the origin request policy id from step 4. Add lambda:InvokeFunctionUrl and lambda:InvokeFunction permissions to the cloudfront.amazonaws.com principal scoped to the distribution ARN. Poll GetDistribution until Status == "Deployed".
6. GET https://<distribution DomainName>/ -> expect HTTP 200 whose JSON body carries the run's marker and whose echoed header names include "x-amz-cf-id". Observed: no HTTP response is ever obtained, and the run is cut at the 60 s cap.
7. GET the function URL directly, unsigned -> expect HTTP 403 with no marker in the body.
8. Replay cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied to confirm.
