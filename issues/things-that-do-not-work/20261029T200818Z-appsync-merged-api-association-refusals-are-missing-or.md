Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-associate-merged-graphql-api-duplicate-error
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-associate-merged-graphql-api-duplicate-error appsync-duplicate-source-association-conflict appsync-error-association-not-found-on-merged-api appsync-source-api-association-error-paths appsync-source-api-association-not-found-errors appsync-list-types-by-association-lifecycle appsync-merge-source-api-association-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|AssociateMergedGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|AssociateMergedGraphqlApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|AssociateMergedGraphqlApi|4xx->4xx|status
Issue-Key: appsync|AssociateSourceGraphqlApi|4xx->2xx|$.message
Issue-Key: appsync|AssociateSourceGraphqlApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|AssociateSourceGraphqlApi|4xx->2xx|status
Issue-Key: appsync|UpdateSourceApiAssociation|4xx->4xx|$.message
Issue-Key: appsync|UpdateSourceApiAssociation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|UpdateSourceApiAssociation|4xx->4xx|status
Issue-Key: appsync|DisassociateSourceGraphqlApi|4xx->4xx|$.message
Issue-Key: appsync|DisassociateSourceGraphqlApi|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|DisassociateSourceGraphqlApi|4xx->4xx|status
Issue-Key: appsync|DeleteGraphqlApi|4xx->2xx|$
Issue-Key: appsync|DeleteGraphqlApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appsync|DeleteGraphqlApi|4xx->2xx|status
Issue-Key: appsync|DisassociateSourceGraphqlApi|2xx->4xx|status
Issue-Key: appsync|DeleteGraphqlApi|2xx->4xx|status
Issue-Key: appsync|DisassociateMergedGraphqlApi|2xx->4xx|$.sourceApiAssociationStatus
Issue-Key: appsync|DisassociateMergedGraphqlApi|2xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync merged-API association refusals are missing or mis-coded, and associations cannot be looked up by id (8 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateMergedGraphqlApi

## Description
The merged-API association surface refuses the wrong things: duplicate and multi-merged associations are either accepted or reported as a concurrency conflict, unknown association ids are classified as bad-request instead of not-found, a source API associated with a merged API can be deleted, and association ids minted by the emulator cannot subsequently be resolved (every by-id disassociate/delete answers 404). 8 net-new causes.

Cause 1 of 8 — AssociateMergedGraphqlApi for an association that already exists answers ConcurrentModificationException instead of BadRequestException.
AWS: 400 BadRequestException with the fixed sentence "SourceApiAssociation already exists." (recorded behavior; Smithy: AssociateMergedGraphqlApi errors include BadRequestException httpError 400 and ConcurrentModificationException httpError 409).
Emulator: 409 with x-amzn-errortype ConcurrentModificationException and 'SourceApiAssociations for Merged API <mergedApiId> are currently being updated, please wait and try again.'
Why: 409/ConcurrentModification is a retryable signal, so a client retries forever against a permanently duplicate request instead of surfacing a configuration error.
Fix: AssociateMergedGraphqlApi validation — an existing association must be detected as a duplicate before the concurrency gate.
Traces: appsync-associate-merged-graphql-api-duplicate-error#task#5.

Cause 2 of 8 — AssociateSourceGraphqlApi does not refuse associating one source API with a second merged API.
AWS: 400 BadRequestException with the fixed sentence "Source Api can not be associated with multiple Merged APIs." (recorded behavior).
Emulator: 200, with no error body and no x-amzn-errortype header — the second association is created.
Why: the emulator permits a topology AWS forbids, so a client builds state that cannot exist on AWS and only fails on migration.
Fix: AssociateSourceGraphqlApi validation — reject when the source API already has a merged-API association.
Traces: appsync-duplicate-source-association-conflict#task#6.

Cause 3 of 8 — UpdateSourceApiAssociation with an unrecognized associationId answers 400 BadRequestException instead of 404 NotFoundException.
AWS: 404 NotFoundException with the fixed sentence "No source API association was found" (recorded behavior; Smithy: UpdateSourceApiAssociation errors include NotFoundException httpError 404 and BadRequestException httpError 400).
Emulator: 400 BadRequestException 'Invalid SourceApi Association ID <associationId>'.
Why: a client cannot distinguish "the id is syntactically wrong" from "no such association"; existence probes that catch NotFoundException fail open.
Fix: UpdateSourceApiAssociation handler — a well-formed but unknown id must reach the not-found path, not the format-validation path.
Traces: appsync-error-association-not-found-on-merged-api#task#5, appsync-source-api-association-error-paths#task#5, appsync-source-api-association-not-found-errors#task#5.

Cause 4 of 8 — DisassociateSourceGraphqlApi with an unrecognized associationId answers 400 instead of 404.
AWS: 404 NotFoundException "No source API association was found" (recorded behavior).
Emulator: 400 BadRequestException 'Invalid SourceApi Association ID <associationId>'.
Fix: same classification defect as cause 3, in the DisassociateSourceGraphqlApi handler.
Traces: appsync-error-association-not-found-on-merged-api#task#8, appsync-source-api-association-error-paths#task#6, appsync-source-api-association-not-found-errors#task#6.

Cause 5 of 8 — DeleteGraphqlApi does not refuse deleting a source API that is associated with a merged API.
AWS: 400 BadRequestException with the fixed sentence "API can not be deleted as it is associated to a Merged API" (recorded behavior; Smithy: DeleteGraphqlApi errors include BadRequestException httpError 400).
Emulator: 200 with an empty body and no x-amzn-errortype header — the API is deleted.
Why: the merged API is left pointing at a source API that no longer exists, a state AWS prevents; a client never learns it must disassociate first.
Fix: DeleteGraphqlApi validation — check inbound source-API associations before deleting.
Traces: appsync-list-types-by-association-lifecycle#teardown#9, appsync-merge-source-api-association-lifecycle#task#5.

Cause 6 of 8 — DisassociateSourceGraphqlApi cannot resolve an association the emulator itself created.
AWS: 200 for a disassociate addressed by the associationId the associate call returned.
Emulator: 404.
Why: there is no way to tear an association down, so every association is permanent and cleanup leaks state across a client's test runs.
Fix: the association index used by DisassociateSourceGraphqlApi must key on the id the associate handler minted (see also the id-generation cause in the association-projection issue).
Traces: appsync-list-types-by-association-lifecycle#teardown#10.

Cause 7 of 8 — DeleteGraphqlApi for an API the emulator created answers 404.
AWS: 200.
Emulator: 404 — the API cannot be deleted at teardown (consequent to cause 5 having already removed it, or to the api/association bookkeeping disagreeing).
Why: a client's teardown fails and leaves the account in a state it cannot clean up.
Fix: GraphqlApi state/deletion bookkeeping in the DeleteGraphqlApi handler.
Traces: appsync-list-types-by-association-lifecycle#teardown#12, appsync-merge-source-api-association-lifecycle#teardown#9.

Cause 8 of 8 — DisassociateMergedGraphqlApi answers 404 instead of scheduling the deletion.
AWS: 200 with sourceApiAssociationStatus DELETION_SCHEDULED (Smithy: SourceApiAssociationStatus includes DELETION_SCHEDULED; DisassociateMergedGraphqlApi output member sourceApiAssociationStatus).
Emulator: 404 and no sourceApiAssociationStatus at all.
Why: the merged-API side of the teardown is unusable, and a client polling for DELETION_SCHEDULED/DELETION_IN_PROGRESS never observes the transition.
Fix: DisassociateMergedGraphqlApi handler — resolve the association by id and drive it to DELETION_SCHEDULED.
Traces: appsync-merge-source-api-association-lifecycle#task#6.

Related open/fixed issues that do NOT cover these: "AppSync merged-API association rejections return non-AWS message text..." (20261024T113246Z, fixed) and "AppSync DisassociateSourceGraphqlApi rejects an unrecognized associationId..." (20261026T015602Z, fixed) — a resolved issue never covers a current failure, and neither carries these replay keys.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-associate-merged-graphql-api-duplicate-error#task#5 AssociateMergedGraphqlApi: status expected 400 got 409; $.message expected 'SourceApiAssociation already exists.' got 'SourceApiAssociations for Merged API hmjorqg7rq7qnh5svkzpa4ydag are currently being updated, please wait and try again.'; x-amzn-errortype got 'ConcurrentModificationException'.
2) appsync-duplicate-source-association-conflict#task#6 AssociateSourceGraphqlApi: status expected 400 got 200; $.message expected 'Source Api can not be associated with multiple Merged APIs.' got None.
3) appsync-error-association-not-found-on-merged-api#task#5 UpdateSourceApiAssociation: status expected 404 got 400; expected 'No source API association was found' got 'Invalid SourceApi Association ID 00000000-0000-0000-0000-000000000000'; errortype got 'BadRequestException'.
4) appsync-error-association-not-found-on-merged-api#task#8 DisassociateSourceGraphqlApi: identical 404->400 divergence.
5) appsync-list-types-by-association-lifecycle#teardown#9 DeleteGraphqlApi: status expected 400 got 200; expected body message 'API can not be deleted as it is associated to a Merged API', got {}.
6) appsync-list-types-by-association-lifecycle#teardown#10 DisassociateSourceGraphqlApi: status expected 200, got 404.
7) appsync-list-types-by-association-lifecycle#teardown#12 DeleteGraphqlApi: status expected 200, got 404.
8) appsync-merge-source-api-association-lifecycle#task#6 DisassociateMergedGraphqlApi: status expected 200 got 404; $.sourceApiAssociationStatus expected 'DELETION_SCHEDULED' got None.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. Fixtures appsync-associate-merged-graphql-api-duplicate-error, appsync-duplicate-source-association-conflict, appsync-error-association-not-found-on-merged-api, appsync-source-api-association-error-paths, appsync-source-api-association-not-found-errors, appsync-list-types-by-association-lifecycle and appsync-merge-source-api-association-lifecycle mismatch at the steps under Traces.
3. By hand: AssociateMergedGraphqlApi twice for the same (merged, source) pair (cause 1); AssociateSourceGraphqlApi for one source API against two merged APIs (cause 2); Update/DisassociateSourceGraphqlApi with associationId 00000000-0000-0000-0000-000000000000 (causes 3-4); DeleteGraphqlApi on an associated source API (cause 5); then try DisassociateSourceGraphqlApi / DeleteGraphqlApi / DisassociateMergedGraphqlApi with the ids the associate calls returned (causes 6-8).
