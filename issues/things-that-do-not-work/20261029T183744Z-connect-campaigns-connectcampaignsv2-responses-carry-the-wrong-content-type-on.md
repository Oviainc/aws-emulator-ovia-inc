Issue-Source: trace-sweep-divergence
Issue-Trace: connectcampaignsv2-campaign-config-updates-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcampaignsv2-campaign-config-updates-not-found connectcampaignsv2-campaign-creation-validation connectcampaignsv2-campaign-lifecycle-not-found connectcampaignsv2-campaign-outbound-requests connectcampaignsv2-instance-integrations-and-limits connectcampaignsv2-instance-onboarding-lifecycle
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connect-campaigns|UpdateCampaignName|4xx->4xx|$.message
Issue-Key: connect-campaigns|UpdateCampaignName|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignSchedule|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignSource|4xx->4xx|$.message
Issue-Key: connect-campaigns|UpdateCampaignSource|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignEntryLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|UpdateCampaignEntryLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignCommunicationLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|UpdateCampaignCommunicationLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignCommunicationTime|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignChannelSubtypeConfig|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|UpdateCampaignFlowAssociation|4xx->4xx|$.message
Issue-Key: connect-campaigns|UpdateCampaignFlowAssociation|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteCampaignChannelSubtypeConfig|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteCampaignCommunicationLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteCampaignCommunicationLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteCampaignCommunicationTime|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteCampaignEntryLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteCampaignEntryLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|CreateCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|CreateCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|ListCampaigns|2xx->2xx|$headers.content-type
Issue-Key: connect-campaigns|DescribeCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|GetCampaignState|4xx->4xx|$.message
Issue-Key: connect-campaigns|GetCampaignState|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|StartCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|StartCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|PauseCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|PauseCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|ResumeCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|ResumeCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|StopCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|StopCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteCampaign|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteCampaign|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|PutOutboundRequestBatch|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|GetCampaignStateBatch|2xx->2xx|$.failedRequests[*]
Issue-Key: connect-campaigns|GetCampaignStateBatch|2xx->2xx|$headers.content-type
Issue-Key: connect-campaigns|PutConnectInstanceIntegration|4xx->4xx|$.message
Issue-Key: connect-campaigns|PutConnectInstanceIntegration|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|ListConnectInstanceIntegrations|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteConnectInstanceIntegration|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteConnectInstanceIntegration|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|PutInstanceCommunicationLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|PutInstanceCommunicationLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|GetInstanceCommunicationLimits|4xx->4xx|$.message
Issue-Key: connect-campaigns|GetInstanceCommunicationLimits|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|GetConnectInstanceConfig|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|GetInstanceOnboardingJobStatus|4xx->4xx|$.message
Issue-Key: connect-campaigns|GetInstanceOnboardingJobStatus|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|$.message
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteInstanceOnboardingJob|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteInstanceOnboardingJob|4xx->4xx|$headers.content-type
Issue-Key: connect-campaigns|DeleteConnectInstanceConfig|4xx->4xx|$.message
Issue-Key: connect-campaigns|DeleteConnectInstanceConfig|4xx->4xx|$headers.content-type
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# ConnectCampaignsV2 responses carry the wrong Content-Type on every reply

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCampaignName

## Description
ONE cause, reproduced by every one of the 34 replayed ConnectCampaignsV2 steps in the wire corpus — success replies (HTTP 200) and refusals (HTTP 400/404) alike.

=== Cause 1 of 1: the response Content-Type header is 'application/json' where ConnectCampaignsV2 answers 'application/x-amz-json-1.1' ===

AWS: recorded behavior only. Smithy: AmazonConnectCampaignServiceV2 (sdkId ConnectCampaignsV2, apiVersion 2024-04-23, sigv4Name/arnNamespace 'connect-campaigns') declares protocol `aws.protocols#restJson1`, whose generic default response media type is `application/json`. The recorded AWS responses for this service nevertheless carry `Content-Type: application/x-amz-json-1.1` on EVERY reply — the service's front end labels its JSON bodies with the awsJson1_1 media type regardless of the modelled protocol. The model states nothing about this, so the recording is the requirement: the header value is a fixed literal with no variable part, and it is the same literal on a 200 body, on a ValidationException 400 and on a ResourceNotFoundException 404.

