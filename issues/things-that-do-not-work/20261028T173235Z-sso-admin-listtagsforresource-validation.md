Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62eaf53b582c9d7bec5520c65fe502b1fdb205cf

# ListTagsForResource serves a resource whose instance is still CREATE_IN_PROGRESS, a window the crate's own refusal helper says only DescribeInstance and DeleteInstance may be called in

_Filed: 2026-10-28T17:32:35Z_
_Operation: ListTagsForResource | Class: validation_

> _Note: this issue was identified against the code base at commit `62eaf53b582c9d7bec5520c65fe502b1fdb205cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While a `CreateInstance` is still in flight, the model says the instance serves exactly two operations. The crate implements that rule for every WRITE that touches the instance or a resource under it — `AddRegion`, `RemoveRegion`, `UpdateInstance`, `CreateApplication`, `CreateTrustedTokenIssuer`, `TagResource` and `UntagResource` all answer the modelled `ConflictException` built by `crate::state::instance_creation_in_progress_message` — but `ListTagsForResource` answers 200 with the tag set in that same window.

The asymmetry lives in one helper pair fifteen lines apart in `state.rs`: `tag_target_mut` (the resolution the two tag writes share) asks `tag_write_holder_ready`, which refuses while the holding instance's creation has not finished; `tag_target` (the resolution `ListTagsForResource` shares) does not ask it at all. The narrowing is stated nowhere in the model: the helper that BUILDS the refusal documents it as the one "an operation OTHER than the instance's own describe and delete earns" (state.rs:263-265) and its message restates the model sentence verbatim, while `instance_creation_in_progress` (state.rs:2653-2658) silently re-scopes the same rule to "every operation that MUTATES an instance".

Concretely: `CreateInstance` with `Tags` → within the activation span, `TagResource {ResourceArn: <new instance>}` is refused 409 `ConflictException` "Instance ... is in status CREATE_IN_PROGRESS. While the instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance operations.", while `ListTagsForResource {ResourceArn: <new instance>}` answers 200 with the tags — a call the sentence that refusal quotes does not allow. The same gate is missing from the other instance-scoped reads this window can reach (`DescribeRegion`, `ListRegions`, `ListApplications`, `ListTrustedTokenIssuers`), which resolve the CREATE_IN_PROGRESS instance and report from it; `ListTagsForResource` is the one where the crate already holds the check in the sibling half of the very helper it calls. `ListTagsForResource` models no `ConflictException` (its declared errors are `AccessDeniedException` 403, `ResourceNotFoundException` 404, `ValidationException` 400, `ThrottlingException` 429, `InternalServerException` 500), so the channel is for the implementer to pick, exactly as `AddRegion` picks `AccessDeniedException` for the missing instance because its model declares no `ResourceNotFoundException`.

## Evidence

MODEL (`smithy_query(query="doc",shape="DescribeInstance")`), the sentence the crate itself quotes in eight places:
> "CREATE_IN_PROGRESS - The instance is in the process of being created. ... While the instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance operations."

THE CRATE'S OWN STATEMENT OF THE RULE — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:263-265:
```
/// The refusal an operation OTHER than the instance's own describe and delete earns while
/// that instance's creation has not finished, as the message of the modelled
/// `ConflictException`.
```
and the message it builds (state.rs:277-283): `"Instance {instance_arn} is in status CREATE_IN_PROGRESS. While the instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance operations."`

THE WRITE HALF ASKS — src/state.rs:3124-3137:
```
    pub fn tag_target_mut(
        &mut self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Result<&mut Vec<Tag>, TagWriteRefusal> {
        self.tag_request_instance_scope(instance_arn)
            .map_err(TagWriteRefusal::NotFound)?;
        self.tag_write_holder_ready(resource_arn)
            .map_err(TagWriteRefusal::Conflict)?;
```
with src/state.rs:3149-3158:
```
    /// Whether the instance holding the named resource is far enough along to take a tag
    /// write, as the `Err` message of the conflict it earns while it is not.
    fn tag_write_holder_ready(&self, resource_arn: &str) -> Result<(), String> {
        match self
            .taggable_resource_instance(resource_arn)
            .filter(|holder| self.instance_creation_in_progress(holder))
```

THE READ HALF DOES NOT — src/state.rs:3108-3116:
```
    pub fn tag_target(
        &self,
        instance_arn: Option<&str>,
        resource_arn: &str,
    ) -> Result<&Vec<Tag>, String> {
        self.tag_request_instance_scope(instance_arn)?;
        self.taggable_resource_tags(instance_arn, resource_arn)
            .ok_or_else(|| taggable_resource_not_found_message(resource_arn))
    }
```
and the handler that calls it maps only the not-found — src/ops/list_tags_for_resource.rs:204-214:
```
        let ordered = store
            .tag_target(input.instance_arn.as_deref(), &input.resource_arn)
            .map(|tags| crate::state::tags_in_key_order(tags))
            .map_err(|message| {
                ListTagsForResourceErrorModeled::ResourceNotFoundException(
```

THE SILENT RE-SCOPING of the quoted sentence — src/state.rs:2653-2658:
```
    /// Whether the instance `instance_arn` names is still being CREATED
    /// ([`Instance::creation_in_progress`]) — what every operation that MUTATES an
    /// instance, or a resource held under it, asks before it writes: only the instance's
    /// own describe and delete are available in that window, so everything else is
    /// refused on its modelled conflict channel
```

TEST COVERAGE: the behaviour is pinned on the write side only — tests/all/instance_create_in_progress_writes.rs:10-15 ("every operation that MUTATES that instance — or any resource held under it — is refused with the modelled `ConflictException`") and tests/all/tag_resource_resolution.rs:486-489 + 531-533:
```
/// Step 3, which only the two WRITES take: while the instance holding the resource is
/// still being created, both writes are refused with the modelled `ConflictException` and
/// store nothing, while the REPORT of the same resource's tags is served
...
        // The report is served throughout, and it shows both writes stored nothing.
```
That is the test whose stated reason the quoted model sentence contradicts: the sentence permits DescribeInstance and DeleteInstance, not "every read". No test sends `ListTagsForResource`, `DescribeRegion` or `ListRegions` against a CREATE_IN_PROGRESS instance expecting a refusal.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.CreateInstance` and body `{"Name":"Win","Tags":[{"Key":"Owner","Value":"identity"}]}` against an account that holds no instance; keep the returned `InstanceArn`. The instance is `CREATE_IN_PROGRESS` for `crate::state::INSTANCE_ACTIVATION_SPAN` (500 ms) — or hold the window open as tests/all/tag_resource_resolution.rs:477-483 does by moving the stored `active_due`.
2. Inside that window, POST / with `X-Amz-Target: SWBExternalService.TagResource` and `{"ResourceArn":"<instance arn>","Tags":[{"Key":"Tier","Value":"test"}]}` — 409 `ConflictException`, message "Instance <arn> is in status CREATE_IN_PROGRESS. While the instance is in the CREATE_IN_PROGRESS state, you can call only DescribeInstance and DeleteInstance operations."
3. Inside the SAME window, POST / with `X-Amz-Target: SWBExternalService.ListTagsForResource` and `{"ResourceArn":"<instance arn>"}` — 200 with `{"Tags":[{"Key":"Owner","Value":"identity"}]}`, although `ListTagsForResource` is neither `DescribeInstance` nor `DeleteInstance`.
4. Compare with the same window's `DescribeRegion {"InstanceArn":"<arn>","RegionName":"<primary>"}` and `ListRegions {"InstanceArn":"<arn>"}` — both answer 200 as well, while `AddRegion`/`RemoveRegion` on the same instance answer the conflict (src/ops/add_region.rs:243-247, src/ops/remove_region.rs:240-249).
