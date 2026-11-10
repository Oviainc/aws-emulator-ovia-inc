Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|run#57|PublishedUrlRejected|_url, _route = ctx.endpoint_route(direct_url)
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|run#42|PublishedUrlRejected|_url, _route = ctx.endpoint_route(function_url)
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked|run#17|AssertionError|assert isinstance(country, str) and re.fullmatch(r"[A-Z]{2}", country), (
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|run#44|AssertionError|assert isinstance(viewer_address, str) and ":" in viewer_address, (
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront__lambda: 2 net-new causes — a Lambda function URL is published on the real AWS lambda-url host the emulator does not serve, and the edge generates no CloudFront-Viewer-Country/-Address headers

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
TWO net-new root causes; a fixer must close both before submitting. The open 20261109T181319Z-cloudfront-lambda-edge-enforcement-over-a-lambda-origin.md and 20261102T102137Z-cloudfront-lambda-a-deployed-distribution-over-a-lambda.md describe a distribution DomainName that is unserved; these two causes are different and are not carried by either.

Cause 1 of 2: the Lambda function URL the emulator publishes names a host the emulator does not serve.
AWS: Smithy (lambda): CreateFunctionUrlConfig / GetFunctionUrlConfig return FunctionUrl, the HTTPS endpoint at which the function is invocable; on AWS that is https://<url-id>.lambda-url.<region>.on.aws/ and AWS actually serves it. Recorded behavior: a client (or a CloudFront origin) that takes FunctionUrl verbatim and issues a request to it reaches the function.
Emulator: FunctionUrl is published as https://<32-hex-id>.lambda-url.<region>.on.aws/ — a real AWS production hostname that this emulator does not answer on. Any caller that uses the published value is routed off the emulator entirely; the harness refuses the address with "endpoint_url was given https://<id>.lambda-url.<region>.on.aws/ ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". Both checks die before they can configure the distribution, so the AllowedMethods and OAC-signing contracts over a Lambda-URL origin are unreachable.
Why: the emulator announces an address other than the one it is reached on. Every client that round-trips FunctionUrl — a CloudFront origin pointed at a function URL, a test harness, any SDK consumer — sends traffic to AWS production or to nothing at all, and the whole Lambda-URL-as-origin topology is untestable.
Fix: the Lambda function-URL publication path — FunctionUrl must be an address the emulator serves (its own host/port, or a per-resource listener), in CreateFunctionUrlConfig/UpdateFunctionUrlConfig/GetFunctionUrlConfig and anywhere else the URL is serialized (lambda function-url ops and the state that mints the URL).
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected, cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied.

Cause 2 of 2: the edge generates no CloudFront-Viewer-Country or CloudFront-Viewer-Address header.
AWS: recorded behavior only: for every viewer request, the CloudFront edge generates CloudFront-Viewer-Country as an ISO 3166-1 alpha-2 two-letter code and CloudFront-Viewer-Address as "<ip>:<port>" of the viewer connection, and forwards them to the origin when the behavior's origin request policy includes them. GeoRestriction decisions and signed-URL IpAddress conditions are evaluated against that same viewer identity.
Emulator: the origin sees None for CloudFront-Viewer-Country and None for CloudFront-Viewer-Address, so neither the geo-blocklist 403 nor the source-IP-scoped signed-URL denial can be observed.
Why: a client cannot test geo restriction or IP-scoped signed URLs at all, and an origin that personalizes or authorizes on these headers gets nothing where production gives it a value.
Fix: the edge request pipeline — generate CloudFront-Viewer-Country (two-letter code) and CloudFront-Viewer-Address ("<ip>:<port>") per request and use them for GeoRestriction and signed-URL IpAddress evaluation (cloudfront edge data plane, generated-header step).
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked, cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied.

## Evidence
Cause 1 (both checks, raised before any edge request):
simparity.published.PublishedUrlRejected: endpoint_url was given https://e5179053976940f78c418f8f67c2a5bb.lambda-url.us-west-2.on.aws/?nonce=... (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34739/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
and
simparity.published.PublishedUrlRejected: endpoint_url was given https://5fc8794aaf054ccf8748235b3e0be8e5.lambda-url.us-west-2.on.aws/ ... needs http(s)://127.0.0.1:36703/<resource path> ...
Expected: a FunctionUrl on a host the emulator answers on. Actual: the AWS production lambda-url host. (The url-id, the host:port and the nonce are per-run values; the requirement is that the announced host is one the emulator serves.)
Cause 2: "AssertionError: CloudFront-Viewer-Country must be a two-letter ISO 3166-1 alpha-2 code, got None" and "AssertionError: CloudFront-Viewer-Address must be '<ip>:<port>', got None".

## Steps to Reproduce
1. Point lambda and cloudfront clients at the emulator.
2. Cause 1: create a function, CreateFunctionUrlConfig(FunctionName=..., AuthType=...), read FunctionUrl from the response (or GetFunctionUrlConfig) -> it is https://<id>.lambda-url.<region>.on.aws/, a host the emulator does not serve; issuing a request to it verbatim does not reach the emulator.
3. Cause 2: create a distribution over a reachable origin that echoes its request headers, with a GeoRestriction blocklist and an origin request policy including the CloudFront viewer headers; wait for Status Deployed; GET through the distribution's DomainName -> the origin reports CloudFront-Viewer-Country None and CloudFront-Viewer-Address None, and the geo-blocked request is not answered 403.
4. Or replay the four check ids in trace_ids.
