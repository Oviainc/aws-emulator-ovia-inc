Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the dlm emulator

_Filed: 2026-10-21T22:07:49Z_
_Operation: service-com-amazonaws-dlm-dlm-20180112 | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 8 DLM (Amazon Data Lifecycle Manager) operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds zero state (e.g. `src/ops/create_lifecycle_policy.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`). Implement a FIRST stateful slice: the lifecycle-policy CRUD loop, wire protocol `restJson1`, signing name `dlm`.

### Happy path (implement these 5, in order)
1. **CreateLifecyclePolicy** — `POST /policies`. Input: `ExecutionRoleArn` (required, `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`), `Description` (required, `^[0-9A-Za-z _-]+$`, <=500), `State` (required, ENABLED|DISABLED), plus optional `PolicyDetails`, `Tags`, `DefaultPolicy`, `CreateInterval`, `RetainInterval`, `CopyTags`, `ExtendDeletion`, `CrossRegionCopyTargets`, `Exclusions`. Must mint a NEW unique `PolicyId` matching `^policy-[a-f0-9]+$`, persist the whole record (including the derived `PolicyArn` `arn:aws:dlm:<region>:<account>:policy/<id>`, `DateCreated`, `DateModified`, and whether it is a default policy), and return only `PolicyId`. Two creates must yield two distinct ids and two distinct stored policies.
2. **GetLifecyclePolicy** — `GET /policies/{PolicyId}`. Returns `Policy` (`LifecyclePolicy`) read back from the store: `PolicyId`, `Description`, `State` (GettablePolicyStateValues: ENABLED|DISABLED|ERROR), `StatusMessage`, `ExecutionRoleArn`, `DateCreated`, `DateModified`, `PolicyDetails`, `Tags`, `PolicyArn`, `DefaultPolicy` (boolean). Every field must be what Create/Update actually stored — not echoed from this request. Unknown id -> `ResourceNotFoundException` (HTTP 404).
3. **GetLifecyclePolicies** — `GET /policies`. Returns `Policies` (list of `LifecyclePolicySummary`: `PolicyId`, `Description`, `State`, `Tags`, `PolicyType`, `DefaultPolicy`) built by iterating the store, with the query filters applied: `policyIds`, `state`, `resourceTypes`, `targetTags`, `tagsToAdd`, `defaultPolicyType`. A created policy must appear; a deleted one must not. `policyIds` naming an unknown id -> `ResourceNotFoundException` (404).
4. **UpdateLifecyclePolicy** — `PATCH /policies/{PolicyId}`. Partial update: only the members present in the request (`ExecutionRoleArn`, `State`, `Description`, `PolicyDetails`, `CreateInterval`, `RetainInterval`, `CopyTags`, `ExtendDeletion`, `CrossRegionCopyTargets`, `Exclusions`) overwrite the stored policy; absent members keep their stored values. Bump `DateModified`. Empty output body. Unknown id -> `ResourceNotFoundException` (404). A following GetLifecyclePolicy must show the new values.
5. **DeleteLifecyclePolicy** — `DELETE /policies/{PolicyId}`. Removes the record (and its tags) from the store; empty output body. Unknown id (or a second delete of the same id) -> `ResourceNotFoundException` (404). Afterwards GetLifecyclePolicy 404s and GetLifecyclePolicies omits it.

Optional stretch if cheap, since tags are already stored by Create: **TagResource** / **UntagResource** / **ListTagsForResource** on `/tags/{ResourceArn}`, resolving `ResourceArn` back to the policy's stored `PolicyArn` (unknown ARN -> `ResourceNotFoundException` 404), with `ListTagsForResource` returning exactly the tags that Create/TagResource stored.

### NEVER STUBS
Each of these five must compute its response from persisted state. No hardcoded `PolicyId`, no echoing the request back as the stored policy, no minting an id without storing a record. A create followed by a get must return what was created; a delete must be observable by a later get and list.

