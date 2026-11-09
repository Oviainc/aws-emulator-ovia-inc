Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-channel-lifecycle-tags-logs
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-channel-lifecycle-tags-logs mediapackage-conflict-error-paths mediapackage-harvest-job-workflow mediapackage-listing-and-validation mediapackage-origin-endpoint-lifecycle mediapackage-credential-rotation
Issue-Shape-Id: com.amazonaws.mediapackage#MediaPackage
Issue-Key: mediapackage|CreateChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|DescribeChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|ListChannels|2xx->2xx|$.channels[*].hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|UpdateChannel|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|ConfigureLogs|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|RotateChannelCredentials|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Key: mediapackage|RotateIngestEndpointCredentials|2xx->2xx|$.hlsIngest.ingestEndpoints[*].url
Issue-Commit: 0c7f0017b5788ebb0b2bf7aa1a6523722bad5325

# Channel HLS ingest endpoint URLs advertise AWS-owned hosts the emulator does not serve

> _Note: this issue was identified against the code base at commit `0c7f0017b5788ebb0b2bf7aa1a6523722bad5325`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Wire replay of simparity-wire-aws-v1 against this checkout shows that EVERY mediapackage (v1) operation that projects a Channel emits its `hlsIngest.ingestEndpoints[].url` under the PUBLIC AWS domain `https://<label>.mediapackage.<region>.amazonaws.com/in/v2/<channel-label>/<endpoint-label>/channel` instead of under a host the emulator itself answers on. The replayer's `host_not_own` verdict fires on each such member: the emulator announced an address other than the one it is reached on (the control-plane call itself was made to the emulator's own data listener). In most clusters the expected and actual strings are byte-identical, which is exactly the point — the recording's host is a real AWS name, and re-serving that name verbatim is the defect, because the emulator is not that host.

The underlying fault is ONE shared ingest-URL minting/persisting path, but each operation serializes a different member path (top-level `$.hlsIngest...`, list-wrapped `$.channels[n].hlsIngest...`) and the replay clusters them separately, so fixing only the create path leaves the read, update, list, logging and credential-rotation paths diverging. This is an EIGHT-cause bundle; a fixer MUST close all eight causes below and re-replay every fixture/step listed per cause before submitting.

Related but NOT covering: the open behavioral issue 20261102T091035Z-mediapackage-hls-ingest-endpoint-unauthenticated-put-returns-no.md concerns the Digest challenge a PUT to the ingest URL must return once that URL is reached, and 20261022T213438Z-mediapackage-data-plane-hls-ingest-url-is-unrouted.md concerns the ingest URL not being routed. Both are about what happens AFTER the URL is dereferenced; neither owns any replay key here and neither covers the host advertised in the control-plane response. The two mediapackagev2 host issues (20261109T115329Z-mediapackagev2-channel-group-and-channel-responses-advertise-aws-owned.md and ...origin-endpoint-manifest-urls...) are the analogous defect on a DIFFERENT service (mediapackagev2) and do not cover mediapackage v1.

Cause 1 of 8: CreateChannel's hlsIngest.ingestEndpoints[].url hosts are not the emulator's.
  AWS: Smithy CreateChannel -> POST /channels, code 200; output member HlsIngest (jsonName "hlsIngest", shape HlsIngest) carries IngestEndpoints (jsonName "ingestEndpoints", list of IngestEndpoint). IngestEndpoint.Url (jsonName "url", __string) is documented "The ingest URL to which the source stream should be sent." — i.e. the address an encoder PUTs to, which must therefore be an address the responder answers on. A channel is created with two ingest endpoints, each with its own distinct URL. Recorded behavior only for the host component: the recording shows the responder returning URLs rooted at the endpoint the client is talking to; the generated labels are per-environment and are NOT the requirement, the host being servable by the responder is. Status 200 on this path matched.
  Emulator: both `$.hlsIngest.ingestEndpoints[0].url` and `[1].url` are returned as `https://<label>.mediapackage.<region>.amazonaws.com/in/v2/<channel-label>/<endpoint-label>/channel` — host_not_own on both, status 200. The two-endpoint cardinality, the per-endpoint path uniqueness and the `/in/v2/.../channel` path shape are all correct; only the host is a public AWS name the emulator does not serve.
  Why: an encoder handed these URLs pushes its source stream at real AWS (or fails name resolution offline) instead of at the emulator, so no ingest workflow can be exercised against the emulator even though the control plane reported success. The client cannot tell from the 200 that the address it was given is unusable.
  Fix: the CreateChannel handler's ingest-URL minting, and the channel state record that persists those URLs — the host must be derived from the emulator's advertised endpoint instead of a hard-wired amazonaws.com template.
  Traces: mediapackage-channel-lifecycle-tags-logs#task#1 (cluster also covers mediapackage-conflict-error-paths#setup#1, mediapackage-harvest-job-workflow#setup#1, mediapackage-listing-and-validation#setup#1, mediapackage-listing-and-validation#setup#2, mediapackage-origin-endpoint-lifecycle#setup#1).

