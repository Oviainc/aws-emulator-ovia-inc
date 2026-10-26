Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.op-contract-disassociateSourceGraphqlApi
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.op-contract-disassociateSourceGraphqlApi|run#26|AssertionError|assert code == "BadRequestException", f"unexpected error code: {code}"
Issue-Commit: 4d92b800bb5d886658635a287b2dfefdc5faa046

# AppSync DisassociateSourceGraphqlApi rejects an unrecognized associationId with 404 NotFoundException instead of 400 BadRequestException "Invalid SourceApi Association ID <id>"

> _Note: this issue was identified against the code base at commit `4d92b800bb5d886658635a287b2dfefdc5faa046`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateSourceGraphqlApi

## Description
One root cause in the AppSync source-API-association delete path: the association-reference refusal is collapsed onto a single NotFound classification, so a reference AWS refuses as a malformed/unrecognized association id at request-validation time comes back as a resource-missing refusal with a different HTTP status, a different error code and different message text.

Cause 1 of 1 — an associationId that is not a recognizable association reference on the addressed Merged API is answered NotFoundException (404) instead of BadRequestException (400) with AWS's request-derived message.

AWS: Smithy (com-amazonaws-appsync-awsdeepdishcontrolplaneservice): DisassociateSourceGraphqlApi, DELETE /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}, success 200, modelled errors BadRequestException (errorKind client, httpError 400), ConcurrentModificationException (409), NotFoundException (client, 404), UnauthorizedException (401), InternalFailureException (500); both path members mergedApiIdentifier and associationId are @required httpLabels. The model documents BadRequestException as "The request is not well formed. For example, a value is invalid or a required field is missing" and NotFoundException as "The resource specified in the request was not found". Recorded behavior: AWS splits the two cases on the shape of the reference, not merely on presence — (a) a well-formed associationId that AppSync itself minted and has since removed is refused 404 NotFoundException with the fixed, identifier-free text "No source API association was found" (the emulator already matches this case, and the check passes it); (b) an associationId that is not a recognizable association reference for that Merged API is refused 400 BadRequestException whose message is the template "Invalid SourceApi Association ID <associationId>" — the fixed text is "Invalid SourceApi Association ID " and the trailing value is the associationId copied verbatim from the request path (in the replay the request carried the literal `assoc-does-not-exist-1234`, which is a value from the request and not part of the requirement). Hypothesis: the discriminator is whether the supplied id parses as an AppSync-issued association identifier, since AWS cannot otherwise distinguish "never existed" from "existed and was deleted" — only the two outcomes above are recorded, not the parsing rule.

Emulator: the same call returns NotFoundException (HTTP 404). The handler services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/disassociate_source_graphql_api.rs builds a crate::state::SourceApiDisassociationRequest and lets crate::state::delete_source_api_association decide the refusal; every unknown association reference comes back as AssociationRefusal::NotFound, which `refuse()` maps to NotFoundException. No AssociationRefusal::BadRequest is ever produced for an unrecognizable associationId, so the 400 class is unreachable on this path and the "Invalid SourceApi Association ID …" message is never emitted.

Why: a client gets the wrong error class for a caller mistake. A bad association id is a permanent request defect (400, do not retry, fix the input) while 404 NotFoundException on this API is the eventually-consistent "it is gone" answer that SDK retry/idempotency wrappers and the check's own cleanup path treat as benign — so a typo'd or stale-format association reference is silently swallowed as "already disassociated" instead of surfacing as an invalid-argument failure, and nothing tells the caller which id was rejected because the emulator's not-found text names no identifier.

Fix: the shared association-delete rule crate::state::delete_source_api_association (reached from both disassociate handlers) must classify an associationId that is not a recognizable association reference for the addressed Merged API as AssociationRefusal::BadRequest carrying the message "Invalid SourceApi Association ID <associationId>" built from the request's associationId, leaving the existing NotFound("No source API association was found") classification for an association that was minted and then removed; disassociate_source_graphql_api.rs's `refuse()` already maps BadRequest to the 400 BadRequestException, so no new error variant is needed.

