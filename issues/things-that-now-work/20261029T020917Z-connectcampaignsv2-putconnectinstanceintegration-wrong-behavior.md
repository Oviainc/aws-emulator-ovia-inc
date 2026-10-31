Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3

# The integration operations leave no audit record when they refuse with their own ValidationException, unlike every sibling instance mutator

_Filed: 2026-10-29T02:09:17Z_
_Operation: PutConnectInstanceIntegration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eb1bf6ad15d70a5e618c7ac6aaae48664667c1d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::audit_onboarding_change` is documented as the one place this service records a state-changing Connect-instance call, and as being called on EVERY path such a call returns through — "the accepted one and each refusal, named by the modelled error the refusal answers with".

`PutConnectInstanceIntegration` honours that for three of its four client-visible exits: the registry not-found, the not-onboarded not-found and the store-full conflict each audit before returning, and so does the accepted path. Its FIRST exit does not: the `integrationConfig` union check at the top of `handle` — the one the handler's own comment says exists because "the centralized constraint walk does not reach" a value behind a union, so it is the handler and not the boundary that refuses — returns its `ValidationException` with no audit call at all. `DeleteConnectInstanceIntegration` has the identical hole on its `integrationIdentifier` check.

So an attempt to configure (or remove) an instance's integration with a malformed resource ARN is invisible in the audit trail, while the same operation's not-found and conflict refusals, and the handler-raised `ValidationException` refusals of `StartInstanceOnboardingJob` and `DeleteConnectInstanceConfig` on the very same instance, are all recorded. This is the same rule the open PutInstanceCommunicationLimits issue describes, and that issue's claim that the limits put is "the one remaining hole" is incomplete: these two operations share it. Closing it changes no response body and no status — only the structured record the crate promises for every attempt.

## Evidence

The rule, services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1249-1255:

```
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields. Called on EVERY path such a call returns through
/// — the accepted one and each refusal, named by the modelled error the refusal
/// answers with — so an attempt to change an instance is as visible when it was
/// refused as when it succeeded.
```

The unaudited refusal — src/ops/put_connect_instance_integration.rs:177-194:

```
        // The integration's ARN sits behind a union member, which the centralized
        // constraint walk does not reach, so the shared check holds it to the
        // `@length` and `@pattern` of the shape it targets — as a bad request, before
        // the instance is looked up or anything is stored.
        if let Some(message) = crate::state::integration_config_violation(
            "integrationConfig",
            &input.integration_config,
        ) {
            return Err(
                PutConnectInstanceIntegrationErrorModeled::ValidationException(
                    ValidationException { message, x_amz_error_type: None },
                )
                .into(),
            );
        }
```

while the three refusals below it in the same handler all audit — src/ops/put_connect_instance_integration.rs:225-230, :262-267 and :293-298, e.g.:

```
                crate::state::audit_onboarding_change(
                    ctx.parts(),
                    Self::NAME,
                    &input.connect_instance_id,
                    "ResourceNotFoundException",
                );
```

The same hole in the sibling delete — src/ops/delete_connect_instance_integration.rs:182-195 returns `ValidationException` from `crate::state::integration_identifier_violation` with no audit call, although :230-235, :270-275, :300-305 and :324-329 audit every other exit.

Siblings audit exactly this kind of handler-raised `ValidationException`: src/ops/start_instance_onboarding_job.rs:222-230 (`check_encryption_config` → `audit_onboarding_change(.., "ValidationException")`) and src/ops/delete_connect_instance_config.rs:193-201, whose comment states the rule: "A refused attempt to change the instance is still an attempt to change it, so it leaves the same record a successful one does, naming the outcome it ended in."

Test coverage: tests/all/instance_mutation_audit.rs:95-96 calls its table "one row per (operation, outcome) pair this service can answer an instance-scoped call with", and the rows at :161-196 carry only `PutConnectInstanceIntegration`/"accepted", `PutConnectInstanceIntegration`/"ResourceNotFoundException" and `DeleteConnectInstanceIntegration`/"ResourceNotFoundException" — no ValidationException row for either, so nothing pins the current silence. That these refusals really are handler refusals is the premise the handlers' own comments state (quoted above) and that tests/all/union_member_arn_constraints.rs:3-19 exercises end to end ("an ARN-valued member behind a union member is held to the `@length` and `@pattern` of the shape it targets … so a value outside those bounds is HTTP 400 `ValidationException` naming the member, and the call stores nothing").

## Steps to Reproduce

1. Register a Connect instance and onboard it (`PUT /v2/connect-instance/{id}/onboarding`), waiting out `ONBOARDING_JOB_DURATION`.
2. Capture `tracing` events the way tests/all/instance_mutation_audit.rs does (filter on `audit=true`).
3. Call `PUT /v2/connect-instance/{id}/integrations` with `{"integrationConfig": {"customerProfiles": {"domainArn": "x", "objectTypeNames": {}}}}` — a `domainArn` far below `Arn`'s `@length` minimum of 20.
4. Observe HTTP 400 `ValidationException` and NO `audit=true` record for that instance: src/ops/put_connect_instance_integration.rs:185-193 returns before the audit calls at :225, :262, :293 and :313.
5. For contrast, call `PUT /v2/connect-instance/{id}/onboarding` with `{"encryptionConfig": {"enabled": true}}`: the same 400 `ValidationException` emits `operation=StartInstanceOnboardingJob outcome=ValidationException` (src/ops/start_instance_onboarding_job.rs:222-230).
6. Repeat step 3 against `POST /v2/connect-instance/{id}/integrations/delete` with `{"integrationIdentifier": {"lambda": {"functionArn": ""}}}` for the same missing record from src/ops/delete_connect_instance_integration.rs:182-195.
