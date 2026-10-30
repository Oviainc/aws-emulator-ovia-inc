Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f

# Replaying the same UpdateConfigurationDefinition rewinds every deployment status and moves LastModifiedAt, unlike the crate's other @idempotent mutators

_Filed: 2026-10-29T04:04:51Z_
_Operation: UpdateConfigurationDefinition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fb2d49e8bd3d09cec7ea7562ca3d7e59e4fee21f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConfigurationDefinition` is modelled `@idempotent` (OPERATIONS.md:113 "**Traits:** @idempotent"), and its own handler comment states the contract it keeps: "replaying one request leaves exactly the same definition". The code under that comment, however, rewrites observable state on EVERY call, whether or not the stored record actually changes:

* `manager.last_modified_at` is bumped unconditionally;
* `crate::state::restart_deployment(&mut manager.status_summaries, now)` resets the manager's `Deployment` summary to `INITIALIZING` and its `AsyncExecutions` summary to `NONE`;
* `store.deploy_definition(..)` then calls `restart_deployment` on EVERY configuration deployed from that definition (state.rs:1332) and bumps each one's `last_modified_at` (state.rs:1331).

So a client that retries the identical PUT after a timeout — the exact reason the model marks this operation `@idempotent` — sees the reported deployment of the manager and of every configuration rewind from `SUCCEEDED` back to `INITIALIZING`, and every `LastModifiedAt` move, even though the definition the second call stores is byte-for-byte the one the first call stored. The same happens for a request that carries NONE of its four optional members (an empty body is valid: only the two `@httpLabel` members are `@required`), i.e. a request that asks for no change at all still rewinds the deployment.

The crate's sibling `@idempotent` mutators of the same record implement exactly the missing guard: `TagResource` and `UntagResource` track whether the stored record actually changed and move the stamp only then, with a comment tying that guard to the shared `@idempotent` trait. `UpdateConfigurationDefinition` skips it.

## Evidence

Missing guard — services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/update_configuration_definition.rs:376-381 and 396-414:
```
        // Every member but the two labels is optional, and only what the request
        // carries is replaced, so an update naming one member leaves the others as they
        // were stored. `Parameters` is the definition's whole parameter set ...
        // than merging into it, which is also what keeps the operation `@idempotent`:
        // replaying one request leaves exactly the same definition.
...
        // Monotonic: the stamp never moves backwards, whatever the host clock does.
        manager.last_modified_at = now.max(manager.last_modified_at);
        // Updating a definition submits it for deployment again, so every kind of work
        // the manager reports starts over in its first phase ...
        crate::state::restart_deployment(&mut manager.status_summaries, now);
...
        store.deploy_definition(&input.manager_arn, &definition, &targets, now);
```
Fan-out of the restart — src/state.rs:1327-1333 (inside `deploy_definition`):
```
            configuration.last_modified_at = now.max(configuration.last_modified_at);
            restart_deployment(&mut configuration.status_summaries, now);
```
Sibling that DOES guard — src/ops/tag_resource.rs:267-287:
```
        // Assignment is a MERGE: ... Re-assigning the value a key already holds
        // changes nothing, which is what makes this `@idempotent` operation answer the
        // same way however often the same request is repeated.
        let mut changed = false;
...
        // The stamp moves only when the stored record actually changed, so a repeat of
        // an already-served assignment leaves every reported value as it was.
        if changed {
            manager.last_modified_at = now.max(manager.last_modified_at);
        }
```
Same guard in src/ops/untag_resource.rs:255-260 (`if removed > 0 { manager.last_modified_at = ... }`).

Test coverage: tests/all/status_summaries.rs:600-619 only exercises an update that genuinely CHANGES the definition (`TargetRegions` goes from one Region to two) and asserts the restart for that case; no test replays an identical update or sends a body with no members, so the no-change path is unpinned. tests/all/resource_tags.rs:439 and :793 pin the opposite rule for the tag mutators ("the stamp a read reports does not move for a repeat").

## Steps to Reproduce

1. POST /configurationManager with one ConfigurationDefinitions entry (Type `AWSQuickSetupType-SSMHostMgmt`, Parameters `{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-east-1"}`) and note the ManagerArn and the definition Id from GET /configurationManager/{ManagerArn}.
2. Wait past TRANSITION_SPAN (500 ms) and GET /configurationManager/{ManagerArn}: the Deployment summary reports SUCCEEDED; note LastModifiedAt.
3. PUT /configurationDefinition/{ManagerArn}/{Id} with body `{"Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-east-1"}}` — the parameter set already stored — or with an empty body `{}`.
4. GET /configurationManager/{ManagerArn} and GET /getConfiguration/{ConfigurationId}: the Deployment summaries report INITIALIZING again and LastModifiedAt has moved, although the stored definition is unchanged.
5. Compare with the tag path: PUT /tags/{ManagerArn} re-assigning a tag value the manager already holds leaves LastModifiedAt exactly as it was (tag_resource.rs:285-287).
