Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# UpdateFHIRDatastore refuses a data store in transition with AccessDeniedException instead of ConflictException

_Filed: 2026-10-24T00:03:07Z_
_Operation: UpdateFHIRDatastore | Class: error-response_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the target data store exists but is not ACTIVE (CREATING or DELETING — i.e. in a lifecycle transition), the handler answers the modelled `AccessDeniedException` ('Access is denied. Your account is not authorized to perform this operation.'). The condition the service is reporting is a resource-state conflict, which the operation models explicitly as `ConflictException` ('The data store is in a transition state and the user requested action cannot be performed.'). The sibling mutation in the same crate, DeleteFHIRDatastore, correctly answers ConflictException for exactly CREATING/DELETING. Expected: `__type: ConflictException` for an update attempted on a data store in transition (authorization-shaped AccessDeniedException reserved for authorization failures). Actual: `__type: AccessDeniedException`.

## Evidence

Spec: UpdateFHIRDatastore declares ConflictException, documented (`smithy_query(query="doc", shape="ConflictException")`) as 'The data store is in a transition state and the user requested action cannot be performed.'; AccessDeniedException is documented as an authorization failure. Handler: `src/ops/update_fhir_datastore.rs` lines ~192-199 return AccessDeniedException for every non-ACTIVE status; compare `src/ops/delete_fhir_datastore.rs` lines ~186-199 which return ConflictException for CREATING/DELETING. Tests: `tests/all/control_plane_error_semantics.rs` (rule 3) currently pins the AccessDeniedException answer, so the divergent code is locked in by the suite rather than uncovered.

## Steps to Reproduce

1. CreateFHIRDatastore with DatastoreTypeVersion=R4 (record is CREATING for the transition window)
2. Immediately call UpdateFHIRDatastore with that DatastoreId and DatastoreName=renamed
3. Observe the error body carries __type AccessDeniedException instead of ConflictException (and the same for a DELETING record right after DeleteFHIRDatastore)
