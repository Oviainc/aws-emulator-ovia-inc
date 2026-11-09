Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconnect.native-media-ingest-udp-round-trip
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect.native-media-ingest-udp-round-trip|run#13|PublishedUrlRejected|mc, flow_arn, probe_socket, *ctx.endpoint_host(ingest_ip, ingest_port), payload, timeout_s=120
Issue-Commit: cc108f911ed57306e3512a7613c5717a28080cea

# An ACTIVE flow advertises a Source.IngestIp the emulator does not serve (198.51.100.0/24 documentation range), so the advertised ingest endpoint is unreachable

> _Note: this issue was identified against the code base at commit `cc108f911ed57306e3512a7613c5717a28080cea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlow

## Description
One net-new root cause surviving the behavioral replay against the MediaConnect emulator. 12 of the 13 mediaconnect failures are marked OWNED by an existing open issue and are skipped; this is the one that is not.

NOTE ON OVERLAP (neither issue covers this check per the replay report, so it is filed here):
- 20261022T132818Z-mediaconnect-router-interface-union-payload-rejected-no-flow.md lists this same check and its "no native UDP ingest" cause describes a LATER failure of it (Source.PeerIpAddress never populating after packets are sent). The check no longer reaches that point.
- 20261101T213636Z-mediaconnect-an-active-flow-advertises-source-ingestip-in-the.md (OPEN) describes the same advertised-address defect, keyed on the harness's older `EgressRefused` step. The replay now fails at a DIFFERENT harness gate — `simparity.published.PublishedUrlRejected` raised from context.py `endpoint_host` / `_published_target` — so the report does not report this failure as owned by it, and the check is still unclaimed. A fixer closing this issue should re-check that file; the underlying defect is the same one and both should close together.

=== Cause 1 of 1: the ingest endpoint an ACTIVE flow advertises is a documentation-range address, not an address the emulator serves ===
AWS: Smithy model services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json — `Source$IngestIp` (jsonName `ingestIp`) is "The IP address that the flow will be listening on for incoming content", paired with `Source$IngestPort` ("The port that the flow will be listening on for incoming content"); `Source$PeerIpAddress` is "The IP address of the device that is currently sending content to this source". Beyond those member docs this is recorded behavior only: MediaConnect's data plane is raw UDP/RTP, so once a flow reaches ACTIVE the service is listening on exactly the endpoint DescribeFlow advertises as Source.IngestIp:IngestPort, and a sender that puts datagrams on that endpoint is observed there (its address then appears on Source.PeerIpAddress). The requirement the model and the recording share is that the advertised endpoint IS the endpoint the service listens on — IngestIp is an address a client is instructed to send content to, not an opaque label. For an emulator that means the announced address must be an address the emulator itself serves its data plane on; any other value is unreachable by construction.
Emulator: CreateFlow + StartFlow succeed and the flow reaches ACTIVE, but the flow-source materialisation mints IngestIp from the 198.51.100.0/24 (TEST-NET-2) documentation range keyed off the ARN/suffix counter, with no relation to the host and port the emulator is actually reached on — the ingest-endpoint assignment helpers used when a stored source is built from a set-request in this service's state module (`assigned_ingest_ip` / `assigned_source_ingest_endpoint`, called from `StoredSource::from_set_request`), whose own doc comment calls it "the same documentation range a source's own ingest address is drawn from". DescribeFlow then reports that address, and the suite refuses to transmit to it: `simparity.published.PublishedUrlRejected: endpoint_host was given <advertised ip>:<advertised port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address`. The documentation-range prefix is the general behavior; the final octet and port of any one run are per-run counter artifacts and are not themselves the requirement. No error is returned on any API call — the flow is reported healthy while advertising an endpoint nothing serves.
Why: a client that does exactly what the API tells it to do — read the ACTIVE flow's advertised ingest endpoint and send content there — sends to an address nobody serves. On a host with no route the datagrams are silently dropped; on a host with a route they leave for a third party inside a reserved documentation range. The data-plane round trip cannot be attempted at all, Source.PeerIpAddress can never populate, and the caller cannot distinguish "the service announced an address it does not own" from "my content is not arriving": there is no status, no error code and no message, only silence. Every ingest, failover-validation and source-monitoring workflow that keys on the advertised endpoint is unusable against this emulator.
Fix: the flow-source ingest-endpoint assignment in this service's state (`assigned_ingest_ip` / `assigned_source_ingest_endpoint` as used by `StoredSource::from_set_request`) must advertise the address on which the emulator actually serves the MediaConnect flow data plane, and the flow-start path must bind that endpoint when the flow becomes ACTIVE so that packets arriving on it populate Source.PeerIpAddress. The same relation feeds `InputConfiguration$InputIp` for media-stream source configurations, drawn from the same documentation range, and has the same defect.
Not verified by this run (the check never got past the ingest transmit): the check's trailing assertions that CreateFlow with Source.Protocol='udp' is refused with `BadRequestException` and the message "Validation failed:\nInvalid protocol" (fixed text; no variable part), and that the refused flow's name does not appear in ListFlows.
Traces: mediaconnect.native-media-ingest-udp-round-trip. Code: services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs (`assigned_ingest_ip`, `assigned_source_ingest_endpoint`, `StoredSource::from_set_request`); services/service-com-amazonaws-mediaconnect-mediaconnect/src/ops/start_flow.rs.

## Evidence
Replay report: mediaconnect 15/28 passed (54%), 13 failed; 12 of the 13 are marked OWNED by an existing issue. The surviving failure:

[mismatch] mediaconnect.native-media-ingest-udp-round-trip
  steps: create_flow(Name=..., Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':<runner egress CIDR>}) -> start_flow -> poll describe_flow until Flow.Status == 'ACTIVE' -> read Flow.Source.IngestIp / IngestPort -> send 188-byte UDP probe datagrams to that endpoint -> poll describe_flow for Flow.Source.PeerIpAddress.
  expected: the advertised Source.IngestIp:IngestPort is an endpoint the emulator serves, the probe datagrams reach it, and DescribeFlow subsequently reports Source.PeerIpAddress.
  actual:   DescribeFlow advertises an ingest address in the 198.51.100.0/24 documentation range (observed value 198.51.100.3, port 5002 — per-run counter artifacts) while the emulator is reached on an entirely different host, and the transmit is refused before any packet leaves:
            File "context.py", line 619, in endpoint_host
              self._published_target(f"{published_host}:{port}", "endpoint_host")
            File "context.py", line 586, in _published_target
              raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
            simparity.published.PublishedUrlRejected: endpoint_host was given 198.51.100.3:5002 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs an own host[:port]: the endpoint host, a documented own host or, for a loopback endpoint, a loopback address

No API call in the sequence returned a non-2xx: CreateFlow, StartFlow and DescribeFlow all succeeded, so the divergence is purely in the VALUE of Source.IngestIp (and the absence of any listener behind it), not in a status or error code.

Source of the advertised value: services/service-com-amazonaws-mediaconnect-mediaconnect/src/state.rs — `StoredSource::from_set_request` calls `assigned_source_ingest_endpoint(..., Some(assigned_ingest_ip(suffix)), ...)`; the crate's own doc comment near those helpers describes the value as drawn from "the same documentation range a source's own ingest address is drawn from ([`assigned_ingest_ip`])". (Line numbers in the earlier open issue 20261101T213636Z no longer match: the file has drifted, so trace by function name.)

## Steps to Reproduce
1. Boot the emulator and point a botocore mediaconnect client at its endpoint.
2. flow = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'0.0.0.0/0'})
3. mc.start_flow(FlowArn=flow['Flow']['FlowArn']); poll mc.describe_flow until Flow.Status == 'ACTIVE'.
4. src = mc.describe_flow(FlowArn=...)['Flow']['Source']; print(src['IngestIp'], src['IngestPort']).
   Observe IngestIp is in the 198.51.100.0/24 documentation range — not a host the emulator serves.
5. Send UDP datagrams from a plain stdlib socket to (src['IngestIp'], src['IngestPort']) and poll mc.describe_flow for Flow.Source.PeerIpAddress: nothing serves that address, so the packets go nowhere and PeerIpAddress never populates. (The suite harness refuses the transmit outright with PublishedUrlRejected because the announced host is not one of the emulator's own hosts.)
6. Equivalently: run run_behavioral_suite and observe mediaconnect.native-media-ingest-udp-round-trip failing in context.py endpoint_host/_published_target.