Traces: appsync.op-contract-disassociateSourceGraphqlApi — the check completes the whole happy path (associate, settle, disassociate to a DELETION_* state, disappearance from ListSourceApiAssociations, and the repeat-of-a-removed-id NotFoundException with the exact message) and fails only on the final boundary assertion. Not covered by the open issue 20261024T113246Z-appsync-merged-api-association-rejections-return-non-aws.md, which is about ASSOCIATE-side messages whose status and code are already correct; here the status and the code themselves diverge on the DISASSOCIATE path.

## Evidence
Cause 1 (appsync.op-contract-disassociateSourceGraphqlApi):
  Call: DisassociateSourceGraphqlApi(mergedApiIdentifier=<a live MERGED api>, associationId=<an id that is not a recognizable association reference on that api>)
  Expected (AWS): HTTP 400, error code BadRequestException, message "Invalid SourceApi Association ID <associationId>" — fixed text "Invalid SourceApi Association ID ", variable part = the associationId taken verbatim from the request path.
  Actual (emulator): HTTP 404, error code NotFoundException.
  Replay failure: check_appsync_op_contract_disassociateSourceGraphqlApi.py line 110, `assert code == "BadRequestException", f"unexpected error code: {code}"` -> AssertionError: unexpected error code: NotFoundException. (The message assertion on the next line, `msg == "Invalid SourceApi Association ID <id>"`, is never reached.)
  Already correct on the same path, for contrast: re-issuing the disassociate for the association that WAS removed yields NotFoundException with message exactly "No source API association was found" — that assertion passes, which is what isolates the divergence to the unrecognized-reference case.

Code: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/disassociate_source_graphql_api.rs lines 174-191 (delegates to crate::state::delete_source_api_association) and lines 199-220 (`refuse()` maps AssociationRefusal::NotFound -> NotFoundException, AssociationRefusal::BadRequest -> BadRequestException; the BadRequest arm is never selected for an unknown associationId). The module's own doc comment at lines 195-198 states the intent: "a not-found names no identifier, so which side was missing is not reported."

Smithy used: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json — DisassociateSourceGraphqlApi: input {mergedApiIdentifier @required httpLabel, associationId @required httpLabel}, output {sourceApiAssociationStatus: SourceApiAssociationStatus}, errors BadRequestException/400, ConcurrentModificationException/409, NotFoundException/404, UnauthorizedException/401, InternalFailureException/500.

## Steps to Reproduce
1. CreateGraphqlApi(name=<unique>, authenticationType=API_KEY) -> source api id.
2. Create an IAM role trusting appsync.amazonaws.com with appsync:* and CreateGraphqlApi(name=<unique>, authenticationType=API_KEY, apiType=MERGED, mergedApiExecutionRoleArn=<that role>) -> merged api id.
3. AssociateSourceGraphqlApi(mergedApiIdentifier=<merged>, sourceApiIdentifier=<source>) -> associationId; poll GetSourceApiAssociation until sourceApiAssociationStatus leaves MERGE_SCHEDULED/MERGE_IN_PROGRESS.
4. DisassociateSourceGraphqlApi(mergedApiIdentifier=<merged>, associationId=<associationId>) -> 200 with a DELETION_* status; poll ListSourceApiAssociations(apiId=<merged>) until the association is gone. (This all matches today.)
5. Repeat DisassociateSourceGraphqlApi with the same, now-removed associationId -> 404 NotFoundException, message "No source API association was found". (Matches today.)
6. Call DisassociateSourceGraphqlApi(mergedApiIdentifier=<merged>, associationId="assoc-does-not-exist-1234") — an id that is not a recognizable association reference for that merged API.
   Expected: HTTP 400, BadRequestException, message "Invalid SourceApi Association ID assoc-does-not-exist-1234" (template: fixed prefix + the associationId from the request).
   Observed: HTTP 404, NotFoundException.

Full replay: run_behavioral_suite and re-run check appsync.op-contract-disassociateSourceGraphqlApi.