## State Model
Service state (behind `crate::state::service_state(&*self.engine)`'s lock):
- `policies: BTreeMap<String /* PolicyId */, LifecyclePolicyRecord>` — insertion-ordered-by-id map, the single source of truth.
- `LifecyclePolicyRecord { policy_id: String /* "policy-<hex>" */, policy_arn: String, description: String, state: GettablePolicyStateValues, status_message: Option<String>, execution_role_arn: String, date_created: DateTime, date_modified: DateTime, policy_details: Option<PolicyDetails>, policy_type: Option<PolicyTypeValues>, default_policy: bool, default_policy_type: Option<DefaultPolicyTypeValues>, create_interval: Option<i32>, retain_interval: Option<i32>, copy_tags: Option<bool>, extend_deletion: Option<bool>, cross_region_copy_targets: Vec<CrossRegionCopyTarget>, exclusions: Option<Exclusions>, tags: BTreeMap<String, String> }`.
- `next_policy_suffix: u64` (or a counter/random hex source) used only to mint ids that satisfy `^policy-[a-f0-9]+$` and are unique within `policies`.
- An ARN index is not required: `arn -> policy_id` can be resolved by scanning `policies` for a matching `policy_arn` when the tag operations are implemented.

All other operations stay 501 and will be filed as individual gap issues later.

## Evidence

services/service-com-amazonaws-dlm-dlm-20180112/OPERATIONS.md lists all 8 operations with routes, required members and errors (CreateLifecyclePolicy `POST /policies` -> `PolicyId`; GetLifecyclePolicy `GET /policies/{PolicyId}` -> `Policy`; GetLifecyclePolicies `GET /policies` -> `Policies`; UpdateLifecyclePolicy `PATCH /policies/{PolicyId}`; DeleteLifecyclePolicy `DELETE /policies/{PolicyId}`; all three lookups declare `ResourceNotFoundException (HTTP 404)`). smithy_query on `LifecyclePolicy` and `LifecyclePolicySummary` gives the exact read-back fields quoted above (PolicyArn pattern, DateCreated/DateModified as date-time timestamps, DefaultPolicy boolean on the full shape vs. type string on the summary). services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs lines 4 and 204 show the scaffold header and the `not_implemented` return, and all 8 files under src/ops/ are the same generated stub, so there are no tests asserting any DLM behaviour yet.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI dlm client at it.
2. `aws dlm create-lifecycle-policy --description 'nightly ebs' --state ENABLED --execution-role-arn arn:aws:iam::123456789012:role/AWSDataLifecycleManagerDefaultRole --policy-details file://details.json` — expect 200 with a fresh `PolicyId` matching ^policy-[a-f0-9]+$ (today: HTTP 501 NotImplemented).
3. `aws dlm get-lifecycle-policy --policy-id <id>` — expect the created Description, State, ExecutionRoleArn, PolicyDetails, Tags, a well-formed PolicyArn and DateCreated/DateModified.
4. `aws dlm get-lifecycle-policies` — expect the new policy in `Policies` as a summary; repeat with `--state ENABLED` and with `--policy-ids <id>` and expect it still present; `--policy-ids policy-deadbeef` for an unknown id -> ResourceNotFoundException 404.
5. `aws dlm update-lifecycle-policy --policy-id <id> --state DISABLED --description 'nightly ebs v2'`, then get-lifecycle-policy again — expect State=DISABLED, the new Description, the ORIGINAL ExecutionRoleArn/PolicyDetails preserved, and DateModified advanced.
6. `aws dlm delete-lifecycle-policy --policy-id <id>`, then `get-lifecycle-policy --policy-id <id>` -> ResourceNotFoundException 404, and `get-lifecycle-policies` no longer lists it.
7. `aws dlm get-lifecycle-policy --policy-id policy-deadbeef` for an id never created -> ResourceNotFoundException 404, not 501 and not 200.
