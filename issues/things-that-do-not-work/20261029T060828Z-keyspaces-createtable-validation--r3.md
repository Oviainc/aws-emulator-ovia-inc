Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6cada3bea20bb0dfad4860907220259726a5f31e

# CreateTable adds a table to a keyspace that is still CREATING or UPDATING, although the crate treats a non-ACTIVE resource as unusable everywhere else

_Filed: 2026-10-29T06:08:28Z_
_Operation: CreateTable | Class: validation_

> _Note: this issue was identified against the code base at commit `6cada3bea20bb0dfad4860907220259726a5f31e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every write path in this crate gates on the resource it changes being `ACTIVE`, and the crate states the rule in its own words: a status other than `ACTIVE` is the model's "its status might not be ACTIVE" case. `CreateTable` is the exception. Its only keyspace-status gate is `keyspace.is_deleting()`, so a keyspace whose OWN change is still in flight — `CREATING` right after `CreateKeyspace`, or `UPDATING` while `UpdateKeyspace` is replicating all of its tables to an added Region — silently accepts a new table.

The inconsistency is sharpest INSIDE this one handler: a few lines below the `is_deleting()` check, `CreateTable` refuses a column whose type names a user-defined type that is not `ACTIVE` with 404 `ResourceNotFoundException`, quoting exactly the sentence it declines to apply to the keyspace it is writing into. So one and the same model rule is enforced for the types a column mentions and ignored for the keyspace that holds the table.

The transient span is deliberately non-zero (`TRANSIENT_STATUS_SPAN = 500ms`, "deliberately NON-ZERO, so a caller that reads straight after the call still observes the transient status"), so this is reachable by any ordinary `CreateKeyspace` → `CreateTable` sequence, not only by a race. The consequence is a table inserted into a keyspace that `GetKeyspace` still reports as `CREATING`/`UPDATING`, which then feeds `GetKeyspace`'s `tablesReplicationProgress` and `DeleteKeyspace`'s in-flight refusal, and a table created mid-widening that `UpdateKeyspace` had just insisted could not exist (it refuses the widening outright when any table is not `ACTIVE`).

The same missing gate is in `CreateType` (`src/ops/create_type.rs:261-266`) and on the target keyspace of `RestoreTable` (`src/ops/restore_table.rs:461-468`) — the three paths that add a resource to a keyspace — so a fix should cover all three rather than only this handler.

## Evidence

The gap — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_table.rs:361-370` checks ONLY the deleting case:
```
        // A keyspace whose own deletion is already in progress takes no new table: it and
        // everything in it are on their way out, so storing one would create a table that
        // vanishes with the keyspace. That is the modelled conflict — "you try to perform an
        // action and the same or a different action is already in progress".
        if keyspace.is_deleting() {
            return Err(CreateTableErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::keyspace_deleting_rejection(&input.keyspace_name)),
            })
            .into());
        }
```

The same handler, 10 lines later, applying the ACTIVE rule to TYPES — `src/ops/create_table.rs:372-390`:
```
        // A column's type may name "other user-defined types that are contained in the
        // same keyspace" (`FieldDefinition$type`, the same rule for a column), so a name
        // in it that is neither a native type nor a type constructor must be a type of
        // THIS keyspace, in status ACTIVE; any other is a type "that doesn't exist. The
        // resource might not be specified correctly, or its status might not be ACTIVE"
        // (`ResourceNotFoundException`), and nothing is stored.
        let usable_type_names = keyspace.usable_type_names();
```
backed by `src/state.rs:1296-1301`:
```
    pub fn usable_type_names(&self) -> BTreeSet<String> {
        self.types
            .values()
            .filter(|udt| udt.status == TYPE_STATUS_ACTIVE)
```
and the crate's own statement of the rule, `src/state.rs:135-138`:
```
/// `TypeStatus` = `ACTIVE`: the status a created type settles into, and the only
/// one in which the type can be used (`ResourceNotFoundException`: "The resource
/// might not be specified correctly, or its status might not be ACTIVE").
pub const TYPE_STATUS_ACTIVE: &str = "ACTIVE";
```