Emulator: every response the ConnectCampaignsV2 crate serves is labelled `Content-Type: application/json` — the restJson1 default taken straight from the protocol trait. The HTTP status line and the JSON body are otherwise produced correctly for the steps whose only divergence is this header (ListCampaigns 200, DescribeCampaign 404, CreateCampaign 404, PutOutboundRequestBatch 404, UpdateCampaignSchedule/CommunicationTime/ChannelSubtypeConfig 404, DeleteCampaignChannelSubtypeConfig/CommunicationTime 404, ListConnectInstanceIntegrations 404, GetConnectInstanceConfig 404): those steps pass on status and on body and fail only on this header.

Why: the Content-Type is how a client decides how to decode the body. A caller, proxy, recorded-cassette test or content negotiation layer that keys on the media type AWS actually emits for this service sees a different type from the emulator on 100% of calls, so a response that is byte-correct in its body is classified as a different media type than the one the service is contracted to produce. It also makes the emulator indistinguishable, at the header level, from a generic restJson1 service, which is exactly the distinction this service's front end does not make.

Fix: the response serializer / protocol content-type selection for this service — the restJson1 response writer used by the ConnectCampaignsV2 crate must emit `application/x-amz-json-1.1` as the service's response media type instead of deriving `application/json` from the restJson1 protocol default. It is one place, applied uniformly to success and error envelopes; no handler is involved, which is why all 34 steps show it.

Traces: all 34 clusters. Representative connectcampaignsv2-campaign-config-updates-not-found#task#1; the other 33 are listed in trace_ids and must all be re-replayed, since a per-operation fix would leave most of them diverging.

## Evidence
Every replayed step reports the same header divergence:
  $headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')

Steps whose ONLY divergence is this header (status and body matched):
  connectcampaignsv2-campaign-config-updates-not-found#task#2  UpdateCampaignSchedule               404 -> 404
  connectcampaignsv2-campaign-config-updates-not-found#task#6  UpdateCampaignCommunicationTime      404 -> 404
  connectcampaignsv2-campaign-config-updates-not-found#task#7  UpdateCampaignChannelSubtypeConfig   404 -> 404
  connectcampaignsv2-campaign-config-updates-not-found#task#9  DeleteCampaignChannelSubtypeConfig   404 -> 404
  connectcampaignsv2-campaign-config-updates-not-found#task#11 DeleteCampaignCommunicationTime      404 -> 404
  connectcampaignsv2-campaign-creation-validation#task#2       CreateCampaign                       404 -> 404
  connectcampaignsv2-campaign-creation-validation#task#3       ListCampaigns                        200 -> 200
  connectcampaignsv2-campaign-lifecycle-not-found#task#1       DescribeCampaign                     404 -> 404
  connectcampaignsv2-campaign-outbound-requests#task#1         PutOutboundRequestBatch              404 -> 404
  connectcampaignsv2-instance-integrations-and-limits#task#2   ListConnectInstanceIntegrations      404 -> 404
  connectcampaignsv2-instance-onboarding-lifecycle#task#1      GetConnectInstanceConfig             404 -> 404

The remaining 23 steps carry this header divergence in addition to a body divergence (filed separately as the campaign-plane and instance-plane message bundles).

Expected (AWS): Content-Type: application/x-amz-json-1.1 on every response.
Actual (emulator): Content-Type: application/json on every response.
Status codes and, for the 11 steps above, the JSON bodies are correct — only the media type diverges, so this is the serializer's header, not any handler.

## Steps to Reproduce
1. Boot the emulator from mainline and point a SigV4-signed `connectcampaigns` v2 client (signing name 'connect-campaigns') at the data listener.
2. Issue any ConnectCampaignsV2 call that succeeds, e.g. GET /v2/campaigns (ListCampaigns). Inspect the raw response headers: Content-Type is `application/json`, where AWS answers `application/x-amz-json-1.1`.
3. Issue any ConnectCampaignsV2 call that is refused, e.g. GET /v2/campaigns/{random-uuid}/state (GetCampaignState) or GET /v2/connect-instance/{random-uuid}/config (GetConnectInstanceConfig). The 404 error envelope is likewise labelled `application/json`.
4. Replay connectcampaignsv2-campaign-config-updates-not-found#task#1 and every fixture#step in trace_ids: all 34 report `$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')`.
