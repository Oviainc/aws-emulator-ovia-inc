Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 19fca1009697525153c46de34cf9f84b5fd316c3

# PutInstanceCommunicationLimits leaves no audit record when it refuses with ValidationException, unlike every sibling instance mutator

_Filed: 2026-10-27T18:56:31Z_
_Operation: PutInstanceCommunicationLimits | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `19fca1009697525153c46de34cf9f84b5fd316c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::audit_onboarding_change` is documented as the one place this service records a state-changing Connect-instance call together with HOW IT ENDED, and its doc says it is called on EVERY path such a call returns through, naming the modelled error of each refusal. `PutInstanceCommunicationLimits` honours that for two of its three outcomes only: the single audit call sits AFTER the store write and reports either "accepted" or "ResourceNotFoundException". Its own handler-raised `ValidationException` path — the over-long `communicationLimitsList` check at the top of `handle`, which the handler's comment says the centralized constraint walk does not reach, so it is the handler and not the boundary that refuses — returns before any audit call is made, and so does the `InternalServerException` path.

The result is an audit trail in which an attempt to reconfigure an instance's communication limits with an out-of-bounds config is invisible, while the same operation's not-found refusal, and `StartInstanceOnboardingJob`'s and `DeleteConnectInstanceConfig`'s own handler-raised `ValidationException` refusals on the same instance, are all recorded. That is the one remaining hole in the rule the crate states and its siblings all follow.

## Evidence

The helper's contract, src/state.rs:1019-1025:
```
/// The one place this service writes an instance audit record, so a later instance
/// mutator records the same fields. Called on EVERY path such a call returns through
/// — the accepted one and each refusal, named by the modelled error the refusal
/// answers with — so an attempt to change an instance is as visible when it was
/// refused as when it succeeded.
```

The handler's unaudited refusal, src/ops/put_instance_communication_limits.rs:168-186:
```
        // The modelled `@length` of `communicationLimitsList` sits on a union member,
        // which the centralized constraint walk does not reach, so an over-long list
        // is refused here — as a bad request, before anything is looked up or stored,
        // and through the same check every other operation that takes a
        // `CommunicationLimits` uses.
        if let Some(message) = crate::state::communication_limits_violation(
            "communicationLimitsConfig.allChannelSubtypes.communicationLimitsList",
            input.communication_limits_config.all_channel_subtypes.as_ref(),
        ) {
            return Err(
                PutInstanceCommunicationLimitsErrorModeled::ValidationException(
                    ValidationException { message, x_amz_error_type: None },
                )
                .into(),
            );
        }
```
and its only audit call, src/ops/put_instance_communication_limits.rs:218-227:
```
        crate::state::audit_onboarding_change(
            ctx.parts(),
            Self::NAME,
            &input.connect_instance_id,
            if stored { "accepted" } else { "ResourceNotFoundException" },
        );
```

Siblings audit the SAME kind of handler-raised ValidationException:
- src/ops/start_instance_onboarding_job.rs:222-230 — `if let Err(refusal) = check_encryption_config(&input.encryption_config) { crate::state::audit_onboarding_change(ctx.parts(), Self::NAME, &input.connect_instance_id, "ValidationException"); return Err(refusal); }`
- src/ops/delete_connect_instance_config.rs:193-201 — `// A refused attempt to change the instance is still an attempt to change it, so it leaves the same record a successful one does, naming the outcome it ended in.` followed by `audit_onboarding_change(ctx.parts(), Self::NAME, &input.connect_instance_id, "ValidationException")`.

The rule the suite states, tests/all/instance_mutation_audit.rs:10-18:
```
//! * a call that is asked to change a Connect instance leaves EXACTLY ONE structured
//!   record naming the operation, the access key id the request presented, the
//!   instance the request named and how the call ended — the accepted outcome when the
//!   instance changed, and the name of the modelled error otherwise — so an attempt is
//!   as visible when it was refused as when it succeeded, whichever operation was
//!   called;
//! * ...
//! * a reporting call leaves none, and so does a request the modelled constraints
//!   refuse before any handler runs;
```
The table carries rows for `PutInstanceCommunicationLimits` / "accepted" (tests/all/instance_mutation_audit.rs:136-146) and `PutInstanceCommunicationLimits` / "ResourceNotFoundException" (:147-160), and a row for `StartInstanceOnboardingJob` / "ValidationException" (:125-135), but NO row for this operation's ValidationException outcome — so nothing pins the current (missing) behaviour. That the over-long-list refusal really is a handler refusal and not a boundary one is pinned by tests/all/instance_communication_limits.rs:521-530 ("three entries, one past communicationLimitsList's @length maximum of 2" → HTTP 400 ValidationException).

## Steps to Reproduce

1. Register and onboard a Connect instance, waiting out `ONBOARDING_JOB_DURATION` so its job reports SUCCEEDED.
2. Capture `tracing` events (as tests/all/instance_mutation_audit.rs does) and call `PUT /v2/connect-instance/{id}/communication-limits` with `{"communicationLimitsConfig": {"allChannelSubtypes": {"communicationLimitsList": [l1, l2, l3]}}}` — three entries, one past the modelled maximum of 2.
3. Observe HTTP 400 `ValidationException` and NO event carrying `audit=true` for that instance: src/ops/put_instance_communication_limits.rs:177-185 returns before the audit call at :218.
4. For contrast, call `PUT /v2/connect-instance/{id}/onboarding` with `{"encryptionConfig": {"enabled": true}}`: the same 400 `ValidationException` emits an `audit=true` record with `operation=StartInstanceOnboardingJob outcome=ValidationException` (src/ops/start_instance_onboarding_job.rs:222-230).
5. Repeat the first call against an instance that is NOT onboarded: the ResourceNotFoundException refusal IS audited (:218-227), so the gap is specific to the ValidationException path of the same operation.
