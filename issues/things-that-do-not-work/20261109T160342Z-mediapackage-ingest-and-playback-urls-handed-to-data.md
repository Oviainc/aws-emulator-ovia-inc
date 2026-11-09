Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused mediapackage.ingest-endpoint-wrong-credentials-rejected mediapackage.native-ingest-endpoint-requires-basic-auth mediapackage.origin-endpoint-manifest-404-before-ingest mediapackage.origin-endpoint-origination-deny-blocks-playback mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused|_put#9|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: mediapackage.ingest-endpoint-wrong-credentials-rejected|run#10|AssertionError|assert not (p := ctx.published_url_mismatch(url)), p
Issue-Key: mediapackage.native-ingest-endpoint-requires-basic-auth|_put_status_unauthenticated#11|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: mediapackage.origin-endpoint-manifest-404-before-ingest|run#9|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: mediapackage.origin-endpoint-origination-deny-blocks-playback|_get_status#10|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Key: mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip|_get_status#11|PublishedUrlRejected|target_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: d92c3cc324fbe4ad920dfad809c989914fc83363

# mediapackage: ingest and playback URLs handed to data-plane clients advertise AWS-owned hosts the emulator does not serve (2 causes)

> _Note: this issue was identified against the code base at commit `d92c3cc324fbe4ad920dfad809c989914fc83363`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Six behavioral checks under `### mediapackage` fail before they can exercise the data plane at all, because the control-plane responses hand them URLs rooted at a PUBLIC AWS host (`https://<label>.mediapackage.<region>.amazonaws.com/...`) rather than at a host the emulator answers on. The harness rejects the URL with PublishedUrlRejected / published_url_mismatch: "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". This is the emulator's own defect — it announced an address other than the one it is reached on — and it blocks every ingest-auth and playback-authorization check downstream. TWO causes (two distinct URL-minting surfaces); a fixer MUST close both and re-replay all six checks in trace_ids.

Related but NOT covering: the open WIRE issues 20261109T142008Z-mediapackage-channel-hls-ingest-endpoint-urls-advertise-aws-owned.md and 20261109T142008Z-mediapackage-origin-endpoint-playback-url-advertises-an-aws-owned.md describe the same host defect from the wire corpus and own only wire replay keys (mediapackage|CreateChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url etc.) — they carry no behavioral check id, so none of the six checks here is owned by them; they should be closed together with this one. 20261102T091035Z-mediapackage-hls-ingest-endpoint-unauthenticated-put-returns-no.md (missing Digest challenge) and 20261107T231715Z-mediapackage-origin-endpoint-playback-url-is-unrouted-404.md (what the playback URL answers once reached) are about behavior AFTER the URL is dereferenced; they presuppose a reachable host and do not cover the advertised host.

Cause 1 of 2: the channel HLS ingest URL (HlsIngest.IngestEndpoints[].Url) is minted under a public AWS host.
  AWS: Smithy mediapackage CreateChannel -> POST /channels, 200; output HlsIngest (jsonName "hlsIngest") carries IngestEndpoints (jsonName "ingestEndpoints") whose IngestEndpoint.Url (jsonName "url") is documented "The ingest URL to which the source stream should be sent." — i.e. the address an encoder PUTs to, so it must be an address the responder itself answers on. DescribeChannel re-reports the same URL. Recorded behavior only for the host component: the recordings root the URL at the endpoint the client is talking to; the `/in/v2/<channel-label>/<endpoint-label>/channel` path shape and the generated labels are not the requirement, the servable host is.
  Emulator: CreateChannel returns `https://<label>.mediapackage.us-west-2.amazonaws.com/in/v2/<channel-label>/<endpoint-label>/channel`, a host the emulator does not serve, while the control-plane call itself arrived on the emulator's own listener. The harness refuses to dereference it (PublishedUrlRejected / published_url_mismatch).
  Why: an encoder handed this URL pushes its source stream at real AWS, or fails name resolution offline, and the 200 gives the client no hint that the address is unusable. Nothing about ingest — Basic-auth gating, the blanket 403 AWS now returns, WebDAV method handling — can be exercised against the emulator.
  Fix: the CreateChannel handler's ingest-URL minting and the channel state record that persists it, plus the read/list/update/rotate serializers that re-serve the stored value; the host must be derived from the endpoint the request arrived on instead of a hard-wired amazonaws.com template.
  Traces: mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused, mediapackage.ingest-endpoint-wrong-credentials-rejected, mediapackage.native-ingest-endpoint-requires-basic-auth.

