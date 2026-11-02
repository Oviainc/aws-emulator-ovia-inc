Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# DeleteWorkspace ignores the metadata transfer jobs that reference the workspace, so it creates the dangling workspace reference CreateMetadataTransferJob refuses with 404

_Filed: 2026-11-02T08:27:32Z_
_Operation: DeleteWorkspace | Class: validation_

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteWorkspace` refuses a workspace that still holds entities, caller-created component types, scenes or sync jobs, and its own module doc states the principle as covering "every kind" of a workspace's contents. It does not consider the metadata transfer jobs stored BESIDE the workspaces (`RegionalState::metadata_transfer_jobs`), each of which may name a workspace by ARN at either end of the transfer (`SourceConfiguration.iotTwinMakerConfiguration.workspace`, `DestinationConfiguration.iotTwinMakerConfiguration.workspace`).

That is exactly the reference `CreateMetadataTransferJob` refuses to let come into being: it walks `referenced_workspaces(&input)` and answers `ResourceNotFoundException` for any workspace ARN the store does not resolve, with the stated reason that "a workspace this service does not hold is one no transfer could read or write". After a `DeleteWorkspace`, a stored job holds precisely such a reference, and `GetMetadataTransferJob`/`ListMetadataTransferJobs` report it back verbatim (`sources`/`destination` are "Reported as STORED"). The crate states the opposite invariant for itself: `CreateComponentType` says its reference checks exist so that "the store cannot come to hold a dangling reference — the integrity `DeleteComponentType` already relies on in the other direction", and `DeleteComponentType` enforces that direction by refusing while references exist.

So one path of this crate guarantees a job's workspace reference always resolves and another path quietly breaks it: the same ARN is a 404 on the way in and a reported value on the way out.

## Evidence

THE DELETE'S OWN STATED PRINCIPLE, AND THE FOUR KINDS IT CHECKS — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/delete_workspace.rs:178-184:
```
        // A workspace's contents are deleted before the workspace itself —
        // every kind of them, so no contained resource and no ARN it was
        // indexed under can outlive the workspace that owned it. The CALLER's
        // contents, that is: the built-in component types the workspace was
        // provisioned with are the service's own, nothing deletes them on their
        // own, and they go with the workspace
        // (`Workspace::created_component_types`).
```
followed by guards for entities (:185), created component types (:197-208), scenes (:210-221) and sync jobs (:229-240), and then the unconditional removal — delete_workspace.rs:242-255:
```
        let Some(workspace) = state.workspaces.remove(&input.workspace_id) else {
...
        state.arn_index.remove(&workspace.arn);
```
Nothing in the file reads `state.metadata_transfer_jobs` (grep for `metadata_transfer_jobs` in src/ops/delete_workspace.rs: no match).

THE REFERENCE THE CREATE REFUSES — src/ops/create_metadata_transfer_job.rs:250-266:
```
        // Every IoT TwinMaker end of the transfer names a workspace by ARN, and
        // a workspace this service does not hold is one no transfer could read
        // or write — the `ResourceNotFoundException` this operation models
        // (unlike `CreateScene`, which models none and refuses with a
        // `ValidationException`).
        for workspace in referenced_workspaces(&input) {
            if state.resolve_workspace_id(workspace).is_none() {
                return Err(
                    CreateMetadataTransferJobErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: Some(crate::state::workspace_not_found_message(workspace)),
                        },
                    )
                    .into(),
                );
            }
        }
```
and the helper that enumerates them — src/ops/create_metadata_transfer_job.rs:443-461:
```
/// Every workspace ARN this request names, in the order it names them: the
/// workspace of each IoT TwinMaker source, then of an IoT TwinMaker
/// destination. ...
fn referenced_workspaces(input: &CreateMetadataTransferJobInput) -> Vec<&str> {
```

THE INVARIANT THE CRATE CLAIMS FOR ITSELF — src/ops/create_component_type.rs:335-342:
```
        // A reference this create DECLARES has to resolve: the ids it puts in
        // `extendsFrom` and in `compositeComponentTypes` must name component
        // types the service holds ... and the store cannot come to hold a dangling reference — the integrity `DeleteComponentType` already relies on in
        // the other direction.
```
and the delete that honours it — src/ops/delete_component_type.rs:236-253:
```
        // What is built on the component type is deleted before the component
        // type itself, so nothing stored is left instantiating or extending a
        // type the store no longer holds. ...
        let references = workspace.component_type_references(&input.component_type_id);
        if let Some(message) = references.in_use_message(&input.component_type_id) {
```

THE DANGLING VALUE IS THEN REPORTED — src/ops/get_metadata_transfer_job.rs:219-222:
```
            // Reported as STORED: the modelled request shapes the create
            // supplied, so a read reports back exactly what was persisted.
            sources: job.sources.clone(),
            destination: job.destination.clone(),
```
and the state module confirms a job is not owned by any workspace, so nothing cascades — src/state.rs:1025-1029:
```
    /// Region-wide, beside the workspaces rather than inside one: a job is
    /// addressed as `/metadata-transfer-jobs/{metadataTransferJobId}`, which
    /// names no workspace, and one job's source and destination may name
    /// different workspaces (or none at all, for an Amazon S3 or an IoT
    /// SiteWise end), so no single workspace can own it.
```

TEST COVERAGE: tests/all/metadata_transfer_job.rs and tests/all/metadata_transfer_job_read.rs drive create/read of a job whose ends name a workspace, and tests/all/workspace_entity_lifecycle.rs drives DeleteWorkspace's empty-workspace guards; no test in tests/all/ deletes a workspace a stored metadata transfer job references, so neither side of the inconsistency is pinned.

## Steps to Reproduce

1. CreateWorkspace(workspaceId="src") and CreateWorkspace(workspaceId="dst").
2. CreateMetadataTransferJob(metadataTransferJobId="job1", sources=[{"type":"iottwinmaker","iotTwinMakerConfiguration":{"workspace":"<ARN of src>"}}], destination={"type":"s3","s3Configuration":{"location":"<some s3 ARN>"}}) — accepted 200 because src/ops/create_metadata_transfer_job.rs:255 resolved the `src` ARN.
3. DeleteWorkspace(workspaceId="src") — answered HTTP 200 and the workspace plus its ARN index entry are gone (src/ops/delete_workspace.rs:242-255); the four guards above it never look at `state.metadata_transfer_jobs`.
4. GetMetadataTransferJob(metadataTransferJobId="job1") — still 200, and `sources[0].iotTwinMakerConfiguration.workspace` is the ARN of a workspace the store no longer holds.
5. Contrast: CreateMetadataTransferJob(metadataTransferJobId="job2", sources=[{"type":"iottwinmaker","iotTwinMakerConfiguration":{"workspace":"<the same now-deleted src ARN>"}}], destination=<same>) is refused 404 ResourceNotFoundException "Workspace src not found" — the identical reference the store is now holding.
6. Contrast the sibling direction: DeleteComponentType on a type an entity instantiates is refused ValidationException (src/ops/delete_component_type.rs:242-253), which is the rule src/ops/create_component_type.rs:340-342 says keeps the store free of dangling references.
