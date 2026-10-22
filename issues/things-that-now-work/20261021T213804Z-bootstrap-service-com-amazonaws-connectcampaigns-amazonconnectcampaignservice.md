Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the AmazonConnectCampaignService emulator

_Filed: 2026-10-21T21:38:04Z_
_Operation: service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 22 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the campaign lifecycle plus its run-state loop, all computed from persisted state.

Happy path to implement, in order (restJson1, sigv4 signing name `connect-campaigns`):

1. **CreateCampaign** — `PUT /campaigns`. Input: `name` (1..127, required), `connectInstanceId` (required, `^[a-zA-Z0-9_\-.]*$`), `dialerConfig` (required union: `progressiveDialerConfig` | `predictiveDialerConfig` | `agentlessDialerConfig`), `outboundCallConfig` (required; `connectContactFlowId` required, plus `connectSourcePhoneNumber`, `connectQueueId`, `answerMachineDetectionConfig`), optional `tags`. Must mint a fresh `id` and an `arn` (20..500 chars, e.g. `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`), store the whole campaign, and return `id`/`arn`/`tags` from what was stored — not echoed back without storage. Initial run state is `Initialized`.
2. **DescribeCampaign** — `GET /campaigns/{id}`. Returns `campaign` (Campaign: `id`, `arn`, `name`, `connectInstanceId`, `dialerConfig`, `outboundCallConfig`, `tags` — all the required members populated) read from the store; `ResourceNotFoundException` (404) for an unknown id.
3. **ListCampaigns** — `POST /campaigns-summary`. Must enumerate the stored campaigns as `campaignSummaryList` of CampaignSummary (`id`, `arn`, `name`, `connectInstanceId`), honour `filters.instanceIdFilter`, cap by `maxResults` (1..50) and emit/consume `nextToken` so a created campaign appears and a deleted one does not.
4. **StartCampaign / PauseCampaign / StopCampaign** — `POST /campaigns/{id}/{start|pause|stop}`. Empty output, but each must TRANSITION the persisted run state: Initialized/Stopped → `Running` on start, Running → `Paused` on pause, Running/Paused → `Stopped` on stop. An illegal transition (e.g. pause a campaign that is not Running) must return `InvalidCampaignStateException` (409), and an unknown id `ResourceNotFoundException` (404).
5. **GetCampaignState** — `GET /campaigns/{id}/state`. Returns the stored `state` enum (`Failed|Initialized|Paused|Running|Stopped`), which must reflect whatever the last start/pause/stop did.
6. **DeleteCampaign** — `DELETE /campaigns/{id}`. Removes the campaign; a subsequent DescribeCampaign/GetCampaignState must return 404 and ListCampaigns must no longer list it. Idempotent per `@idempotent`? No — the model marks it idempotent, so a second delete of an already-removed id should still surface `ResourceNotFoundException` only if real AWS does; keep the behaviour consistent with DescribeCampaign.

No canned or echoed responses: every one of these must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` (see README — never construct a fresh engine in a handler).

## State Model

In `src/state.rs`, on the service state struct:

- `campaigns: HashMap<String /* campaign id */, Campaign>` where `Campaign { id: String, arn: String, name: String, connect_instance_id: String, dialer_config: DialerConfig, outbound_call_config: OutboundCallConfig, tags: HashMap<String,String>, state: CampaignState /* Initialized|Running|Paused|Stopped|Failed */, created_at: SystemTime }`
- `arn_index: HashMap<String /* arn */, String /* campaign id */>` so the tag operations and any ARN-keyed lookup resolve to a campaign without a scan.
- (room for the later slice, not required now) `instance_configs: HashMap<String /* connectInstanceId */, InstanceConfig>` and `onboarding_jobs: HashMap<String, InstanceOnboardingJobStatus>`, plus `dial_requests: HashMap<String /* campaign id */, Vec<DialRequest>>`.

Out of scope for this first slice (stay 501, to be filed as individual gap issues later): ResumeCampaign, PutDialRequestBatch, GetCampaignStateBatch, StartInstanceOnboardingJob, GetInstanceOnboardingJobStatus, DeleteInstanceOnboardingJob, GetConnectInstanceConfig, DeleteConnectInstanceConfig, TagResource, UntagResource, ListTagsForResource, UpdateCampaignName, UpdateCampaignDialerConfig, UpdateCampaignOutboundCallConfig.

Done = a realistic integration script (create → describe → list → start → get state → pause → get state → stop → delete → verify gone) passes with correct bodies, not just 2xx.

## Evidence

`services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/OPERATIONS.md` lists all 22 operations with their routes, members and errors (CreateCampaign `PUT /campaigns` lines 25-31; DescribeCampaign `GET /campaigns/{id}` lines 57-63; ListCampaigns `POST /campaigns-summary` with `@paginated(inputToken=nextToken, items=campaignSummaryList)` lines 95-101; Start/Pause/Stop `POST /campaigns/{id}/{start|pause|stop}` with `InvalidCampaignStateException (HTTP 409)` lines 111-153; GetCampaignState `GET /campaigns/{id}/state` returning the `Failed|Initialized|Paused|Running|Stopped` enum lines 65-70; DeleteCampaign `DELETE /campaigns/{id}` lines 33-39). Smithy model confirms the Campaign structure's required members (`id`, `arn`, `name`, `connectInstanceId`, `dialerConfig`, `outboundCallConfig`), DialerConfig as a 3-variant union, OutboundCallConfig's required `connectContactFlowId`, and CampaignSummary's four required members. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 22 handler files, including `create_campaign.rs`, `describe_campaign.rs`, `list_campaigns.rs`, `start_campaign.rs`, `pause_campaign.rs`, `stop_campaign.rs`, `get_campaign_state.rs`, `delete_campaign.rs` — each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No test asserts any of this behaviour; only the generated `tests/state_roundtrip.rs` exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `connectcampaigns` client at it.
2. `CreateCampaign` with name="demo", connectInstanceId="inst-1", dialerConfig={progressiveDialerConfig:{bandwidthAllocation:1.0}}, outboundCallConfig={connectContactFlowId:"flow-1"} → expect 200 with a generated `id` and an `arn`; currently 501 NotImplemented.
3. `DescribeCampaign(id)` → expect the Campaign with name/connectInstanceId/dialerConfig/outboundCallConfig exactly as created.
4. `ListCampaigns` with filters.instanceIdFilter={value:"inst-1", operator:"Eq"} → expect the new campaign in `campaignSummaryList`.
5. `GetCampaignState(id)` → expect `Initialized`.
6. `StartCampaign(id)` then `GetCampaignState(id)` → expect `Running`.
7. `PauseCampaign(id)` then `GetCampaignState(id)` → expect `Paused`; call `PauseCampaign(id)` again → expect `InvalidCampaignStateException` (409).
8. `StopCampaign(id)` then `GetCampaignState(id)` → expect `Stopped`.
9. `DeleteCampaign(id)`, then `DescribeCampaign(id)` → expect `ResourceNotFoundException` (404), and `ListCampaigns` no longer returns it.
