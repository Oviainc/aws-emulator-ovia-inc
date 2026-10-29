Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.domain-unit-delete-non-empty-parent-rejected
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.domain-unit-delete-non-empty-parent-rejected|run#8|AssertionError|assert code == "ConflictException", (
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# datazone DeleteDomainUnit is an unimplemented 501 stub, so the non-empty-parent ConflictException and the post-delete denial are never produced

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDomainUnit

## Description
ONE cause. Cause 1 of 1: DeleteDomainUnit answers HTTP 501 `NotImplemented` for every request, so none of the operation's contract — the non-empty-parent refusal, the successful delete of an empty unit, or the post-delete read behaviour — is emulated. No open or fixed issue covers this operation (the open datazone issues cover DeleteProject/DeleteDomain/DeleteFormType-style authorization ordering and error wording, not DeleteDomainUnit; the fixed `datazone-createdomainunit-stub` and `datazone-listdomainunitsforparent-stub` covered the sibling create/list operations only).

AWS: Smithy `DeleteDomainUnit` — DELETE /v2/domains/{domainIdentifier}/domain-units/{identifier}, @idempotent, success HTTP 204 with an empty output; modeled errors AccessDeniedException (403), ConflictException (409), ResourceNotFoundException (404), ValidationException (400), ThrottlingException (429), UnauthorizedException (401), InternalServerException (500). Recorded behavior: deleting a domain unit that still has a live child domain unit is refused with 409 ConflictException whose message is the fixed template `The following objects could not be deleted: domainUnit <parentDomainUnitId> deletion prevented by domainUnit <blockingChildDomainUnitId>. ` — the fixed text is `The following objects could not be deleted: domainUnit `, ` deletion prevented by domainUnit `, and the trailing `. ` (note the trailing period-plus-space); the two variable slots are the id of the unit being deleted (from the request's `identifier`) and the id of each child that blocks it (from emulator state), never literal ids from one run. The parent must remain readable through GetDomainUnit after the refused delete. Once the blocking child is deleted, DeleteDomainUnit on the parent succeeds (204). Recorded behavior only for the post-delete read: GetDomainUnit on a just-deleted domain unit does NOT 404 — across repeated reads it answers 403 AccessDeniedException with message `User is not permitted to perform operation: GetDomainUnit, or the requested resource does not exist` (entirely fixed text; the operation name is the only substitutable part).

Emulator: every DeleteDomainUnit call returns HTTP 501 with error code `NotImplemented` and message `operation DeleteDomainUnit is scaffolded but not implemented in this emulator`. The check reaches the refusal assertion after successfully creating a parent and a child unit and confirming the child is listed under the parent, then gets code 'NotImplemented' where 'ConflictException' is required.

Why: a client cannot delete domain units at all, and cannot observe the referential-integrity guard. Teardown/cleanup code that deletes units bottom-up sees an unmodeled 501 instead of 204, so retry and error-classification logic treats a routine delete as a server fault; code that relies on the ConflictException to detect a non-empty unit never sees it, and code that reads a deleted unit expects a denial rather than a 404.

Fix: implement the handler for this operation (its only file), adding (a) the child-domain-unit reference check that builds ConflictException with AWS's message template before any deletion, (b) removal from the domain-unit store with a 204 empty response, and (c) the deleted-unit read path in GetDomainUnit so a removed id is answered 403 AccessDeniedException with AWS's wording rather than 404.

Traces: behavioral check datazone.domain-unit-delete-non-empty-parent-rejected.

## Evidence
Replay of check datazone.domain-unit-delete-non-empty-parent-rejected:
  AssertionError: DeleteDomainUnit on a parent with a live child must be rejected with ConflictException, got 'NotImplemented'
Expected: HTTP 409, code ConflictException, message `The following objects could not be deleted: domainUnit <parentId> deletion prevented by domainUnit <childId>. ` (fixed text + ids taken from the request and emulator state); parent still readable via GetDomainUnit; after the child is deleted, DeleteDomainUnit on the parent returns 204; GetDomainUnit on the deleted unit then answers 403 AccessDeniedException `User is not permitted to perform operation: GetDomainUnit, or the requested resource does not exist` consistently.
Actual: HTTP 501, code NotImplemented, message `operation DeleteDomainUnit is scaffolded but not implemented in this emulator`. The handler is still the generated stub (services/service-com-amazonaws-datazone-datazone/src/ops/delete_domain_unit.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDomainUnit"))`).

## Steps to Reproduce
1. CreateDomain (V2) and take rootDomainUnitId.
2. CreateDomainUnit under the root -> parent; CreateDomainUnit under parent -> child; poll ListDomainUnitsForParent(parentDomainUnitIdentifier=parent) until the child is listed.
3. Call DeleteDomainUnit(domainIdentifier=<domain>, identifier=<parent>) while the child exists. Emulator answers 501 NotImplemented; AWS answers 409 ConflictException with the 'could not be deleted ... deletion prevented by domainUnit' message, and the parent remains readable.
4. Delete the child, then delete the parent: both must return 204.
5. GetDomainUnit(identifier=<parent>) repeatedly: each read must answer 403 AccessDeniedException 'User is not permitted to perform operation: GetDomainUnit, or the requested resource does not exist'.
Replay check id: datazone.domain-unit-delete-non-empty-parent-rejected.
