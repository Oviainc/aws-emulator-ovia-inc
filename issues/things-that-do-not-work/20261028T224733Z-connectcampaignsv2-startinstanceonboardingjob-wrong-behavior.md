Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: f4264046b28dc84c23f39b0be798541edaa69830

# The server-fault refusal paths of the instance mutators leave no audit record, although the shared audit helper documents being called on every path a call returns through

_Filed: 2026-10-28T22:47:33Z_
_Operation: StartInstanceOnboardingJob | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f4264046b28dc84c23f39b0be798541edaa69830`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::audit_onboarding_change` is documented as the one place this service writes an instance audit record and as being called on EVERY path a state-changing Connect-instance call returns through — "the accepted one and each refusal, named by the modelled error the refusal answers with" — so that an attempt to change an instance is as visible when it was refused as when it succeeded.

`StartInstanceOnboardingJob` honours that for three of its five exits and breaks it for two. The `ValidationException` exit (bad `encryptionConfig`), the `ResourceNotFoundException` exit (the account's Connect registry holds no such instance) and the `ConflictException` exit (onboarding store full) each call the helper with the name of the modelled error they answer with. The two `InternalServerException` exits — the store being unreachable, and the Amazon Connect instance registry being unreachable — return without calling it at all, so an attempt to onboard an instance that was refused leaves no trace, while the refusal one line earlier in the same handler does.

The registry-unavailable exit is the notable one: it is a refusal of a request that named a specific instance and asked to change what this service holds for it, decided after the handler already audited a different refusal of the same request shape, so the audit trail records some refused onboarding attempts and silently drops others depending only on which refusal fired. The same omission sits on the `InternalServerException` exit of every sibling instance mutator (`put_connect_instance_integration.rs:176-187`, `put_instance_communication_limits.rs:188-198`, `delete_connect_instance_config.rs:218-228`, `delete_instance_onboarding_job.rs:168-177`, `delete_connect_instance_integration.rs:175-186`), which is why the rule belongs on the one path the helper's own doc describes rather than in each handler's happy-path tail. No response body or status changes if the gap is closed — only the structured record the crate promises for every attempt.

## Evidence

The rule, stated by the helper this crate centralizes auditing in — services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/state.rs:1019-1025:

```
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields. Called on EVERY path such a call returns through
/// — the accepted one and each refusal, named by the modelled error the refusal
/// answers with — so an attempt to change an instance is as visible when it was
/// refused as when it succeeded.
```

The handler obeys it on the validation refusal — src/ops/start_instance_onboarding_job.rs:222-230:

```
        if let Err(refusal) = check_encryption_config(&input.encryption_config) {
            crate::state::audit_onboarding_change(
                ctx.parts(),
                Self::NAME,
                &input.connect_instance_id,
                "ValidationException",
            );
            return Err(refusal);
        }
```

and breaks it twelve lines later on the registry-unavailable refusal — src/ops/start_instance_onboarding_job.rs:242-252 (no `audit_onboarding_change` call on this path):

```
        let Some(instance_exists) =
            crate::state::connect_instance_exists(&*self.engine, &input.connect_instance_id)
        else {
            return Err(StartInstanceOnboardingJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Amazon Connect instance registry is unavailable".to_string(),
                    x_amz_error_type: None,
                },
            )
            .into());
        };
```

and again on the store-unavailable refusal — src/ops/start_instance_onboarding_job.rs:207-216:

```
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(StartInstanceOnboardingJobErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the ConnectCampaignsV2 instance onboarding store is unavailable"
                        .to_string(),
```

Test coverage: tests/all/instance_mutation_audit.rs:95-96 describes its table as "one row per (operation, outcome) pair this service can answer an instance-scoped call with, plus the reporting calls that must leave nothing", and its rows cover only `accepted`, `ValidationException`, `ResourceNotFoundException` and `InvalidStateException` (lines 103-276) — there is no row for the server-fault outcome, so nothing pins the current silence. The companion assertion `every_instance_mutation_records_how_it_ended` (line 359) is where a row for it would land.

## Steps to Reproduce

1. Read src/state.rs:1019-1025: `audit_onboarding_change` is documented as called on EVERY path a state-changing instance call returns through, naming the modelled error of each refusal.
2. Read src/ops/start_instance_onboarding_job.rs:222-230, 253-267 and 283-304: the ValidationException, ResourceNotFoundException and ConflictException refusals each call `audit_onboarding_change` before returning.
3. Read src/ops/start_instance_onboarding_job.rs:207-216 and 242-252: both `InternalServerException` refusals return with no call to `audit_onboarding_change`, so the attempt leaves no record.
4. Grep the sibling instance mutators for the same shape (`put_connect_instance_integration.rs:176-187`, `put_instance_communication_limits.rs:188-198`, `delete_connect_instance_config.rs:218-228`, `delete_instance_onboarding_job.rs:168-177`, `delete_connect_instance_integration.rs:175-186`): every one answers its server fault without auditing the attempt.
5. Compare with tests/all/instance_mutation_audit.rs:101-276: the per-(operation, outcome) table has no row for the server-fault outcome, so the omission is unpinned.
