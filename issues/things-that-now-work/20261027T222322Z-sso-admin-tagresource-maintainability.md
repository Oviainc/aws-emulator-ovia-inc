Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# The taggable-resource resolution rule is written out three times across the tag surface instead of once

_Filed: 2026-10-27T22:23:22Z_
_Operation: TagResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tag surface has one rule for resolving the resource a request names: (1) when the optional `InstanceArn` is sent and this account holds no instance under it, the call is `ResourceNotFoundException(instance_not_found_message)`; (2) the resource is resolved through `taggable_resource_tags`/`taggable_resource_tags_mut` scoped by that `InstanceArn`, and an ARN that resolves to nothing is `ResourceNotFoundException(taggable_resource_not_found_message)`; and for the two WRITES additionally (3) a resource whose holding instance is still `CREATE_IN_PROGRESS` is `ConflictException(instance_creation_in_progress_message)`.

That rule is open-coded three times, in `tag_resource.rs`, `untag_resource.rs` and `list_tags_for_resource.rs` — step (1) and (2) in all three, step (3) byte-for-byte identically in the two writes, comment text included. The copies can drift: a change to how an optional `InstanceArn` scopes a tag read, or to which instance states withhold a tag write, has to be made in two or three places and silently applies to only the ones that were edited.

The crate already has the seam for this: `state.rs` owns `taggable_resource`, `taggable_resource_instance`, `taggable_resource_tags` and `taggable_resource_tags_mut`, and it already uses the "state helper returns a refusal the handler maps onto its own error enum" pattern elsewhere (`Instance::add_region` returns `Result<(), String>` and `cursor_after` returns `Result<_, &'static str>`, each mapped by the caller into its own modelled error). Lifting the resolution into one such helper leaves every response byte and every existing test unchanged.

## Evidence

Copy 1 — `src/ops/tag_resource.rs:187-236`:
```
// `InstanceArn` is optional on this operation, but when a caller sends one it
// names the instance the resource is expected to live under: an ARN this account
// holds no instance for can hold no resource either.
if input
    .instance_arn
    .as_deref()
    .is_some_and(|instance_arn| store.instance(instance_arn).is_none())
{
    return Err(TagResourceErrorModeled::ResourceNotFoundException(
        ResourceNotFoundException {
            message: Some(crate::state::instance_not_found_message(
...
let holder = store
    .taggable_resource_instance(&input.resource_arn)
    .map(str::to_string);
if let Some(holder) = holder.filter(|arn| store.instance_creation_in_progress(arn)) {
    return Err(TagResourceErrorModeled::ConflictException(ConflictException {
        message: Some(crate::state::instance_creation_in_progress_message(&holder)),
...
let stored = store
    .taggable_resource_tags_mut(input.instance_arn.as_deref(), &input.resource_arn)
    .ok_or_else(|| {
        TagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
            message: Some(crate::state::taggable_resource_not_found_message(
```

Copy 2 — `src/ops/untag_resource.rs:184-235`, the same three steps with the same comments:
```
// `InstanceArn` is optional on this operation, but when a caller sends one it
// names the instance the resource is expected to live under: an ARN this account
// holds no instance for can hold no resource either.
if input
    .instance_arn
    .as_deref()
    .is_some_and(|instance_arn| store.instance(instance_arn).is_none())
...
let holder = store
    .taggable_resource_instance(&input.resource_arn)
    .map(str::to_string);
if let Some(holder) = holder.filter(|arn| store.instance_creation_in_progress(arn)) {
...
let stored = store
    .taggable_resource_tags_mut(input.instance_arn.as_deref(), &input.resource_arn)
    .ok_or_else(|| {
        UntagResourceErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
            message: Some(crate::state::taggable_resource_not_found_message(
```

Copy 3 — `src/ops/list_tags_for_resource.rs:199-234`, steps (1) and (2) again:
```
// When a caller sends the optional `InstanceArn`, it names the instance the
// resource is expected to live under: an ARN this account holds no instance for
// can hold no resource either.
if input
    .instance_arn
    .as_deref()
    .is_some_and(|instance_arn| store.instance(instance_arn).is_none())
...
let ordered = store
    .taggable_resource_tags(input.instance_arn.as_deref(), &input.resource_arn)
    .map(|tags| crate::state::tags_in_key_order(tags))
    .ok_or_else(|| {
        ListTagsForResourceErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: Some(crate::state::taggable_resource_not_found_message(
```

The pattern for lifting it already exists in this crate — `src/state.rs:1424-1427`:
```
    pub fn add_region(&mut self, region: Region) -> Result<(), String> {
        ...
            return Err(region_ceiling_message(&self.instance_arn));
```
mapped by `src/ops/add_region.rs:289-297` into that operation's own `ServiceQuotaExceededException`; and `src/state.rs:4219-4229` (`cursor_after` returning `Result<Option<String>, &'static str>`) mapped by every paginated handler into its own `ValidationException`.

Test coverage that must stay green and unchanged: `tests/all/resource_tags.rs`, `tests/all/resource_untagging.rs`, `tests/all/instance_create_in_progress_writes.rs` (the step-(3) conflict), `tests/all/tags_from_creates.rs` and `tests/all/create_tag_fingerprint.rs`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/tag_resource.rs` at lines 187-236 and `.../src/ops/untag_resource.rs` at lines 184-235: the optional-`InstanceArn` refusal, the `taggable_resource_instance` + `instance_creation_in_progress` conflict and the `taggable_resource_tags_mut` not-found are the same three branches, with the same comment text, differing only in the per-operation error enum.
2. Open `.../src/ops/list_tags_for_resource.rs` at lines 199-234: the first and third branches appear a third time, against `taggable_resource_tags`.
3. Confirm the lifting pattern already in the crate: `.../src/state.rs:1424` (`add_region` -> `Result<(), String>`, mapped in `add_region.rs:289-297`) and `.../src/state.rs:4219` (`cursor_after` -> `Result<_, &'static str>`, mapped in every paginated handler).
4. Run `cargo test -p service-com-amazonaws-ssoadmin-swbexternalservice` and note that `resource_tags`, `resource_untagging`, `instance_create_in_progress_writes`, `tags_from_creates` and `create_tag_fingerprint` all pass; they must keep passing unchanged after the three copies are replaced by one helper.
