Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 97cefcb07ab7801eeb37fff053a06a3892425e6e

# DeleteType counts a table or parent type whose own deletion is already in flight as a user of the type, against its own stated rule

_Filed: 2026-10-29T08:00:38Z_
_Operation: DeleteType | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `97cefcb07ab7801eeb37fff053a06a3892425e6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteType` refuses a type that is still used by a table or another type, and the handler states the rule it means to apply: the users are "derived from the keyspace as it stands now ... so a table or type created after this one counts, and a reference dropped by deleting its table stops counting".

The code under that comment does not implement the second half. `Keyspace::direct_referring_tables` and `Keyspace::direct_parent_types` scan every table and every type of the keyspace with NO status filter, and a deletion in this crate is deliberately not instantaneous: `DeleteTable` accepts the delete, sets the table to `DELETING` and leaves the record in the keyspace until `settle` removes it one `TRANSIENT_STATUS_SPAN` (500 ms) later. So for that whole span a table the caller has ALREADY successfully deleted still counts as a user, and `DeleteType` answers 409 `ConflictException` naming that table — the reference the comment says "stops counting" has not stopped counting.

The same holds for the parent-type half, and there it is sharper: a type in `DELETING` is excluded from `Keyspace::usable_type_names` (which keeps only `ACTIVE`), so no new table or type can ever reference it again — yet it still blocks the deletion of the child type it refers to. `DeleteType` on parent P followed by `DeleteType` on child C inside the span answers 409 for C, blaming a type that is itself on its way out.

This contradicts how the rest of the crate treats `DELETING`: the keyspace-wide in-flight check states "A resource already `DELETING` does NOT count: it is on its way out", which is exactly the judgement `DeleteType` declines to make. The practical effect is that the ordinary "drop the table, then drop the UDT it used" (or "drop the parent UDT, then the child") sequence fails with a conflict unless the caller sleeps out the transient span, even though both resources' deletions were accepted.

Note the derivation is also what `GetType` reports as `directReferringTables`/`directParentTypes`, where listing a still-existing `DELETING` table is fine; only the refusal in `DeleteType` is wrong.

## Evidence

The handler's own rule and the code that breaks it — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_type.rs:233-241`:
```
        // "You can only delete a type that is not used in a table or another UDT." The
        // users of a type are derived from the keyspace as it stands now — the same
        // derivation `GetType` reports as `directReferringTables` and
        // `directParentTypes` — so a table or type created after this one counts, and a
        // reference dropped by deleting its table stops counting. The type stays exactly
        // as it was: the request is answered with the conflict and nothing is written.
        let referring_tables = keyspace.direct_referring_tables(&type_name);
        let parent_types = keyspace.direct_parent_types(&type_name);
        if !referring_tables.is_empty() || !parent_types.is_empty() {
```

The derivations it calls carry no status filter — `src/state.rs:1330-1345`:
```
    pub fn direct_referring_tables(&self, type_name: &str) -> Vec<String> {
        self.tables
            .values()
            .filter(|table| {
                table
                    .schema_definition
                    .iter()
                    .flat_map(|schema| &schema.all_columns)
                    .any(|column| {
                        referenced_type_names(&column.r#type)
                            .iter()
                            .any(|referenced| referenced == type_name)
                    })
            })
            .map(|table| table.table_name.clone())
```
and `src/state.rs:1311-1322` (`direct_parent_types`) scans `self.types.values()` the same way, excluding only the type itself (`candidate.type_name != type_name`).

