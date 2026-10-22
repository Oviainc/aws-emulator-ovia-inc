Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.ingest-endpoint-wrong-credentials-rejected mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused mediapackage.native-ingest-endpoint-requires-basic-auth mediapackage.origin-endpoint-origination-deny-blocks-playback mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.ingest-endpoint-wrong-credentials-rejected|run#15|AssertionError|assert wrong_status == 403, f"expected 403 for wrong ingest password, got {wrong_status}"
Issue-Key: mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused|run#10|AssertionError|assert wrong_status == 403, f"expected 403 for bad ingest basic-auth credentials, got {wrong_status}"
Issue-Key: mediapackage.native-ingest-endpoint-requires-basic-auth|_digest_put#10|AssertionError|assert challenge, "expected a WWW-Authenticate challenge on the 401 probe"
Issue-Key: mediapackage.origin-endpoint-origination-deny-blocks-playback|_wait_for_status#15|Timeout|time.sleep(interval_s)
Issue-Key: mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip|_wait_for_status#13|Timeout|time.sleep(interval_s)
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# MediaPackage data plane: HLS ingest URL is unrouted (404, no Digest challenge) and the playback URL ignores Origination=DENY and Whitelist (4 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel (HlsIngest ingest URL data plane), CreateOriginEndpoint/UpdateOriginEndpoint (playback URL data plane)

