Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing
Issue-Key: cloudfront__kinesis.obs-cloudfront-3-realtime-log-role-without-put-record-delivers-nothing|_served#31|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(published)
Issue-Key: cloudfront__kinesis.realtime-log-config-delivers-edge-request-record-to-kinesis|run#41|PublishedUrlRejected|edge, route_headers = ctx.endpoint_route(domain)
Issue-Key: cloudfront__kinesis.xsvc-cloudfront-3-realtime-log-role-denied-putrecord-delivers-nothing|run#40|PublishedUrlRejected|base, route_headers = ctx.endpoint_route(dist["DomainName"])
Issue-Commit: 17c4643c966c9683f16ddaee3534ae6f6497ef31

# cloudfront__kinesis: real-time log delivery is unreachable — the distribution's DomainName is published as an unserved <id>.cloudfront.net host

> _Note: this issue was identified against the code base at commit `17c4643c966c9683f16ddaee3534ae6f6497ef31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the CloudFront edge address the emulator publishes is not one it serves, so no edge request can be made and no real-time log record can be produced.

AWS: CreateDistribution returns Distribution.DomainName and a request to https://<DomainName>/<path> is answered by the CloudFront edge. A distribution with a real-time log configuration then writes one record per viewer request to the configured Kinesis stream via the configured IAM role; if that role lacks (or is denied) kinesis:PutRecord, the requests still succeed at the edge and no record is delivered — which is exactly the difference the negative checks grade (recorded behavior only; DomainName is an unconstrained string in the Smithy model).

Emulator: DomainName is published as "<lowercased-distribution-id>.cloudfront.net", an address that is not one of the emulator's own hosts, and the viewer request is refused before it is sent: "endpoint_url was given https://<id>.cloudfront.net (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The emulator announced an address other than the one it is reached on.

Why: real-time logging cannot be observed at all. The positive case (a record reaching Kinesis) and the two permission-denied negatives (nothing delivered) are indistinguishable — every one of them yields an empty stream, so a client cannot tell a working log pipeline from a broken IAM role.

Fix: the CloudFront distribution serializer must publish a DomainName on a host:port the emulator serves; the edge request router must answer viewer requests there and, per request, emit the real-time log record through the configured role to the configured Kinesis stream.

Traces: the three cloudfront__kinesis checks in trace_ids. The open issue 20261025T232706Z-cloudfront-kinesis-cloudfront-real-time-log-configs-never.md describes the same end effect from an earlier run and does not own these checks; the failure mode here is the published-host refusal.

## Evidence
Expected: GET https://<DomainName>/<path> is answered by the emulator's edge and (for the permitted role) one record per request appears on the Kinesis stream.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://dgzni4pnd7h2s3.cloudfront.net (and .../obj-<id>.txt) (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener. All three checks fail at this first request.

## Steps to Reproduce
1. Create a Kinesis stream and an IAM role for real-time logging; CreateRealtimeLogConfig naming them.
2. CreateDistribution whose default cache behavior references that real-time log config; wait for Status=Deployed.
3. Read Distribution.DomainName and send GET https://<DomainName>/<key> — the published host is <id>.cloudfront.net, which the emulator does not serve, so no request reaches the edge and the stream stays empty regardless of the role's permissions.