Cause 2 of 8: DescribeChannel re-serves both unservable ingest URLs.
  AWS: Smithy DescribeChannel -> GET /channels/{Id}, code 200; output HlsIngest.IngestEndpoints[].Url as in cause 1 — a read re-reports the same reachable ingest URLs the create returned.
  Emulator: `$.hlsIngest.ingestEndpoints[0].url` and `[1].url` both host_not_own, status 200.
  Why: a client that discovers an existing channel rather than creating it is pointed off-emulator identically; and because the wrong host is PERSISTED, the read path keeps leaking it even after the create path is fixed.
  Fix: the DescribeChannel response serializer, plus deriving the host at read time so already-stored channel records stop serving the public-AWS host.
  Traces: mediapackage-channel-lifecycle-tags-logs#task#2 (cluster also covers #task#5, #task#11, mediapackage-conflict-error-paths#task#5, mediapackage-credential-rotation#task#2, #task#5).

Cause 3 of 8: ListChannels emits unservable hosts inside channels[].hlsIngest.ingestEndpoints[].url (single channel).
  AWS: Smithy ListChannels -> GET /channels, code 200, paginated (items Channels); output Channels (jsonName "channels", list of Channel), each element carrying that channel's HlsIngest.IngestEndpoints with their reachable ingest Urls.
  Emulator: `$.channels[0].hlsIngest.ingestEndpoints[0].url` and `[1].url` both host_not_own, status 200 — the list-wrapped serializer uses the same public-AWS host.
  Why: enumeration is how a client discovers channels it did not create, so every ingest URL harvested from a listing is unusable.
  Fix: the ListChannels response serializer / the shared ingest-URL derivation used when projecting state records into list items.
  Traces: mediapackage-channel-lifecycle-tags-logs#task#3.

Cause 4 of 8: UpdateChannel re-serves the unservable ingest URLs.
  AWS: Smithy UpdateChannel -> PUT /channels/{Id}, code 200; output includes HlsIngest.IngestEndpoints[].Url — the ingest URLs survive a description update and are re-reported, still reachable.
  Emulator: both ingestEndpoints urls host_not_own, status 200.
  Why: a client that reconfigures a channel and re-reads the ingest URLs from the update response is pointed off-emulator again, so the modify path must be fixed alongside create and read.
  Fix: the UpdateChannel response serializer / shared derivation.
  Traces: mediapackage-channel-lifecycle-tags-logs#task#4.

Cause 5 of 8: ConfigureLogs re-serves the unservable ingest URLs.
  AWS: Smithy ConfigureLogs -> PUT /channels/{Id}/configure_logs, code 200; its output is the full Channel projection including HlsIngest.IngestEndpoints[].Url, unchanged by the logging change and still reachable.
  Emulator: both ingestEndpoints urls host_not_own, status 200 (the EgressAccessLogs/IngressAccessLogs members themselves matched).
  Why: enabling access logging is a routine operational step that returns the channel; a client that caches the returned ingest URLs from it gets the unreachable host.
  Fix: the ConfigureLogs response serializer / shared derivation.
  Traces: mediapackage-channel-lifecycle-tags-logs#task#10.

