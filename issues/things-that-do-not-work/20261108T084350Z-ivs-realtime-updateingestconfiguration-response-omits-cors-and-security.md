Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.access-control-allow-origin
Issue-Key: ivs-realtime|UpdateIngestConfiguration|2xx->2xx|$headers.content-security-policy
Issue-Commit: a946db3d2c92d3ec27b0f0d3beb3e4ffad06b536

# ivs-realtime UpdateIngestConfiguration response omits CORS and security headers

> _Note: this issue was identified against the code base at commit `a946db3d2c92d3ec27b0f0d3beb3e4ffad06b536`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIngestConfiguration

## Description
The ivs-realtime UpdateIngestConfiguration response serializer does not emit the modeled CORS/security response headers AWS attaches to its 200 answers. One cause, in response-header serialization rather than in the operation's business logic.

Cause 1 of 1: UpdateIngestConfiguration omits Access-Control-Allow-Origin entirely and serves the wrong Content-Security-Policy.
AWS: Smithy: UpdateIngestConfiguration (POST /UpdateIngestConfiguration, code 200) output UpdateIngestConfigurationResponse binds these as HTTP response headers - member accessControlAllowOrigin @httpHeader("Access-Control-Allow-Origin"), contentSecurityPolicy @httpHeader("Content-Security-Policy"), plus accessControlExposeHeaders, cacheControl, strictTransportSecurity, xContentTypeOptions, xFrameOptions; the service AmazonInteractiveVideoServiceRealTime itself carries smithy.api#cors, so the API is CORS-enabled. The concrete values are recorded behavior only: AWS answers 200 with `access-control-allow-origin: *` and `content-security-policy: default-src 'self'; upgrade-insecure-requests;`. Both strings are fixed text for this service - no part of either is taken from the request or from emulator state.
Emulator: answers the expected 200 with the expected body but emits NO access-control-allow-origin header at all, and emits `content-security-policy: default-src 'none'` instead of `default-src 'self'; upgrade-insecure-requests;`.
Why: a browser-based client (the IVS RealTime web SDK is exactly that) receives no Access-Control-Allow-Origin on the response, so the user agent rejects the fetch and the call appears to fail even though the service answered 200; a client that reads the modeled accessControlAllowOrigin output member gets None, and one that reads contentSecurityPolicy gets a value stricter than and different from what AWS declares, so a page embedding the response behaves differently here than against AWS.
Fix: the restJson1 response serializer for the UpdateIngestConfiguration handler - and the shared ivs-realtime response-header layer that is supposed to populate the modeled accessControlAllowOrigin / contentSecurityPolicy (and sibling) output members - must set both headers to the service's fixed values on every 200.
Traces: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#3 (representative); the same cluster also fails at ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#5.

Related but not covering this: open issue 20261031T073914Z-ivs-realtime-responses-omit-modeled-security-and-cors.md files the same header fault for CreateIngestConfiguration and the missing content-length on TagResource/UntagResource; its replay key and Issue-Key lines name CreateIngestConfiguration, TagResource and UntagResource only, so the UpdateIngestConfiguration response path above is not exercised or closed by it. Open issue 20261029T202151Z-ivs-realtime-ingest-and-participant-semantics-redundantingest-update.md owns the redundantIngest body semantics on this same fixture and carries no response-header cause.

## Evidence
Fixture ivs-realtime-update-ingest-configuration-redundant-ingest-toggle, step task#3, operation UpdateIngestConfiguration, verdict mismatch: expected status 200 and observed status 200, but headers diverge -
  $headers.access-control-allow-origin: missing (expected '*', got None)
  $headers.content-security-policy: value_mismatch (expected "default-src 'self'; upgrade-insecure-requests;", got "default-src 'none'")
Cluster affected_count 2: ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#3 and ivs-realtime-update-ingest-configuration-redundant-ingest-toggle#task#5 fail identically. divergence_key: service=ivs-realtime|UpdateIngestConfiguration|status=200->200|keys=$headers.access-control-allow-origin,$headers.content-security-policy. owned_by: none.
Model evidence: smithy_query query=operation operation=UpdateIngestConfiguration on services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/smithy-model/com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json reports output members accessControlAllowOrigin @httpHeader("Access-Control-Allow-Origin") and contentSecurityPolicy @httpHeader("Content-Security-Policy") (plus accessControlExposeHeaders, cacheControl, strictTransportSecurity, xContentTypeOptions, xFrameOptions) on UpdateIngestConfigurationResponse, http POST /UpdateIngestConfiguration code 200.
Fixed vs variable text: both header values are entirely fixed service text; neither contains any request- or state-derived part, so no per-run value is being asserted.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Replay fixture ivs-realtime-update-ingest-configuration-redundant-ingest-toggle step task#3 (POST /UpdateIngestConfiguration with arn of an existing ingest configuration and redundantIngest toggled) and inspect the response headers: access-control-allow-origin is absent where AWS sends '*', and content-security-policy is "default-src 'none'" where AWS sends "default-src 'self'; upgrade-insecure-requests;".
4. Replay step task#5 of the same fixture (the second UpdateIngestConfiguration call) to see the identical header divergence.