Cause 2 of 2: the origin-endpoint playback URL (OriginEndpoint.Url) is minted under a public AWS host.
  AWS: Smithy mediapackage CreateOriginEndpoint -> POST /origin_endpoints, 200; OriginEndpoint.Url (jsonName "url") is documented "The URL of the packaged OriginEndpoint for consumption", with ManifestName "A short string appended to the end of the OriginEndpoint URL" — the address a player fetches, hence an address the responder answers on. DescribeOriginEndpoint re-reports it. Recorded behavior only for the host component.
  Emulator: CreateOriginEndpoint returns `https://<label>.mediapackage.us-west-2.amazonaws.com/out/v1/<endpoint-label>/index.m3u8`; the host is not one of the emulator's own, so the harness rejects it before any GET is made.
  Why: a player can never fetch the advertised manifest from the emulator, so the endpoint's access controls and its empty-channel behavior are unobservable: Origination=DENY, a non-matching Whitelist and the no-manifest-before-ingest case can none of them be verified, and any client that caches this URL leaves the emulator entirely.
  Fix: the CreateOriginEndpoint handler's playback-URL minting and the origin-endpoint state record that persists it, plus the describe/list/update serializers; derive the host from the emulator's advertised endpoint, at read time too, so records created before the fix stop serving the public-AWS host.
  Traces: mediapackage.origin-endpoint-manifest-404-before-ingest, mediapackage.origin-endpoint-origination-deny-blocks-playback, mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip.

## Evidence
Suite: simparity-behavioral-aws-v1, 52/60 passed; per-service line `mediapackage 52/58 (90%) 6 failed`. None of the six checks was marked ALREADY FILED or OWNED by.

Cause 1 (ingest URL):
  [mismatch] mediapackage.ingest-endpoint-wrong-credentials-rejected — check line 48: `assert not (p := ctx.published_url_mismatch(url)), p` ->
    AssertionError: the emulator published https://<label>.mediapackage.us-west-2.amazonaws.com/in/v2/<channel-label>/<endpoint-label>/channel: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.
    `url` is created["HlsIngest"]["IngestEndpoints"][0]["Url"] from CreateChannel; Username/Password were provisioned fine, so only the host is at fault.
  [mismatch] mediapackage.ingest-endpoint-webdav-basic-auth-scheme-refused and [mismatch] mediapackage.native-ingest-endpoint-requires-basic-auth — both raise simparity.published.PublishedUrlRejected inside context.py `endpoint_route` / `_published_target` for the same `/in/v2/.../channel` URL: "endpoint_url was given https://<label>.mediapackage.us-west-2.amazonaws.com/in/v2/<...>/<...>/channel (published by the emulator or built by the check): its host is not one of the emulator's own hosts".

Cause 2 (playback URL):
  [mismatch] mediapackage.origin-endpoint-manifest-404-before-ingest, [mismatch] mediapackage.origin-endpoint-origination-deny-blocks-playback, [mismatch] mediapackage.origin-endpoint-whitelist-blocks-non-whitelisted-ip — all three raise PublishedUrlRejected in context.py `endpoint_route`/`_published_target` on the origin-endpoint Url: "endpoint_url was given https://<label>.mediapackage.us-west-2.amazonaws.com/out/v1/<endpoint-label>/index.m3u8 ... its host is not one of the emulator's own hosts".

In every case the control-plane status was 200 and the non-address members matched; the divergence is confined to the host component of the advertised URL. The quoted labels, ports and ids are one run's generated values and are not part of the requirement — the `*.mediapackage.<region>.amazonaws.com` host is the defect.

Model evidence: the mediapackage Smithy model (services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json) documents IngestEndpoint.Url as "The ingest URL to which the source stream should be sent." and OriginEndpoint.Url as "The URL of the packaged OriginEndpoint for consumption". The ingest/playback wire protocols themselves are not modeled, hence "recorded behavior only" for the host requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite; the six mediapackage checks above fail with published-URL rejections.
2. Cause 1, directly with boto3 pointed at the emulator (region us-west-2):
   a. ch = mp.create_channel(Id="ch1"); read ch["HlsIngest"]["IngestEndpoints"][0]["Url"] (and [1]).
   b. Observed host `*.mediapackage.us-west-2.amazonaws.com`; expected a host the emulator answers on (e.g. the 127.0.0.1:<data port> it was called on, or a per-resource listener host). An HTTP PUT to the advertised URL never reaches the emulator.
   c. mp.describe_channel(Id="ch1") re-reports the same unservable host, so stored records must be fixed too.
3. Cause 2:
   a. oe = mp.create_origin_endpoint(ChannelId="ch1", Id="ch1-oe", HlsPackage={}); read oe["Url"].
   b. Observed `https://<label>.mediapackage.us-west-2.amazonaws.com/out/v1/<label>/index.m3u8`; expected an emulator-served host. A GET of it never reaches the emulator.
   c. mp.describe_origin_endpoint(Id="ch1-oe")["Url"] re-reports it.
4. Pass criterion for both causes: the host in every HlsIngest.IngestEndpoints[].Url and every OriginEndpoint.Url (on create, describe, list, update and credential-rotation paths, including records created before the fix) is one the emulator answers on, derived from the endpoint the request arrived on, so a PUT to the ingest URL and a GET of the playback URL both reach the emulator. Generated labels and ports may differ freely; the `/in/v2/<channel>/<endpoint>/channel` and `/out/v1/<endpoint>/<manifestName>.m3u8` path shapes and their per-resource uniqueness must be preserved.
