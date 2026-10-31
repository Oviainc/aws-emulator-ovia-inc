Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-ingest-configuration-created-inactive
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-ingest-configuration-created-inactive ivs-realtime-update-ingest-configuration-redundant-ingest-toggle ivs-realtime-tag-and-untag-resource-roundtrip
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.access-control-allow-origin
Issue-Key: ivs-realtime|CreateIngestConfiguration|2xx->2xx|$headers.content-security-policy
Issue-Key: ivs-realtime|TagResource|2xx->2xx|$headers.content-length
Issue-Key: ivs-realtime|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: d0f42600c4826d232c2bfaa9dd72cae06a688894

# ivs-realtime responses omit modeled security and CORS headers

> _Note: this issue was identified against the code base at commit `d0f42600c4826d232c2bfaa9dd72cae06a688894`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIngestConfiguration

## Description
The ivs-realtime response layer does not emit the HTTP response headers AWS attaches to its answers: the modeled CORS/security headers on the ingest-configuration JSON responses, and the entity-length header on the empty-bodied tagging responses. Three causes, all in how a response is serialized rather than in any operation's business logic.

Cause 1 of 3: CreateIngestConfiguration omits Access-Control-Allow-Origin and serves the wrong Content-Security-Policy.
AWS: CreateIngestConfigurationResponse models these as bound response headers - Smithy: CreateIngestConfiguration output member accessControlAllowOrigin @httpHeader("Access-Control-Allow-Origin"), contentSecurityPolicy @httpHeader("Content-Security-Policy") (also cacheControl, strictTransportSecurity, xContentTypeOptions, xFrameOptions, accessControlExposeHeaders); the service itself carries smithy.api#cors (additionalExposedHeaders x-amzn-errortype, x-amzn-requestid, ...), so the API is CORS-enabled. The concrete values are recorded behavior only: AWS answers 200 with `access-control-allow-origin: *` and `content-security-policy: default-src 'self'; upgrade-insecure-requests;`. Both are fixed text for this service - no part of either is derived from the request or from state.
Emulator: answers 200 with the correct body but with NO access-control-allow-origin header at all, and with `content-security-policy: default-src 'none'` instead of `default-src 'self'; upgrade-insecure-requests;`.
Why: a browser-based client (the IVS RealTime web SDK is exactly that) gets no Access-Control-Allow-Origin on the preflighted response, so the fetch is rejected by the user agent and the call appears to fail even though the service answered 200. A client that reads the modeled accessControlAllowOrigin / contentSecurityPolicy output members gets None and a wrong value respectively. `default-src 'none'` is also stricter than what AWS declares, so a page that embeds the response under AWS's policy behaves differently here.
Fix: the restJson1 response serializer for the ingest-configuration handlers (and the shared response-header layer that populates the modeled accessControlAllowOrigin/contentSecurityPolicy output members for this service) must set both headers to the service's values on every 200.
Traces: ivs-realtime-ingest-configuration-created-inactive#task#2 (representative); also ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2.

Cause 2 of 3: TagResource's empty 204 response carries no Content-Length header.
AWS: Smithy: TagResource output TagResourceResponse is an empty structure (no members), POST /tags/{resourceArn}. Recorded behavior only for the framing: AWS answers with an empty body and a Content-Length header present (the corpus asserts presence, not the value; for an empty body the value is the body's byte count, i.e. 0). Presence of the header is fixed; its value is derived from the body.
Emulator: answers the expected 204 but emits no content-length header at all.
Why: a strict HTTP client or proxy that requires an explicit entity length on a non-chunked response has to guess how the message is framed; clients that key off content-length to decide whether a body is present see an indeterminate response where AWS is explicit.
Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#2.
Fix: the tagging handler's response serializer / the shared empty-body response builder must set content-length for bodyless responses.

Cause 3 of 3: UntagResource's empty 204 response carries no Content-Length header - the same defect on the other tagging operation.
AWS: Smithy: UntagResource output UntagResourceResponse is an empty structure, DELETE /tags/{resourceArn}, @idempotent. Recorded behavior only: empty body with Content-Length present.
Emulator: answers 204 with no content-length header.
Why: same as cause 2 - the response's framing is left implicit where AWS states it, so length-sensitive clients and intermediaries diverge.
Fix: same shared empty-body response builder as cause 2; both tagging operations must go through it.
Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#4.

Not covered by any open issue: 20261029T202151Z-ivs-realtime-ingest-and-participant-semantics-redundantingest-update.md owns the redundantIngest/redundantIngestCredentials body divergences and the participant-token/disconnect divergences on the same fixtures, but it carries no response-header cause; the header defects above survive independently of it.

## Evidence
Cause 1 - fixture ivs-realtime-ingest-configuration-created-inactive, step task#2, CreateIngestConfiguration: status 200 expected and 200 observed, but `$headers.access-control-allow-origin: missing (expected '*', got None)` and `$headers.content-security-policy: value_mismatch (expected "default-src 'self'; upgrade-insecure-requests;", got "default-src 'none'")`. Same cluster member: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2 (affected_count 2).

Cause 2 - fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#2, TagResource: status 204 expected and 204 observed, but `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`.

Cause 3 - fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#4, UntagResource: status 204 expected and 204 observed, but `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`.

Model evidence: smithy_query operation=CreateIngestConfiguration shows output members accessControlAllowOrigin @httpHeader("Access-Control-Allow-Origin") and contentSecurityPolicy @httpHeader("Content-Security-Policy"); smithy_query service_info shows smithy.api#cors on AmazonInteractiveVideoServiceRealTime (2020-07-14, restJson1, endpointPrefix ivsrealtime). TagResource/UntagResource outputs are empty structures.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: replay fixture ivs-realtime-ingest-configuration-created-inactive step task#2 (POST /CreateIngestConfiguration with ingestProtocol set) and inspect the response headers - access-control-allow-origin is absent and content-security-policy is "default-src 'none'" instead of "default-src 'self'; upgrade-insecure-requests;". Reproduces identically at ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#2.
4. Cause 2: replay fixture ivs-realtime-tag-and-untag-resource-roundtrip step task#2 (POST /tags/{resourceArn}) and inspect the 204 response headers - content-length is absent.
5. Cause 3: replay the same fixture's step task#4 (DELETE /tags/{resourceArn}?tagKeys=...) and inspect the 204 response headers - content-length is absent.
