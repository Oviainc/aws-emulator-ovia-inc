Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive apigateway__cloudfront__s3.cloudfront-apigw-origin-positive apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive|run#9|AssertionError|assert reflected1["header"] == "header-alpha", f"expected header 'header-alpha' forwarded, got {reflected1!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-positive|run#16|AssertionError|assert resp_headers.get("content-type", "").startswith("application/json"), (
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive|_poll_http_get#18|AssertionError|raise AssertionError(f"GET {url} never satisfied predicate within {timeout_s}s; last status={status}, body={body!r}")
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative|_poll_get#18|AssertionError|raise AssertionError(f"condition never satisfied for {url} within {timeout_s}s (last={last!r})")
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update|_wait_for_cache_hit#27|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow|_poll_options#29|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow|_wait_for_cache_hit#32|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough|_http_get#12|Timeout|with urllib.request.urlopen(req, timeout=timeout, context=_https_context(verify)) as resp:
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive|_poll_for_log_lines#24|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive|_poll_http_get#34|Timeout|return status, body
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive|_poll_http_get#19|Timeout|time.sleep(interval_s)
Issue-Commit: 69cb8c4786d799a8a66c2117bf8eebb4fb69db3f

# apigateway__cloudfront__s3: 5 net-new causes at the distribution edge — whitelisted viewer headers are not forwarded, responses carry no Content-Type, an OAI-granted S3 origin is refused 403, an http-only origin protocol mismatch answers 200 instead of 502/504, and eight edge checks never reach their asserted state

