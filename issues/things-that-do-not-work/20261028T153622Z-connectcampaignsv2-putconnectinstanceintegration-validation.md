Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 527cd3726300db8090bc233c982f66d05e13948e

# objectTypeNames keys are never checked against the EventType catalog, unlike every other unreachable map key this crate compensates for

_Filed: 2026-10-28T15:36:21Z_
_Operation: PutConnectInstanceIntegration | Class: validation_

> _Note: this issue was identified against the code base at commit `527cd3726300db8090bc233c982f66d05e13948e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutConnectInstanceIntegration` stores the `integrationConfig` it is handed verbatim, including `customerProfiles.objectTypeNames` — a map whose KEY shape the model declares as the `EventType` enum (`ObjectTypeNamesMap`: key `EventType`). Nothing checks those keys:

* the framework's centralized constraint walk never validates a map key at all (vendor: "the key itself is not a constrained leaf"), a fact this crate states in its own words and compensates for everywhere else it accepts a map key — the tag maps of `TagResource`/`CreateCampaign` go through `crate::state::tag_keys_violation`, and the `DayOfWeek`-keyed `dailyHours` map behind the `OpenHours` union goes through `crate::state::communication_time_violation`;
* the generated input type for this operation holds the map as `HashMap<String, String>` (no enum type to constrain it), and the handler adds no check of its own before writing.

So an off-catalog event type (e.g. `"Campaign-Fax"`) is accepted with HTTP 200, persisted on the instance's integration record, and then echoed back by `ListConnectInstanceIntegrations` inside `IntegrationSummary.customerProfiles.objectTypeNames`, whose modelled key shape is the same `EventType` enum — i.e. the service hands a client a map key the model does not model. The catalog needed to refuse it already exists in this crate (`EVENT_TYPE_VALUES`), and is already used by this operation's own test file, so the missing check is the one thing absent.

(The same union-nested gap also leaves `customerProfiles.domainArn` / `lambda.functionArn` length and pattern bounds unchecked — this handler applies no compensating check for anything inside the `integrationConfig` union, while `CreateCampaign`, `UpdateCampaignCommunicationLimits`, `PutInstanceCommunicationLimits` and `UpdateCampaignCommunicationTime` all do for the union-nested constraints they take.)

## Evidence

Model (smithy_query shape=CustomerProfilesIntegrationConfig): `"objectTypeNames": {"target": "ObjectTypeNamesMap", "required": true, "type": "map"}`; `ObjectTypeNamesMap` is `{"type": "map", "key": "DayOfWeek"-analogue "EventType", "value": ObjectTypeName}` and `EventType` is an enum: `"values": ["Campaign-Email", "Campaign-SMS", "Campaign-Telephony", "Campaign-Orchestration", "Campaign-WhatsApp", "Campaign-WebNotification"]`.

No check in the handler — the config goes straight to the store:
src/ops/put_connect_instance_integration.rs:217-226
```
        // The store decides whether this is an update of an integration the instance
        // already holds or a new one, from the identity the config carries. ...
        let Some(_stored) = store.put_instance_integration(
            &region,
            &input.connect_instance_id,
            input.integration_config,
        ) else {
```
src/state.rs:2268 `existing.config = config;` / src/state.rs:2276 `let record = StoredIntegration { sequence, config };`
and it is echoed back verbatim — src/state.rs:1443-1446:
```
                IntegrationSummary::customerProfiles(CustomerProfilesIntegrationSummary {
                    domain_arn: config.domain_arn.clone(),
                    object_type_names: config.object_type_names.clone(),
                })
```
The generated input carries no enum typing — src/ops/types/put_connect_instance_integration_types.rs:32-35:
```
pub struct CustomerProfilesIntegrationConfig {
    pub domain_arn: String,
    pub object_type_names: std::collections::HashMap<String, String>,
}
```
The crate's OWN statement that a map key reaches a handler unchecked, and its compensation elsewhere — src/state.rs:205-213:
```
/// Tag keys reach a handler UNCHECKED, for two reasons that compound: the boundary
/// walk captures a map's key as the path segment of that entry's value rather than
/// validating it as a leaf of its own, and `TagKey`'s `@pattern` carries a leading
/// negative lookahead ... So this is the one place the key shape's `@length` and `@pattern` are
/// enforced, shared by every operation that takes a tag map
```
src/ops/create_campaign.rs:217-219: `// A tag map's KEYS are a constrained shape like any other input member, and // the centralized walk reaches neither their @length (a map key is captured // as a path segment, never validated as a leaf) ...`
The sibling enum-keyed map IS compensated — src/ops/update_campaign_communication_time.rs:178-185:
```
        // A `dailyHours` key names a day of the week, the model's own `DayOfWeek`
        // catalog, and that map sits behind the `OpenHours` union, which the
        // centralized constraint walk does not reach: a key naming no day is refused
        // here as a bad request, before anything is looked up or stored.
        if let Some(message) = crate::state::communication_time_violation(
```
src/state.rs:392-394: `.find(|day| !DAY_OF_WEEK_VALUES.contains(day))`
Framework confirmation — vendor/smithy-schema-server/src/constraints.rs:1089-1103:
```
        // Map key: the codec writes the entry key as a string against the map
        // member schema before the value. Capture it as the value's segment and
        // toggle the frame; the key itself is not a constrained leaf.
```
The catalog is already in the crate: src/ops/types/shared_types.rs:536 `pub const EVENT_TYPE_VALUES: &[&str] = &["Campaign-Email", "Campaign-Orchestration", ...];` — imported by this operation's test (tests/all/connect_instance_integrations.rs:30), whose cases only ever send in-catalog keys (`object_type_names()` takes `EVENT_TYPE_VALUES.iter().take(count)`, tests/all/connect_instance_integrations.rs:47-55), so no test pins the off-catalog key as accepted.

## Steps to Reproduce

1. Register a Connect instance and run `StartInstanceOnboardingJob` for it, then wait out `ONBOARDING_JOB_DURATION` so the instance is onboarded.
2. Call `PUT /v2/connect-instance/{instance}/integrations` with `{"integrationConfig": {"customerProfiles": {"domainArn": "arn:aws:profile:us-east-1:000000000000:domains/d", "objectTypeNames": {"Campaign-Fax": "obj"}}}}` — `Campaign-Fax` is not in `EventType`'s catalog.
3. Observe HTTP 200: the off-catalog key was accepted and stored (expected: HTTP 400 ValidationException, as the sibling enum-keyed `dailyHours` map is refused with).
4. Call `GET /v2/connect-instance/{instance}/integrations` and observe the response's `integrationSummaryList[0].customerProfiles.objectTypeNames` carrying the key `Campaign-Fax`, which the modelled `EventType` key shape does not admit.
