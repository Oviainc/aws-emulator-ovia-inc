Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-channel-group-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-channel-group-lifecycle mediapackagev2-error-paths mediapackagev2-tagging-workflow mediapackagev2-channel-lifecycle mediapackagev2-multiple-origin-endpoints-isolation
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|GetChannelGroup|2xx->2xx|$.EgressDomain
Issue-Key: mediapackagev2|UpdateChannelGroup|2xx->2xx|$.EgressDomain
Issue-Key: mediapackagev2|CreateChannelGroup|2xx->2xx|$.EgressDomain
Issue-Key: mediapackagev2|CreateChannel|2xx->2xx|$.IngestEndpoints[*].Url
Issue-Key: mediapackagev2|GetChannel|2xx->2xx|$.IngestEndpoints[*].Url
Issue-Key: mediapackagev2|UpdateChannel|2xx->2xx|$.IngestEndpoints[*].Url
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# Channel-group and channel responses advertise AWS-owned hostnames the emulator does not serve

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetChannelGroup

## Description
Wire replay of the simparity-wire-aws-v1 corpus (72/96 steps passed, 24 mismatched) shows that every address-bearing member the mediapackagev2 channel-group and channel surface returns is minted under the PUBLIC AWS domain `<label>.<kind>.<account-label>.mediapackagev2.<region>.amazonaws.com` instead of under a host the emulator itself serves. The replayer's `host_not_own` verdict fires on the host component of the value: the emulator announced an address other than the one it is reached on, so the announced host is not routable to the emulator. This is a SIX-cause bundle; a fixer MUST close all six causes below and re-replay every fixture/step listed per cause before submitting.

The root fault is shared: one URL/domain-minting path builds these values from a hard-wired `amazonaws.com` template rather than from the emulator's own advertised endpoint. Causes are enumerated separately because each operation's serializer emits a different member set, and the replay clusters them separately — fixing only the channel-group path leaves the channel path diverging.

Note: the open behavioral issue 20261029T201731Z-mediapackagev2-five-rejection-messages-worded-differently-from-aws.md covers causes 6 and 7 of its own bundle — that the ingest/manifest data plane answers 404 UnknownService / 404 instead of a 403 denial WHEN reached. That is a different defect: it is about the status returned by the data-plane route, this one is about the host the control plane advertises in the first place. Neither it nor any other open mediapackagev2 issue covers any cause here.

Cause 1 of 6: GetChannelGroup's EgressDomain names a host the emulator does not serve.
  AWS: Smithy GetChannelGroup -> GET /channelGroup/{ChannelGroupName}, output member EgressDomain (target String, @required). AWS returns the channel group's egress domain, and that domain is a host AWS's own MediaPackage egress data plane answers on — the member exists so a client can reach it. Recorded behavior only for the host component: the recording shows the responder returning a domain rooted at the endpoint the client is talking to. The requirement for an emulator is therefore structural, not literal: EgressDomain must name a host the emulator itself serves (derived from the endpoint the request arrived on). The label portion is generated per environment and is NOT part of the requirement.
  Emulator: returns an EgressDomain in the fixed public form `<label>.egress.<account-label>.mediapackagev2.<region>.amazonaws.com`, a hostname owned by AWS and not served by the emulator; status 200 is correct and every other member matches.
  Why: a client that takes EgressDomain and builds a playback/egress URL from it is sent to real AWS (or to NXDOMAIN in an offline environment) instead of to the emulator. No egress request the emulator could answer is ever addressed to it, so playback-path testing against the emulator is impossible even though the control plane reports success.
  Fix: the mediapackagev2 channel-group read handler's response serializer, and behind it the shared domain/URL-minting helper that must derive the host from the emulator's advertised endpoint instead of a hard-wired amazonaws.com template.
  Traces: mediapackagev2-channel-group-lifecycle#task#2 (also task#4).

