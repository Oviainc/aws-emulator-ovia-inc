Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the ConnectCampaignsV2 emulator

_Filed: 2026-10-21T21:38:10Z_
_Operation: service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and stores nothing, so no client workflow works at all. Implement a FIRST, SMALL slice: the campaign lifecycle, backed by real persisted state.

Happy path to implement, in order (restJson1, sigv4 signing name `connect-campaigns`):

1. **CreateCampaign** — `PUT /v2/campaigns`. Input: `name` (required, 1..127), `connectInstanceId` (required, 1..256, `^[-_.a-zA-Z0-9]+$`), `channelSubtypeConfig`, `type` (JOURNEY|MANAGED), `source`, `connectCampaignFlowArn`, `schedule`, `entryLimitsConfig`, `communicationTimeConfig`, `communicationLimitsOverride`, `tags`. Must mint a fresh `id` matching `^[-:/a-zA-Z0-9]+$`, build an `arn` of the form `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`, store the whole campaign (including every config block and tags) and return `{id, arn, tags}` from what was stored. Initial campaign state must be `Initialized`.
2. **DescribeCampaign** — `GET /v2/campaigns/{id}`. Returns `campaign` (Campaign) rebuilt from stored state: `id`, `arn`, `name`, `connectInstanceId` (all required in the Campaign shape) plus `channelSubtypeConfig`, `type`, `source`, `connectCampaignFlowArn`, `schedule`, `entryLimitsConfig`, `communicationTimeConfig`, `communicationLimitsOverride`, `tags` exactly as supplied at create time. Unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **ListCampaigns** — `POST /v2/campaigns-summary`. Honor `maxResults` (1..50) and `nextToken` (opaque, derived from the stored ordering) and return `campaignSummaryList` of `CampaignSummary{id, arn, name, connectInstanceId, channelSubtypes (unique list derived from the stored channelSubtypeConfig), type, schedule, entryLimitsConfig, connectCampaignFlowArn}`. Apply `filters` (CampaignFilters — at minimum the instance-id filter). `nextToken` must be absent on the last page.
4. **StartCampaign** / **PauseCampaign** / **StopCampaign** — `POST /v2/campaigns/{id}/{start|pause|stop}`, empty outputs. These must mutate the stored campaign state: Initialized|Paused|Stopped -> Running on start, Running -> Paused on pause, any active state -> Stopped on stop. An illegal transition (e.g. pause while not Running) must return `InvalidCampaignStateException` (HTTP 409); unknown id -> `ResourceNotFoundException` (404).
5. **GetCampaignState** — `GET /v2/campaigns/{id}/state`. Returns `state` (Completed|Failed|Initialized|Paused|Running|Stopped) read back from the stored campaign — never a constant. Must reflect the transition performed in step 4.
6. **DeleteCampaign** — `DELETE /v2/campaigns/{id}`. Removes the campaign from the store; a following DescribeCampaign/GetCampaignState must return `ResourceNotFoundException` (404) and ListCampaigns must no longer include it. Deleting an unknown id -> 404.

No operation in this slice may return a canned or echoed response: every response field must be computed from the persisted record. A mint-an-id-without-storing CreateCampaign is a failure, as is a GetCampaignState that returns a fixed enum value.

## State Model

- `ConnectCampaignsV2State { campaigns: Map<CampaignId, Campaign> }`, guarded by the crate's usual state lock and keyed by the generated campaign id so lookups by `{id}` httpLabel are O(1).
- `Campaign { id: String, arn: String, name: String, connect_instance_id: String, channel_subtype_config: Option<ChannelSubtypeConfig>, campaign_type: Option<ExternalCampaignType>, source: Option<Source>, connect_campaign_flow_arn: Option<String>, schedule: Option<Schedule>, entry_limits_config: Option<EntryLimitsConfig>, communication_time_config: Option<CommunicationTimeConfig>, communication_limits_override: Option<CommunicationLimitsConfig>, tags: Map<String,String>, state: CampaignState, created_at: Timestamp }` — `state` is the single source of truth read by GetCampaignState and written by Start/Pause/StopCampaign.
- An insertion-ordered view (e.g. a Vec of ids, or a BTreeMap keyed by `(created_at, id)`) so ListCampaigns paginates deterministically and `nextToken` can encode the last-returned id.

Out of scope for this first slice (they stay 501 and will be filed as individual gap issues): the instance-config/onboarding family (StartInstanceOnboardingJob, GetInstanceOnboardingJobStatus, DeleteInstanceOnboardingJob, GetConnectInstanceConfig, DeleteConnectInstanceConfig), integrations (Put/Delete/ListConnectInstanceIntegrations), instance communication limits, the granular Update*/Delete* config mutators, ResumeCampaign, GetCampaignStateBatch, PutOutboundRequestBatch, PutProfileOutboundRequestBatch and the tagging operations.

## Evidence

OPERATIONS.md for this crate lists all 37 operations with the routes, input/output members and errors cited above (CreateCampaign `PUT /v2/campaigns` -> `{id, arn, tags}`; DescribeCampaign `GET /v2/campaigns/{id}` -> `campaign`; ListCampaigns `POST /v2/campaigns-summary` @paginated(inputToken=nextToken, outputToken=nextToken, items=campaignSummaryList); Start/Pause/StopCampaign `POST /v2/campaigns/{id}/...` with InvalidCampaignStateException HTTP 409; GetCampaignState `GET /v2/campaigns/{id}/state` -> state enum; DeleteCampaign `DELETE /v2/campaigns/{id}` with ResourceNotFoundException HTTP 404). The Campaign and CampaignSummary shapes queried from the Smithy model give the required response members used in the state model. `grep -l '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/` matches every handler file in the crate, including create_campaign.rs, describe_campaign.rs, list_campaigns.rs, start_campaign.rs, pause_campaign.rs, stop_campaign.rs, get_campaign_state.rs and delete_campaign.rs — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and holds zero state. No tests assert any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `connectcampaigns` (ConnectCampaignsV2) at it.
2. CreateCampaign with name="demo-campaign", connectInstanceId="instance-1", a telephony channelSubtypeConfig and tags={"env":"test"} — expect 200 with a generated id, an arn containing that id, and the tags echoed from storage (currently 501).
3. DescribeCampaign with the returned id — expect 200 and campaign.name=="demo-campaign", campaign.connectInstanceId=="instance-1", campaign.arn equal to the create response arn, and the channelSubtypeConfig/tags exactly as created.
4. ListCampaigns with maxResults=1 — expect the new campaign in campaignSummaryList with its id/arn/name/connectInstanceId and channelSubtypes derived from the stored config.
5. GetCampaignState with the id — expect state=="Initialized".
6. StartCampaign with the id, then GetCampaignState — expect state=="Running".
7. PauseCampaign, then GetCampaignState — expect state=="Paused"; call PauseCampaign again and expect InvalidCampaignStateException (HTTP 409).
8. StopCampaign, then GetCampaignState — expect state=="Stopped".
9. DeleteCampaign with the id — expect 200/204; then DescribeCampaign with the same id expects ResourceNotFoundException (HTTP 404) and ListCampaigns no longer returns it.
