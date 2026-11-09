Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-conflict-error-paths
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-conflict-error-paths mediapackage-harvest-job-workflow mediapackage-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateOriginEndpoint|2xx->2xx|$.url
Issue-Key: mediapackage|DescribeOriginEndpoint|2xx->2xx|$.url
Issue-Key: mediapackage|ListOriginEndpoints|2xx->2xx|$.originEndpoints[*].url
Issue-Key: mediapackage|UpdateOriginEndpoint|2xx->2xx|$.url
Issue-Commit: 0c7f0017b5788ebb0b2bf7aa1a6523722bad5325

# Origin-endpoint playback url advertises an AWS-owned host the emulator does not serve

> _Note: this issue was identified against the code base at commit `0c7f0017b5788ebb0b2bf7aa1a6523722bad5325`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint

## Description
Wire replay of simparity-wire-aws-v1 shows the same host defect as the channel ingest surface, on the mediapackage (v1) origin-endpoint PLAYBACK url: every `url` the emulator returns for an OriginEndpoint is minted under the PUBLIC AWS domain `https://<label>.mediapackage.<region>.amazonaws.com/out/v1/<endpoint-label>/index.m3u8` rather than under a host the emulator itself serves, so the replayer's `host_not_own` verdict fires on each one — the emulator announced an address other than the one it is reached on. On the create path the fixture's expected side is the unresolved placeholder `{{endpoint_url}}`, which confirms what the recording asserts: the url must be rooted at the endpoint the client is talking to.

The underlying fault is one shared playback-URL minting/persisting path, but each operation serializes a different member path (`$.url` vs list-wrapped `$.originEndpoints[n].url`) and the replay clusters them separately, so fixing only the create path leaves the read, list and update paths diverging. This is a FOUR-cause bundle; a fixer MUST close all four causes below and re-replay every fixture/step listed per cause before submitting.

Related but NOT covering: the open behavioral issue 20261107T231715Z-mediapackage-origin-endpoint-playback-url-is-unrouted-404.md covers what the advertised url answers once dereferenced (404 where AWS returns 403 under Origination=DENY / a non-matching Whitelist). That is a different defect — the STATUS of the data-plane listener — and it owns only behavioral check ids, not any wire replay key here. Fixing the host advertised in the control-plane response is a precondition for that issue, not a duplicate of it. The analogous mediapackagev2 issue 20261109T115329Z-mediapackagev2-origin-endpoint-manifest-urls-advertise-aws-owned-egress.md is a different service and does not cover mediapackage v1.

Cause 1 of 4: CreateOriginEndpoint's url host is not the emulator's.
  AWS: Smithy CreateOriginEndpoint -> POST /origin_endpoints, code 200; output member Url (jsonName "url", __string) is documented on shape OriginEndpoint as "The URL of the packaged OriginEndpoint for consumption" — i.e. the address a player fetches, which must therefore be an address the responder answers on. The path incorporates the endpoint id and the ManifestName ("A short string appended to the end of the OriginEndpoint URL"). Recorded behavior only for the host component: the fixture asserts the url against `{{endpoint_url}}`, the endpoint the client is talking to. The generated label is per environment and is NOT the requirement; the host being servable by the responder is. Status 200 matched.
  Emulator: `$.url` is returned as `https://<label>.mediapackage.<region>.amazonaws.com/out/v1/<endpoint-label>/index.m3u8` — host_not_own, status 200. The `/out/v1/<label>/<manifest>.m3u8` path shape is correct; only the host is a public AWS name the emulator does not serve.
  Why: a player handed this url fetches from real AWS (or fails name resolution offline) instead of the emulator, so the documented playback contract cannot be exercised against the emulator at all even though the control plane reported success — and the access controls on that url (Origination, Whitelist) can never be observed.
  Fix: the CreateOriginEndpoint handler's playback-URL minting, and the origin-endpoint state record that persists the url — the host must be derived from the emulator's advertised endpoint instead of a hard-wired amazonaws.com template.
  Traces: mediapackage-conflict-error-paths#setup#2 (cluster also covers mediapackage-harvest-job-workflow#setup#2 and mediapackage-origin-endpoint-lifecycle#task#2).

Cause 2 of 4: DescribeOriginEndpoint re-serves the unservable url.
  AWS: Smithy DescribeOriginEndpoint -> GET /origin_endpoints/{Id}, code 200; output Url as in cause 1 — a read re-reports the same reachable playback url the create returned.
  Emulator: `$.url` host_not_own, status 200 (the string round-trips byte-for-byte against the recording; the verdict is that the host is not one the emulator serves).
  Why: reading an endpoint back is the normal way a client obtains its playback url, so a client that discovers rather than creates is pointed off-emulator identically; and because the wrong host is PERSISTED, the read path keeps leaking it even after the create path is fixed.
  Fix: the DescribeOriginEndpoint response serializer, plus deriving the host at read time so already-stored records stop serving the public-AWS host.
  Traces: mediapackage-origin-endpoint-lifecycle#task#3 (cluster also covers mediapackage-origin-endpoint-lifecycle#task#6).

