Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6eca256d767263dcce8cda9bce6eb5315c5aad3d

# A deregistered application's operation records are reported as the history of a NEW registration that reuses the ApplicationId

_Filed: 2026-10-29T11:27:39Z_
_Operation: ListOperations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6eca256d767263dcce8cda9bce6eb5315c5aad3d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegionState::settle` cleans up after a completed deregistration by dropping everything held for the application EXCEPT its `Operation` records: its tags and resource permissions are forgotten per ARN (`forget_resource`) and its configuration check runs are dropped (`forget_configuration_checks_for`), while `operations` keeps every `StoredOperation` filed against it.

But an operation record is joined back to an application by id alone (`operations_for` filters `record.resource_id == application_id`), and an `ApplicationId` is free again the instant the removal completes — `RegisterApplication` only refuses an id currently in `applications`. So registering the same `ApplicationId` again produces an application whose `ListOperations` reports the PREVIOUS registration's records, including its `DEREGISTER_APPLICATION` record at status `SUCCESS`. The records are indistinguishable from the new application's own: `ResourceId` is the same id and `ResourceArn` is the same ARN (ARNs here are a pure function of the ids they embed), so a client reads `GetApplication` = `ACTIVATED` and `ListOperations` = "this application was deregistered (SUCCESS)" at the same instant.

The two cleanups therefore disagree about what survives a registration: the crate states that a resource later derived under the same ARN is a NEW resource carrying nothing of the old one, and it enforces that for tags, permissions and configuration check runs, but the operation listing hands the new registration the dead one's history.

## Evidence

What the completed-deregistration cleanup drops and what it keeps — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2811-2818 and 2837-2841:
```
// ... Its `Operation` records stay: they are the history of calls already
// made, which `GetOperation` resolves by id, and [`MAX_RETAINED_OPERATIONS`] is
// what bounds them.
...
for (id, arns) in completed {
    self.applications.remove(&id);
    destroyed.extend(arns);
    self.forget_configuration_checks_for(&id);
}
```
The principle the sibling cleanup is written to guarantee — state.rs:2902-2906 (`forget_resource`):
```
/// ... The ARNs of this service are a pure function of the ids
/// they embed, so a resource later derived under the SAME ARN is a new resource that
/// carries neither — which is what this guarantees.
```
The join that defeats it — state.rs:3359-3364:
```
pub fn operations_for(&self, application_id: &str) -> Vec<&StoredOperation> {
    self.operations
        .values()
        .filter(|record| record.resource_id == application_id)
        .collect()
}
```
(`record_operation` writes `resource_id: application.id` / `resource_arn: application.arn`, state.rs:3316-3317.)

The id is reusable — ops/register_application.rs:225-237 refuses only an id currently present in `state.region(&region).applications`.

The listing's own stated standard of accuracy — ops/list_operations.rs:177-187:
```
// `ApplicationId` is required, so the listing is always scoped to ONE
// application ... so a reference no registration answers is the
// `ValidationException` it does model rather than an empty page, which would
// report an application that exists and has had nothing done to it.
```
Test coverage: tests/all/deregistration_window.rs and tests/all/region_partition_reclamation.rs pin the deregistration window and the reclamation of what was held per resource; tests/all/list_operations.rs pins the records of a live application. No test re-registers a deregistered `ApplicationId` and re-reads `ListOperations`, which is where this surfaces.

## Steps to Reproduce

1. RegisterApplication ApplicationId="HDB1" (HANA, with Sid/SapInstanceNumber/Credentials); wait out the registration transition.
2. StartApplication("HDB1") and let it settle, so a START_APPLICATION record exists.
3. DeregisterApplication("HDB1") and make a later call after TRANSITION_SPAN so the removal completes (GetApplication now refuses the id).
4. RegisterApplication ApplicationId="HDB1" again (accepted: the id is free).
5. ListOperations(ApplicationId="HDB1") -> the page contains the previous registration's REGISTER_APPLICATION, START_APPLICATION and DEREGISTER_APPLICATION (SUCCESS) records alongside the new registration's own, all carrying the same ResourceId/ResourceArn, while GetApplication("HDB1") reports the freshly registered application.
