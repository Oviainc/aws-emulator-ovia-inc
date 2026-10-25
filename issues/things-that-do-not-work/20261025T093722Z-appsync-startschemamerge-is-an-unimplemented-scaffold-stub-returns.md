Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.start-schema-merge-without-source-association-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.start-schema-merge-without-source-association-rejected|run#14|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Commit: 0048f4bcdfe707d7c82784a2fe2676fa8dc1d5ca

# appsync StartSchemaMerge is an unimplemented scaffold stub: returns 501 NotImplemented instead of merging or refusing an unresolvable associationId with 404 NotFoundException

> _Note: this issue was identified against the code base at commit `0048f4bcdfe707d7c82784a2fe2676fa8dc1d5ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSchemaMerge

## Description
Cause 1 of 1: `StartSchemaMerge` is still a scaffold stub, so every call to it — valid or invalid — is answered 501 `NotImplemented`. No open issue covers this operation (the nearest open items are the per-operation stub issues for StartSchemaCreation, GetSchemaCreationStatus, ListSourceApiAssociations, UpdateSourceApiAssociation and DisassociateSourceGraphqlApi; none of them name StartSchemaMerge, and the replay report did not mark this check as ALREADY FILED or OWNED).

AWS: Smithy model `com.amazonaws.appsync#StartSchemaMerge` — POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/merge, success 200 with output member `sourceApiAssociationStatus` (SourceApiAssociationStatus: AUTO_MERGE_SCHEDULE_FAILED | DELETION_FAILED | DELETION_IN_PROGRESS | DELETION_SCHEDULED | MERGE_FAILED | MERGE_IN_PROGRESS | MERGE_SCHEDULED | MERGE_SUCCESS). Modeled errors: BadRequestException (400), ConcurrentModificationException (409), InternalFailureException (500), NotFoundException (404), UnauthorizedException (401). Documented behavior: "Initiates a merge operation. Returns a status that shows the result of the merge operation"; NotFoundException — "The resource specified in the request was not found." So an `associationId` that resolves to no source-API association on an otherwise valid MERGED api must be refused with HTTP 404 / error code `NotFoundException`, and an associationId that DOES resolve must NOT be refused with NotFoundException (it either starts the merge with a 200 status or draws BadRequestException while unreconciled). Recorded message for the unresolvable-association case, as an example: `No source API association was found`.

Emulator: both legs answer HTTP 501 with error code `NotImplemented` and message "operation StartSchemaMerge is scaffolded but not implemented in this emulator". The handler body in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/start_schema_merge.rs is still `Err(crate::scaffolded::Scaffolded::not_implemented("StartSchemaMerge"))` and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker. The replay asserted the error code of the negative leg and got `'NotImplemented'` where `'NotFoundException'` is required.

Why: a client cannot merge a source API into a Merged API at all, and cannot distinguish a bad associationId from a server-side gap: AWS's 404 `NotFoundException` is a retriable-never client error a caller handles (re-resolve the association, re-associate), whereas 501 `NotImplemented` is an unmodeled status no AppSync SDK models for this operation, so it surfaces to the caller as a generic unknown-error and any merge-orchestration flow (associate → merge → poll status) stalls. The positive leg is equally broken: no merge is ever scheduled, so merged-API schema propagation never happens.

Fix: implement the operation handler at services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/start_schema_merge.rs — resolve (mergedApiIdentifier, associationId) against the source-API-association state, return `NotFoundException` when the associationId resolves to no association on that merged API, otherwise transition the association's merge state and return `sourceApiAssociationStatus`; then delete the SCAFFOLDED marker line.

Traces: appsync.start-schema-merge-without-source-association-rejected (mismatch — AssertionError: expected NotFoundException, got 'NotImplemented').

## Evidence
Check appsync.start-schema-merge-without-source-association-rejected [mismatch]:
  File "check_appsync_start_schema_merge_without_source_association_rejected.py", line 80, in run
    assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
  AssertionError: expected NotFoundException, got 'NotImplemented'

Expected (AWS / Smithy model): StartSchemaMerge with an associationId that resolves to no source-API association on a valid MERGED api -> HTTP 404, error code NotFoundException (example message: "No source API association was found"). With a resolvable associationId -> HTTP 200 and `sourceApiAssociationStatus`, or a modeled BadRequestException while unreconciled — never NotFoundException.

Actual (emulator): HTTP 501, error code NotImplemented, message "operation StartSchemaMerge is scaffolded but not implemented in this emulator", for both the resolvable and the unresolvable associationId.

Source: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/start_schema_merge.rs lines 4-6 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 171 (`Err(crate::scaffolded::Scaffolded::not_implemented("StartSchemaMerge"))`).

## Steps to Reproduce
1. Create an IAM role AppSync can assume for merged-API execution, then appsync CreateGraphqlApi(name=..., authenticationType=API_KEY, apiType=MERGED, mergedApiExecutionRoleArn=<role>) -> note mergedApiIdentifier.
2. appsync CreateGraphqlApi(name=..., authenticationType=API_KEY) for a source API; appsync AssociateSourceGraphqlApi(mergedApiIdentifier=<merged>, sourceApiIdentifier=<source>) -> note associationId.
3. appsync StartSchemaMerge(mergedApiIdentifier=<merged>, associationId=<that real associationId>). Expected: HTTP 200 with sourceApiAssociationStatus (or a modeled BadRequestException). Observed: HTTP 501 NotImplemented.
4. appsync StartSchemaMerge(mergedApiIdentifier=<merged>, associationId=<a random UUID that matches no association>). Expected: HTTP 404, error code NotFoundException. Observed: HTTP 501, error code NotImplemented.
