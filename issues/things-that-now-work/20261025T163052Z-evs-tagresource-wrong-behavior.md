Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd

# TagResource re-stamps the environment's modifiedAt even when the merge changes no tag

_Filed: 2026-10-25T16:30:52Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a4b61295d097ae4da0c45de1d4f9e67ffc2c86fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` is `@idempotent`, and the crate's rule everywhere else is that a call which changes nothing does not move `modifiedAt`. `TagResource` is the one writer that stamps unconditionally: it computes `merged` (stored tags + request tags) and then assigns `environment.modified_at_ms = now_ms()` without ever asking whether `merged` differs from what was stored. So tagging a resource with exactly the tags it already carries — the idempotent replay of a TagResource call — advances the environment's `modifiedAt`, which `GetEnvironment`, `ListEnvironments`, `CreateEnvironmentHost` and `DeleteEnvironmentConnector` all report. Its own sibling on the same tagging surface, `UntagResource`, computes a `removed` flag and skips the stamp when nothing was removed; `DisassociateEipFromVlan`, `UpdateEnvironmentConnector` and `DeleteEnvironment` each do the same for their own records and say so in a comment. The fix is the `changed`/`removed` guard the siblings already have: stamp only when the merged map differs from the stored one.

## Evidence

The unconditional stamp — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/tag_resource.rs:244-245:
```
        environment.tags = merged;
        environment.modified_at_ms = crate::state::now_ms();
```
with no comparison anywhere above it; the merge at tag_resource.rs:226-227 is `let mut merged = environment.tags.clone(); merged.extend(input.tags);`.

The sibling on the same surface, which DOES guard — src/ops/untag_resource.rs:203-209:
```
        let mut removed = false;
        for key in &input.tag_keys {
            removed |= environment.tags.remove(key).is_some();
        }
        if removed {
            environment.modified_at_ms = crate::state::now_ms();
        }
```

The crate states the rule explicitly in three more places:
* src/ops/disassociate_eip_from_vlan.rs:257-259 — "`@idempotent`: the end state is what the call asks for ... Only a call that really removed something re-stamps the record."
* src/ops/update_environment_connector.rs:277-279 — "A value the connector already carries is NOT a change: the record keeps its state and its modifiedAt, so repeating the call neither restarts the connector nor moves a stamp."
* src/ops/delete_environment_connector.rs:298-300 — "deleting a child resource of it is no modification of the environment itself, so its own `modifiedAt` does not move."

That `TagResource` is idempotent: OPERATIONS.md:54-56 — "## TagResource / - **Traits:** @idempotent".

Test coverage: tests/all/tagging_resource_arn_resolution.rs:198-224 only pins that a REFUSED tagging call does not move `modifiedAt` ("a refused tagging call must not move modifiedAt"); no test in tests/all/ asserts anything about `modifiedAt` after an accepted no-op TagResource (grep for `modifiedAt` across tests/all shows only environment_deletion_hosts.rs, connector_deletion.rs, tagging_resource_arn_resolution.rs, connector_secret_arn.rs, environment_lifecycle.rs, connectors.rs, none of them this case), so the guard can be added with every existing assertion intact.

## Steps to Reproduce

1. Create an environment (`CreateEnvironment`) and let its CREATING -> CREATED transition settle.
2. Call `TagResource` with `{"resourceArn": <arn>, "tags": {"team": "platform"}}`.
3. Call `GetEnvironment` and record `environment.modifiedAt` (call it T1).
4. Call `TagResource` again with the identical body — the stored tag map is already exactly `{"team": "platform"}`, so the merge writes the same map back.
5. Call `GetEnvironment` again: `modifiedAt` is now later than T1 even though no tag changed. Repeating `UntagResource` for a key the resource no longer carries, by contrast, leaves `modifiedAt` where it was (untag_resource.rs:207-209).
