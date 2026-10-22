Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackagev2.cmaf-ingest-native-protocol-round-trip mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2.cmaf-ingest-native-protocol-round-trip|run#16|AssertionError|assert no_auth_status == 403, (
Issue-Key: mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable|run#14|AssertionError|assert denied_status == 403, (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# mediapackagev2 data plane not served: CMAF ingest URL and HLS egress manifest URL answer 404/UnknownService where AWS answers 403

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Replay of the mediapackagev2 behavioral checks (55/66 passed) surfaced 2 distinct emulator defects, both in the MediaPackage v2 DATA plane (the URLs the control plane hands out). They are bundled because they share one service; a fixer MUST close BOTH causes below before submitting.

Cause 1 of 2: the ingest endpoint URL returned by CreateChannel is not served by any mediapackagev2 data-plane handler — an unauthenticated PUT is answered by the emulator's generic request router with 404 UnknownService instead of 403 Access denied.
  AWS: CreateChannel returns IngestEndpoints[*].Url (Smithy: shape IngestEndpoint, member Url, doc "The ingest domain URL where the source stream should be sent"); the model defines no ingest credential member (Id and Url only), so the ingest domain is authenticated with SigV4. Recorded behavior only: that URL is a live HTTPS listener that enforces two DISTINCT negative classes — a PUT with no Authorization header is rejected with HTTP 403 and an 'Access denied' body, and a PUT carrying a syntactically well-formed but cryptographically invalid SigV4 Authorization header is rejected with HTTP 403 and a signature-verification-specific body (the two denial bodies differ). The requirement is the 403 status for both classes and that the two denials are distinguishable; the exact sentences are examples.
  Emulator: the PUT never reaches a mediapackagev2 ingest handler at all. The emulator's front door answers HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — i.e. the ingest host/path advertised by CreateChannel is routed only by SigV4 credential scope / X-Amz-Target, so a native-protocol push cannot be dispatched, and the wrongly-signed class is never reached either.
  Why: a client that drives ingest the way a live encoder does (plain HTTPS PUT to the advertised ingest URL) gets a 404 'unknown service' instead of an authorization decision. Code that distinguishes 'not authorized to ingest' from 'endpoint does not exist', or that retries/refreshes credentials on 403, takes the wrong branch, and the URL the control plane published looks nonexistent.
  Fix: route the channel ingest host/path (IngestEndpoints[*].Url) to a mediapackagev2 ingest data-plane handler rather than the generic service-resolution router, and have that handler return 403 for a missing Authorization header and 403 with a signature-specific denial for an invalid SigV4 signature.
  Traces: check mediapackagev2.cmaf-ingest-native-protocol-round-trip (expected 403, got HTTP 404 UnknownService).

Cause 2 of 2: the HLS egress manifest URL returned by CreateOriginEndpoint is not authorization-gated — an anonymous GET before any resource policy grants mediapackagev2:GetObject returns 404 instead of 403.
  AWS: CreateOriginEndpoint returns HlsManifests[*].Url (Smithy: shape GetHlsManifestConfiguration, member Url, required, doc "The egress domain URL for stream delivery from MediaPackage"). Recorded behavior only (the check documents that this was replay-confirmed against real AWS and that an 'always 404' rule was disproven): a MediaPackage v2 origin endpoint grants no anonymous read until a resource policy explicitly allows mediapackagev2:GetObject, so an anonymous GET of the manifest URL is HTTP 403; only AFTER PutOriginEndpointPolicy grants Principal "*" / Action mediapackagev2:GetObject does the same GET become HTTP 404 (endpoint live, no content ingested yet). The requirement is the 403-before-grant / 404-after-grant distinction.
  Emulator: the manifest URL is reachable but answers HTTP 404 for the anonymous GET made before any policy was put, i.e. the endpoint's resource policy is not consulted on the egress path at all and the unauthorized and authorized-but-empty cases are collapsed into one status.
  Why: a player or test that asserts the endpoint is closed by default cannot tell a locked endpoint from a nonexistent one; code that reacts to 403 by attaching or fixing the origin-endpoint policy never runs, and an emulator that serves egress without consulting the policy silently validates configurations real AWS would deny.
  Fix: the mediapackagev2 egress data-plane handler for the origin-endpoint manifest path — evaluate the origin endpoint's resource policy (the document written by PutOriginEndpointPolicy) before serving, returning 403 when no mediapackagev2:GetObject grant applies and 404 only when access is allowed but no content exists.
  Traces: check mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable (expected 403 pre-grant from .../out/v1/<group>/<channel>/<endpoint>/index.m3u8, got 404).

