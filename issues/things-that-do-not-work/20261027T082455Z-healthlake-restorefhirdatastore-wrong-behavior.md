Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# RestoreFHIRDatastore silently drops every resource that was updated after the restore point, so a point-in-time restore loses data the source held at that instant

_Filed: 2026-10-27T08:24:55Z_
_Operation: RestoreFHIRDatastore | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

RestoreFHIRDatastore copies the source data store's FHIR resources through `Datastore::fhir_resources_as_of(&restore_point)`, which selects resources by `last_updated <= restore_point`. Because the FHIR data plane's update interaction rewrites a resource in place and keeps no version history, a resource that was CREATED before the restore point and UPDATED after it has a `last_updated` past the cutoff and is dropped from the restored data store altogether — it reads back 404 on the restored store's endpoint, even though the source demonstrably held that resource at the requested instant.

The state module itself labels this a KNOWN LIMITATION and states what the correct behaviour is: the selection "should ... pick the newest version at or before `at` instead, which is exactly what the real point-in-time restore does". So the crate does not claim the current selection is right; it records that it diverges from a point-in-time restore and names the missing piece (a version history on `FhirResource`).

The user-visible effect is silent data loss on the one operation whose whole purpose is reproducing a data store as it stood: the restore answers 200, the new data store comes up ACTIVE, and the resources a caller edited after the chosen instant are simply absent rather than present at the version they had then. A caller has no way to tell that from a resource that never existed — both answer the same OperationOutcome 404 from `fhir_rest::read`.

(Disclosure: the current behaviour is pinned by a test that restates the same limitation as its reason; it pins the symptom of the gap the state module admits, not an independent rule.)

## Evidence

The crate's own admission — services/service-com-amazonaws-healthlake-healthlake/src/state.rs:729-739:
```
    /// KNOWN LIMITATION, and the reason this is a `<=` filter rather than a
    /// version walk: [`FhirResource`] holds ONE version — the FHIR update
    /// interaction rewrites it in place and keeps no history — so a resource
    /// created before `at` and updated after it has no version this service can
    /// answer for `at`. It is left out rather than copied at the version it
    /// reached later, because that content did not exist at `at` and a restore
    /// must not invent it. Whoever gives [`FhirResource`] a version history
    /// (charging the retained bytes the way the create and update paths already
    /// charge [`MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE`]) should make this pick
    /// the newest version at or before `at` instead, which is exactly what the
    /// real point-in-time restore does.
```
The code under that comment — state.rs:744-750:
```
        let cutoff = epoch_nanos(at);
        let resources: BTreeMap<(String, String), FhirResource> = self
            .fhir_resources
            .iter()
            .filter(|(_, resource)| epoch_nanos(&resource.last_updated) <= cutoff)
```
The handler that relies on it — services/service-com-amazonaws-healthlake-healthlake/src/ops/restore_fhir_datastore.rs:319 and :435-438:
```
            let (fhir_resources, fhir_resource_bytes) = source.fhir_resources_as_of(&restore_point);
```
```
            // The source's resources as of the restore point, copied verbatim:
            // the restored data store serves exactly what the source held then.
            fhir_resources: snapshot.fhir_resources,
```
— the comment claims "exactly what the source held then", which is what the `<=` filter cannot deliver for an updated resource.

The single-version record that causes it — state.rs:591-594:
```
    /// `meta.versionId`: 1 for a resource that has only been created.
    pub version: u64,
    /// `meta.lastUpdated`, the instant the stored version was written.
    pub last_updated: ::aws_smithy_types::DateTime,
```
and the in-place rewrite — src/service/fhir_rest.rs:453-459:
```
        datastore.fhir_resource_bytes = datastore
            .fhir_resource_bytes
            .saturating_sub(released)
            .saturating_add(bytes);
        datastore
            .fhir_resources
            .insert((resource_type.to_string(), id.to_string()), resource);
```

Model text the gap is measured against — smithy_query(query="digest") for RestoreFHIRDatastore: "Restore a backup-enabled data store to a point in time. Creates a new data store from the backup."

Existing coverage that documents the limitation (and that a version-history fix would have to be re-pointed): tests/all/restore_fhir_datastore.rs:993-1042 `a_resource_whose_only_version_postdates_the_restore_point_is_not_restored`, asserting "a resource this service holds no version of as of the restore point is not restored (it keeps no version history)". tests/all/restore_fhir_datastore.rs:225-335 `a_restore_reproduces_the_source_as_of_the_restore_point` covers only resources written once.

## Steps to Reproduce

1. CreateFHIRDatastore with a BackupConfiguration whose Status is enabled, and wait past crate::state::LIFECYCLE_TRANSITION so it is ACTIVE
2. POST <DatastoreEndpoint>Patient with { "resourceType": "Patient", "active": true } — note the assigned logical id P (meta.versionId 1)
3. Record the current instant T (a point after the create)
4. PUT <DatastoreEndpoint>Patient/P with { "resourceType": "Patient", "id": P, "active": false } — answers 200 with meta.versionId 2, rewriting the record in place
5. RestoreFHIRDatastore with SourceDatastoreId = the source and RestoreConfiguration.ContinuousBackupRestoreConfiguration.RestorePointTime = T; wait past the transition so the restored store is ACTIVE
6. GET <restored DatastoreEndpoint>Patient/P: 404 OperationOutcome not-found, although the source held Patient/P (version 1) at T — the restored store is missing a resource the restore point includes, instead of carrying the version that existed then
