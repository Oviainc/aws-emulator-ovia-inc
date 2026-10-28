Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6c1cc1a79be554bed82ba757ddf92c753ad05f12

# TagResource and UntagResource write the policy's tags without advancing DateModified, so a read reports a changed policy with a stale "last modified" time

_Filed: 2026-10-28T14:44:17Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6c1cc1a79be554bed82ba757ddf92c753ad05f12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`LifecyclePolicyRecord::date_modified` is the field this crate reports as `LifecyclePolicy$DateModified`, documented "The local date and time when the lifecycle policy was last modified." `UpdateLifecyclePolicy` quotes that very sentence as its reason for stamping the field on every accepted write, and a test pins that rule.

`TagResource` and `UntagResource` mutate the SAME record — `policy.tags`, which `to_policy()` reports as `LifecyclePolicy$Tags` on the identical projection that reports `DateModified` — and never touch `date_modified`. So `GetLifecyclePolicy` answers with a policy whose `Tags` a client just changed while its `DateModified` still names the instant of the create (or of the last `UpdateLifecyclePolicy`). The reported "last modified" time is therefore not the time the policy was last modified, and a client that diffs policies by `DateModified` (or caches on it) never sees the tag write at all.

This is the same write path that `UpdateLifecyclePolicy` guards: both acquire the store lock, resolve one `LifecyclePolicyRecord` and mutate it in place. One applies the timestamp rule, the other two skip it. `date_modified` is a plain field on the shared record, so the stamp the update already performs is available to the tag handlers unchanged.

## Evidence

Model, `smithy_query(query="shape", shape="LifecyclePolicy")` — `DateModified`: "The local date and time when the lifecycle policy was last modified." (and `Tags`: "The tags." on the same shape, so both are members of the one projection a read returns).

The sibling write handler that applies the rule, quoting that sentence — `services/service-com-amazonaws-dlm-dlm-20180112/src/ops/update_lifecycle_policy.rs:412-413`:
```
        // "The local date and time when the lifecycle policy was last modified."
        policy.date_modified = now;
```

The two handlers that write the same record and skip it — `src/ops/tag_resource.rs:161-170`:
```
        let policy = store
            .policy_mut_by_arn(&region, &input.resource_arn)
            .map_err(TagResourceErrorModeled::ResourceNotFoundException)?;

        // Adding a tag whose key is already held replaces that tag's value.
        for (key, value) in input.tags {
            policy.tags.insert(key, value);
        }

        Ok(TagResourceOutput {})
```
and `src/ops/untag_resource.rs:146-156`:
```
        let policy = store
            .policy_mut_by_arn(&region, &input.resource_arn)
            .map_err(UntagResourceErrorModeled::ResourceNotFoundException)?;

        // Removing a key the policy does not hold is not an error: the request
        // asks for those keys to be absent, and afterwards they are.
        for key in &input.tag_keys {
            policy.tags.remove(key);
        }

        Ok(UntagResourceOutput {})
```
Neither mentions, nor writes, `date_modified`; a repo-wide grep for `date_modified` under `src/ops/` matches only `create_lifecycle_policy.rs` and `update_lifecycle_policy.rs`.

Both mutated members come back from the one projection — `src/state.rs:734-748`:
```
    pub fn to_policy(&self) -> LifecyclePolicy {
        LifecyclePolicy {
            ...
            date_created: Some(::aws_smithy_types::DateTime::from(self.date_created)),
            date_modified: Some(::aws_smithy_types::DateTime::from(self.date_modified)),
            policy_details: self.effective_policy_details(),
            tags: self.wire_tags(),
```

Test coverage: the rule is pinned for the update only, and nothing pins it across a tag write — `tests/all/lifecycle_policy_crud.rs:1105-1136`:
```
/// `DateModified` is "the local date and time when the lifecycle policy was last
/// modified", so an update moves it forward while `DateCreated` stays put.
#[tokio::test]
async fn an_update_advances_date_modified_and_leaves_date_created() {
...
    assert!(
        modified_after > modified_before,
        "DateModified must advance past {modified_before:?}, got {modified_after:?}"
    );
```
A grep for `DateModified|date_modified` under `tests/` matches only `policy_read_projection.rs`, `interval_validation.rs` and `lifecycle_policy_crud.rs` — never `tag_arn_resolution.rs`, the file that exercises `TagResource` / `UntagResource` — so no existing test constrains the timestamp across a tag write.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with {"ExecutionRoleArn": "arn:aws:iam::<emulator account>:role/AWSDataLifecycleManagerDefaultRole", "Description": "tag timestamp subject", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "backup", "Value": "daily"}], "Schedules": [{"Name": "daily", "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 7}}]}} — note the PolicyId
2. GET /policies/{PolicyId} — record Policy.PolicyArn and Policy.DateModified
3. Wait ~2 seconds so any new instant is distinguishable at the date-time precision the response renders
4. POST /tags/{PolicyArn URL-encoded} with {"Tags": {"env": "prod"}} — observe HTTP 204
5. GET /policies/{PolicyId} — Policy.Tags now contains {"env": "prod"}, but Policy.DateModified is byte-identical to the value recorded in step 2. Expected: DateModified advanced, since the policy a read reports was modified; compare with PATCH /policies/{PolicyId} {"State": "DISABLED"}, which does advance it
6. DELETE /tags/{PolicyArn URL-encoded}?tagKeys=env — observe HTTP 200, then GET /policies/{PolicyId}: Policy.Tags lost the key and DateModified is still unchanged
