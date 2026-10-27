Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# TagResource and UntagResource rewrite a record's tag map without refreshing the record's documented `updated_at`

_Filed: 2026-10-27T02:22:06Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeploymentParameterRecord::updated_at` is documented in this crate as "When the record was last written". `PutDeploymentParameter` honours that: both its create arm and its update-in-place arm assign `updated_at = now`. The two tagging writes do not. `TagResource` replaces the record's whole tag map (`record.tags = merged;`) and `UntagResource` removes keys from it (`record.tags.remove(key);`) — both are writes to the stored record — yet neither assigns `updated_at`, so after a tag write the field reports the instant of the last `PutDeploymentParameter` rather than the instant the record was last written.

Both handlers already sample the clock one statement earlier (`let now = ::std::time::SystemTime::now();`, used for `purge_expired`), so the value the field's own documentation calls for is in scope and simply not applied. The result is a stored field that contradicts its own doc comment and that disagrees with the sibling handler of the same crate: two records written at the same instant — one by `PutDeploymentParameter`, one by `TagResource` — report different "last written" times.

No operation of this model reports `updated_at` on the wire, so there is no response change; the defect is in the persisted record, which is `pub` on the crate's `pub` state and which the crate's own tests read.

## Evidence

The rule, in this crate's state module — `services/.../src/state.rs:367-370`:
```
    /// When the record was first written.
    pub created_at: SystemTime,
    /// When the record was last written.
    pub updated_at: SystemTime,
```

The sibling handler that applies it — `services/.../src/ops/put_deployment_parameter.rs:373` (update arm) and `:416` (create arm):
```
                record.updated_at = now;
```
```
                        updated_at: now,
```

The two handlers that write the record and skip it — `services/.../src/ops/tag_resource.rs:206` then `:240`:
```
        let now = ::std::time::SystemTime::now();
...
        record.tags = merged;

        Ok(TagResourceOutput {})
```
and `services/.../src/ops/untag_resource.rs:179` then `:197-199`:
```
        let now = ::std::time::SystemTime::now();
...
        for key in &input.tag_keys {
            record.tags.remove(key);
        }
```

A repo-wide grep confirms `updated_at` is assigned nowhere outside `put_deployment_parameter.rs`:
```
src/state.rs:370:     pub updated_at: SystemTime,
src/state.rs:387:             .field("updated_at", &self.updated_at)
src/ops/put_deployment_parameter.rs:373:                 record.updated_at = now;
src/ops/put_deployment_parameter.rs:416:                         updated_at: now,
```

Test coverage: the only assertion on the field is for the Put path — `tests/all/expiration_date_wire_form.rs:586`: `record.updated_at >= created_at,` — so no test pins the tagging paths' behaviour either way.

## Steps to Reproduce

1. Build the crate's test harness (`tests/all/deployment_parameters.rs::emulator`) and provision a target with `state::seed_agreements`.
2. Call `PutDeploymentParameter` for (catalog=AWSMarketplace, productId, agreementId, deploymentParameter.name) and note the minted `resourceArn`.
3. Read the store through `state::service_state(&*engine)` and record `store.parameters[&arn].updated_at` — call it T1.
4. Sleep briefly, then call `TagResource` on that ARN with `tags={"team":"platform"}` and assert it answered 204.
5. Read `store.parameters[&arn]` again: `tags` now contains `team=platform` (the record WAS written), but `updated_at` is still exactly T1 — contradicting "When the record was last written" (state.rs:369) and diverging from `put_deployment_parameter.rs:373`, which refreshes it for the same kind of in-place record write.
6. Repeat with `UntagResource` on `tagKeys=["team"]`: the key is removed from the stored map while `updated_at` stays T1.
