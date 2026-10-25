Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# Campaign and onboarding stores are not region-scoped: a campaign created in one region is listed and mutated from another

_Filed: 2026-10-24T07:51:25Z_
_Operation: ListCampaigns | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State` holds one global `campaigns` map (and one global `instance_onboarding` map); the request's SigV4 credential-scope region (`crate::state::request_region`) is used ONLY to mint the ARN at create time. Nothing else consults it. As a result:

* `ListCampaigns` signed for eu-west-1 returns campaigns created while signing for us-east-1 (their `arn` even names us-east-1), so the page is a mixture of regions no AWS endpoint would ever answer with;
* `DescribeCampaign` / `GetCampaignState` / `StartCampaign` / `PauseCampaign` / `StopCampaign` / `DeleteCampaign` / `PutOutboundRequestBatch` resolve and MUTATE a campaign created in another region, instead of 404-ing;
* `GetConnectInstanceConfig` / `GetInstanceOnboardingJobStatus` report an instance onboarded in another region as onboarded there too, and `CreateCampaign`'s gate passes in a region where the instance was never onboarded.

Connect campaigns are regional resources behind regional endpoints: a campaign created in us-east-1 does not exist for a client talking to eu-west-1, which answers HTTP 404 `ResourceNotFoundException` for its id and omits it from `ListCampaigns`. The emulator is also internally inconsistent about this — its tag operations ARE region-aware (`campaign_by_arn` compares the whole ARN, and tests/all/instance_onboarding_tags_and_outbound_requests.rs asserts that "a stored campaign's id in another region" is refused), so the same campaign is unreachable by ARN from eu-west-1 yet fully mutable by id.

Expected: the store is keyed by the credential-scope region (as other crates in this repo do, e.g. apprunner's `names_quota_and_listing_are_region_scoped`, dynamodb's per-region tables), so listing, lookups and lifecycle writes only see the region they were signed for. Actual: one global store shared by every region.

## Evidence

Spec/semantics: `CampaignArn` embeds the region (`arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`, minted in src/state.rs::campaign_arn from `request_region`), which is the model's own statement that a campaign belongs to one region; AWS regional endpoints have isolated resource namespaces, so `ListCampaigns` and the `{id}` operations are per-region. `ListCampaigns` models no region input, so the only region a response can be scoped by is the credential scope.

State/handlers: src/state.rs — `campaigns: HashMap<String, StoredCampaign>`, `order: Vec<(u64, String)>`, `instance_onboarding: HashMap<String, StoredInstanceOnboarding>` are all region-less; `campaign()`, `campaign_mut()`, `remove_campaign()`, `campaigns_in_order()`, `is_onboarded()` take no region. src/ops/list_campaigns.rs:167-219 walks `campaigns_in_order()` filtered only by `filters.instanceIdFilter`, and its handler takes `_ctx` (the region is never read). src/ops/describe_campaign.rs, get_campaign_state.rs, start/pause/stop_campaign.rs, delete_campaign.rs and put_outbound_request_batch.rs likewise ignore `ctx`.

Test coverage: no test asserts cross-region isolation for id-based operations; the only region-sensitive assertions are ARN-based (tag ops), which proves the two halves disagree rather than covering this gap.

## Steps to Reproduce

1. Boot the emulator. Sign a request for us-east-1: onboard instance-1 (PUT /v2/connect-instance/instance-1/onboarding) and PUT /v2/campaigns {"name":"east","connectInstanceId":"instance-1"}; note the id and that the returned arn names us-east-1.
2. Now sign for eu-west-1 and POST /v2/campaigns-summary {} -> observe the us-east-1 campaign in campaignSummaryList; expected an empty list.
3. Sign for eu-west-1 and GET /v2/campaigns/{id} -> observe HTTP 200; expected HTTP 404 ResourceNotFoundException ("CampaignId not found.").
4. Sign for eu-west-1 and POST /v2/campaigns/{id}/start, then GET /v2/campaigns/{id}/state from us-east-1 -> observe "Running": a client on a different regional endpoint mutated the campaign; expected the start to 404 and the state to stay Initialized.
5. Sign for eu-west-1 and GET /v2/connect-instance/instance-1/config -> observe HTTP 200 with a config for an instance onboarded only in us-east-1; expected HTTP 404.
