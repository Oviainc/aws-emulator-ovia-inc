Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# UpdateKeyspace accepts a replication widening while a user-defined type of the keyspace is still CREATING or DELETING, although the sibling keyspace-wide operation refuses on types too

_Filed: 2026-10-28T20:36:52Z_
_Operation: UpdateKeyspace | Class: validation_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateKeyspace` refuses the Region addition while the keyspace itself is not `ACTIVE`, and while any TABLE of the keyspace is not `ACTIVE`. It never looks at the keyspace's user-defined types, so a type whose `CreateType` (or `DeleteType`) has only just been accepted — `CREATING`/`DELETING` for the whole `TRANSIENT_STATUS_SPAN` — does not block the widening. The keyspace is then moved to `MULTI_REGION` and marked `UPDATING`, and `GetKeyspace` reports the replication group as complete once the span elapses, while a type of that keyspace was mid-change throughout.

The crate already has the helper for exactly this question — `Keyspace::resource_in_flight()`, which walks the keyspace's tables AND its types — and the other keyspace-wide write, `DeleteKeyspace`, calls it and answers the modelled `ConflictException` naming either kind. `UpdateKeyspace` instead hand-rolls a tables-only scan, so the one rule the crate states for "a resource of this keyspace still has a change of its own in flight" is enforced by one handler and skipped by the other.

The crate's own test for this rule states it in the general form the handler does not implement: "a keyspace is replicable only from `ACTIVE`, and only while every table in it is `ACTIVE` too, so a resource the clock still owes a transition is the modelled conflict" — a type in `CREATING` is a resource the clock still owes a transition, and the handler accepts it.

## Evidence

The tables-only scan in the handler — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/update_keyspace.rs:307-326`:
```
        // Every table of the keyspace is replicated to the added Region(s), so a table
        // that is itself mid-change makes this the modelled conflict, exactly as a
        // keyspace that is not `ACTIVE` does above.
        if let Some(busy) = keyspace
            .tables
            .values()
            .find(|table| table.status != crate::state::TABLE_STATUS_ACTIVE)
        {
            return Err(UpdateKeyspaceErrorModeled::ConflictException(ConflictException {
                message: Some(format!(
                    "Table '{}.{}' is in status {} and the keyspace cannot be replicated \
                     to another Region until every table is {}.",
```
Nothing below it consults `keyspace.types`; the next statement is the acceptance at `src/ops/update_keyspace.rs:354-362`:
```
        let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
        keyspace.replication_strategy = crate::state::MULTI_REGION_STRATEGY.to_string();
        keyspace.replication_regions = regions;
        keyspace.status = crate::state::KEYSPACE_STATUS_UPDATING.to_string();
```

The sibling keyspace-wide write, which uses the existing helper and covers types — `src/ops/delete_keyspace.rs:225-242`:
```
        // "The DeleteKeyspace operation deletes a keyspace and all of its tables" — so a
        // table or type of it that has a change of its own in flight is dropped by this
        // call, ...
        // A keyspace holding one is therefore not
        // deletable yet either, and that is the same modelled conflict as above — the one
        // `UpdateKeyspace` already answers for a table of its keyspace that is mid-change.
        if let Some((kind, name, status)) = keyspace.resource_in_flight() {
```
The helper it calls, which `UpdateKeyspace` does not — `src/state.rs:909-932`:
```
    /// The first resource of this keyspace that still has a change of its OWN in flight:
    /// what kind it is (`"Table"` or `"Type"`), its name, and the status it is in.
    ...
    pub fn resource_in_flight(&self) -> Option<(&'static str, &str, &str)> {
        self.tables
            .values()
            .find(|table| is_in_flight_status(&table.status))
            .map(|table| ("Table", table.table_name.as_str(), table.status.as_str()))
            .or_else(|| {
                self.types
                    .values()
                    .find(|udt| is_in_flight_status(&udt.status))
                    .map(|udt| ("Type", udt.type_name.as_str(), udt.status.as_str()))
            })
    }
```

The rule as this crate's own test states it — `tests/all/update_keyspace.rs:793-797`:
```
/// RULE 2, from the refusal side: a keyspace is replicable only from `ACTIVE`, and only
/// while every table in it is `ACTIVE` too, so a resource the clock still owes a
/// transition is the modelled conflict.
#[tokio::test]
async fn update_keyspace_conflicts_while_a_resource_is_still_settling() {
```
That test exercises only the keyspace (CREATING), a table (CREATING) and the keyspace again (UPDATING) — `tests/all/update_keyspace.rs:809-845` — never a type, so no test pins the current acceptance and nothing constrains the fix.

The in-flight window is reachable: `CreateType` stores the type in `CREATING` with a due instant one span away — `src/ops/create_type.rs:315-330`:
```
        let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
        keyspace.types.insert(
            type_name.clone(),
            crate::state::UserDefinedType {
...
                status: crate::state::TYPE_STATUS_CREATING.to_string(),
                status_due_at: Some(settles_at),
```

## Steps to Reproduce

1. Start the Keyspaces emulator signed for us-west-2
2. CreateKeyspace {"keyspaceName":"ks"}; wait out the transient span so it is ACTIVE
3. CreateTable {"keyspaceName":"ks","tableName":"t","schemaDefinition":{...},"clientSideTimestamps":{"status":"ENABLED"}}; wait out the transient span so the table is ACTIVE
4. CreateType {"keyspaceName":"ks","typeName":"addr","fieldDefinitions":[{"name":"street","type":"text"}]} — do NOT wait; GetType {"keyspaceName":"ks","typeName":"addr"} reports status CREATING
5. Immediately: UpdateKeyspace {"keyspaceName":"ks","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-west-2","us-east-1"]}}
6. Expected: 409 ConflictException naming the type still in flight, the way DeleteKeyspace answers for the same keyspace in the same state (DeleteKeyspace {"keyspaceName":"ks"} at this moment DOES answer 409 naming Type 'ks.addr'). Actual: 200, the keyspace becomes MULTI_REGION and UPDATING, and after the span GetKeyspace reports the replication group complete
