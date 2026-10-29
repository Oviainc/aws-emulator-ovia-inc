Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# TagResource (and UntagResource) change a keyspace or table whose deletion is already in progress, which this crate's own rule forbids

_Filed: 2026-10-29T02:06:52Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for a resource whose deletion has been accepted but has not completed yet: it "takes no further change until its deletion completes", and the refusal has ONE wording "so every operation refuses it the same way". Four handlers call the helper that states it (`CreateTable`, `CreateType`, `RestoreTable`, `DeleteKeyspace`).

The two tagging writes never check it. `TagResource` resolves the ARN with `ComAmazonawsKeyspacesKeyspacesserviceState::resource_tags_mut` and writes the tag set with no status gate at any level: not the owning keyspace's `is_deleting()`, not the table's `DELETING` status. `UntagResource` has the identical hole on the same helper.

So during the span between `DeleteKeyspace`/`DeleteTable` accepting a deletion and `settle` materialising it, a `TagResource` call against the doomed keyspace (or table) answers 200 and mutates stored state that is then thrown away wholesale when `settle` drops the record — a write the crate's own rule says must have been refused, and a success the caller cannot distinguish from a durable one. `ListTagsForResource` reports the new tag for the rest of the span, then answers 404 for the same ARN.

The handler's own comment shows the completed case was considered and the in-flight case was not: it explains `settle` as covering "a table whose deletion has COMPLETED", which leaves exactly the DELETING window unguarded.

Scope note for a fixer: the gate is `DELETING` only (`Keyspace::is_deleting()` / `TABLE_STATUS_DELETING`). Tagging a resource that is still `CREATING` must keep working — `tests/all/resource_tagging.rs` tags keyspaces and tables immediately after creating them and asserts success, and nothing in that file waits for a transition. Both `src/ops/tag_resource.rs` and `src/ops/untag_resource.rs` need the same check.

## Evidence

The rule, stated by the helper every other write uses — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:364-375`:
```
/// The refusal message a request that would add a resource to — or otherwise change — a
/// keyspace whose deletion is already in progress carries: ONE wording, so every
/// operation refuses it the same way.
///
/// The caller's recourse is to wait for that deletion to finish, so the status it is
/// waiting on is in the message.
pub fn keyspace_deleting_rejection(keyspace_name: &str) -> String {
    format!(
        "Keyspace '{keyspace_name}' is in status {KEYSPACE_STATUS_DELETING} and takes no \
         further change until its deletion completes."
    )
}
```
and the predicate it pairs with — `src/state.rs:903-908`:
```
    /// Whether this keyspace's own deletion is already in progress — the status
    /// [`KEYSPACE_STATUS_DELETING`] — so it is on its way out and takes no further
    /// change.
    pub fn is_deleting(&self) -> bool {
        self.status == KEYSPACE_STATUS_DELETING
    }
```

A sibling write that DOES apply it — `src/ops/create_table.rs:361-370`:
```
        // A keyspace whose own deletion is already in progress takes no new table: it and
        // everything in it are on their way out, so storing one would create a table that
        // vanishes with the keyspace. That is the modelled conflict — "you try to perform
        // an action and the same or a different action is already in progress".
        if keyspace.is_deleting() {
            return Err(CreateTableErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::keyspace_deleting_rejection(&input.keyspace_name)),
            })
            .into());
        }
```
(the same block is in `src/ops/create_type.rs:261-266` and `src/ops/restore_table.rs:461-468`.)

The break — `src/ops/tag_resource.rs:186-213`, which goes straight from the service-managed check to the write:
```
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        // The status transitions the clock already owed at this instant, so a table whose
        // deletion has completed is no longer a resource this call can tag.
        store.settle(::std::time::SystemTime::now());
...
        let Some(tags) = store.resource_tags_mut(&region, &input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                crate::state::arn_not_found(&input.resource_arn),
            )
            .into());
        };
```
and `crate::state::merge_tags(tags, &input.tags)` right after it (`src/ops/tag_resource.rs:221`). The same hole in `src/ops/untag_resource.rs:190-204`:
```
        let Some(tags) = store.resource_tags_mut(&region, &input.resource_arn) else {
...
        for tag in &input.tags {
            tags.remove(&tag.key);
        }
```
The lookup they share carries no status gate either — `src/state.rs:1909-1928`:
```
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&mut TagSet> {
        let keyspaces = self.regions.get_mut(region)?;
        match parse_resource_arn(resource_arn)? {
            ResourceRef::Keyspace { keyspace_name } => {
                let keyspace = keyspaces.get_mut(&keyspace_name)?;
                (keyspace.resource_arn == resource_arn).then_some(&mut keyspace.tags)
```

That the window exists and is observable — `src/ops/delete_keyspace.rs:249-255`:
```
        let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
        keyspace.status = crate::state::KEYSPACE_STATUS_DELETING.to_string();
        keyspace.status_due_at = Some(settles_at);
        for table in keyspace.tables.values_mut() {
            table.status = crate::state::TABLE_STATUS_DELETING.to_string();
            table.status_due_at = Some(settles_at);
        }
```

Test coverage: nothing pins the DELETING case, so nothing constrains the fix — `tests/all/resource_tagging.rs` never mentions `DELETING` and never waits out a transition (no `settle_wait` in the file). What it DOES pin is tagging a freshly created (`CREATING`) resource, `tests/all/resource_tagging.rs:150-152`:
```
/// RULE 1 and RULE 2: tagging adds a key the resource does not hold, replaces the value
/// of one it does, and the reader reports exactly the resulting set — for a keyspace ARN
/// and for a table ARN alike.
```
so the gate must be `DELETING` alone.

## Steps to Reproduce

1. Start the Keyspaces emulator and drive it over awsJson1_0 as tests/all/resource_tagging.rs does
2. CreateKeyspace {"keyspaceName":"tag_del_ks"}, then CreateTable {"keyspaceName":"tag_del_ks","tableName":"t","schemaDefinition":<minimal>}
3. Wait out the transient CREATING span so both are ACTIVE
4. DeleteKeyspace {"keyspaceName":"tag_del_ks"} — accepted; the keyspace and its table are now DELETING for TRANSIENT_STATUS_SPAN
5. Immediately (inside that span) call TagResource {"resourceArn":"arn:aws:cassandra:<region>:<account>:/keyspace/tag_del_ks/","tags":[{"key":"env","value":"prod"}]} — expected: refused, since the crate's own rule is that a keyspace in DELETING "takes no further change until its deletion completes" (the refusal CreateTable/CreateType/RestoreTable give for the same keyspace); actual: HTTP 200, and ListTagsForResource for that ARN reports {"env":"prod"}
6. Wait out the span and call ListTagsForResource for the same ARN — 404 ResourceNotFoundException: the tag the 200 reported was written onto a record that was already on its way out
7. Repeat with a table ARN after DeleteTable {"keyspaceName":...,"tableName":"t"} (table DELETING, keyspace ACTIVE) — TagResource and UntagResource both answer 200 the same way