Cause 2 of 6: UpdateChannelGroup's EgressDomain has the same unservable host.
  AWS: Smithy UpdateChannelGroup output member EgressDomain (String, @required) — same contract as cause 1; an update re-reports the group's reachable egress domain unchanged.
  Emulator: same `<label>.egress.<account-label>.mediapackagev2.<region>.amazonaws.com` form, host_not_own, status 200.
  Why: as cause 1 — and because the value is re-asserted on update, a client that re-reads it after a modification is pointed off-emulator again.
  Fix: the UpdateChannelGroup handler's response serializer / the same shared domain-minting helper.
  Traces: mediapackagev2-channel-group-lifecycle#task#3.

Cause 3 of 6: CreateChannelGroup's EgressDomain has the same unservable host.
  AWS: Smithy CreateChannelGroup -> POST /channelGroup, code 200, output member EgressDomain (String, @required). The domain is minted at create time and is the host the group's egress is reachable on.
  Emulator: same public-AWS form, host_not_own. (The label also differs from the recording's label, but a generated label is a per-environment value and is NOT a defect; only the unservable host is.)
  Why: the very first value a client learns for a new channel group already points away from the emulator, so everything derived from it downstream is unreachable.
  Fix: the CreateChannelGroup handler where the group's egress domain is minted and stored (the state record keeps the wrong host, so later reads re-serve it).
  Traces: mediapackagev2-error-paths#task#5 (also mediapackagev2-tagging-workflow#task#1).

Cause 4 of 6: CreateChannel's IngestEndpoints[].Url names hosts the emulator does not serve.
  AWS: Smithy CreateChannel -> POST /channelGroup/{ChannelGroupName}/channel, output member IngestEndpoints (list of IngestEndpoint{Id, Url}); IngestEndpoint.Url is documented as "The ingest domain URL where the source stream should be sent." The URL is the address a contribution encoder pushes to, so it must be reachable on the responder.
  Emulator: returns two ingest URLs of the form `https://<label>-<n>.ingest.<account-label>.mediapackagev2.<region>.amazonaws.com/in/v1/<group>/<n>/<channel>/index` — both flagged host_not_own. The path component and the two-endpoint shape are right; only the host is a public AWS name the emulator does not answer on.
  Why: a client doing the documented live-ingest round trip pushes its stream at real AWS instead of the emulator, so ingest can never be exercised; in an isolated environment the push fails at name resolution with no indication the control-plane response was at fault.
  Fix: the CreateChannel handler's ingest-endpoint minting (and the channel state record that persists the URLs) — host must come from the emulator's advertised endpoint.
  Traces: mediapackagev2-channel-lifecycle#task#2 (also mediapackagev2-error-paths#task#7, mediapackagev2-multiple-origin-endpoints-isolation#setup#2).

Cause 5 of 6: GetChannel re-serves the same unservable IngestEndpoints[].Url values.
  AWS: Smithy GetChannel -> GET /channelGroup/{ChannelGroupName}/channel/{ChannelName}/, output IngestEndpoints (list of IngestEndpoint) — a read returns the same reachable ingest URLs the create returned.
  Emulator: both IngestEndpoints[0].Url and IngestEndpoints[1].Url are host_not_own on read, status 200.
  Why: a client that discovers an existing channel's ingest endpoints (rather than creating it) is pointed off-emulator identically; the read path must be fixed even after the create path is, or persisted records keep leaking the wrong host.
  Fix: the GetChannel handler's response serializer, plus a migration/derivation of the host at read time so already-stored records do not keep serving the public-AWS host.
  Traces: mediapackagev2-channel-lifecycle#task#3 (also mediapackagev2-channel-lifecycle#task#5).

Cause 6 of 6: UpdateChannel re-serves the same unservable IngestEndpoints[].Url values.
  AWS: Smithy UpdateChannel output IngestEndpoints (list of IngestEndpoint) — the ingest URLs survive an update and are re-reported, still reachable.
  Emulator: both ingest URLs host_not_own, status 200.
  Why: as cause 5, on the modify path — a client that updates a channel and re-reads its endpoints from the response gets an address it cannot reach.
  Fix: the UpdateChannel handler's response serializer / the same shared ingest-URL derivation.
  Traces: mediapackagev2-channel-lifecycle#task#4.

## Evidence
Replay: trace_set_id simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, 10 fixtures replayed, 96 steps, 72 passed, 24 mismatched, 0 errored, 0 blocked, score 75.0%. None of the clusters below carried a stub_path or an owned_by.

Cause 1 — fixture mediapackagev2-channel-group-lifecycle step task#2, GetChannelGroup, expected status 200 / actual 200:
  $.EgressDomain: host_not_own — the emulator announced `<label>.egress.<account-label>.mediapackagev2.us-west-2.amazonaws.com`, a host it is not reached on. divergence_key service=mediapackagev2|GetChannelGroup|status=200->200|keys=$.EgressDomain. Cluster also covers task#4.

Cause 2 — mediapackagev2-channel-group-lifecycle#task#3, UpdateChannelGroup, 200/200:
  $.EgressDomain: host_not_own, same public-AWS egress domain form. divergence_key ...|UpdateChannelGroup|status=200->200|keys=$.EgressDomain.

Cause 3 — mediapackagev2-error-paths#task#5, CreateChannelGroup, 200/200:
  $.EgressDomain: host_not_own. Recorded value and emulator value differ only in the generated label (recording `cq8go8.egress.z4xeig...`, emulator `096530b1.egress.4a962433403d...`) — that label difference is a per-environment value and is NOT the finding; the unservable `*.mediapackagev2.us-west-2.amazonaws.com` host is. Cluster also covers mediapackagev2-tagging-workflow#task#1.

Cause 4 — mediapackagev2-channel-lifecycle#task#2, CreateChannel, 200/200:
  $.IngestEndpoints[0].Url and $.IngestEndpoints[1].Url: host_not_own. Emulator returned `https://<label>-1.ingest.<account-label>.mediapackagev2.us-west-2.amazonaws.com/in/v1/<group>/1/<channel>/index` and the matching `-2`/`/2/` sibling. Cluster also covers mediapackagev2-error-paths#task#7 and mediapackagev2-multiple-origin-endpoints-isolation#setup#2.

Cause 5 — mediapackagev2-channel-lifecycle#task#3, GetChannel, 200/200:
  $.IngestEndpoints[0].Url, $.IngestEndpoints[1].Url: host_not_own (the strings round-trip byte-for-byte against the recording; the verdict is on the host being one the emulator does not serve). Cluster also covers mediapackagev2-channel-lifecycle#task#5.

Cause 6 — mediapackagev2-channel-lifecycle#task#4, UpdateChannel, 200/200:
  $.IngestEndpoints[0].Url, $.IngestEndpoints[1].Url: host_not_own.

In all six clusters the HTTP status, the member set and every non-address member matched; the divergence is confined to the host component of the advertised address.

## Steps to Reproduce
Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus, or reproduce directly with botocore pointed at the emulator endpoint:

Cause 1/2/3: CreateChannelGroup(ChannelGroupName=g) and read EgressDomain from the 200 response; then GetChannelGroup(g) and UpdateChannelGroup(g, Description=...) and read EgressDomain from each. In every case the host is `*.mediapackagev2.<region>.amazonaws.com`. Confirm it is not the emulator's: resolve/connect to that host — nothing the emulator serves answers on it, while the control-plane request itself was made to the emulator's own endpoint. Fixtures: mediapackagev2-channel-group-lifecycle#task#2, #task#3; mediapackagev2-error-paths#task#5.

Cause 4/5/6: CreateChannel(ChannelGroupName=g, ChannelName=c) and read IngestEndpoints[*].Url; then GetChannel(g, c) and UpdateChannel(g, c, Description=...) and read the same member. Each URL's host is a public AWS ingest name; an HTTP PUT to it never reaches the emulator. Fixtures: mediapackagev2-channel-lifecycle#task#2, #task#3, #task#4.

Pass criterion for all six: the host in EgressDomain / IngestEndpoints[].Url must be one the emulator answers on (derived from the endpoint the request arrived on), so a client can push to or fetch from the advertised address without leaving the emulator. Generated labels, ports and ids may differ freely.
