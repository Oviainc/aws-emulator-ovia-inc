Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness mediapackagev2-multiple-origin-endpoints-isolation mediapackagev2-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|CreateOriginEndpoint|2xx->2xx|$.HlsManifests[*].Url
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.HlsManifests[*].Url
Issue-Key: mediapackagev2|ListOriginEndpoints|2xx->2xx|$.Items[*].HlsManifests[*].Url
Issue-Key: mediapackagev2|UpdateOriginEndpoint|2xx->2xx|$.HlsManifests[*].Url
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# Origin-endpoint manifest URLs advertise AWS-owned egress hostnames the emulator does not serve

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOriginEndpoint

## Description
Wire replay of simparity-wire-aws-v1 shows the same host defect as the channel surface, on the origin-endpoint manifest URLs: every HlsManifests[].Url the emulator returns is minted under the PUBLIC AWS domain `<label>.egress.<account-label>.mediapackagev2.<region>.amazonaws.com` rather than under a host the emulator itself serves, so the replayer's `host_not_own` verdict fires on each one — the emulator announced an address other than the one it is reached on. This is a SEVEN-cause bundle; a fixer MUST close all seven causes below and re-replay every fixture/step listed per cause before submitting.

The underlying fault is one shared manifest-URL minting path, but each operation serializes a different member path (single manifest, multiple manifests, list-wrapped `$.Items[n]`) and the replay clusters them separately, so fixing only the create path leaves the read, update and list paths diverging.

Note: the open behavioral issue 20261029T201731Z-mediapackagev2-five-rejection-messages-worded-differently-from-aws.md (cause 7 of its bundle) covers the STATUS the manifest data plane returns once reached (404 where AWS returns 403 before a read policy is granted). That is a different defect from the host advertised in the control-plane response and does not cover any cause here; no other open mediapackagev2 issue does either.

Cause 1 of 7: CreateOriginEndpoint's HlsManifests[].Url hosts are not the emulator's, with multiple manifests.
  AWS: Smithy CreateOriginEndpoint -> POST /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint, code 200; output HlsManifests is a list of GetHlsManifestConfiguration whose Url (String, @required) is documented as "The egress domain URL for stream delivery from MediaPackage." Each manifest in the list gets its own distinct URL ending in that manifest's ManifestName, and the URL is the address a player fetches — i.e. a host the responder answers on. Recorded behavior only for the host component: the recording shows the responder returning URLs rooted at the endpoint the client is talking to. The generated label is per environment and is NOT the requirement; the host being servable by the responder is.
  Emulator: both HlsManifests[0].Url and HlsManifests[1].Url are returned as `https://<label>.egress.<account-label>.mediapackagev2.<region>.amazonaws.com/out/v1/<group>/<channel>/<endpoint>/<manifest>.m3u8` — host_not_own on both. Status 200, the per-manifest uniqueness and the path structure are correct; only the host is a public AWS name the emulator does not serve.
  Why: a player handed these URLs fetches from real AWS (or fails name resolution offline) instead of the emulator, so the documented playback contract cannot be exercised against the emulator at all even though the control plane reported success.
  Fix: the CreateOriginEndpoint handler's manifest-URL minting (and the origin-endpoint state record that persists the URLs) — the host must be derived from the emulator's advertised endpoint instead of a hard-wired amazonaws.com template.
  Traces: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3.

Cause 2 of 7: GetOriginEndpoint re-serves both unservable multi-manifest URLs.
  AWS: Smithy GetOriginEndpoint output HlsManifests (list of GetHlsManifestConfiguration, Url @required) — a read re-reports the same reachable URLs the create returned, one per manifest.
  Emulator: HlsManifests[0].Url and HlsManifests[1].Url both host_not_own, status 200.
  Why: a client that discovers an existing endpoint rather than creating it is pointed off-emulator identically, and because the wrong host is persisted, the read path keeps leaking it even after the create path is fixed.
  Fix: the GetOriginEndpoint response serializer, plus deriving the host at read time so already-stored records stop serving the public-AWS host.
  Traces: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#4.

