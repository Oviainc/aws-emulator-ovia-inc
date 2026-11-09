Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge|_warm_edge#1|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc} (last {last.get('v')!r})") from exc
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# CloudFront distribution domain name is published but never served: the emulator announces <id>.cloudfront.net, which it does not answer on

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: CloudFront has no data plane — the DomainName it publishes is not an address the emulator serves.

AWS: `CreateDistribution` returns `Distribution.DomainName` (`<distribution-id>.cloudfront.net`), and once the distribution is Deployed that domain is the viewer-facing endpoint: HTTP(S) requests to it are answered by the edge, which applies the cache policy and origin request policy, forwards to the configured origin, and adds the edge-generated headers (`CloudFront-Viewer-Country`, `CloudFront-Forwarded-Proto`, `X-Cache: Hit from cloudfront` / `Miss from cloudfront`). Smithy (cloudfront): CreateDistribution -> Distribution{Id, DomainName, Status, DistributionConfig}. Recorded behavior: all eight checks issue ordinary HTTP requests at the published DomainName.

Emulator: the control plane accepts the distribution and publishes a `<id>.cloudfront.net` DomainName, but that host is not one of the emulator's own hosts and nothing listens for it — every viewer request against the published address is refused before it reaches any handler. The emulator announced an address other than the one it is reachable on, so no cache-key, origin-request-policy, origin-path, compression or edge-header behavior can be exercised at all.

Why: a client follows the DomainName the API gave it. Publishing an unroutable domain makes the whole CDN data plane unusable: cache behavior, origin shielding, header forwarding and edge-header generation are all unobservable, and any application fronted by CloudFront is unreachable even though its distribution reports Deployed.

Fix: the CloudFront data plane — either serve the published `<id>.cloudfront.net` host on the emulator's own listener (host-based routing) or publish a DomainName that resolves to the emulator (a per-distribution listener / path on the emulator's own host). Belongs in the CreateDistribution/GetDistribution serialisation of DomainName together with a viewer-request handler.

Traces: the eight `deep-cloudfront-3-cf-origin-*` checks listed in trace_ids, all failing identically with DATAPLANE_UNIMPLEMENTED on the published DomainName. The same root cause also reaches `cloudfront__dynamodb__lambda__s3__stepfunctions.e2e-cloudfront-distribution-origin-error-diagnosis-lifecycle`, which is already filed as 20261108T031654Z-cloudfront-dynamodb-lambda-s3-stepfuncti-....md.

## Evidence
Expected: GET https://<distribution-id>.cloudfront.net/<path> is served by the emulator's CloudFront data plane (cache-key/origin-policy behavior observable, X-Cache and CloudFront-* headers present).
Actual: AssertionError: DATAPLANE_UNIMPLEMENTED: endpoint_url was given https://<distribution-id>.cloudfront.net/<path> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener (last None). The distribution id, host and port are per-run values; the requirement is that the published DomainName is answered.

## Steps to Reproduce
1. cloudfront:CreateDistribution with an origin pointing at a reachable origin (Lambda/API or S3) and a cache policy.
2. Read Distribution.DomainName from the response (or GetDistribution).
3. Issue GET https://<DomainName>/<path>.
4. Observe the host is not served by the emulator, so no viewer request ever reaches the edge or the origin.