## Description
Four distinct data-plane root causes in the mediapackage emulator (crate services/service-com-amazonaws-mediapackage-mediapackage). The control plane is healthy in all four (CreateChannel/DescribeChannel/CreateOriginEndpoint/UpdateOriginEndpoint all answer 200 and the published URLs pass the harness's published_url_mismatch check), so these are purely the egress/ingress listeners the generated Urls point at.

Cause 1 of 4: a PUT to the generated HlsIngest IngestEndpoint Url answers 404 instead of a credential/scheme refusal.
AWS: recorded behavior only (the ingest data plane is not modelled in Smithy — CreateChannel merely returns HlsIngest.IngestEndpoints[*].{Id,Url,Username,Password}). The recordings show that listener alive and refusing HTTP Basic: a PUT presenting `Authorization: Basic <generated user:pass>` and a PUT presenting Basic with a wrong password both draw the identical bare HTTP 403 with no WWW-Authenticate header, identically on the base Url and on /index.m3u8 and /seg1.ts sub-paths.
Emulator: the same PUT returns HTTP 404 — i.e. the ingest Url the control plane publishes is not routed to any listener at all, so there is no 403 refusal class and no "wrong vs correct credentials are indistinguishable" equivalence.
Why: an encoder (or any client that pushes to the Url CreateChannel handed it) cannot tell "your push was refused" from "this endpoint does not exist"; 404 is a routing/ existence signal, 403 an authorization one, and clients branch differently on them.
Fix: the ingest data-plane listener for the published HlsIngest Url (the channel's ingest routing/handler and the URL the CreateChannel handler publishes) must exist and answer 403 for a Basic-authenticated PUT.
Traces: mediapackage.ingest-endpoint-wrong-credentials-rejected, mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused

Cause 2 of 4: an unauthenticated PUT to the ingest Url does not return a 401 carrying a `WWW-Authenticate: Digest realm="WebDAV"` challenge.
AWS: recorded behavior only. A PUT with no Authorization header at all is challenged with HTTP 401 and `WWW-Authenticate: Digest realm="WebDAV", ..., algorithm=MD5, qop="auth"` (RFC 2617); the generated Username/Password then authenticate over Digest, and a wrong-password Digest response is refused with 401. The challenge parameters nonce/opaque are per-response values, not requirements — the requirement is a 401 whose WWW-Authenticate advertises Digest with realm="WebDAV".
Emulator: no WWW-Authenticate header is present on the unauthenticated probe at all (the check fails on `assert challenge` after the 401 probe), so no Digest round-trip is possible and credential validation on the ingest plane is unverifiable.
Why: a Digest client is driven entirely by the challenge — with no WWW-Authenticate it cannot compute a response and can never push content, and it also cannot distinguish a credential-gated listener from a closed one.
Fix: the ingest data-plane listener's authentication layer: emit 401 + `WWW-Authenticate: Digest realm="WebDAV", algorithm=MD5, qop="auth"` for an unauthenticated PUT and validate the channel's generated Username/Password over Digest (401 on a wrong password).
Traces: mediapackage.native-ingest-endpoint-requires-basic-auth, mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused

Cause 3 of 4: the origin endpoint's playback Url ignores Origination=DENY.
AWS: Smithy — CreateOriginEndpoint/UpdateOriginEndpoint carry Origination (enum Origination, jsonName "origination"); recorded behavior: with Origination=DENY, GETs against the endpoint's live playback Url are rejected with HTTP 403, and toggling back to ALLOW removes that rejection on the identical Url.
Emulator: the control-plane field round-trips (DescribeOriginEndpoint reports DENY), but the playback Url never returns 403 — the check polls the Url and never observes 403, so it exhausts its budget and the check is killed at the 60s harness timeout.
Why: Origination=DENY is an access control. A client (or an operator shutting off distribution) that sets DENY believes playback is blocked while the emulator keeps serving it — an overly permissive divergence that also makes any test of the DENY path hang rather than fail fast.
Fix: the origin-endpoint playback/egress handler must consult the endpoint's stored Origination and answer 403 while it is DENY.
Traces: mediapackage.origin-endpoint-origination-deny-blocks-playback

Cause 4 of 4: the origin endpoint's playback Url ignores Whitelist (source-IP CIDR allow list).
AWS: Smithy — CreateOriginEndpoint/UpdateOriginEndpoint carry Whitelist (__listOf__string, jsonName "whitelist"); recorded behavior: an endpoint whose Whitelist excludes the caller's source IP rejects playback GETs with HTTP 403, a sibling endpoint on the same channel with no Whitelist is not blocked, and clearing the Whitelist restores playback on the identical Url.
Emulator: the Whitelist field round-trips through Create/Update/Describe, but the playback Url never returns 403 for a caller outside the whitelist — the check polls for 403, never sees it, and is killed at the 60s harness timeout.
Why: the whitelist is the endpoint's only source-IP access control; an emulator that stores it but never enforces it reports a secured endpoint while serving every caller.
Fix: the origin-endpoint playback/egress handler must evaluate the caller's source address against the endpoint's stored Whitelist CIDRs and answer 403 when it does not match (and serve normally when the list is empty).
Traces: mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip

Related but separate: the open issues "MediaPackage VOD: egress manifest URL serves nothing ..." and the mediapackagev2 divergence issue describe comparable egress gaps in the mediapackage-vod and mediapackagev2 services; neither covers these mediapackage (v1) checks.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), service mediapackage: 35/58 passed, 23 failed.

Cause 1 — [mismatch] mediapackage.ingest-endpoint-wrong-credentials-rejected:
  AssertionError: expected 403 for wrong ingest password, got 404
  (the same check's control-plane positive controls — CreateChannel credentials present, DescribeChannel echoing the same Username, published_url_mismatch clean — all passed before this line)
  [mismatch] mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused:
  AssertionError: expected 403 for bad ingest basic-auth credentials, got 404

Cause 2 — [mismatch] mediapackage.native-ingest-endpoint-requires-basic-auth:
  AssertionError: expected a WWW-Authenticate challenge on the 401 probe
  (raised in _digest_put after _put_status_unauthenticated returned no WWW-Authenticate header; the same unauthenticated-PUT arm of ingest-endpoint-webdav-basic-auth-scheme-refused asserts 401 + `Digest realm="WebDAV"`)

Cause 3 — [mismatch] mediapackage.origin-endpoint-origination-deny-blocks-playback:
  Error: Check timed out after 60.0s — the check sets Origination=DENY (DescribeOriginEndpoint confirms 'DENY') and then polls GET <endpoint Url> for status==403 on a 90s deadline; the emulator never answers 403, so the poll outlives the harness timeout.
  expected: HTTP 403 on playback while Origination=DENY, non-403 after reverting to ALLOW.

Cause 4 — [mismatch] mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip:
  Error: Check timed out after 60.0s — endpoint created with Whitelist=["203.0.113.0/32"] (RFC 5737 TEST-NET-3, never the caller's IP); sibling endpoint with no whitelist correctly non-403; polling the whitelisted endpoint's Url for 403 never succeeds.
  expected: HTTP 403 for a non-whitelisted caller, non-403 once the Whitelist is cleared.

Model evidence: smithy_query(operation="CreateOriginEndpoint") on services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json shows Origination (enum) and Whitelist (__listOf__string) as input and output members; the ingest/playback wire protocols themselves are not modelled, hence "recorded behavior only" for causes 1-2.

## Steps to Reproduce
1. Build/boot the emulator from the checkout and point boto3 at it (region us-west-2).
2. Causes 1-2: created = mp.create_channel(Id="c1"); take ep = created["HlsIngest"]["IngestEndpoints"][0]. PUT bytes to ep["Url"] with `Authorization: Basic base64(ep[Username]:ep[Password])` and again with a wrong password -> both return 404; AWS returns an identical bare 403 with no WWW-Authenticate. Then PUT with no Authorization header -> no WWW-Authenticate header is returned; AWS answers 401 with `WWW-Authenticate: Digest realm="WebDAV", algorithm=MD5, qop="auth"` and accepts a correctly computed Digest response for the generated credentials while refusing a wrong password with 401.
3. Cause 3: oe = mp.create_origin_endpoint(Id="oe1", ChannelId="c1", HlsPackage={}); GET oe["Url"] (non-403 under default ALLOW); mp.update_origin_endpoint(Id="oe1", HlsPackage={}, Origination="DENY"); GET oe["Url"] repeatedly -> never 403; AWS returns 403 while DENY and stops once reverted to ALLOW.
4. Cause 4: mp.create_origin_endpoint(Id="oe-blocked", ChannelId="c1", HlsPackage={}, Whitelist=["203.0.113.0/32"]); GET its Url repeatedly -> never 403; AWS returns 403 for a caller outside the list and serves again after mp.update_origin_endpoint(..., Whitelist=[]).
5. Or replay directly: run_behavioral_suite and re-run the four check ids in trace_ids.
