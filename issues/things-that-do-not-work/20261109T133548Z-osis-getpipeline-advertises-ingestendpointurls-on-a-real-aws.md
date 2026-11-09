Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.native-protocol-http-ingest-round-trip
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.native-protocol-http-ingest-round-trip|run#12|PublishedUrlRejected|ingest_url = f"{ctx.endpoint_url(ingest_urls[0])}/ingest"
Issue-Commit: f1d315614d2491eea3b7c6e4236ba64fe2dea133

# osis: GetPipeline advertises IngestEndpointUrls on a real-AWS hostname the emulator does not serve, so the pipeline ingest data plane is unreachable

> _Note: this issue was identified against the code base at commit `f1d315614d2491eea3b7c6e4236ba64fe2dea133`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPipeline

## Description
One net-new root cause.

Cause 1 of 1: GetPipeline publishes `Pipeline.IngestEndpointUrls` as an AWS-production hostname (`<pipeline-name>-<suffix>.<region>.osis.amazonaws.com`) instead of an address on the emulator's own listener, so a client that follows the URL the emulator itself handed out never reaches the emulator at all.

AWS: Smithy com.amazonaws.osis#Pipeline member `IngestEndpointUrls` (IngestEndpointUrlsList), documented as "The ingestion endpoints for the pipeline, which you can send data to" — returned by GetPipeline/CreatePipeline. The model does not constrain the host (recorded behavior only for the host shape): in AWS the value is the host a SigV4-signed POST to the Data Prepper http source path is served on, so the contract is that the advertised endpoint is the address at which this deployment actually serves pipeline ingest. For an emulator, that address is the emulator's own host — every other URL the emulator publishes resolves to its own listener. Recorded behavior: after CreatePipeline reaches ACTIVE, GetPipeline returns a non-empty IngestEndpointUrls, and `POST <ingest url><source path>` signed SigV4 for service `osis` returns 2xx and the record reaches the configured sink.

Emulator: GetPipeline returns an ingest URL of the form `https://sim2real-osis-<hash>-<hash>.us-west-2.osis.amazonaws.com` — a host that is not one of the emulator's own hosts. The check's endpoint resolution refuses it before any request is made: `simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-osis-...us-west-2.osis.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. (The specific hostname, hash suffixes and port are per-run values; the requirement is only that the published host is the emulator's own.) The data plane behind the URL is therefore never even contacted.

Why: a client reads IngestEndpointUrls from the API and posts documents to it — that is the only way to use OpenSearch Ingestion. Against the emulator that URL points at real AWS (or at nothing, if egress is blocked), so the client either leaves the emulator entirely or fails with a DNS/TLS/connection error that looks like a network fault rather than an emulator gap. The emulator reports the pipeline as ACTIVE while advertising an address it does not serve, so there is no reachable ingest path at all and no ingest-then-read-the-sink flow can run.

Fix: the osis GetPipeline/CreatePipeline response construction and the pipeline state that holds the published endpoint (the osis service crate's pipeline state plus its get_pipeline/create_pipeline handlers) — derive IngestEndpointUrls from the emulator's own serving host/port (the same base the control plane is reached on, or a per-resource listener on an own host) rather than synthesizing an `*.osis.amazonaws.com` name, and serve the pipeline's configured http-source path on that address.

Traces: osis.native-protocol-http-ingest-round-trip.

Relation to the open issue 20261029T191455Z-osis-refuses-implemented-path-calls-with-notimplemented-putresourcepolicy.md: that issue's Cause 4 lists this same check, but it is keyed on a DIFFERENT failure — the ingest POST being answered HTTP 501 NotImplemented after the URL had resolved onto the emulator. The suite report does not mark this check as OWNED by it, and the check no longer reaches that POST: it now fails earlier, at endpoint resolution, because the published host changed to an AWS-production name. Publishing an unreachable host is a distinct defect from the data plane behind it answering 501, and it must be fixed first for that cause's assertion to be reachable again.

## Evidence
run_behavioral_suite (suite simparity-behavioral-aws-v1): 15/23 passed, bootstrap_signal false; osis 15/22 (68%) — the osis emulator is up and answering, CreatePipeline reached ACTIVE and GetPipeline returned IngestEndpointUrls, so only the host of the published URL diverges.

[mismatch] osis.native-protocol-http-ingest-round-trip — after `_wait_for_pipeline_status(... {"ACTIVE","CREATE_FAILED"})` returned ACTIVE and `osis.get_pipeline(PipelineName=<name>)["Pipeline"]["IngestEndpointUrls"]` came back non-empty, the check's `ctx.endpoint_url(ingest_urls[0])` raised:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://sim2real-osis-d9dafc7fa6b6-d1b2864737.us-west-2.osis.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:41657/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(frames: context.py:572 endpoint_route -> context.py:586 _published_target). The hostname's random segments, the region label and the port 41657 are per-run values; the divergence is that the host is not the emulator's.

Expected instead: the published URL's host is the emulator's own serving host, so `POST <ingest url>/ingest` SigV4-signed for service `osis` with body `[{"sim2real_marker": "<uuid>", "message": "hello-osis-ingest"}]` returns 2xx and the configured s3 sink bucket subsequently contains an object carrying that record.

Model grounding (services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json): com.amazonaws.osis#Pipeline.IngestEndpointUrls -> IngestEndpointUrlsList, "The ingestion endpoints for the pipeline, which you can send data to"; no host constraint is modelled, so the requirement is the recorded one — the advertised endpoint is the address this deployment serves ingest on.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay osis.native-protocol-http-ingest-round-trip (run_behavioral_suite).
2. Or by hand: osis CreatePipeline with MinUnits=1, MaxUnits=1 and a Data Prepper configuration whose source is `http` with path `/ingest` and whose sink is an s3 bucket; poll GetPipeline until Status == ACTIVE.
3. Read Pipeline.IngestEndpointUrls from GetPipeline and inspect the host of the first URL. Observe a `*.osis.amazonaws.com` hostname rather than the emulator's own host/port.
4. After the fix, that URL's host must be the emulator's own serving host, and a SigV4-signed (service `osis`) POST of `[{"marker": "<any value>", "message": "<any value>"}]` to `<ingest url>/ingest` must return 2xx, with the posted record subsequently readable from the configured sink bucket via s3 ListObjectsV2/GetObject.
5. Re-replay osis.native-protocol-http-ingest-round-trip to confirm.