Cause 3 of 7: CreateOriginEndpoint's single HlsManifests[0].Url host is not the emulator's.
  AWS: as cause 1, for an endpoint created with a single HLS manifest — Url (@required) is the egress address for stream delivery and must be servable by the responder.
  Emulator: HlsManifests[0].Url returned as `https://<label>.egress.<account-label>.mediapackagev2.<region>.amazonaws.com/out/v1/<group>/<channel>/<endpoint>/<manifest>.m3u8`, host_not_own, status 200. (The fixture's expected value here is an unresolved placeholder, so only the host verdict is the finding; the emulator's path structure is correct.)
  Why: as cause 1 — the single-manifest shape is the common case, so most clients get an unreachable playback URL.
  Fix: the same CreateOriginEndpoint manifest-URL minting path.
  Traces: mediapackagev2-multiple-origin-endpoints-isolation#task#3 (also #task#4).

Cause 4 of 7: ListOriginEndpoints emits unservable hosts inside Items[].HlsManifests[].Url for multiple endpoints.
  AWS: Smithy ListOriginEndpoints returns Items, each carrying that endpoint's HlsManifests with their egress Urls; each listed endpoint keeps its own distinct, reachable manifest URL.
  Emulator: $.Items[0].HlsManifests[0].Url and $.Items[1].HlsManifests[0].Url are both host_not_own, status 200 — the list-wrapped serializer uses the same public-AWS host.
  Why: enumeration is how a client discovers endpoints it did not create; every URL it harvests from a listing is unreachable, so bulk/playlist workflows built on ListOriginEndpoints all point off-emulator.
  Fix: the ListOriginEndpoints response serializer / the shared manifest-URL derivation used when projecting state records into list items.
  Traces: mediapackagev2-multiple-origin-endpoints-isolation#task#5.

Cause 5 of 7: GetOriginEndpoint re-serves the unservable single-manifest URL.
  AWS: as cause 2, for the single-manifest shape — the read re-reports a reachable Url.
  Emulator: HlsManifests[0].Url host_not_own, status 200.
  Why: as cause 2; this is the widest-blast-radius cluster in the corpus (four affected steps across two fixtures), because reading an endpoint back is the normal way a client obtains its playback URL.
  Fix: the GetOriginEndpoint response serializer / shared derivation.
  Traces: mediapackagev2-multiple-origin-endpoints-isolation#task#6 (also #task#7, mediapackagev2-origin-endpoint-lifecycle#task#4, mediapackagev2-origin-endpoint-lifecycle#task#6).

Cause 6 of 7: UpdateOriginEndpoint re-serves the unservable manifest URL.
  AWS: Smithy UpdateOriginEndpoint output HlsManifests (list of GetHlsManifestConfiguration, Url @required) — the manifest URL survives an update and is re-reported, still reachable.
  Emulator: HlsManifests[0].Url host_not_own, status 200.
  Why: a client that reconfigures an endpoint and re-reads the playback URL from the update response is pointed off-emulator again, so the modify path must be fixed alongside create and read.
  Fix: the UpdateOriginEndpoint response serializer / shared derivation.
  Traces: mediapackagev2-origin-endpoint-lifecycle#task#5.

Cause 7 of 7: ListOriginEndpoints emits an unservable host inside Items[0].HlsManifests[0].Url for a single endpoint.
  AWS: as cause 4, for a listing with one endpoint.
  Emulator: $.Items[0].HlsManifests[0].Url host_not_own, status 200.
  Why: as cause 4 — the single-item listing is clustered separately by the replay, so it must be re-replayed separately to confirm the fix.
  Fix: the ListOriginEndpoints response serializer / shared derivation.
  Traces: mediapackagev2-origin-endpoint-lifecycle#task#7.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, 96 steps, 72 passed, 24 mismatched, score 75.0%. No cluster below carried a stub_path or an owned_by.

Cause 1 — mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3, CreateOriginEndpoint, expected 200 / actual 200:
  $.HlsManifests[0].Url and $.HlsManifests[1].Url: host_not_own. Emulator returned `https://<label>.egress.<account-label>.mediapackagev2.us-west-2.amazonaws.com/out/v1/<group>/<channel>/<endpoint>/primary.m3u8` and the `secondary.m3u8` sibling. divergence_key service=mediapackagev2|CreateOriginEndpoint|status=200->200|keys=$.HlsManifests[0].Url,$.HlsManifests[1].Url.

