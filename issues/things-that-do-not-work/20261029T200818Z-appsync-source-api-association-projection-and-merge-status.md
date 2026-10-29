Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-associate-merged-graphql-api-duplicate-error
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-associate-merged-graphql-api-duplicate-error appsync-merged-api-association-from-source-side appsync-merged-api-association-lifecycle appsync-duplicate-source-association-conflict appsync-list-types-by-association-lifecycle appsync-merged-api-source-association-management appsync-source-api-association-auto-merge-reconfigure appsync-merge-source-api-association-lifecycle appsync-merge-source-into-merged-api-lifecycle appsync-source-api-association-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|AssociateMergedGraphqlApi|2xx->2xx|$.sourceApiAssociation.associationId
Issue-Key: appsync|AssociateMergedGraphqlApi|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatusDetail
Issue-Key: appsync|AssociateSourceGraphqlApi|2xx->2xx|$.sourceApiAssociation.associationArn
Issue-Key: appsync|AssociateSourceGraphqlApi|2xx->2xx|$.sourceApiAssociation.associationId
Issue-Key: appsync|AssociateSourceGraphqlApi|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatusDetail
Issue-Key: appsync|AssociateMergedGraphqlApi|2xx->2xx|$.sourceApiAssociation.associationArn
Issue-Key: appsync|GetSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatusDetail
Issue-Key: appsync|GetSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.associationId
Issue-Key: appsync|GetSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.lastSuccessfulMergeDate
Issue-Key: appsync|GetSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatus
Issue-Key: appsync|UpdateSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.associationId
Issue-Key: appsync|UpdateSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.lastSuccessfulMergeDate
Issue-Key: appsync|UpdateSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatus
Issue-Key: appsync|UpdateSourceApiAssociation|2xx->2xx|$.sourceApiAssociation.sourceApiAssociationStatusDetail
Issue-Key: appsync|ListSourceApiAssociations|2xx->2xx|$.sourceApiAssociationSummaries[*]
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# AppSync source-API association projection and merge status diverge from AWS (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateMergedGraphqlApi

## Description
The SourceApiAssociation projection diverges from AWS on two axes: association identity (the associationId AWS mints is a UUID, and the associationArn is built from it) and merge outcome (the emulator always reports a scheduled-or-successful merge with its own status detail sentence, where AWS evaluates the merge and reports MERGE_FAILED with a diagnostic when the source API has no types). 10 net-new causes.

Cause 1 of 10 — AssociateMergedGraphqlApi emits a sourceApiAssociationStatusDetail AWS leaves absent.
AWS: the association reply omits sourceApiAssociationStatusDetail at association time; the detail only appears once a merge has been attempted (Smithy: SourceApiAssociation.sourceApiAssociationStatusDetail, optional String, "the detailed message related to the current state").
Emulator: populates it with 'The source API merge is scheduled and has not been performed yet.' — an emulator-authored sentence for a member AWS omits.
Why: a client that treats the presence of a status detail as "a merge has been attempted" is misled on the very first reply.
Fix: the AssociateMergedGraphqlApi serializer / association state — do not synthesize a status detail at association time.
Traces: appsync-associate-merged-graphql-api-duplicate-error#task#4, appsync-merged-api-association-from-source-side#task#4, appsync-merged-api-association-lifecycle#task#5.

Cause 2 of 10 — AssociateSourceGraphqlApi mints a non-UUID associationId, so the associationArn is wrong too.
AWS: associationId is a UUID (8-4-4-4-12 hex) and associationArn is arn:aws:appsync:<region>:<account>:apis/<mergedApiId>/sourceApiAssociations/<associationId> built from it (Smithy: SourceApiAssociation.associationId / .associationArn). The ARN's variable parts are region, account, merged API id and that association id; the fixed part is the arn prefix and the /sourceApiAssociations/ segment. The id value itself is per-run and is not a requirement — its FORM is.
Emulator: mints a 26-character base32-style id (the AppSync api-id alphabet) and embeds it in the ARN, so the ARN's last segment has the wrong shape.
Why: clients and IaC tools that parse or validate the association ARN's trailing UUID reject it, and the emulator's association ids are indistinguishable from API ids.
Fix: the association-id generator used by AssociateSourceGraphqlApi/AssociateMergedGraphqlApi state, and the ARN builder that consumes it.
Traces: appsync-duplicate-source-association-conflict#task#5, appsync-list-types-by-association-lifecycle#task#5, appsync-merged-api-source-association-management#task#6, appsync-source-api-association-auto-merge-reconfigure#task#4.

