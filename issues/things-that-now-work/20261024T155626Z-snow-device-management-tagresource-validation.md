Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Tagging operations ignore the Region in resourceArn (and the signed Region), so a cross-Region ARN tags a resource the endpoint cannot see

_Filed: 2026-10-24T15:56:26Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`POST /tags/{resourceArn}` resolves the ARN with `crate::state::parse_tagged_resource_arn`, which validates the partition, service namespace, account and resource id but accepts ANY non-empty `region` field and then looks the record up by id alone (`store.devices.get(&resource.id)` / `store.tasks.get(&resource.id)`). The request's own SigV4 Region is never consulted either (`handle` takes `_ctx`).

Consequence: a device or task that lives in Region A can be tagged through an ARN naming Region B, and the tag write is then observable from Region A through `DescribeDevice` / `DescribeTask` / `ListDevices` / `ListTasks`. Expected: a regional endpoint only serves resources of its own Region — an ARN whose Region field is not the endpoint's Region names no resource this endpoint holds, so the call is refused (404 not-found for the tagging family, as this emulator already words it, or a 400 invalid-ARN refusal) and no tag is written.

The same parser backs `UntagResource` and `ListTagsForResource`, so the read and remove paths accept a cross-Region ARN identically (an untag applied through a foreign-Region ARN silently removes real tags, and a tag read through one discloses the tags of a resource the endpoint cannot otherwise describe). The emulator is internally inconsistent here: every device-/task-addressed operation is strictly regional (`device_in` / `task_in` filter on `device.region == region`, and a task of another Region is answered 404), while the tagging family alone is region-blind.

## Evidence

Spec: `TagResource` is modeled `POST /tags/{resourceArn}` with `resourceArn` `@required` `@httpLabel` and declares `ValidationException` (400) and `ResourceNotFoundException` (404) — the two refusals that exist for an ARN that does not name a resource of this endpoint. AWS regional endpoints do not serve resources of other Regions (well-established: a cross-Region ARN is not resolvable at a regional endpoint), and this service's own state model agrees — `src/state.rs` stores `region` on `Device` and `Task` and `src/ops/describe_task.rs` answers 404 for a task of another Region.

Handler: `src/ops/tag_resource.rs` lines ~196-252 (ARN parsed, then `store.devices.get_mut(&resource.id)` / `store.tasks.get_mut(&resource.id)` with no Region predicate; `handle` ignores `_ctx`, so the signed Region is unused). `src/state.rs::parse_tagged_resource_arn` lines ~1010-1055: `region.is_empty()` is the ONLY check on the Region field, and the doc comment states the id is resolved "whatever region the ARN names". Same code path for `src/ops/untag_resource.rs` ~205-245 and `src/ops/list_tags_for_resource.rs` ~178-215.

Tests: not covered. `tests/all/resource_tagging.rs` lines ~302-304 state explicitly that "The region and the partition are not part of resolution … so no row varies them" — no test exercises an ARN whose Region differs from the signed Region, so nothing pins the current behavior.

## Steps to Reproduce

1. Start the emulator and activate a device inventory in ap-southeast-2 (the only seam for devices), then sign a CreateTask request for ap-southeast-2 to mint a task, noting its taskArn (region field ap-southeast-2).
2. Sign a TagResource request for us-east-1 against `/tags/arn:aws:snow-device-management:us-east-1:<account>:task/<taskId>` with `{"tags":{"Name":"cross-region"}}` — i.e. an ARN naming a Region that holds no such task.
3. Observe HTTP 200 and no refusal; expected the endpoint to refuse an ARN that names no resource of its Region (404 not-found / 400 invalid ARN).
4. Sign DescribeTask for ap-southeast-2 on the same taskId and observe the tag written through the foreign-Region ARN is now part of that task's tags.
5. Repeat with ListTagsForResource and UntagResource using the same cross-Region ARN: both resolve and act on the ap-southeast-2 record.
