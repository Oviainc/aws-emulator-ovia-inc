Issue-Source: trace-sweep-divergence
Issue-Trace: ram-principal-association-workflow
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-principal-association-workflow ram-resource-share-tagging
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|GetResourceShareAssociations|2xx->2xx|$.resourceShareAssociations[*]
Issue-Key: ram|TagResource|2xx->2xx|$
Issue-Key: ram|UntagResource|2xx->2xx|$
Issue-Commit: 1e44d2b2032d56c599b46458071bcd07c3617563

# RAM resource-share association and tag mutations are not reflected in responses

> _Note: this issue was identified against the code base at commit `1e44d2b2032d56c599b46458071bcd07c3617563`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceShareAssociations

## Description
Three net-new RAM wire divergences, all of them the same shape of fault: a mutation a client just performed on a resource share is not reflected in what RAM reports back. The association store models only the two statuses of the happy-path attach, and the two tag-mutating operations answer with a body that carries nothing about the mutation they performed. All three steps answer HTTP 200, so no client sees an error — they see a share whose state silently never changes.

Cause 1 of 3: GetResourceShareAssociations never reports a disassociated PRINCIPAL association (fixture ram-principal-association-workflow, step task#6).
AWS: ResourceShareAssociation$status targets ResourceShareAssociationStatus, whose modelled values are ASSOCIATING, ASSOCIATED, FAILED, DISASSOCIATING, DISASSOCIATED, SUSPENDED, SUSPENDING, RESTORING (Smithy: shape ResourceShareAssociationStatus; ResourceShareAssociation$status "The current status of the association", statusMessage "A message about the status of the association"). After the share/principal association has been torn down, GetResourceShareAssociations (POST /getresourceshareassociations, 200) still reports the association record, with status DISASSOCIATED and no statusMessage — the recording shows exactly that for the principal 123456789012 on associationType PRINCIPAL.
Emulator: the same read returns the association still in status ASSOCIATING, carrying the pending-invitation statusMessage (recorded example: "Waiting for the principal to either accept or reject the resource share invitation."). The association store knows no disassociated state at all: state.rs declares only ASSOCIATION_STATUS_ASSOCIATING and ASSOCIATION_STATUS_ASSOCIATED, and the only status write in the store is settle_due_associations(), which can move a record to ASSOCIATED and nowhere else. DisassociateResourceShare is still an unimplemented scaffold (it returns the not_implemented channel), so nothing can ever take a record out of the attach path — that stub has its own issue; this cause is the read-side half, which would still mis-report once the stub is written, because there is no DISASSOCIATING/DISASSOCIATED representation for it to write.
Why: a client that disassociates a principal and then polls GetResourceShareAssociations for DISASSOCIATED (the normal wait-for-teardown loop, and what the console and the CLI waiters do) polls forever, and a client that reads the association to decide whether the principal still has access is told it is mid-attach and still carries an invitation message that no longer applies. The whole disassociate half of the association lifecycle is unobservable.
Fix: the association record's status lifecycle in the service's state (the association store and its settle/transition step in services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs), plus the GetResourceShareAssociations handler so a disassociated record is still reported and its statusMessage is cleared when the status is terminal.
Traces: ram-principal-association-workflow#task#6 — $.resourceShareAssociations[0] expected status DISASSOCIATED with no statusMessage, got status ASSOCIATING with statusMessage; both sides HTTP 200.

Cause 2 of 3: TagResource answers with an empty body instead of reporting the tags it attached (fixture ram-resource-share-tagging, step task#2).
AWS: recorded behavior only — the Smithy model's TagResourceResponse is an empty structure, so the model says nothing about a response body (Smithy: TagResource, POST /tagresource, 200, output shape TagResourceResponse with no members). The recording shows the 200 response carrying the tags the call created, one entry per request tag, keyed by the tag's key and value (example: tagsCreated [{key: cost-center, value: 42}]). Hypothesis: the corpus was recorded against an API revision that echoes the created tags, which the checked-in model does not yet carry.
Emulator: returns 200 with an empty JSON object. services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/tag_resource.rs merges the tags into the share and then returns TagResourceOutput {} — the output type has no member to report the mutation with.
Why: a client that reads the response to confirm which tag pairs were actually attached (rather than issuing a follow-up GetResourceShares) gets an empty set and concludes nothing was tagged, even though the share was tagged.
Fix: the TagResource response shape and its serializer — the output structure needs the created-tag member and the handler must populate it from the pairs it merged; because the member is absent from the checked-in model, the model slice for the operation has to be reconciled with the recording first.
Traces: ram-resource-share-tagging#task#2 — $ expected a body reporting the created tag, got {}; both sides HTTP 200.

Cause 3 of 3: UntagResource answers with an empty body instead of reporting the tags it removed (fixture ram-resource-share-tagging, step task#4).
AWS: recorded behavior only — UntagResourceResponse is likewise an empty structure in the model (Smithy: UntagResource, POST /untagresource, 200, output shape UntagResourceResponse with no members). The recording shows the 200 response carrying the tags the call deleted, one entry per removed key (example: deletedTags [{key: team}]). Same hypothesis as cause 2.
Emulator: returns 200 with an empty JSON object. services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/untag_resource.rs removes the keys from the share and returns UntagResourceOutput {}.
Why: symmetric to cause 2 — a client cannot tell from the response which of the keys it named were actually present and removed, so an untag that was a no-op and one that removed a tag are indistinguishable.
Fix: the UntagResource response shape and its serializer — the output structure needs the deleted-tag member, populated from the keys the handler actually removed; the model slice needs the same reconciliation as cause 2.
Traces: ram-resource-share-tagging#task#4 — $ expected a body reporting the deleted tag, got {}; both sides HTTP 200.

None of the three is covered by an open issue: the five open issues are for docdb, rds, fis, dynamodb and a sagemaker stub, and none of them names a RAM fixture step. The DisassociateResourceShare scaffold stub noted under cause 1 is a separate, auto-filed finding; closing it does not close cause 1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, bootstrap_signal false, state_reset_failures 0), 3 divergence clusters, all service=ram, none with a stub_path or an owning open issue.

1) fixture ram-principal-association-workflow, step task#6, GetResourceShareAssociations, verdict mismatch, expected_status 200 / actual_status 200.
   divergence_key: service=ram|GetResourceShareAssociations|status=200->200|keys=$.resourceShareAssociations[0]
   Expected $.resourceShareAssociations[0]: {associatedEntity: '123456789012', associationType: 'PRINCIPAL', status: 'DISASSOCIATED', external: true, resourceShareArn/resourceShareName present, creationTime/lastUpdatedTime shaped-not-asserted} and NO statusMessage member.
   Actual:   {associatedEntity: '123456789012', associationType: 'PRINCIPAL', status: 'ASSOCIATING', statusMessage: 'Waiting for the principal to either accept or reject the resource share invitation.', ...}.
   Requirement diverging: the association's status value (DISASSOCIATED vs ASSOCIATING) and the presence of statusMessage on a terminal association. The recorded message text itself is only an example.
   Code: state.rs defines ASSOCIATION_STATUS_ASSOCIATING / ASSOCIATION_STATUS_ASSOCIATED only; settle_due_associations() (state.rs ~line 351) writes ASSOCIATED and no other status; ops/disassociate_resource_share.rs:217 returns Scaffolded::not_implemented("DisassociateResourceShare").

2) fixture ram-resource-share-tagging, step task#2, TagResource, verdict mismatch, expected_status 200 / actual_status 200.
   divergence_key: service=ram|TagResource|status=200->200|keys=$
   Expected $: {tagsCreated: [{key: 'cost-center', value: '42'}]} (the pairs the request attached). Actual $: {} (empty_body).
   Code: ops/tag_resource.rs:293 `Ok(TagResourceOutput {})`; Smithy TagResourceResponse has no members.

