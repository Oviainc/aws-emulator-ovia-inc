Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed

# DeleteGroup leaves the deleted group's tag-sync tasks in the store, so GetTagSyncTask keeps reporting a group that no longer exists

_Filed: 2026-10-25T04:30:41Z_
_Operation: DeleteGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dbe9ca7a8a80909c811e91f1b58fc3cbb14634ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteGroup` removes only the `GroupRecord` from `state.groups`. Tag-sync tasks live in a second map (`state.tag_sync_tasks`, keyed by task ARN) and hold a copy of the group's ARN and name, and nothing in the delete path touches that map. After deleting a group that had a task, `GetGroup` answers 404 for the group while `GetTagSyncTask` still answers 200 for the task and reports `GroupArn`/`GroupName` of the group that no longer exists — and if a group of the same name is created again, the surviving task reports itself as attached to that new, unrelated group (the stored `group_arn` string is name-derived). The store's own documentation of that field states the opposite invariant.

## Evidence

src/state.rs:913-915 (TagSyncTaskRecord::group_arn):
```
    /// The STORED ARN of the group the task syncs resources into, so a reader
    /// reports the same ARN `GetGroup` does for that group.
    pub group_arn: String,
```
After a delete `GetGroup` reports NO ARN for that group, so the invariant is broken.

src/ops/delete_group.rs:228-230 removes the group record only, and its comment enumerates what goes with it — memberships and tags, not tasks:
```
        // Removing the record removes the group's memberships and tags with it:
        // both are held on the record itself.
        let removed = store.groups.remove(&key).ok_or_else(not_found)?;
```
src/ops/get_tag_sync_task.rs:211-214 resolves a task purely out of the task map, with no check that its group still exists:
```
        let task = store
            .tag_sync_tasks
            .get(&input.task_arn)
            .filter(|task| task.region == region)
```
No test pins the surviving-task behaviour: tests/all/tag_sync_tasks.rs never calls `/delete-group` (grep for `delete-group` matches only the lifecycle, protection, owner and region suites).

## Steps to Reproduce

1. CreateGroup Name=app (name short enough for a task ARN).
2. StartTagSyncTask Group=app, TagKey=k, TagValue=v, RoleArn=arn:aws:iam::000000000000:role/r — 200, note the returned TaskArn.
3. DeleteGroup Group=app — 200.
4. GetGroup Group=app — 404 NotFoundException (the group is gone).
5. GetTagSyncTask TaskArn=<the ARN from step 2> — actual: 200 reporting GroupArn/GroupName of the deleted group; the task outlives the group it syncs into.
6. CreateGroup Name=app again, then GetTagSyncTask with the same TaskArn — the orphaned task now presents itself as a task of the newly created, unrelated group.
