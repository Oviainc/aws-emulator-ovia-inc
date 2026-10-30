Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 117179165722d096b61d7b4919fba6e95c8d84ca

# The IntegrationIdentifier union's ARN @length/@pattern are never checked, unlike every other union-borne constraint this crate compensates for by hand

_Filed: 2026-10-29T00:13:16Z_
_Operation: DeleteConnectInstanceIntegration | Class: validation_

> _Note: this issue was identified against the code base at commit `117179165722d096b61d7b4919fba6e95c8d84ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteConnectInstanceIntegration` takes a required `integrationIdentifier` (an `IntegrationIdentifier` union) and reads the ARN straight out of it (`crate::state::integration_identifier_key`) with no check of the modelled constraints that sit on the three identifier structures' ARN members: `CustomerProfilesIntegrationIdentifier$domainArn` and `QConnectIntegrationIdentifier$knowledgeBaseArn` carry `@required`, `@length 20..500` and the service's ARN `@pattern`, and `LambdaIntegrationIdentifier$functionArn` carries `@length 1..140` and the Lambda ARN `@pattern`.

Those members sit BEHIND a union, which this crate states in its own words the centralized constraint walk does not reach — which is precisely why the crate hand-checks every other union-borne constraint it accepts (`crate::state::communication_limits_violation` for the `@length` on `CommunicationLimits$communicationLimitsList`, `crate::state::communication_time_violation` for the `DayOfWeek` keys behind `OpenHours`). This handler applies no such compensating check, so a request carrying `{"integrationIdentifier": {"lambda": {"functionArn": "nope"}}}` reaches the store, resolves to no record, and is answered HTTP 404 `ResourceNotFoundException` with "Connect instance <id> has no lambda integration under the integrationIdentifier provided" — i.e. malformed input is reported as a missing resource instead of the bad request the modelled constraints make it. The two refusals are different for a client: a 404 says "nothing is integrated under that identity", which is false for an ARN no integration could ever have been stored under.

The gap is on the identifier union of the delete path, which is a different shape and a different operation from the already-filed `PutConnectInstanceIntegration` config gap.

## Evidence

The model puts the constraints on the identifier members (smithy_query shape=CustomerProfilesIntegrationIdentifier / LambdaIntegrationIdentifier):

```
"CustomerProfilesIntegrationIdentifier": {"members": {"domainArn": {"target": "Arn", "required": true, "length": {"min": 20, "max": 500}, "pattern": "^arn:[a-zA-Z0-9-]+:..."}}}
"LambdaIntegrationIdentifier": {"members": {"functionArn": {"target": "LambdaArn", "required": true, "length": {"min": 1, "max": 140}, "pattern": "^arn:aws[a-zA-Z-]*:lambda:..."}}}
```

The handler reads the ARN and never checks it — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/delete_connect_instance_integration.rs:190-192:

```
        // Which integration the request names, read off the identifier the same way a
        // put reads it off a config, so the two cannot resolve to different records.
        let kind = crate::state::integration_identifier_key(&input.integration_identifier).0;
```

and the only refusal it can reach for a malformed ARN is the not-found one — src/ops/delete_connect_instance_integration.rs:286-299:

```
            return Err(
                DeleteConnectInstanceIntegrationErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "Connect instance {} has no {} integration under the \
                             integrationIdentifier provided",
                            input.connect_instance_id,
                            kind.member_name()
                        ),
```

The other side of the inconsistency — the crate DOES hand-check constraints that hide behind a union, and says why. src/ops/update_campaign_communication_limits.rs:171-177:

```
        // The modelled `@length` of `communicationLimitsList` sits on a union member,
        // which the centralized constraint walk does not reach, so an over-long list
        // is refused here — as a bad request, before anything is looked up or stored.
        if let Some(message) = crate::state::communication_limits_violation(
```

and src/state.rs:364-370 (doc of `communication_time_violation`):

```
/// The one place this service checks a `CommunicationTimeConfig`'s `dailyHours`
/// keys, so every operation that accepts one (`CreateCampaign`,
/// `UpdateCampaignCommunicationTime`) refuses a day naming no weekday the same way.
/// Those keys sit behind the `OpenHours` union, which the centralized constraint
/// walk does not reach, so without this check a campaign could be stored opening a
/// day the model does not model.
```

Test coverage: nothing pins the current answer. tests/all/connect_instance_integration_deletion.rs and tests/all/connect_instance_integrations.rs contain no malformed-ARN row (a grep for `400`/`ValidationException` in the integration suites matches only the paging cursor and `maxResults` cases — tests/all/connect_instance_integration_list_paging.rs:139-145, 195-201, tests/all/connect_instance_integrations.rs:417-423).

## Steps to Reproduce

1. Register a Connect instance, run `StartInstanceOnboardingJob` for it and wait out `ONBOARDING_JOB_DURATION` so the instance is onboarded.
2. Call `POST /v2/connect-instance/{instance}/integrations/delete` with body `{"integrationIdentifier": {"lambda": {"functionArn": "nope"}}}` — `nope` satisfies neither `LambdaArn`'s `@length 1..140` minimum semantics nor its `@pattern`.
3. Observe HTTP 404 `ResourceNotFoundException` ("Connect instance ... has no lambda integration under the integrationIdentifier provided") rather than the HTTP 400 `ValidationException` the modelled `@pattern` makes it: the union member was never checked.
4. Compare with the sibling union-borne constraint on the same crate: `POST /v2/campaigns/{id}/communication-limits` with three entries in `communicationLimitsOverride.allChannelSubtypes.communicationLimitsList` is refused 400 by `crate::state::communication_limits_violation` (src/ops/update_campaign_communication_limits.rs:171-187) because that handler compensates for the walk not reaching behind the union.