The sibling write paths that DO gate on the keyspace/table being ACTIVE:
`src/ops/update_keyspace.rs:257-272`:
```
        // A keyspace is updatable only from `ACTIVE`: "The operation tried to access a
        // keyspace, table, or type that doesn't exist. The resource might not be specified
        // correctly, or its status might not be ACTIVE." While another change is still
        // settling, this is the modelled conflict — "you try to perform an action and the
        // same or a different action is already in progress".
        if keyspace.status != crate::state::KEYSPACE_STATUS_ACTIVE {
            return Err(UpdateKeyspaceErrorModeled::ConflictException(ConflictException {
```
`src/ops/delete_keyspace.rs:201-223`:
```
        // A keyspace is deletable only from `ACTIVE`. While a change of its own is still
        // settling — the creation `CreateKeyspace` started, the replication change
        // `UpdateKeyspace` started, or a deletion a previous call to this operation
        // started — this is the modelled conflict...
        if keyspace.status != crate::state::KEYSPACE_STATUS_ACTIVE {
```
`src/ops/update_table.rs:375-391` (the same rule for a table):
```
        // A table is updatable only from `ACTIVE`: "The operation tried to access a
        // keyspace, table, or type that doesn't exist. The resource might not be
        // specified correctly, or its status might not be ACTIVE."
        if table.status != crate::state::TABLE_STATUS_ACTIVE {
```
and `src/ops/delete_type.rs:221-231` for a type.

UpdateKeyspace even refuses its own widening because an in-flight table may not exist during it — `src/ops/update_keyspace.rs:307-326`:
```
        // Every table of the keyspace is replicated to the added Region(s), so a table
        // that is itself mid-change makes this the modelled conflict, exactly as a
        // keyspace that is not `ACTIVE` does above.
        if let Some(busy) = keyspace
            .tables
            .values()
            .find(|table| table.status != crate::state::TABLE_STATUS_ACTIVE)
```
yet `CreateTable` is free to insert a brand-new `CREATING` table into that same keyspace while the widening is still settling.

The helper the create path skips already exists — `src/state.rs:385-391`:
```
pub fn is_in_flight_status(status: &str) -> bool {
    status == KEYSPACE_STATUS_CREATING
        || status == KEYSPACE_STATUS_UPDATING
        || status == TABLE_STATUS_RESTORING
}
```
(used by `Keyspace::resource_in_flight`, `src/state.rs:922-933`, and by `settle`).

Reachability — `src/state.rs:162-165`:
```
/// deliberately NON-ZERO, so a caller that reads straight after the call still
/// observes the transient status the model documents, and short enough that a
/// test can wait it out.
pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);
```

Test coverage: nothing pins the accepted case, so nothing constrains a fix. `tests/all/keyspace_table_lifecycle.rs` and `tests/all/delete_keyspace_in_flight_resources.rs` exercise create-then-wait and the keyspace-wide in-flight refusals; no test creates a table while its keyspace is `CREATING` or `UPDATING`.

## Steps to Reproduce

1. Start the Keyspaces emulator
2. CreateKeyspace {"keyspaceName":"app"} — the keyspace is stored in status CREATING with its ACTIVE transition due 500ms later
3. IMMEDIATELY (inside that span) call GetKeyspace {"keyspaceName":"app"} and observe "status"-bearing reports still showing the keyspace as CREATING (replicationGroupStatuses/keyspaceStatus for a MULTI_REGION keyspace)
4. Inside the same span call CreateTable {"keyspaceName":"app","tableName":"t","schemaDefinition":{"allColumns":[{"name":"id","type":"text"}],"partitionKeys":[{"name":"id"}]}} — actual: 200 with the table's ARN, and the table is stored inside a keyspace that is not usable yet; expected (by the rule this crate applies to a non-ACTIVE type in the very same handler, and to a non-ACTIVE keyspace in UpdateKeyspace/DeleteKeyspace): a refusal naming the keyspace's status
5. Second shape: create a MULTI_REGION keyspace, wait for ACTIVE, then call UpdateKeyspace to add a Region (keyspace and all tables go UPDATING for 500ms). Inside that span call CreateTable in the same keyspace — accepted, although UpdateKeyspace itself refuses the widening whenever any table of the keyspace is not ACTIVE
6. Repeat step 4 with CreateType (src/ops/create_type.rs) and with RestoreTable's target keyspace (src/ops/restore_table.rs) — both accept the same way
