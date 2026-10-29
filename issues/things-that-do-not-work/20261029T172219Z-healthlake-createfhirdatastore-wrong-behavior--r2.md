Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# The preloaded-create byte-total refusal is charged against DELETED data stores that the same handler reaps before its count refusal

_Filed: 2026-10-29T17:22:19Z_
_Operation: CreateFHIRDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateFHIRDatastore` holds a new data store to two store-wide ceilings, and answers them inconsistently.

For the COUNT ceiling (`MAX_DATASTORES`) the handler first sweeps terminal records and only then refuses, and it states the rule it is applying: "Terminal records are reaped before the refusal, so a caller that deletes what it created can always create again; only a genuinely full store is refused."

Forty lines later the same handler charges the BYTE ceiling (`MAX_FHIR_RESOURCE_BYTES_TOTAL`) for a preloaded create through `store.fhir_resource_bytes_total()` — and does NOT sweep first. `fhir_resource_bytes_total()` sums every record the registry still holds, which includes a data store that has reached the terminal `DELETED` status and is only lingering for `DELETED_RETENTION` (60s); `state.rs` documents that those bytes come back exactly by dropping the record ("a reaped data store gives its bytes back by being dropped"), and documents `reap_terminal_records` as "the answer to a full store, taken before a create is refused".

So a caller that fills the store with FHIR resources, deletes every data store it created, and then creates a data store with `PreloadDataConfig` is refused 400 `ThrottlingException` ("The preloaded data store would push the FHIR resources this service retains past its 67108864 byte ceiling") for bytes held ONLY by records the crate's own rule says are given back — records this very call would have dropped had the count been the binding ceiling instead. The refusal is not a genuinely full store: the same request succeeds once the 60s retention elapses and nothing else changed.

`RestoreFHIRDatastore` carries the identical copy of the same skipped sweep (reap before the count refusal at src/ops/restore_fhir_datastore.rs:353-358, no reap before the byte refusal at :372-387), and it is the more reachable path because a restore always copies the source's bytes.

This is not a new feature: the on-demand sweep already exists, is already called by this handler for the other ceiling, and `state.rs` already names it as the answer a create takes before refusing.

## Evidence

THE RULE THIS HANDLER STATES AND APPLIES TO THE COUNT CEILING — services/service-com-amazonaws-healthlake-healthlake/src/ops/create_fhir_datastore.rs:292-308:
```
        // The store holds a bounded number of data stores. Terminal records are
        // reaped before the refusal, so a caller that deletes what it created
        // can always create again; only a genuinely full store is refused.
        if store.datastores.len() >= crate::state::MAX_DATASTORES {
            store.reap_terminal_records();
        }
        if store.datastores.len() >= crate::state::MAX_DATASTORES {
            return Err(CreateFHIRDatastoreErrorModeled::ThrottlingException(
```

THE SAME HANDLER CHARGING THE BYTE CEILING WITHOUT THAT SWEEP — src/ops/create_fhir_datastore.rs:337-357:
```
        // A preload RETAINS bytes, so it charges the ceiling every data store's
        // resources are held to together, exactly as a FHIR write and a restore
        // do.
        if store
            .fhir_resource_bytes_total()
            .saturating_add(fhir_resource_bytes)
            > crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
        {
            return Err(CreateFHIRDatastoreErrorModeled::ThrottlingException(
                ThrottlingException {
                    message: Some(format!(
                        "The preloaded data store would push the FHIR resources this service \
                         retains past its {} byte ceiling",
                        crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
                    )),
```

THE TOTAL IS A SUM OVER HELD RECORDS, AND REAPING IS WHAT GIVES ITS BYTES BACK — src/state.rs:1770-1781:
```
    /// How many BYTES of FHIR resources every data store retains together.
    ///
    /// Summed over the data stores' own counters rather than tracked as a second
    /// total, so a reaped data store gives its bytes back by being dropped and
    /// the two can never disagree. The walk is bounded by [`MAX_DATASTORES`].
    pub fn fhir_resource_bytes_total(&self) -> usize {
        self.datastores
            .values()
            .fold(0usize, |total, datastore| {
                total.saturating_add(datastore.fhir_resource_bytes)
            })
    }
```

AND THE SWEEP IS DOCUMENTED AS THE ANSWER A CREATE TAKES BEFORE REFUSING — src/state.rs:1916-1921:
```
    /// Reap every terminal record NOW, whatever its retention: the answer to a
    /// full store, taken before a create is refused, so a caller that deletes
    /// what it created can always create again.
    pub fn reap_terminal_records(&mut self) {
        self.reap_datastores_where(|datastore| datastore.status == DatastoreStatus::Deleted);
    }
```
A `DELETED` record is still in the registry while its retention runs — src/state.rs:1812-1816 sets `datastore.reap_at = Some(now + DELETED_RETENTION)` when the record reaches `DELETED`, and src/state.rs:79-86 states `DELETED_RETENTION` is 60s of readability.

THE IDENTICAL COPY IN THE SIBLING MINT — src/ops/restore_fhir_datastore.rs:353-358 (sweep before the count refusal) against :370-387 (byte refusal, no sweep):
```
        if store.datastores.len() >= crate::state::MAX_DATASTORES {
            store.reap_terminal_records();
        }
...
        // A restore copies the source's FHIR resources, so it charges the bytes
        // every data store retains TOGETHER, exactly as a FHIR write does.
        if store
            .fhir_resource_bytes_total()
            .saturating_add(snapshot.fhir_resource_bytes)
            > crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
```

TEST COVERAGE: `tests/all/datastore_reaping.rs:1-12` pins the sweep only for the COUNT path ("the sweep of terminal records a full store takes before refusing a create (reached from `CreateFHIRDatastore`)"), and no test anywhere exercises `MAX_FHIR_RESOURCE_BYTES_TOTAL` (`grep -rn 'BYTES_TOTAL\|push the FHIR resources' tests/` matches nothing), so sweeping before the byte refusal leaves every existing test unchanged.

## Steps to Reproduce

1. Create four data stores and wait out their CREATING transition.
2. Through each data store's FHIR REST endpoint, POST resources until the four together retain ~64 MiB (`MAX_FHIR_RESOURCE_BYTES_TOTAL`, 16 MiB per data store being `MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`).
3. Call DeleteFHIRDatastore on all four and wait out the DELETING -> DELETED transition (500 ms), but stay inside the 60 s DELETED_RETENTION window.
4. Call DescribeFHIRDatastore on each: all four report DatastoreStatus DELETED, so nothing the caller created is live any more.
5. Call CreateFHIRDatastore with {"DatastoreTypeVersion":"R4","PreloadDataConfig":{"PreloadDataType":"SYNTHEA"}}.
6. Observe 400 ThrottlingException "The preloaded data store would push the FHIR resources this service retains past its 67108864 byte ceiling" — charged entirely against the four DELETED records, which `reap_terminal_records()` would have dropped had the data-store COUNT been the binding ceiling in the same handler.
7. Wait out the remainder of DELETED_RETENTION and repeat the same create: it is now served, proving the refusal came from records whose bytes the crate's own rule gives back.
8. Same shape for RestoreFHIRDatastore: leave one live backup-enabled source holding 16 MiB plus three DELETED stores holding 48 MiB, then restore the source — refused for the 48 MiB the terminal records hold.
