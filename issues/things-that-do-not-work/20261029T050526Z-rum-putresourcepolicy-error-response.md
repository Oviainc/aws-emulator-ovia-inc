Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 314ff66d020c8bfcdd48f21d567324d2b402cbc6

# PutResourcePolicy judges PolicyDocument's size and syntax only after resolving the monitor and authorizing, unlike every sibling handler in the crate

_Filed: 2026-10-29T05:05:26Z_
_Operation: PutResourcePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `314ff66d020c8bfcdd48f21d567324d2b402cbc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate judges the request's OWN members — the checks that stand in for the constraints the framework's boundary walk cannot apply — before it resolves the addressed resource and before it makes the authorization decision. `UpdateRumMetricDefinition` states that ordering as the crate's rule in so many words: "Judged FIRST, because that walk runs in front of everything a handler judges."

`PutResourcePolicy` is the one handler that breaks it. Its two STATELESS member checks — `PolicyDocument`'s 4 KB ceiling (`PolicySizeLimitExceededException`) and its IAM-JSON well-formedness (`MalformedPolicyDocumentException`) — are the handler's to make precisely because, in the handler's own words, `PolicyDocument` "is modelled as a bare `String`, so NOTHING about its contents is refused before a handler runs". Yet they are placed after the app-monitor resolution (which answers `ResourceNotFoundException`) and after `authorization::authorize` (which answers `AccessDeniedException`), even though neither check needs the store or the caller's identity.

Consequences, both observable from the wire:

* a put carrying a malformed or oversized `PolicyDocument` for an app monitor that does not exist (or exists in another region) answers `ResourceNotFoundException` (404), where the eleven sibling handlers answer the member refusal the model declares for that member;
* the two modelled errors `MalformedPolicyDocumentException` and `PolicySizeLimitExceededException` are unreachable for any caller that is not allowed `rum:PutResourcePolicy` on the monitor — the 403 is answered first — so a client cannot learn that its policy document is itself invalid.

The two checks read nothing from the store, so moving them in front of the resolution (where every sibling puts its own member checks) changes no other response.

## Evidence

The crate states the ordering rule itself — `services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:177-181`:
```
        // The modelled constraints of the definition's `DimensionKeys` that the boundary
        // walk leaves unapplied — the map KEY's `@length`, which it applies to no map key,
        // and the VALUE's look-around `@pattern`, which its engine cannot compile. Judged
        // FIRST, because that walk runs in front of everything a handler judges.
        if let Some(message) = crate::state::dimension_keys_refusal(&input.metric_definition) {
```

Siblings all follow it — member checks first, resolution and `authorize` after:
* `src/ops/create_app_monitor.rs:210` (`app_monitor_refusal`), `:230` (`domain_choice_refusal`), `:243` (`app_monitor_configuration_refusal`), `:257` (`deobfuscation_configuration_refusal`), `:283` (`tag_keys_refusal`) — all ahead of `:299` `crate::state::authorization::authorize_identity(`;
* `src/ops/put_rum_metrics_destination.rs:190-200` (`metric_destination_choice_refusal` / `metric_destination_role_refusal`) ahead of `:211` `store.app_monitor(&region, &input.app_monitor_name)` and `:221` `authorize(`;
* `src/ops/batch_create_rum_metric_definitions.rs:221-261` (dimension keys, destination pairing, batch bound) ahead of `:270` resolution and `:280` `authorize(`;
* `src/ops/get_app_monitor_data.rs:188-202` (the filter-key refusal) ahead of `:211` resolution and `:219` `authorize(`;
* `src/ops/tag_resource.rs:315` (`tag_keys_refusal`) ahead of `:326` resolution and `:336` `authorize(`;
* `src/ops/untag_resource.rs:375-379` (`tag_key_list_refusal`) ahead of `:391` resolution and `:401` `authorize(`.

`PutResourcePolicy` inverts it — `services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs:189-198` resolves and refuses first:
```
        let (resource_arn, held_policy) = {
            let store = state.lock();
            let Some(record) = store.app_monitor(&region, &input.name) else {
                return Err(PutResourcePolicyErrorModeled::ResourceNotFoundException(
                    crate::state::app_monitor_not_found(&input.name),
                )
                .into());
            };
```
then authorizes — `:202-214`:
```
        if let Err(denial) = crate::state::authorization::authorize(
            &*self.engine,
            ctx,
            crate::state::authorization::PUT_RESOURCE_POLICY,
```
and only THEN judges the member, admitting in the same breath that nothing judged it earlier — `:226-251`:
```
        // `PolicyDocument` is modelled as a bare `String`, so its size and its syntax are
        // this handler's to refuse, each with the error the model declares for it: 4 KB
        // is the documented ceiling, and anything that is not a well-formed IAM JSON
        // policy is "not formatted correctly".
        if input.policy_document.len() > crate::state::MAX_POLICY_DOCUMENT_BYTES {
...
        if let Some(message) = crate::state::policy_document_refusal(&input.policy_document) {
```

Neither check touches the store: `crate::state::policy_document_refusal` takes only `&str` (`src/state.rs:2543`) and `MAX_POLICY_DOCUMENT_BYTES` is a constant (`src/state.rs:425`), so the ordering is not forced by a data dependency.

Test coverage: `tests/all/resource_policy.rs` is the suite for this operation; it exercises the malformed-document and size refusals against an app monitor that EXISTS and with an allowed caller, so no existing test pins the precedence above, and moving the two stateless checks in front of the resolution leaves every asserted response unchanged.

## Steps to Reproduce

1. CreateAppMonitor(Name="m1", Domain="example.com") with an allowed caller — 200
2. PutResourcePolicy(Name="does-not-exist", PolicyDocument="{ not json") — observed ResourceNotFoundException (404, "Resource not found does-not-exist"); every sibling handler refuses the request's own member first, so the expected answer is the modelled MalformedPolicyDocumentException (400) the handler itself owns for that member
3. PutResourcePolicy(Name="m1", PolicyDocument=<4 KB + 1 bytes of text>) signed by a principal holding no rum:PutResourcePolicy grant — observed AccessDeniedException (403); the modelled PolicySizeLimitExceededException (400) is unreachable for that caller, although the check needs neither the store nor the caller's identity
4. Compare with PutRumMetricsDestination(AppMonitorName="does-not-exist", Destination="CloudWatch", DestinationArn="arn:aws:evidently:us-east-1:111122223333:experiment/e") — the member-pairing ValidationException (400) is answered ahead of the not-found, which is the ordering this crate applies everywhere else