3) fixture ram-resource-share-tagging, step task#4, UntagResource, verdict mismatch, expected_status 200 / actual_status 200.
   divergence_key: service=ram|UntagResource|status=200->200|keys=$
   Expected $: {deletedTags: [{key: 'team'}]} (the keys the request removed). Actual $: {} (empty_body).
   Code: ops/untag_resource.rs:250 `Ok(UntagResourceOutput {})`; Smithy UntagResourceResponse has no members.

Model references used: smithy_query operation=GetResourceShareAssociations,TagResource,UntagResource and shape=ResourceShareAssociationStatus,ResourceShareAssociation,TagResourceResponse,UntagResourceResponse on services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json.

## Steps to Reproduce
Cause 1: replay fixture ram-principal-association-workflow and inspect step task#6.
  1. CreateResourceShare, then AssociateResourceShare with principals=[<account id>] on the resulting resource-share ARN.
  2. DisassociateResourceShare for that same principal (today this answers the scaffold's not-implemented refusal).
  3. POST /getresourceshareassociations with associationType=PRINCIPAL and the share ARN.
  Expected: 200 with the association reported at status DISASSOCIATED and no statusMessage. Actual: 200 with status ASSOCIATING and the pending-invitation statusMessage still attached — and no sequence of calls can produce a DISASSOCIATED status, since the store models no such value.

Causes 2 and 3: replay fixture ram-resource-share-tagging and inspect steps task#2 and task#4.
  1. CreateResourceShare to get a resource-share ARN.
  2. POST /tagresource with resourceShareArn=<arn> and tags=[{key: cost-center, value: 42}].
     Expected: 200 whose body reports the created tag pair. Actual: 200 with body {}.
  3. POST /untagresource with resourceShareArn=<arn> and tagKeys=[team] (a key the share carries).
     Expected: 200 whose body reports the deleted tag key. Actual: 200 with body {}.
  The share's stored tags do change in both cases (a follow-up GetResourceShares shows the merge/removal), so the defect is purely in what the mutating responses report.