A deleted table is still in that map for the whole transient span — `src/ops/delete_table.rs:221-228`:
```
            // "If the table is in the ACTIVE state, you can delete it." The table enters
            // DELETING now and the clock owes its removal one span from here.
            crate::state::TABLE_STATUS_ACTIVE => {
                let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
                table.status = crate::state::TABLE_STATUS_DELETING.to_string();
                table.status_due_at = Some(settles_at);
                Some(settles_at)
            }
```
removed only later by `src/state.rs:1798-1800`:
```
                        // The deletion the clock owed has completed, so the table is
                        // no longer a resource of this keyspace at all.
                        TABLE_STATUS_DELETING => false,
```
and the span is deliberately observable — `src/state.rs:162-165`:
```
/// deliberately NON-ZERO, so a caller that reads straight after the call still
/// observes the transient status the model documents, and short enough that a
/// test can wait it out.
pub const TRANSIENT_STATUS_SPAN: Duration = Duration::from_millis(500);
```

The same is true of a parent TYPE, which this very handler puts into `DELETING` — `src/ops/delete_type.rs:267-271`:
```
        let settles_at = now + crate::state::TRANSIENT_STATUS_SPAN;
        if let Some(udt) = keyspace.types.get_mut(&type_name) {
            udt.status = crate::state::TYPE_STATUS_DELETING.to_string();
            udt.status_due_at = Some(settles_at);
        }
```
while a `DELETING` type can no longer be referenced at all — `src/state.rs:1296-1301`:
```
    pub fn usable_type_names(&self) -> BTreeSet<String> {
        self.types
            .values()
            .filter(|udt| udt.status == TYPE_STATUS_ACTIVE)
            .map(|udt| udt.type_name.clone())
            .collect()
```

The crate's stated treatment of `DELETING` for exactly this "does this resource block the change?" question — `src/state.rs:913-918`:
```
    /// A resource in flight ([`is_in_flight_status`]) is one the clock still owes a
    /// transition to, and dropping it is exactly what `DeleteTable` refuses for a table
    /// in that state — so a keyspace holding one cannot be dropped either, since
    /// `DeleteKeyspace` "deletes a keyspace and all of its tables". A resource already
    /// `DELETING` does NOT count: it is on its way out, which is where dropping the
    /// keyspace takes it anyway.
```

Test coverage: nothing pins the accepted or refused shape of this case, so nothing constrains a fix. `tests/all/delete_type.rs` exercises the `DELETING` status and the settled absence (`tests/all/delete_type.rs:10`: "`DELETING` status the clock settles into absence, after which neither `GetType`") but never deletes a referring table or a parent type first; grep for "still used by" / "can only be deleted once nothing uses it" over `tests/all/` finds no match, and the only `ConflictException` assertions in `tests/all/type_lifecycle.rs:239` and `:826` are duplicate-name creates.

## Steps to Reproduce

1. Start the Keyspaces emulator
2. CreateKeyspace {"keyspaceName":"app"}; wait out the transient span so it is ACTIVE
3. CreateType {"keyspaceName":"app","typeName":"address","fieldDefinitions":[{"name":"street","type":"text"}]}; wait for ACTIVE
4. CreateTable {"keyspaceName":"app","tableName":"people","schemaDefinition":{"allColumns":[{"name":"id","type":"text"},{"name":"home","type":"frozen<address>"}],"partitionKeys":[{"name":"id"}]}}; wait for ACTIVE
5. DeleteTable {"keyspaceName":"app","tableName":"people"} — 200, the table is accepted for deletion and now reports status DELETING
6. IMMEDIATELY (inside the 500 ms span) DeleteType {"keyspaceName":"app","typeName":"address"} — actual: 409 ConflictException "Type 'app.address' is still used by table 'app.people' and can only be deleted once nothing uses it.", naming a table whose deletion the service has already accepted; expected by the handler's own comment ("a reference dropped by deleting its table stops counting") and by the crate's rule that a DELETING resource "does NOT count: it is on its way out": the delete is accepted
7. Parent-type shape: CreateType "child", then CreateType "parent" with a field of type frozen<child>; DeleteType "parent" (200, parent goes DELETING), then immediately DeleteType "child" — 409 blaming 'app.parent', although a DELETING type is excluded from usable_type_names and can never gain a new reference
8. Sleep past TRANSIENT_STATUS_SPAN and repeat either DeleteType — it now succeeds, confirming the refusal was only the in-flight record