Cause 2 — same fixture #task#4, GetOriginEndpoint, 200/200: both manifest Urls host_not_own (strings round-trip byte-for-byte against the recording; the verdict is that the host is not one the emulator serves).

Cause 3 — mediapackagev2-multiple-origin-endpoints-isolation#task#3, CreateOriginEndpoint, 200/200:
  $.HlsManifests[0].Url: host_not_own — emulator `https://<label>.egress.<account-label>.mediapackagev2.us-west-2.amazonaws.com/out/v1/<group>/<channel>/<endpoint-a>/index-a.m3u8`. The fixture's expected side is the unresolved placeholder `https://placeholder.egress.mediapackagev2.us-west-2.amazonaws.com/out/v1/placeholder/index-a.m3u8`, so the path/label difference is not the finding — the unservable host is. Cluster also covers #task#4.

Cause 4 — mediapackagev2-multiple-origin-endpoints-isolation#task#5, ListOriginEndpoints, 200/200:
  $.Items[0].HlsManifests[0].Url and $.Items[1].HlsManifests[0].Url: both host_not_own (per-endpoint paths .../<endpoint-a>/index-a.m3u8 and .../<endpoint-b>/index-b.m3u8 are correct and distinct).

Cause 5 — mediapackagev2-multiple-origin-endpoints-isolation#task#6, GetOriginEndpoint, 200/200:
  $.HlsManifests[0].Url: host_not_own. affected_count 4 — also #task#7, mediapackagev2-origin-endpoint-lifecycle#task#4 and #task#6.

Cause 6 — mediapackagev2-origin-endpoint-lifecycle#task#5, UpdateOriginEndpoint, 200/200:
  $.HlsManifests[0].Url: host_not_own.

Cause 7 — mediapackagev2-origin-endpoint-lifecycle#task#7, ListOriginEndpoints, 200/200:
  $.Items[0].HlsManifests[0].Url: host_not_own.

In all seven clusters the HTTP status and every non-address member matched; the divergence is confined to the host component of the advertised manifest URL.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus, or reproduce directly with botocore against the emulator endpoint:

Setup: CreateChannelGroup(g) -> CreateChannel(g, c).

Cause 1/2: CreateOriginEndpoint(g, c, e, ContainerType='TS', HlsManifests=[{'ManifestName':'primary'},{'ManifestName':'secondary'}]) and read HlsManifests[*].Url; then GetOriginEndpoint(g, c, e) and read the same members. Fixtures: mediapackagev2-create-origin-endpoint-multiple-hls-manifests-url-uniqueness#task#3, #task#4.

Cause 3/5: CreateOriginEndpoint with a single HLS manifest and read HlsManifests[0].Url; GetOriginEndpoint and read it again. Fixtures: mediapackagev2-multiple-origin-endpoints-isolation#task#3, #task#6.

Cause 4/7: create two endpoints (then one) under the same channel and call ListOriginEndpoints(g, c); read Items[*].HlsManifests[0].Url. Fixtures: mediapackagev2-multiple-origin-endpoints-isolation#task#5, mediapackagev2-origin-endpoint-lifecycle#task#7.

Cause 6: UpdateOriginEndpoint on an existing endpoint and read HlsManifests[0].Url from the update response. Fixture: mediapackagev2-origin-endpoint-lifecycle#task#5.

In every case the host is `*.mediapackagev2.<region>.amazonaws.com`; an HTTP GET of the advertised URL never reaches the emulator, while the control-plane call itself was made to the emulator's own endpoint.

Pass criterion for all seven: the host in every HlsManifests[].Url (including the list-wrapped Items[n] form) must be one the emulator answers on, derived from the endpoint the request arrived on, so a player can fetch the advertised URL without leaving the emulator. Generated labels, ports and ids may differ freely; the per-manifest and per-endpoint path uniqueness must be preserved.