Cause 3 of 10 — AssociateMergedGraphqlApi has the same non-UUID associationId/associationArn defect plus the synthesized status detail.
AWS: as cause 2, for the merged-API-side association call.
Emulator: 26-character id in both associationId and associationArn, plus the scheduled-merge status detail.
Fix: same generator as cause 2, reached through the AssociateMergedGraphqlApi handler.
Traces: appsync-merge-source-api-association-lifecycle#task#4, appsync-merge-source-into-merged-api-lifecycle#task#4, appsync-merged-api-source-association-management#task#10.

Cause 4 of 10 — AssociateSourceGraphqlApi also emits the absent sourceApiAssociationStatusDetail.
AWS: member omitted at association time.
Emulator: 'The source API merge is scheduled and has not been performed yet.'
Fix: as cause 1, in the AssociateSourceGraphqlApi serializer.
Traces: appsync-source-api-association-lifecycle#task#4.

Cause 5 of 10 — the successful-merge status detail is emulator-authored.
AWS: after a successful merge, sourceApiAssociationStatusDetail is the template "Successfully merged source GraphQL API (API Id: <sourceApiId>, Association Id: <associationId>)" — fixed text "Successfully merged source GraphQL API (API Id: ", ", Association Id: " and the closing paren, with the source API id and the association id taken from state (recorded behavior).
Emulator: 'The source API was merged into the Merged API.' — a fixed sentence with no variable parts.
Why: the detail no longer identifies which source API and association the merge covered, which is the only correlation handle a multi-source merged API has.
Fix: the merge routine's status-detail template in association state.
Traces: appsync-list-types-by-association-lifecycle#task#6.

Cause 6 of 10 — GetSourceApiAssociation reports MERGE_SUCCESS where AWS reports MERGE_FAILED for a source API with no types.
AWS: merging a source API that defines no types fails: sourceApiAssociationStatus MERGE_FAILED and sourceApiAssociationStatusDetail "There's no types found in this API (AccountID: <accountId>, ApiId: <sourceApiId>)" — fixed text with the account id and source API id taken from the request/state; lastSuccessfulMergeDate stays absent (Smithy: SourceApiAssociationStatus values include MERGE_FAILED; SourceApiAssociation.lastSuccessfulMergeDate timestamp).
Emulator: MERGE_SUCCESS with 'The source API was merged into the Merged API.' and a populated lastSuccessfulMergeDate.
Why: the emulator never fails a merge, so a client's merge-failure handling is unreachable and a merged API is reported as carrying a schema it does not have.
Fix: the schema-merge routine must evaluate the source API's type set and drive the association state machine to MERGE_FAILED (leaving lastSuccessfulMergeDate unset).
Traces: appsync-merged-api-association-from-source-side#task#6, appsync-merged-api-source-association-management#task#7, appsync-source-api-association-lifecycle#task#5, appsync-source-api-association-lifecycle#task#8.

Cause 7 of 10 — UpdateSourceApiAssociation's reply reports MERGE_SUCCESS where AWS reports MERGE_FAILED.
AWS: as cause 6, projected through the UpdateSourceApiAssociation response (Smithy: UpdateSourceApiAssociation output member sourceApiAssociation).
Emulator: MERGE_SUCCESS, emulator-authored detail, lastSuccessfulMergeDate populated.
Fix: same merge evaluation as cause 6, observed through UpdateSourceApiAssociation.
Traces: appsync-merged-api-source-association-management#task#9, appsync-source-api-association-lifecycle#task#7.

Cause 8 of 10 — UpdateSourceApiAssociation that reconfigures auto-merge leaves the association in MERGE_SCHEDULED instead of MERGE_FAILED.
AWS: MERGE_FAILED with the "There's no types found in this API (AccountID: ..., ApiId: ...)" detail.
Emulator: MERGE_SCHEDULED with 'The source API merge is scheduled and has not been performed yet.' — the merge is never performed at all on the auto-merge reconfigure path.
Why: the association never leaves the scheduled state, so a client polling for a terminal merge status never finishes.
Fix: the auto-merge reconfiguration path must actually run (and settle) the merge.
Traces: appsync-source-api-association-auto-merge-reconfigure#task#6.

Cause 9 of 10 — GetSourceApiAssociation after that reconfigure also reports MERGE_SCHEDULED.
AWS: MERGE_FAILED with the no-types detail.
Emulator: MERGE_SCHEDULED with the scheduled sentence.
Fix: same as cause 8, observed through GetSourceApiAssociation.
Traces: appsync-source-api-association-auto-merge-reconfigure#task#7.

