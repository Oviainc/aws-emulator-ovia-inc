Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|run#38|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked|run#17|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|run#36|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Key: cloudfront__lambda.origin-access-control-signs-lambda-url-origin-and-direct-call-denied|run#34|PublishedUrlRejected|cdn_url, route_headers = ctx.endpoint_route(f"{domain}/")
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudfront__lambda: edge enforcement over a Lambda origin is unreachable — the distribution's DomainName is published as an unserved <id>.cloudfront.net host

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the emulator publishes a CloudFront edge address it does not serve, so none of the edge-enforced contracts over a Lambda function-URL origin can be exercised.

AWS: CreateDistribution returns Distribution.DomainName and viewer requests to https://<DomainName>/<path> are answered by the CloudFront edge. The behaviors these checks grade exist only there: a POST to a behavior whose AllowedMethods are GET/HEAD is refused at the edge (405 with CloudFront's error body, origin never invoked); a viewer in a GeoRestriction blocklist country is refused 403 at the edge; a signed URL whose policy pins a source IP is refused for a different IP; and an OAC-signed Lambda function-URL origin is fetched with SigV4 while a direct call to the function URL is refused 403 (recorded behavior only; DomainName is an unconstrained string in the Smithy model).

Emulator: DomainName is published as "<lowercased-distribution-id>.cloudfront.net", which is not one of the emulator's own hosts, and each viewer request is refused before it is sent: "endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The emulator announced an address other than the one it is reached on.

Why: a client gets no enforcement and no evidence of it. Method restriction, geo restriction, signed-URL conditions and OAC origin signing all appear configured in the control plane and none of them can act, so a configuration that would be rejected or blocked on AWS looks fine here.

Fix: the CloudFront distribution serializer must publish a DomainName on a host:port the emulator serves, and an edge request router must answer viewer requests there — applying AllowedMethods, GeoRestriction and trusted-signer/key-group policy checks before the origin fetch, and SigV4-signing the origin request when an OAC is attached.

Traces: the four cloudfront__lambda checks in trace_ids. Several open issues report the same end effect for this heading from earlier runs (20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md, 20261102T102137Z-cloudfront-lambda-a-deployed-distribution-over-a-lambda.md, 20261103T062355Z-cloudfront-lambda-a-deployed-distribution-s-edge-domain-3d1d10.md, 20261025T220525Z-cloudfront-lambda-the-oac-signed-lambda-function-url.md); none owns these checks in this run.

## Evidence
Expected: GET/POST to https://<DomainName>/<path> is answered by the emulator's edge with the enforced outcome (405 / 403 / 200 as configured).
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://d5ni6znxjb8do0.cloudfront.net (also dfgu4i109b1t52.cloudfront.net, dgzni4pnd7h2s3.cloudfront.net/) (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener. All four checks fail at this first request; the host is always the distribution id lowercased + ".cloudfront.net".

## Steps to Reproduce
1. Create a Lambda function with a function URL.
2. CreateDistribution over that origin with AllowedMethods GET/HEAD (and, per check, a GeoRestriction blocklist, TrustedKeyGroups, or an OriginAccessControl with signing enabled); wait for Status=Deployed.
3. Read Distribution.DomainName and send the viewer request (POST /, GET with a geo header, a signed URL from another IP, GET /) — the published host is <id>.cloudfront.net, which the emulator does not serve, so no request reaches the edge.
