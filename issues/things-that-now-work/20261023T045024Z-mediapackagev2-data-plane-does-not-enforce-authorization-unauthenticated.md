Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable mediapackagev2.cmaf-ingest-native-protocol-round-trip
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable|run#14|AssertionError|assert denied_status == 403, (
Issue-Key: mediapackagev2.cmaf-ingest-native-protocol-round-trip|run#16|AssertionError|assert no_auth_status == 403, (
Issue-Commit: 134aa7970ed0c23d5be465e9c36f71c1d4a91758

# mediapackagev2 data plane does not enforce authorization: unauthenticated ingest push and un-granted manifest read both answer 404 instead of 403

> _Note: this issue was identified against the code base at commit `134aa7970ed0c23d5be465e9c36f71c1d4a91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint

## Description
Replay of the mediapackagev2 behavioral checks (55/66 passed) surfaced 2 distinct emulator defects in the data plane that the control-plane operations publish URLs for. They are bundled because they share one service; a fixer MUST close BOTH causes below before submitting.

Cause 1 of 2: the egress (HLS manifest) URL published by CreateOriginEndpoint serves anonymous reads with 404 instead of denying them with 403.
  AWS: CreateOriginEndpoint returns HlsManifests[].Url, modeled as the live delivery URL (Smithy: GetHlsManifestConfiguration.Url, required String, doc "The egress domain URL for stream delivery from MediaPackage."). The model says nothing about the data plane's authorization, so this is recorded behavior only: a v2 OriginEndpoint carries NO anonymous-read grant until a resource policy explicitly allows mediapackagev2:GetObject, and a GET of the manifest URL before such a policy is attached is answered HTTP 403. Only AFTER PutOriginEndpointPolicy grants Principal "*" / Action mediapackagev2:GetObject does the same URL become 404 (endpoint reachable, no content ingested yet). The requirement is the two-state status sequence 403-before-grant, 404-after-grant; the response bodies are not asserted.
  Emulator: the GET before any policy is attached returns HTTP 404 — i.e. the published egress host answers, but the endpoint's resource policy is not consulted at all, so the un-granted state is indistinguishable from the granted-but-empty state.
  Why: a client (or a player/CDN integration) that distinguishes "not authorized to read this endpoint" from "authorized, nothing published yet" cannot: a missing or wrong resource policy is silently reported as empty content, so the authorization step of a live workflow is never exercised and a policy regression is invisible. Any retry/alerting logic keyed on 403-vs-404 takes the wrong branch.
  Fix: the data-plane egress handler for origin-endpoint manifest URLs — it must authorize the request against the endpoint's stored resource policy (the one written by PutOriginEndpointPolicy) and return 403 when no statement allows mediapackagev2:GetObject for the caller, falling through to 404 only once access is granted.
  Traces: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable (expected 403 from the published index.m3u8 URL before any policy, got 404).

Cause 2 of 2: the ingest URL published by CreateChannel is not served by any data-plane listener — an unauthenticated push is answered by the control-plane front door with 404 UnknownService instead of 403.
  AWS: CreateChannel returns IngestEndpoints[].Url (Smithy: IngestEndpoint.Url, doc "The ingest domain URL where the source stream should be sent"); v2 returns no ingest username/password, so the ingest domain is SigV4-protected. Recorded behavior only: the ingest URL is a live TLS listener that rejects a PUT with no Authorization header with HTTP 403 and an "Access denied"-class body, and rejects a syntactically valid but cryptographically wrong SigV4 Authorization with a DIFFERENT HTTP 403 whose body is signature-verification specific — two distinct negative classes, never one canned response. The requirement is the 403 status for both classes and that the two denials are distinguishable; the exact body text is an example.
  Emulator: a PUT to the ingest URL returns HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the request never reaches a mediapackagev2 ingest handler; the generic router cannot attribute the ingest hostname to a service at all, so no authorization decision is made.
  Why: an encoder or client pointed at the published ingest URL gets a protocol-level "unknown service" 404 where AWS gives an authorization verdict, so neither the unauthenticated nor the mis-signed push path can be developed or tested against the emulator, and a stub that accepted everything would be indistinguishable from one that rejects.
  Fix: the data-plane ingest listener/route for the hostnames CreateChannel publishes in IngestEndpoints[].Url — the request must be resolved to mediapackagev2 ingest by its Host header and then answered 403 with an access-denied response when unauthenticated and a 403 with a signature-mismatch response when the SigV4 signature does not verify.
  Traces: mediapackagev2.cmaf-ingest-native-protocol-round-trip (expected 403 for an unauthenticated PUT, got 404 UnknownService).

Not included (deliberately, not oversight): nine further failing checks match AWS on HTTP status, error code and the modeled discriminator member (ValidationExceptionType / ResourceTypeNotFound) and differ ONLY in the wording of the human-readable Message, whose generating rule is neither modeled (Smithy: ValidationException/ResourceNotFoundException carry only an unconstrained Message member, no documented text) nor documented — mediapackagev2.cancel-harvest-job-unknown-job-name-not-found, .cancel-harvest-job-unknown-job-name-not-found-v2, .create-harvest-job-end-time-before-start-time-rejected, .harvest-job-end-time-before-start-time-rejected, .create-origin-endpoint-container-manifest-mismatch-rejected, .origin-endpoint-container-manifest-mismatch-rejected, .put-channel-policy-invalid-json-rejected, .put-origin-endpoint-policy-invalid-policy-rejected, .tag-resource-nonexistent-resource-arn-rejected. The one open mediapackagev2 issue (20261022T103649Z-mediapackagev2-emulator-diverges-from-recorded-aws-responses-missing.md) is a wire-corpus issue about harvest-job S3 validation, policy text round-tripping, ETag encoding, per-endpoint ingest hostnames and Content-Length on 204s; it touches IngestEndpoints[].Url only as a response-value shape and covers neither cause above.

## Evidence
Replay summary: suite simparity-behavioral-aws-v1, total 55/66 passed (83.3%), mismatched 11, errored 0, skipped 0, bootstrap_signal false (the emulator was up and answered).

Cause 1 — mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable, assertion at line 59:
  expected: HTTP 403 from the published HLS manifest URL while NO origin-endpoint policy is attached (the check notes an "always 404" rule was disproven against real AWS on a prior run), then HTTP 404 after PutOriginEndpointPolicy grants Principal "*"/mediapackagev2:GetObject.
  actual:   HTTP 404 for the un-granted read — AssertionError: expected 403 (no read policy granted yet) from https://<label>.egress.<label>.mediapackagev2.us-west-2.amazonaws.com/out/v1/<group>/<channel>/<endpoint>/index.m3u8, got 404. (Hostname labels, group/channel/endpoint names are per-run and are not the requirement.)

Cause 2 — mediapackagev2.cmaf-ingest-native-protocol-round-trip, assertion at line 59:
  expected: HTTP 403 with an "access denied"-class body for a PUT carrying no Authorization header to IngestEndpoints[0].Url, and a second, distinct HTTP 403 with a "signature"-class body for a PUT carrying a wrongly-signed SigV4 Authorization.
  actual:   AssertionError: expected 403 for an unauthenticated push, got HTTP 404: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'. The second negative class is never reached because the first assertion fails.

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 behavioral checks (suite simparity-behavioral-aws-v1).

Cause 1 — check mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable: CreateChannelGroup, CreateChannel, then CreateOriginEndpoint with ContainerType=TS and HlsManifests=[{ManifestName: index}]. Take HlsManifests[0].Url from the response and GET it over HTTPS with no credentials and with NO origin-endpoint policy attached. Expect HTTP 403; observe HTTP 404. Then PutOriginEndpointPolicy with {"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject","Resource":<endpoint Arn>} and GET the same URL again — only now should it be 404.

Cause 2 — check mediapackagev2.cmaf-ingest-native-protocol-round-trip: CreateChannelGroup, then CreateChannel with InputType=HLS. Take IngestEndpoints[0].Url and PUT an arbitrary body to it with NO Authorization header. Expect HTTP 403 with an access-denied body; observe HTTP 404 with {"__type":"UnknownService",...}. Repeat the PUT with a SigV4 Authorization header signed with the account's control-plane credentials for service "mediapackagev2": expect a second, different HTTP 403 whose body is signature-verification specific.