Cause 3 of 4: ListOriginEndpoints emits an unservable host inside originEndpoints[].url.
  AWS: Smithy ListOriginEndpoints -> GET /origin_endpoints, code 200, paginated; output OriginEndpoints (jsonName "originEndpoints") whose elements each carry that endpoint's reachable playback Url.
  Emulator: `$.originEndpoints[0].url` host_not_own, status 200 — the list-wrapped serializer uses the same public-AWS host.
  Why: enumeration is how a client discovers endpoints it did not create, so every playback url harvested from a listing is unreachable and bulk/playlist workflows built on ListOriginEndpoints all point off-emulator.
  Fix: the ListOriginEndpoints response serializer / the shared playback-URL derivation used when projecting state records into list items.
  Traces: mediapackage-origin-endpoint-lifecycle#task#4.

Cause 4 of 4: UpdateOriginEndpoint re-serves the unservable url.
  AWS: Smithy UpdateOriginEndpoint -> PUT /origin_endpoints/{Id}, code 200; output Url — the playback url survives an update and is re-reported, still reachable.
  Emulator: `$.url` host_not_own, status 200.
  Why: a client that reconfigures an endpoint and re-reads the playback url from the update response is pointed off-emulator again, so the modify path must be fixed alongside create, read and list.
  Fix: the UpdateOriginEndpoint response serializer / shared derivation.
  Traces: mediapackage-origin-endpoint-lifecycle#task#5.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, emulator_up=true ("/__admin/health answered 200 and the :32855 data listener accepts"), bootstrap_signal=false, 7 fixtures replayed, 65 steps, 36 passed, 29 mismatched, score 55.4%, 14 divergence clusters. None of the four clusters below carried a stub_path or an owned_by.

Cause 1 - mediapackage-conflict-error-paths#setup#2, CreateOriginEndpoint, expected 200 / actual 200, affected_count 3:
  $.url: host_not_own (expected '{{endpoint_url}}', got 'https://be07297cf53abd5e9f21bf454fd80cd7.mediapackage.us-west-2.amazonaws.com/out/v1/1b2ffd71a30c2cdafa4452721ec26247/index.m3u8')
  divergence_key service=mediapackage|CreateOriginEndpoint|status=200->200|keys=$.url
  The expected side is the unresolved placeholder for the emulator's own endpoint, so the path/label difference is not the finding — the unservable host is. The quoted label is one run's generated value and is not part of the requirement.

Cause 2 - mediapackage-origin-endpoint-lifecycle#task#3, DescribeOriginEndpoint, 200/200, affected_count 2 (also #task#6):
  $.url: host_not_own (expected and actual are the identical string 'https://<label>.mediapackage.us-west-2.amazonaws.com/out/v1/<endpoint-label>/index.m3u8'; the verdict is that the host is not one the emulator serves).

Cause 3 - mediapackage-origin-endpoint-lifecycle#task#4, ListOriginEndpoints, 200/200:
  $.originEndpoints[0].url: host_not_own, same host and path.

Cause 4 - mediapackage-origin-endpoint-lifecycle#task#5, UpdateOriginEndpoint, 200/200:
  $.url: host_not_own, same host and path.

In all four clusters the HTTP status and every non-address member matched; the divergence is confined to the host component of the advertised playback URL.

Model evidence: smithy_query(query="shape", shape="OriginEndpoint", model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json") shows OriginEndpoint.Url (jsonName "url", __string) documented "The URL of the packaged OriginEndpoint for consumption", alongside ManifestName ("A short string appended to the end of the OriginEndpoint URL"), Origination and Whitelist. The playback wire protocol itself is not modeled, hence "recorded behavior only" for the host requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus; the four clusters above reappear as mismatches with verdict host_not_own.
2. Or reproduce directly with boto3 pointed at the emulator endpoint (region us-west-2):
   a. Setup: mp.create_channel(Id="ch1").
   b. Cause 1: oe = mp.create_origin_endpoint(Id="oe1", ChannelId="ch1", HlsPackage={}) and read oe["Url"]. Fixture: mediapackage-conflict-error-paths#setup#2.
   c. Cause 2: mp.describe_origin_endpoint(Id="oe1")["Url"]. Fixture: mediapackage-origin-endpoint-lifecycle#task#3.
   d. Cause 3: mp.list_origin_endpoints() and read OriginEndpoints[0]["Url"]. Fixture: mediapackage-origin-endpoint-lifecycle#task#4.
   e. Cause 4: mp.update_origin_endpoint(Id="oe1", HlsPackage={}, Description="d2") and read ["Url"]. Fixture: mediapackage-origin-endpoint-lifecycle#task#5.
3. In every case the host is `*.mediapackage.<region>.amazonaws.com`; an HTTP GET of the advertised url never reaches the emulator, while the control-plane call itself was made to the emulator's own endpoint.

Pass criterion for all four: the host in the origin-endpoint `url` (including the list-wrapped originEndpoints[n] form, and for records created BEFORE the fix) must be one the emulator answers on, derived from the endpoint the request arrived on, so a player can fetch the advertised url without leaving the emulator. Generated labels and ports may differ freely; the `/out/v1/<endpoint-label>/<manifestName>.m3u8` path structure and its per-endpoint uniqueness must be preserved.
