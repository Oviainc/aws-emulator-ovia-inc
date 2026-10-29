Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# The "drop a data store and everything it owns" sequence is implemented twice in state.rs, so the two reapers can drift

_Filed: 2026-10-25T14:37:57Z_
_Operation: CreateFHIRDatastore | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsHealthlakeHealthlakeState` has two reapers that differ ONLY in which records they select, and each one re-implements the rule for what dropping a data store means: remove the registry entry, then forget the idempotency tokens that named it, its export jobs and its import jobs. `reap_expired_records` (reached from `settle_lifecycle`, which every operation in the crate calls) selects records whose `reap_at` has fallen due; `reap_terminal_records` (called by `CreateFHIRDatastore` and `RestoreFHIRDatastore` when the store is full) selects records already at `DELETED`. The collect-then-remove body under the two different predicates is otherwise character-for-character the same four-statement sequence.

Because the sequence is written twice, the copies can drift: the state module owns four per-data-store collections today (`client_tokens`, `export_jobs`, `import_jobs`, plus the resources held on the record itself), and `import_jobs`' own doc notes that `StartFHIRImportJob` is still a stub, so this surface is still growing. Anyone who adds a fifth owned collection — or a fifth `forget_*_for` call — has to remember to edit BOTH reapers; editing only one leaves a reaped data store's dependents alive in the other path, which is exactly the leak both functions exist to prevent.

The fix is local and behaviour-preserving: keep the two entry points and their doc comments, and route both through one private helper that takes the selection predicate (or a single `drop_datastore(&mut self, id: &str)` that owns the four removals). Every response and every existing test stays unchanged, because neither the predicate nor the removal set changes.

## Evidence

Copy 1 — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:1534-1547:
```
    fn reap_expired_records(&mut self, now: Instant) {
        let reaped: Vec<String> = self
            .datastores
            .iter()
            .filter(|(_, datastore)| datastore.reap_at.is_some_and(|at| now >= at))
            .map(|(id, _)| id.clone())
            .collect();
        for id in reaped {
            self.datastores.remove(&id);
            self.forget_client_tokens_for(&id);
            self.forget_export_jobs_for(&id);
            self.forget_import_jobs_for(&id);
        }
    }
```

Copy 2 — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:1552-1565:
```
    pub fn reap_terminal_records(&mut self) {
        let reaped: Vec<String> = self
            .datastores
            .iter()
            .filter(|(_, datastore)| datastore.status == DatastoreStatus::Deleted)
            .map(|(id, _)| id.clone())
            .collect();
        for id in reaped {
            self.datastores.remove(&id);
            self.forget_client_tokens_for(&id);
            self.forget_export_jobs_for(&id);
            self.forget_import_jobs_for(&id);
        }
    }
```
The two bodies differ only in the `.filter(...)` predicate; the four-statement removal sequence is duplicated verbatim.

Both entry points are reached from the same handler — services/service-com-amazonaws-healthlake-healthlake/src/ops/create_fhir_datastore.rs:208 `store.settle_lifecycle();` (which ends in `self.reap_expired_records(now);`, state.rs:1528) and create_fhir_datastore.rs:296 `store.reap_terminal_records();`.

The drift hazard is named by the state module itself — state.rs:1456-1462 on `import_jobs`: "A job's record is dropped with the data store it belonged to, by this state's own `forget_import_jobs_for`. The ceiling on how many job records there can be belongs with the operation that MINTS them (`StartFHIRImportJob`, still a scaffold stub)" — i.e. a per-data-store collection whose owning operation is not written yet, so the removal set is still subject to change.

Test coverage the fix must leave unchanged:
- `reap_expired_records` path — tests/all/fhir_export_job_listing.rs:718 `a_job_does_not_outlive_the_data_store_it_exported_from`, which sets `record.reap_at = Some(Instant::now());` (line 745) and then asserts both `ResourceNotFoundException` for the data store and `held == 0` for its export jobs (lines 758-770).
- `reap_terminal_records` path — tests/all/fhir_datastore_lifecycle.rs:1012 `the_store_bounds_how_many_data_stores_it_holds`, which fills the store to `MAX_DATASTORES`, deletes one and creates again (lines 1044-1059).

## Steps to Reproduce

1. Read services/service-com-amazonaws-healthlake-healthlake/src/state.rs:1534-1547 (`reap_expired_records`) and :1552-1565 (`reap_terminal_records`) side by side
2. Observe the two functions differ only in the `.filter(...)` predicate, while the `self.datastores.remove` + `forget_client_tokens_for` + `forget_export_jobs_for` + `forget_import_jobs_for` sequence is written out twice
3. Note that src/ops/create_fhir_datastore.rs reaches both copies (line 208 via `settle_lifecycle` -> state.rs:1528, and line 296 directly)
4. Simulate the drift: add a hypothetical fifth per-data-store collection (e.g. the `transformation_jobs`-style map `StartFHIRImportJob` will need once state.rs:1456-1462 is no longer a stub) and forget it in one copy — the expiry path then drops it while the full-store path leaves it behind, or the reverse, with nothing in the crate catching the asymmetry