Cause 6 of 8: RotateChannelCredentials re-serves the unservable ingest URLs.
  AWS: Smithy RotateChannelCredentials -> PUT /channels/{Id}/credentials, code 200 (operation marked @deprecated, "Please use RotateIngestEndpointCredentials instead"); output is the Channel projection whose HlsIngest.IngestEndpoints[].Url is re-reported after the username/password rotation. The URL itself is unchanged by a rotation and must remain reachable.
  Emulator: both ingestEndpoints urls host_not_own, status 200 (the rotated SensitiveString username/password members matched).
  Why: the whole point of the rotation response is to hand the operator the endpoint plus its new credentials; the credentials are useless against an address the emulator does not answer on.
  Fix: the RotateChannelCredentials handler / shared ingest-URL derivation.
  Traces: mediapackage-credential-rotation#task#3.

Cause 7 of 8: RotateIngestEndpointCredentials re-serves the unservable ingest URLs.
  AWS: Smithy RotateIngestEndpointCredentials -> PUT /channels/{Id}/ingest_endpoints/{IngestEndpointId}/credentials, code 200; output is the Channel projection with both ingest endpoints' Urls, including the untouched sibling's.
  Emulator: both ingestEndpoints urls host_not_own, status 200.
  Why: as cause 6, for the non-deprecated per-endpoint rotation — the recommended API is the one most clients use, so this is the rotation path that matters.
  Fix: the RotateIngestEndpointCredentials handler / shared derivation.
  Traces: mediapackage-credential-rotation#task#4.

Cause 8 of 8: ListChannels emits unservable hosts for MULTIPLE channels (four URLs in one body).
  AWS: as cause 3, for a listing containing two channels — each listed channel keeps its own distinct, reachable pair of ingest URLs.
  Emulator: `$.channels[0].hlsIngest.ingestEndpoints[0].url`, `[0]...[1].url`, `$.channels[1]...[0].url` and `[1]...[1].url` are ALL host_not_own, status 200; the per-channel and per-endpoint path uniqueness is correct.
  Why: as cause 3 — the replay clusters the multi-channel listing separately from the single-channel one, so it must be re-replayed separately to confirm the fix; a client enumerating a populated account gets nothing usable.
  Fix: the ListChannels response serializer / shared derivation.
  Traces: mediapackage-listing-and-validation#task#3.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, emulator_up=true ("/__admin/health answered 200 and the :32855 data listener accepts"), bootstrap_signal=false, 7 fixtures replayed, 65 steps, 36 passed, 29 mismatched, 0 errored, 0 blocked, score 55.4%, 14 divergence clusters. None of the eight clusters below carried a stub_path or an owned_by.