> _Note: this issue was identified against the code base at commit `69cb8c4786d799a8a66c2117bf8eebb4fb69db3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront viewer GET at a distribution edge (API Gateway / S3 origin)

## Description
Five distinct root causes in the apigateway__cloudfront__s3 suite, all observed now that the edge answers. The open issues 20261109T152222Z-apigateway-cloudfront-s3-a-distribution-s-domainname-is.md and 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md describe an edge that answers nothing at all; none of them carries these checks' replay keys and none describes a wrong answer.

Cause 1 of 5: the origin never receives the viewer headers the cache behavior whitelists, nor the CloudFront-generated headers.
AWS: a cache behavior (or origin request policy) that whitelists a header name forwards that header's viewer value to the origin on every request, alongside CloudFront's own generated headers; a whitelisted query string is forwarded the same way. The origin therefore echoes both the query value and the header value.
Emulator: the origin echoes the query string but both the whitelisted viewer header and the CloudFront-generated header arrive empty — the forwarded request carries only Host, User-Agent, Via and X-Amz-Cf-Id.
Why: a client whose backend keys on a forwarded header (tenant id, Accept-Language, Authorization, CloudFront-Viewer-*) sees it as absent, so routing, authorization and content negotiation behind CloudFront all silently fall through to the default branch.
Fix: the edge's origin-request builder — apply the behavior's/origin-request-policy's header allowlist and emit the CloudFront-generated headers.
Traces: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive.

Cause 2 of 5: the edge response carries no Content-Type at all.
AWS: CloudFront relays the origin's entity headers to the viewer; an API Gateway origin returning a JSON body answers with Content-Type: application/json (plus the origin's charset when it sets one).
Emulator: the viewer response has no content-type header whatsoever (the check reads None).
Why: a browser or SDK client cannot parse the body — JSON is treated as an opaque download and any client branching on the media type misroutes it.
Fix: the edge response serializer must relay the origin's Content-Type (and the other entity headers) onto the viewer response.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-positive.

Cause 3 of 5: a legacy origin-access-identity-granted S3 origin is refused 403 AccessDenied at the edge.
AWS: recorded behavior only — with an S3OriginConfig naming an origin access identity and the bucket granting READ to that identity's canonical user (or a bucket policy allowing s3:GetObject to its IAM ARN), the signed origin fetch succeeds and the edge answers 200 with the object's bytes.
Emulator: the edge answers 403 '<Error><Code>AccessDenied</Code><Message>Access Denied</Message><RequestId>...</RequestId><HostId>...</HostId></Error>' for the whole 120 s poll (RequestId/HostId are per-run values, not part of the requirement).
Why: no private object can be served through a legacy-OAI distribution, and because a granted and an ungranted bucket produce the same 403, the OAI authorization contract cannot be verified at all.
Fix: the edge's origin-access-identity origin signing and the S3-side evaluation of the OAI canonical-user grant.
Traces: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive.

Cause 4 of 5: an OriginProtocolPolicy of http-only against an HTTPS-only origin relays the origin's response instead of failing the origin connection.
AWS: when the configured origin protocol cannot be used — here plaintext HTTP against an API Gateway origin that only speaks HTTPS — the origin request never obtains an HTTP response, and CloudFront answers the viewer 502 Bad Gateway (or 504 Gateway Timeout if the connect stalls). The origin's body is never relayed.
Emulator: the same request is answered 200 and the origin's JSON body leaks through to the viewer ('{"origin_label": ..., "headers": {...}, "path": "/broken/probe"}'), i.e. OriginProtocolPolicy is ignored and the origin is contacted over whatever scheme works.
Why: a client misconfiguring its origin protocol gets a working distribution in the emulator and a hard 502 in AWS, so the misconfiguration is only discovered in production; and the 502/504 origin-failure contract every CloudFront runbook keys on is unreachable.
Fix: the edge's origin connector must honour the origin's OriginProtocolPolicy (and the origin SSL protocol set) and surface a failed origin connection as 502/504.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative.

Cause 5 of 5: eight edge data-plane checks never reach their asserted state and burn their entire budget.
AWS: once GetDistribution reports Status=Deployed, the distribution's edge serves the configured behaviour within seconds — the invalidated object is re-fetched, an S3 CORS preflight is answered, a synced object is served, a whitelisted viewer header reaches the origin, standard access logs are delivered to the configured bucket and prefix, an OAI/trusted-signer-restricted object is served to a valid signed URL, and an S3 website-endpoint custom origin serves its index document.
Emulator: none of these states is ever observed; each check is reported only as "Check timed out after 180.0s", so the edge never converges on the configured behaviour.
Why: a client cannot observe invalidation, CORS, logging, signed-URL restriction or website-endpoint origins through the emulated edge at all — every workflow built on them hangs rather than failing fast.
Fix: the CloudFront edge data-plane handler and the delivery paths it drives (invalidation cache purge, standard log delivery, trusted-signer/signed-URL evaluation, S3 website-endpoint custom origin).
Traces: apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update, apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow, apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow, apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough, apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive, apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive, apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive, apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive.

## Evidence
Cause 1 — expected header 'header-alpha' forwarded; got {'query': 'alpha', 'header': '', 'cfHeader': ''}.

Cause 2 — expected content-type to start with 'application/json'; got None.

Cause 3 — GET https://<edge>/index.txt never satisfied the predicate within 120.0s; last status=403, body=b'<Error><Code>AccessDenied</Code><Message>Access Denied</Message><RequestId>...</RequestId><HostId>...</HostId></Error>'.

Cause 4 — expected origin-connect failure 502 or 504 from the http-only origin, with no origin body relayed; got, for the whole 90 s poll, (200, {'x-cache': 'Miss from cloudfront', ...}, b'{"origin_label": "protocol-mismatch", "headers": {"Host": "<api-id>.execute-api.<region>.amazonaws.com", "User-Agent": "Amazon CloudFront", "Via": "1.1 ... (CloudFront)", "X-Amz-Cf-Id": "..."}, "path": "/broken/probe"}').

Cause 5 — all eight checks report only 'Check timed out after 180.0s' with no asserted state ever observed.

## Steps to Reproduce
Replay the behavioral suite and re-run the apigateway__cloudfront__s3 checks in trace_ids. By hand: (1) create a REST API that echoes its request headers/query, deploy it, create a distribution with that API as a custom origin and a cache behavior whitelisting a header name and a query-string name; GET through the edge with that header set and inspect what the origin echoed and what Content-Type the viewer response carries. (2) add a second origin for the same API with OriginProtocolPolicy=http-only and GET through the behavior routed to it. (3) create a bucket with an object, a CloudFrontOriginAccessIdentity, grant that identity READ, create a distribution with S3OriginConfig.OriginAccessIdentity set, wait for Deployed and GET the object. (4) exercise invalidation-after-update, an S3 CORS preflight, s3 sync + invalidate, standard access-log delivery, a trusted-signer signed URL and an S3 website-endpoint origin through the edge.
