Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# Two different 404 messages for an un-onboarded instance, each documented in state.rs as "the one text"

_Filed: 2026-10-27T23:07:10Z_
_Operation: GetConnectInstanceConfig | Class: error-response_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Eight instance-scoped handlers resolve a `connectInstanceId` against the SAME predicate — `ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State::onboarded_instance` / `is_onboarded` — and refuse an instance that is not onboarded with HTTP 404. Four of them answer one sentence and four answer a different one, while BOTH helper functions carry a doc comment claiming to be the single shared text for exactly that refusal. Each claim is falsified by the crate's own handlers.

- `instance_config_not_found_message` -> "ConnectInstanceConfig not found {id}" is used by GetConnectInstanceConfig, GetInstanceCommunicationLimits, PutInstanceCommunicationLimits and DeleteConnectInstanceConfig, yet its doc says it is "The one text every instance-scoped operation shares" — ListConnectInstanceIntegrations, PutConnectInstanceIntegration, DeleteConnectInstanceIntegration and CreateCampaign are instance-scoped operations that do not use it.
- `instance_not_found_message` -> "Connect instance {id} does not exist or is not enabled to use Campaigns" is used by those four, yet its doc says it is "The one text every operation that resolves an instance id against the instance-onboarding store uses, so two of them cannot disagree about a request they both refuse" — GetConnectInstanceConfig resolves an instance id against the instance-onboarding store (it calls `onboarded_instance`) and disagrees.

So a client asking two 404-answering operations about one and the same un-onboarded instance id is given two different sentences for one condition, and neither helper's stated invariant holds. One of the two groups is using the wrong text; the comments cannot both be true.

## Evidence

Model: the two refusals are the same condition — `GetConnectInstanceConfig` ("Get the specific Connect instance config.") and `ListConnectInstanceIntegrations` ("Provides summary information about the integration under the specified Connect instance.") both declare only `ResourceNotFoundException (HTTP 404)` — "The specified resource was not found."

Repo side 1 — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1104-1113:
```
/// What this service answers when a `connectInstanceId` names no Connect instance
/// onboarded to Campaigns: there is no instance configuration to report, nor any to
/// configure.
///
/// Composed from the request, so the refusal names the instance the caller asked
/// about. The one text every instance-scoped operation shares, so two of them cannot
/// disagree about an instance this service holds nothing for.
pub fn instance_config_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceConfig not found {connect_instance_id}")
}
```

Repo side 2 — src/state.rs:1271-1282:
```
/// What this service answers when a Connect instance id names no instance onboarded
/// to Campaigns.
///
/// The one text every operation that resolves an instance id against the
/// instance-onboarding store uses, so two of them cannot disagree about a request
/// they both refuse. Composed from the request, which is why it names the id it was
/// given.
pub fn instance_not_found_message(connect_instance_id: &str) -> String {
    format!(
        "Connect instance {connect_instance_id} does not exist or is not enabled to use Campaigns"
    )
}
```

Both are reached from the same predicate. src/ops/get_connect_instance_config.rs:167-177 resolves through the onboarding store and then uses the *config* text:
```
        let config = store
            .onboarded_instance(&region, &input.connect_instance_id, now)
            .map(crate::state::StoredInstanceOnboarding::to_config);
        let Some(config) = config else {
...
                        message: crate::state::instance_config_not_found_message(
```
while src/ops/list_connect_instance_integrations.rs:179-185 resolves through the same store and uses the other text:
```
        if !store.is_onboarded(&region, &input.connect_instance_id, now) {
...
                        message: crate::state::instance_not_found_message(
```
The split across the crate (grep): `instance_config_not_found_message` at get_connect_instance_config.rs:174, get_instance_communication_limits.rs:171, put_instance_communication_limits.rs:235, delete_connect_instance_config.rs:259; `instance_not_found_message` at create_campaign.rs:266, list_connect_instance_integrations.rs:183, put_connect_instance_integration.rs:209, delete_connect_instance_integration.rs:214, start_instance_onboarding_job.rs:262.

Test coverage: tests/all/instance_mutation_audit.rs asserts only the HTTP status and error code for these rows (e.g. lines 147-160, 172-182), never the message, so no test pins either sentence.

## Steps to Reproduce

1. Start the emulator and do NOT onboard any Connect instance.
2. GET /v2/connect-instance/my-instance/config — observe HTTP 404 with message "ConnectInstanceConfig not found my-instance".
3. GET /v2/connect-instance/my-instance/integrations — observe HTTP 404 with message "Connect instance my-instance does not exist or is not enabled to use Campaigns".
4. Both refusals came from the same predicate (`is_onboarded`/`onboarded_instance` in src/state.rs), yet each helper's doc comment in src/state.rs claims to be the single text shared by every operation that makes this refusal.
