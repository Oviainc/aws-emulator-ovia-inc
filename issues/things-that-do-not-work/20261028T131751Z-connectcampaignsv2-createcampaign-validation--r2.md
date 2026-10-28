Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# CreateCampaign never consults the Amazon Connect instance registry, although state.rs declares that refusal class as shared by "creating a campaign"

_Filed: 2026-10-28T13:17:51Z_
_Operation: CreateCampaign | Class: validation_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::INSTANCE_NOT_FOUND_MESSAGE` documents the refusal class this service answers for an unusable Connect instance, and defines it as covering TWO cases — an instance the account's Connect registry does not hold, OR one with no finished onboarding job — and names the operations that share it: "starting an onboarding job, creating a campaign, and the instance-integration operations".

Only `StartInstanceOnboardingJob` implements the first case: it calls the crate's own helper `crate::state::connect_instance_exists`. `CreateCampaign` implements only the second: its single gate is `store.is_onboarded(...)`, which reads nothing but this service's own onboarding store. The same is true of the instance-integration operations (`PutConnectInstanceIntegration`, `DeleteConnectInstanceIntegration`, `ListConnectInstanceIntegrations`), which the same doc names — all three gate on `is_onboarded` alone.

So after the Connect instance is removed from the account's registry, this service still treats it as a usable campaign target: `CreateCampaign` answers 200 and mints a campaign whose `connectInstanceId` names an instance the account does not hold, while `StartInstanceOnboardingJob` for the very same id answers 404 with `INSTANCE_NOT_FOUND_MESSAGE`. The two "disagree about a request they both refuse", which is exactly what the doc says cannot happen. The helper exists, a sibling handler already calls it, and the state module states the rule — the create simply skips it.

## Evidence

The rule, stated in this crate (services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1290-1304):

```
/// What this service answers when a request whose own resource is NOT the instance's
/// configuration names a Connect instance that is not usable by Campaigns: one the
/// account's Amazon Connect registry does not hold, or one that no finished onboarding
/// job has enabled for Campaigns in the request's region.
...
/// The one text shared by the operations that merely require a usable instance (starting
/// an onboarding job, creating a campaign, and the instance-integration operations), so
/// two of them cannot disagree about a request they both refuse.
pub const INSTANCE_NOT_FOUND_MESSAGE: &str =
    "Connect Instance does not exist or is not enabled to use Campaigns.";
```

The helper the crate already has for the registry half (src/state.rs:1324):

```
pub fn connect_instance_exists<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> Option<bool> {
```
with its own doc at src/state.rs:1312-1313: "an instance created or deleted over the Connect API is onboardable, or not, with no propagation step."

The sibling that calls it (src/ops/start_instance_onboarding_job.rs:242-243):

```
        let Some(instance_exists) =
            crate::state::connect_instance_exists(&*self.engine, &input.connect_instance_id)
```

The path that does not (src/ops/create_campaign.rs:262):

```
        if !store.is_onboarded(&region, &input.connect_instance_id, ::std::time::SystemTime::now())
```
That is the handler's only instance gate — `connect_instance_exists` is never named in create_campaign.rs. Same for src/ops/put_connect_instance_integration.rs:199, src/ops/delete_connect_instance_integration.rs:204 and src/ops/list_connect_instance_integrations.rs:179, which all gate on `store.is_onboarded(...)` alone.

The registry really does lose entries (services/service-com-amazonaws-connect-amazonconnectservice/src/ops/delete_instance.rs:177):

```
        if store.instances.remove(&input.instance_id).is_none() {
```

Test coverage: tests/all/instance_onboarding_requires_a_connect_instance.rs pins the registry gate for `StartInstanceOnboardingJob` only ("`StartInstanceOnboardingJob` configures an EXISTING Amazon Connect instance...", lines 10-14). No test exercises `CreateCampaign` against an instance deleted from the registry after onboarding, so nothing pins the current behaviour.

## Steps to Reproduce

1. Register a Connect instance (Connect `CreateInstance`) with id `inst-1`.
2. Call ConnectCampaignsV2 `StartInstanceOnboardingJob` for `inst-1` and wait out `crate::state::ONBOARDING_JOB_DURATION` so its job reaches SUCCEEDED.
3. Call Connect `DeleteInstance` for `inst-1`, which executes `store.instances.remove(&input.instance_id)` (delete_instance.rs:177).
4. Call ConnectCampaignsV2 `StartInstanceOnboardingJob` for `inst-1` again: HTTP 404 `ResourceNotFoundException` with "Connect Instance does not exist or is not enabled to use Campaigns." — the registry gate fires.
5. Call `CreateCampaign` with `connectInstanceId: "inst-1"`: HTTP 200, a campaign id and ARN are minted and `DescribeCampaign` reports a campaign belonging to an instance the account no longer holds. Expected per state.rs:1290-1300: the same 404 / `INSTANCE_NOT_FOUND_MESSAGE` the onboarding call gives.
6. Same with `PutConnectInstanceIntegration` / `ListConnectInstanceIntegrations` for `inst-1`: both still serve the instance.