Not included: the other nine failing mediapackagev2 checks (cancel-harvest-job-unknown-job-name-not-found, cancel-harvest-job-unknown-job-name-not-found-v2, create-harvest-job-end-time-before-start-time-rejected, harvest-job-end-time-before-start-time-rejected, create-origin-endpoint-container-manifest-mismatch-rejected, origin-endpoint-container-manifest-mismatch-rejected, put-channel-policy-invalid-json-rejected, put-origin-endpoint-policy-invalid-policy-rejected, tag-resource-nonexistent-resource-arn-rejected) match AWS on HTTP status, error code and (where asserted) ValidationExceptionType, and differ ONLY in the wording of the human-readable Message, whose generating rule is neither modeled nor documented — so they are not filed as defects. The open issue 20261022T103649Z-mediapackagev2-emulator-diverges-from-recorded-aws-responses-missing.md covers wire-level serialization/validation causes (harvest-job S3 validation, policy text round-trip, ETag encoding, duplicate ingest hostnames, 204 content-length) and does NOT cover either data-plane cause above; 20261022T023331Z-mediapackagev2-listtagsforresource-stub.md covers a different operation.

## Evidence
Behavioral suite: 55/66 passed (83.3%), mismatched 11, errored 0, skipped 0, bootstrap_signal false; all failures in mediapackagev2.

Cause 1 — mediapackagev2.cmaf-ingest-native-protocol-round-trip, check line 59:
  assert no_auth_status == 403
  AssertionError: expected 403 for an unauthenticated push, got HTTP 404: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  expected: HTTP 403 with an 'Access denied' body for the unauthenticated PUT, then HTTP 403 with a 'signature'-specific, DIFFERENT body for the wrongly-signed PUT.
  actual:   HTTP 404, generic UnknownService routing error; the second (wrongly-signed) negative class is never reached.

Cause 2 — mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable, check line 59:
  assert denied_status == 403
  AssertionError: expected 403 (no read policy granted yet) from https://<egress-host>/out/v1/<group>/<channel>/<endpoint>/index.m3u8, got 404
  expected: 403 for the anonymous GET issued BEFORE PutOriginEndpointPolicy, and 404 for the same GET AFTER a Principal "*" / mediapackagev2:GetObject grant.
  actual:   404 already before any policy exists (host reachable, policy not consulted).
  (Host labels, generated group/channel/endpoint names and request ids in the URL are environment-generated and are not part of the requirement.)

## Steps to Reproduce
Boot the emulator from this checkout and replay the mediapackagev2 behavioral checks.

Cause 1 (check mediapackagev2.cmaf-ingest-native-protocol-round-trip): CreateChannelGroup, then CreateChannel(InputType=HLS); take IngestEndpoints[0].Url and issue a plain HTTPS PUT of arbitrary bytes to it with NO Authorization header. Expect HTTP 403 with an 'Access denied' body; observe HTTP 404 with {"__type":"UnknownService", ...}. Then repeat the PUT with a SigV4 Authorization header signed with control-plane credentials for service 'mediapackagev2' and expect a DIFFERENT 403 whose body is signature-specific.

Cause 2 (check mediapackagev2.origin-endpoint-manifest-url-data-plane-reachable): CreateChannelGroup, CreateChannel, then CreateOriginEndpoint(ContainerType=TS, HlsManifests=[{ManifestName: index}]); GET HlsManifests[0].Url anonymously BEFORE putting any policy and expect HTTP 403; observe 404. Then PutOriginEndpointPolicy with {"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject","Resource":<endpoint Arn>} and GET again, expecting HTTP 404 (endpoint live, no content ingested).
