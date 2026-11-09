Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published
Issue-Key: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected|run#20|PublishedUrlRejected|cf_url, cf_headers = ctx.endpoint_route(f"{domain}/{key}")
Issue-Key: cloudfront__s3.default-root-object-serves-root-but-tenant-directory-rejected|_get#12|PublishedUrlRejected|target, route_headers = ctx.endpoint_route(url)
Issue-Key: cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object|_get#14|PublishedUrlRejected|target, route_headers = ctx.endpoint_route(url)
Issue-Key: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden|run#14|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{dist['DomainName']}/{key}")
Issue-Key: cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key|_get#10|PublishedUrlRejected|target, route_headers = ctx.endpoint_route(url)
Issue-Key: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden|run#9|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{dist['DomainName']}/{key}")
Issue-Key: cloudfront__s3.oai-origin-edge-rejects-a-missing-key-with-access-denied|_get#10|PublishedUrlRejected|target, route_headers = ctx.endpoint_route(url)
Issue-Key: cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published|_get#12|PublishedUrlRejected|target, route_headers = ctx.endpoint_route(url)
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudfront__s3: the S3-origin edge is unreachable for eight checks — the distribution's DomainName is published as an unserved <id>.cloudfront.net host

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the emulator publishes a CloudFront edge address it does not serve, so nothing can be fetched through a distribution over an S3 origin.

AWS: CreateDistribution returns Distribution.DomainName and a request to https://<DomainName>/<path> is answered by the CloudFront edge, which fetches from the S3 origin with the configured OAC/OAI identity. The contracts these checks grade live there: an OAC-locked bucket serves through the edge while a direct anonymous S3 GET is refused 403 AccessDenied; DefaultRootObject serves the root object while a tenant directory path is refused; an invalidation makes the edge re-fetch and re-cache an updated object exactly once; an OAC grant scoped to another distribution's ARN yields 403 at the edge; a missing key under an OAI origin yields 403 AccessDenied (not 404) when the policy grants no s3:ListBucket; and a viewer-request function keeps running its LIVE code until an edit is published (recorded behavior only; DomainName is an unconstrained string in the Smithy model).

Emulator: DomainName is published as "<lowercased-distribution-id>.cloudfront.net", an address that is not one of the emulator's own hosts, and each viewer request is refused before it is sent: "endpoint_url was given https://<id>.cloudfront.net/<path> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The emulator announced an address other than the one it is reached on.

Why: the primary CloudFront use case — a private S3 bucket served only through the distribution — cannot be used or verified. A client cannot distinguish a correctly locked-down origin from a misconfigured one, because both serve nothing, and cache/invalidation semantics are entirely unobservable.

Fix: the CloudFront distribution serializer must publish a DomainName on a host:port the emulator serves, and an edge request router must answer viewer requests there: resolve the cache behavior, honour DefaultRootObject and the cache key, run attached viewer functions, sign the origin request with the OAC/OAI identity, and surface the origin's 403/404 as CloudFront does.

Traces: the eight cloudfront__s3 checks in trace_ids. Open issues describe the same end effect for this heading from earlier runs (20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, 20261102T102137Z-cloudfront-s3-four-causes-the-s3-origin-edge.md, 20261108T060951Z-cloudfront-s3-an-oac-locked-s3-origin-is.md); none owns these checks in this run.

## Evidence
Expected: GET https://<DomainName>/<path> is answered by the emulator's edge from the S3 origin (200, or 403/404 as the OAC/OAI policy dictates).
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net/asset.txt (also .../, /demo-tenant/index.html, /onyx-test/index.html, https://d89fkddmx3r403.cloudfront.net/index.html, /page1.html, https://du75el7ewhhv91.cloudfront.net/onyx-test/) (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener. All eight checks fail at this first request; the host is always the distribution id lowercased + ".cloudfront.net".

## Steps to Reproduce
1. Create an S3 bucket with an object and a bucket policy granting the distribution's OAC (or OAI) read access.
2. CreateDistribution with that bucket as the origin (OAC attached, DefaultRootObject set as needed) and wait for Status=Deployed.
3. Read Distribution.DomainName and send GET https://<DomainName>/<key> — the published host is <id>.cloudfront.net, which the emulator does not serve, so no request reaches the edge; the direct-to-S3 comparison the checks make is then meaningless.