Cause 1 - mediapackage-channel-lifecycle-tags-logs#task#1, CreateChannel, expected 200 / actual 200, affected_count 6:
  $.hlsIngest.ingestEndpoints[0].url: host_not_own (expected '<shaped-not-asserted>', got 'https://be07297cf53abd5e9f21bf454fd80cd7.mediapackage.us-west-2.amazonaws.com/in/v2/827c6625212b4b13c8f967c3f503ea70/5bdbb4cddb807b46719af4d4b7868b6b/channel')
  $.hlsIngest.ingestEndpoints[1].url: host_not_own (same host, sibling endpoint label)
  divergence_key service=mediapackage|CreateChannel|status=200->200|keys=$.hlsIngest.ingestEndpoints[0].url,$.hlsIngest.ingestEndpoints[1].url
  (the labels/ids quoted here are one run's generated values and are NOT the requirement; the `*.mediapackage.<region>.amazonaws.com` host is)

Cause 2 - mediapackage-channel-lifecycle-tags-logs#task#2, DescribeChannel, 200/200, affected_count 6: both $.hlsIngest.ingestEndpoints[*].url host_not_own. The strings round-trip byte-for-byte against the recording; the verdict is that the host is not one the emulator serves. divergence_key service=mediapackage|DescribeChannel|status=200->200|keys=$.hlsIngest.ingestEndpoints[0].url,$.hlsIngest.ingestEndpoints[1].url

Cause 3 - mediapackage-channel-lifecycle-tags-logs#task#3, ListChannels, 200/200: $.channels[0].hlsIngest.ingestEndpoints[0].url and [1].url both host_not_own.

Cause 4 - mediapackage-channel-lifecycle-tags-logs#task#4, UpdateChannel, 200/200: both $.hlsIngest.ingestEndpoints[*].url host_not_own.

Cause 5 - mediapackage-channel-lifecycle-tags-logs#task#10, ConfigureLogs, 200/200: both $.hlsIngest.ingestEndpoints[*].url host_not_own.

Cause 6 - mediapackage-credential-rotation#task#3, RotateChannelCredentials, 200/200: both $.hlsIngest.ingestEndpoints[*].url host_not_own (host `*.mediapackage.us-west-2.amazonaws.com`, paths /in/v2/<channel>/<endpoint>/channel).

Cause 7 - mediapackage-credential-rotation#task#4, RotateIngestEndpointCredentials, 200/200: both $.hlsIngest.ingestEndpoints[*].url host_not_own.

Cause 8 - mediapackage-listing-and-validation#task#3, ListChannels, 200/200: four members host_not_own - $.channels[0].hlsIngest.ingestEndpoints[0].url, $.channels[0].hlsIngest.ingestEndpoints[1].url, $.channels[1].hlsIngest.ingestEndpoints[0].url, $.channels[1].hlsIngest.ingestEndpoints[1].url.

In all eight clusters the HTTP status and every non-address member matched; the divergence is confined to the host component of the advertised ingest URL.

Model evidence: smithy_query(query="operation", operation="CreateChannel,DescribeChannel,ListChannels,UpdateChannel,ConfigureLogs,RotateChannelCredentials,RotateIngestEndpointCredentials", model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json") shows HlsIngest (jsonName "hlsIngest") as an output member of all seven, each with http code 200; smithy_query(query="shape", shape="HlsIngest,IngestEndpoint") shows HlsIngest.IngestEndpoints (jsonName "ingestEndpoints", __listOfIngestEndpoint) and IngestEndpoint.Url (jsonName "url") documented "The ingest URL to which the source stream should be sent." The ingest wire protocol itself is not modeled, hence "recorded behavior only" for the host requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus; the eight clusters above reappear as mismatches with verdict host_not_own.
2. Or reproduce directly with boto3 pointed at the emulator endpoint (region us-west-2):
   a. Causes 1-5: ch = mp.create_channel(Id="ch1") and read ch["HlsIngest"]["IngestEndpoints"][0]["Url"] and [1]["Url"]; then mp.describe_channel(Id="ch1"), mp.list_channels(), mp.update_channel(Id="ch1", Description="d2") and mp.configure_logs(Id="ch1", EgressAccessLogs={"LogGroupName":"/aws/MediaPackage/egress"}) and read the same members from each response. Fixtures: mediapackage-channel-lifecycle-tags-logs#task#1, #task#2, #task#3, #task#4, #task#10.
   b. Causes 6-7: mp.rotate_channel_credentials(Id="ch1") and mp.rotate_ingest_endpoint_credentials(Id="ch1", IngestEndpointId=<one of the two ids>), reading the ingest Urls from each response. Fixtures: mediapackage-credential-rotation#task#3, #task#4.
   c. Cause 8: create two channels, then mp.list_channels() and read Channels[*].HlsIngest.IngestEndpoints[*].Url. Fixture: mediapackage-listing-and-validation#task#3.
3. In every case the host is `*.mediapackage.<region>.amazonaws.com`; an HTTP request to the advertised URL never reaches the emulator, while the control-plane call itself was made to the emulator's own endpoint.

Pass criterion for all eight: the host in every hlsIngest.ingestEndpoints[].url (including the list-wrapped channels[n] form, and for records created BEFORE the fix) must be one the emulator answers on, derived from the endpoint the request arrived on. Generated labels, ports and ids may differ freely; the two-endpoint cardinality and the per-endpoint `/in/v2/<channel-label>/<endpoint-label>/channel` path uniqueness must be preserved.