Cause 10 of 10 — ListSourceApiAssociations' summary entries do not match AWS's SourceApiAssociationSummary projection.
AWS: each summary carries exactly associationId, associationArn, sourceApiId, sourceApiArn, mergedApiId, mergedApiArn and description (Smithy: ListSourceApiAssociations output sourceApiAssociationSummaries).
Emulator: the replayed summary diverges from the expected entry (reported as both a missing and an extra element for index 0), i.e. the emitted member set/ordering of the summary object does not match AWS's.
Why: a client that round-trips the summary (or diffs inventory between calls) sees entries that do not compare equal to the ones Get/Associate returned.
Fix: the ListSourceApiAssociations serializer's summary projection.
Traces: appsync-merged-api-association-from-source-side#task#5, appsync-merged-api-source-association-management#task#8, appsync-source-api-association-lifecycle#task#6.

Note: association ids, ARNs and merge timestamps are per-run values; what is being filed here is their FORM and the status/detail contract, not any recorded literal.

## Evidence
Replay of simparity-wire-aws-v1.

1) appsync-associate-merged-graphql-api-duplicate-error#task#4 AssociateMergedGraphqlApi 200: $.sourceApiAssociation.sourceApiAssociationStatusDetail expected absent (None), got 'The source API merge is scheduled and has not been performed yet.'; associationId shape not as asserted.
2) appsync-duplicate-source-association-conflict#task#5 AssociateSourceGraphqlApi 200: associationArn expected '.../sourceApiAssociations/28994d70-7086-4f9d-9d91-f4c23bedff4b' (UUID form), got '.../sourceApiAssociations/f7nv3bkw3xu73ngemdfmu6lsqc' (26-char api-id form).
3) appsync-merge-source-api-association-lifecycle#task#4 AssociateMergedGraphqlApi 200: associationArn expected '.../sourceApiAssociations/85c3b397-d6da-46af-80c9-70b57ef63a6c', got '.../aujy5xvrc6yetkjupwzdgpdx3c'.
4) appsync-source-api-association-lifecycle#task#4 AssociateSourceGraphqlApi 200: statusDetail expected absent, got the scheduled sentence.
5) appsync-list-types-by-association-lifecycle#task#6 GetSourceApiAssociation 200: statusDetail expected 'Successfully merged source GraphQL API (API Id: <sourceApiId>, Association Id: <associationId>)', got 'The source API was merged into the Merged API.'
6) appsync-merged-api-association-from-source-side#task#6 GetSourceApiAssociation 200: sourceApiAssociationStatus expected 'MERGE_FAILED' got 'MERGE_SUCCESS'; statusDetail expected "There's no types found in this API (AccountID: <acct>, ApiId: <sourceApiId>)" got 'The source API was merged into the Merged API.'; lastSuccessfulMergeDate expected absent, got a timestamp.
7) appsync-merged-api-source-association-management#task#9 UpdateSourceApiAssociation 200: same MERGE_FAILED vs MERGE_SUCCESS divergence.
8) appsync-source-api-association-auto-merge-reconfigure#task#6 UpdateSourceApiAssociation 200: expected 'MERGE_FAILED' got 'MERGE_SCHEDULED'.
9) appsync-source-api-association-auto-merge-reconfigure#task#7 GetSourceApiAssociation 200: expected 'MERGE_FAILED' got 'MERGE_SCHEDULED'.
10) appsync-merged-api-association-from-source-side#task#5 ListSourceApiAssociations 200: $.sourceApiAssociationSummaries[0] reported both missing and extra — the emitted summary object does not match AWS's projection.

## Steps to Reproduce
1. Clone the repo, boot the emulator, replay simparity-wire-aws-v1.
2. The listed steps in fixtures appsync-associate-merged-graphql-api-duplicate-error, appsync-duplicate-source-association-conflict, appsync-merge-source-api-association-lifecycle, appsync-source-api-association-lifecycle, appsync-list-types-by-association-lifecycle, appsync-merged-api-association-from-source-side, appsync-merged-api-source-association-management and appsync-source-api-association-auto-merge-reconfigure mismatch.
3. By hand: CreateGraphqlApi twice (one MERGED), AssociateSourceGraphqlApi / AssociateMergedGraphqlApi and inspect associationId form, associationArn and sourceApiAssociationStatusDetail (causes 1-4); with a source API that defines no types, GetSourceApiAssociation / UpdateSourceApiAssociation and check the status and detail (causes 5-9); ListSourceApiAssociations and compare the summary to the Get projection (cause 10).
