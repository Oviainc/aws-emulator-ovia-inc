Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3371d8250e53a8cf648176cf4566c882bfa257d5

# Tagging a deployment rewrites its record without refreshing the modifiedAt every sibling mutator refreshes

_Filed: 2026-10-29T07:29:06Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3371d8250e53a8cf648176cf4566c882bfa257d5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` and `UntagResource` mutate the stored `Deployment` record (its `tags` map, which `GetDeployment` reports as part of `DeploymentData` alongside `modifiedAt`), but neither touches `deployment.modified_at`. Every other writer of that same record in this crate refreshes it: `UpdateDeployment` sets `deployment.modified_at = now`, `DeleteDeployment` sets `deployment.modified_at = now`, `CreateDeployment` seeds it, and the clock-owed status steps assign `self.modified_at = step.due_at`. The two tagging handlers are the only mutations of the record that skip that one write.

Consequence: after a successful `TagResource` (or `UntagResource`), `GetDeployment` and `ListDeployments` report a `modifiedAt` that predates the last change to the record they describe — the tag set they report in the same response was written after the timestamp they report for it. There is no comment anywhere in either handler (or on the `tags` / `modified_at` fields in `state.rs`) stating that a tag write is deliberately not a modification of the deployment, and no test pins the current behaviour: `modifiedAt` appears in only three test files (create_deployment_status_sequence.rs, deployment_lifecycle.rs, deployment_update.rs), none of which exercises a tagging call.

The same defect exists on the identical line of `untag_resource.rs`; both handlers should be fixed together, since they write the same field of the same record.

## Evidence

The two tagging mutations, which write the record and leave `modified_at` alone —

services/service-com-amazonaws-launchwizard-launchwizard/src/ops/tag_resource.rs:234-236:

        for (key, value) in tags {
            deployment.tags.insert(key, value);
        }

services/service-com-amazonaws-launchwizard-launchwizard/src/ops/untag_resource.rs:195-199:

        // `@idempotent`: removing a key the resource does not carry is a no-op, so the
        // same request twice leaves the same tag set and answers the same way.
        for key in &tag_keys {
            deployment.tags.remove(key);
        }

The sibling mutators of the SAME record, which all refresh it —

src/ops/update_deployment.rs:337-339:

        deployment.enter_status(crate::state::STATUS_UPDATE_IN_PROGRESS.to_string());
        deployment.status_reason = None;
        deployment.modified_at = now;

src/ops/delete_deployment.rs:178-179:

            deployment.enter_status(crate::state::STATUS_DELETE_INITIATING.to_string());
            deployment.modified_at = now;

src/state.rs:771-780 (`Deployment::materialize_due`):

    /// the state the clock already produced. The event timestamp and `modifiedAt` are
    /// the step's own due instant, not "now", so the record does not depend on when it
    /// was observed.
    ...
            self.modified_at = step.due_at;

That the tags are part of the very record whose `modifiedAt` is reported —

src/state.rs:669-671:

    /// Tags live here, on the deployment, keyed by nothing else — so the tagging
    /// operations can be layered on without a second store.
    pub tags: BTreeMap<String, String>,

src/state.rs:823-836 (`Deployment::to_data`, what `GetDeployment` answers):

            modified_at: Some(to_date_time(self.modified_at)),
            ...
            tags: to_hash_map_member(&self.tags),

And the only three writes of that field in the whole crate (grep `modified_at` over src/ops): update_deployment.rs:339, delete_deployment.rs:179, create_deployment.rs:432 — no tagging handler appears.

Test coverage: grep `modifiedAt|modified_at` over tests/ matches only tests/all/create_deployment_status_sequence.rs:263, tests/all/deployment_lifecycle.rs:270 and tests/all/deployment_update.rs:351 — the tagging suites (tests/all/resource_tagging.rs, tests/all/empty_collection_members.rs) assert the tag set only, so nothing pins the stale timestamp and fixing it changes no existing assertion.

## Steps to Reproduce

1. POST /createDeployment with a valid workload/pattern and the pattern's required specifications; note the returned deploymentId.
2. Wait ~1.5 s so the clock-owed CREATING -> VALIDATING -> IN_PROGRESS -> COMPLETED steps have all been materialised, then POST /getDeployment {"deploymentId": "<id>"} and record deployment.modifiedAt as M1.
3. POST /tags/arn:aws:launchwizard:us-east-1:<account>:deployment/<id> with {"tags": {"Owner": "emulator"}} — answers 200.
4. POST /getDeployment again: deployment.tags now carries "Owner", but deployment.modifiedAt is still exactly M1 — the record changed after the time it reports as its last modification.
5. DELETE /tags/arn:aws:launchwizard:us-east-1:<account>:deployment/<id>?tagKeys=Owner — answers 200; GetDeployment shows the tag gone and modifiedAt still M1.
6. Compare with POST /updateDeployment {"deploymentId": "<id>", "specifications": {...}, "dryRun": false, "force": false}, after which modifiedAt advances (src/ops/update_deployment.rs:339).
