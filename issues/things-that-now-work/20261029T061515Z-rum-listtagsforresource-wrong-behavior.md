Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ccf0a3f01392ba40f6e09b3e7aad3b88797362c9

# ListTagsForResource answers from the snapshot it took before the authorization decision, never re-resolving the resource like every other reader in the crate

_Filed: 2026-10-29T06:15:15Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ccf0a3f01392ba40f6e09b3e7aad3b88797362c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other read path in this crate takes a SHORT lock to get the record's ARN and resource policy, releases it for the authorization decision (which reads the IAM and STS stores), and then takes the lock AGAIN to build the answer off the stored record — answering the modelled ResourceNotFoundException when the record is gone by then. The two ARN-addressed tagging siblings do the same on their write path and say why in a comment.

`ListTagsForResource` is the one reader that does not. It copies the ARN, the policy AND the tag map out under the first lock, runs the authorization decision with no lock held, and then returns the copied values without ever looking at the store again. So a `ListTagsForResource` whose authorization decision overlaps a concurrent `DeleteAppMonitor` answers HTTP 200 with the tags of a resource this emulator no longer holds, where the sibling `UntagResource`/`TagResource` (and the reader `GetAppMonitor`) answer 404 for the very same instant. The crate's own comment in `ListTagsForResource` even asserts the opposite of what the code does — "That covers the deleted monitor too: a delete takes the record and its tags together, so its ARN resolves to nothing afterwards" — which only holds for a delete that lands before the single lookup this handler performs.

The fix is the shape the siblings already have: re-resolve the ARN under a second lock after the decision and project `record.tag_map()` (and `record.arn`) from that record, so a read cannot report a resource the store has dropped.

## Evidence

The handler reads everything up front and answers the copies — `services/service-com-amazonaws-rum-rum/src/ops/list_tags_for_resource.rs:288-325`:
```
        let region = crate::state::request_region(ctx.parts());
        let (resource_arn, resource_policy, tags) = {
            let store = state.lock();
            let Some(record) = store.app_monitor_by_arn(&region, &input.resource_arn) else {
                return Err(ListTagsForResourceError::NotFound(
                    crate::state::tagged_resource_not_found(),
                ));
            };
            (
                record.arn.clone(),
                record.resource_policy_document(),
                record.tag_map(),
            )
        };
        // May this caller read THIS resource's tags? ...
        if let Err(denial) = crate::state::authorization::authorize( ... ) { ... }

        Ok(ListTagsForResourceOutput { resource_arn, tags })
```
Its own doc comment states the rule the code above does not keep (`list_tags_for_resource.rs:283-285`):
> "That covers the deleted monitor too: a delete takes the record and its tags together, so its ARN resolves to nothing afterwards."

The ARN-addressed siblings re-resolve after the decision and say why — `services/service-com-amazonaws-rum-rum/src/ops/tag_resource.rs:349-356`:
```
        let mut store = state.lock();
        // Resolved again under the WRITE lock: the decision above held no lock, so a
        // concurrent `DeleteAppMonitor` may have removed the resource in between.
        let Some(record) = store.app_monitor_by_arn_mut(&region, &resource_arn) else {
            return Err(TagResourceError::NotFound(
                crate::state::tagged_resource_not_found(),
            ));
        };
```
`services/service-com-amazonaws-rum-rum/src/ops/untag_resource.rs:414-421` carries the identical comment and lookup.

The other READ paths do it too. `services/service-com-amazonaws-rum-rum/src/ops/get_app_monitor.rs:184-195`:
```
        let store = state.lock();
        let Some(record) = store.app_monitor(&region, &input.name) else {
            return Err(GetAppMonitorErrorModeled::ResourceNotFoundException(
                crate::state::app_monitor_not_found(&input.name),
            )
            .into());
        };
        // Every output member is rebuilt from the stored record ...
        Ok(GetAppMonitorOutput { app_monitor: Some(record.to_app_monitor()) })
```
`services/service-com-amazonaws-rum-rum/src/ops/list_rum_metrics_destinations.rs:223-233`, `get_resource_policy.rs:195-201`, `get_app_monitor_data.rs:236-244` and `batch_get_rum_metric_definitions.rs:257-265` all re-take the lock after `authorize(...)` and answer `ResourceNotFoundException` when the monitor is gone.

Test coverage: `tests/all/list_tags_for_resource.rs:359-416` only exercises the SEQUENTIAL case (delete, then list → 404), which the single pre-decision lookup already satisfies, and `:254` asserts the read agrees with `GetAppMonitor`'s `AppMonitor$Tags` ("two readers cannot disagree about one resource"). No test states a reason for reading the tags before the decision, and re-resolving after it would change no existing assertion.

## Steps to Reproduce

1. CreateAppMonitor(Name="m1", Domain="example.com", Tags={"team":"web"}) — note the returned ARN
2. Issue ListTagsForResource(ResourceArn=<that ARN>) and DeleteAppMonitor(Name="m1") concurrently, so the delete commits while ListTagsForResource is between its snapshot block (list_tags_for_resource.rs:289-308) and its `Ok(...)` (line 325) — i.e. while it is running the authorization decision with no lock held
3. Observed: ListTagsForResource answers HTTP 200 with ResourceArn and Tags={"team":"web"} for a monitor the store no longer holds
4. Expected: the 404 ResourceNotFoundException (`NotFoundException` body) its ARN-addressed siblings answer for the same instant — TagResource/UntagResource re-resolve under the write lock exactly to catch this, and GetAppMonitor re-resolves and 404s
5. Cross-check: GetAppMonitor(Name="m1") immediately afterwards answers 404, so the two readers of one record disagree
