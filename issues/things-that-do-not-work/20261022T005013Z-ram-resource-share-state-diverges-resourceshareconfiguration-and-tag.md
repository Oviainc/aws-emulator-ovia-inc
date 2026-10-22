Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-share-lifecycle
Issue-Step: teardown#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-share-lifecycle ram-principal-association-workflow ram-promotion-error-paths ram-resource-policies-lookup-error ram-resource-share-permission-association ram-resource-share-tagging ram-resource-share-not-found-errors
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|DeleteResourceShare|2xx->4xx|status
Issue-Key: ram|CreateResourceShare|2xx->2xx|$.resourceShare.resourceShareConfiguration
Issue-Key: ram|CreateResourceShare|2xx->2xx|$.resourceShare.tags
Issue-Key: ram|GetResourceShareAssociations|2xx->2xx|$.resourceShareAssociations[*]
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-promote-1c65f4a8"].resourceShareConfiguration
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-b0397a87"].resourceShareConfiguration
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-b0397a87-renamed"].resourceShareConfiguration
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[*]
Issue-Key: ram|GetResourceShares|4xx->4xx|$.message
Issue-Key: ram|UpdateResourceShare|4xx->4xx|$.message
Issue-Key: ram|DeleteResourceShare|4xx->4xx|$.message
Issue-Key: ram|AssociateResourceShare|4xx->4xx|$.message
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-tags-d9722159"].resourceShareConfiguration
Issue-Key: ram|GetResourceShares|2xx->2xx|$.resourceShares[name="sim2real-probe-ram-ram-share-tags-d9722159"].tags[*]
Issue-Commit: 09c29ed8e99ab10517eef5fd4d5c2097ded28d72

# RAM resource share state diverges: resourceShareConfiguration and tag mutations dropped, deleted shares vanish

> _Note: this issue was identified against the code base at commit `09c29ed8e99ab10517eef5fd4d5c2097ded28d72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceShares

## Description
The RAM emulator answers the resource-share operations with HTTP 200 but its ResourceShare projection and its share lifecycle/tag state diverge from AWS in eight distinct ways. All eight share one root area: the ResourceShare record the emulator keeps and serializes is incomplete, and mutations applied to it (tags, deletion) are not reflected on read. This issue bundles EIGHT causes — a fixer must close all eight before submitting.

Cause 1 of 8: ResourceShare.resourceShareConfiguration is never serialized.
AWS: Smithy (com.amazonaws.ram) — ResourceShare has member `resourceShareConfiguration` targeting ResourceShareConfiguration ("The configuration of the resource share"); CreateResourceShare accepts `resourceShareConfiguration` on input and returns `resourceShare: ResourceShare`, GetResourceShares returns `resourceShares: ResourceShareList`. The recording shows AWS always populating this member on both operations, with the defaults for a share created without an explicit configuration. The on-disk model lists only `retainSharingOnAccountLeaveOrganization` inside ResourceShareConfiguration, while the recorded response also carries `exclusiveAccountAccess` — for that second member this is recorded behavior only.
Emulator: the member is absent from the response body entirely — `$.resourceShare.resourceShareConfiguration: missing (expected {...}, got None)` on CreateResourceShare and `$.resourceShares[name=...].resourceShareConfiguration: missing` on every GetResourceShares.
Why: a client reading share configuration (for example to decide whether sharing is retained when an account leaves the organization) dereferences a null/absent structure and either faults or silently assumes a configuration the service never reported.
Fix: the ResourceShare state record and its serializer — give the stored share a resource-share configuration with service defaults and emit it from both CreateResourceShare and GetResourceShares.
Traces: ram-resource-share-tagging#setup#1, ram-principal-association-workflow#setup#1, ram-promotion-error-paths#task#4, ram-resource-share-lifecycle#task#2, ram-resource-share-lifecycle#task#4, ram-resource-share-tagging#task#3, ram-resource-share-tagging#task#5.

Cause 2 of 8: CreateResourceShare omits `tags` instead of returning an empty list when the request carried no tags.
AWS: Smithy — CreateResourceShare input `tags: TagList`, output ResourceShare.tags: TagList. The recording shows AWS returning `tags: []` for a share created with no tags.
Emulator: `$.resourceShare.tags: missing (expected [], got None)`.
Why: a client that iterates the returned tag list without a null guard breaks on create, and code that distinguishes "no tags" from "tags not reported" cannot.
Fix: the CreateResourceShare serializer — emit the (possibly empty) tag list rather than omitting the member.
Traces: ram-principal-association-workflow#setup#1.

Cause 3 of 8: TagResource does not persist an added tag onto the resource share.
AWS: Smithy — TagResource(resourceShareArn, tags: TagList required) returns an empty body; the added pairs then appear in ResourceShare.tags on a subsequent GetResourceShares. The recording shows the share carrying both its original tag and the newly added one after TagResource succeeded.
Emulator: the later GetResourceShares reports only the pre-existing tag — `$.resourceShares[name=...].tags[1]: missing (expected {'key': 'cost-center', 'value': '42'}, got None)` — so the TagResource call was accepted but changed no state.
Why: a client that tags a share and then reads it back to confirm, or that filters shares by tag, sees the tag it just wrote as absent; any tag-driven workflow silently does nothing.
Fix: the TagResource handler/state — merge the submitted tag pairs into the stored share's tag set.
Traces: ram-resource-share-tagging#task#3.

Cause 4 of 8: UntagResource does not remove the requested tag key from the resource share.
AWS: Smithy — UntagResource(resourceShareArn, tagKeys: TagKeyList required) returns an empty body; the named keys are then gone from ResourceShare.tags. The recording shows the untagged key absent and the remaining tag still present on the next GetResourceShares.
Emulator: the tag set is unchanged after UntagResource — `$.resourceShares[name=...].tags[0]: expected {'key': 'cost-center', 'value': '42'}, got {'key': 'team', 'value': 'platform'}` (reported as both missing and extra at that index), i.e. the key named in tagKeys is still attached.
Why: tag removal appears to succeed but never takes effect, so cleanup and tag-based access/cost attribution stay wrong indefinitely.
Fix: the UntagResource handler/state — delete the named keys from the stored share's tag set.
Traces: ram-resource-share-tagging#task#5.

Cause 5 of 8: a deleted resource share disappears from GetResourceShares instead of being returned with status DELETED.
AWS: Smithy — ResourceShare.status targets ResourceShareStatus whose values are PENDING|ACTIVE|FAILED|DELETING|DELETED, and GetResourceShares takes `resourceShareStatus` as an input filter, so a DELETED share remains retrievable. The recording shows the share returned after deletion with status DELETED, carrying its name, ARN, owningAccountId, featureSet, tags and resourceShareConfiguration.
Emulator: the list comes back without the share at all — `$.resourceShares[0]: missing (expected {... 'status': 'DELETED' ...}, got None)`.
Why: a client cannot observe the terminal state of a share it deleted, and a wait-for-DELETED poll never completes; querying by DELETED status returns an empty page where AWS returns the record.
Fix: the share state store and the GetResourceShares handler — retain deleted shares in a DELETED terminal state and include them when the status filter selects them, instead of removing the record.
Traces: ram-resource-share-lifecycle#task#6, ram-resource-share-tagging#teardown#7.

Cause 6 of 8: DeleteResourceShare on an already-deleted share answers HTTP 400 instead of HTTP 200 (REPRESENTATIVE cause of this bundle).
AWS: Smithy — DeleteResourceShare has http {method: DELETE, uri: /deleteresourceshare, code: 200} and output `returnValue: Boolean`; the recording shows status 200 for the repeat delete of a share already in DELETED state (deletion is idempotent for a share that still exists in the deleted state).
Emulator: `status: expected 200, got 400`.
Why: idempotent teardown and retry paths fail — a client re-issuing a delete (after a timeout, or in a teardown that runs twice) receives a client error where AWS reports success, so the whole teardown is treated as broken.
Fix: the DeleteResourceShare handler — treat deletion of a share already in the DELETED state as successful (200 with returnValue), which follows directly from Cause 5's retained record.
Traces: ram-resource-share-lifecycle#teardown#7.

Cause 7 of 8: GetResourceShareAssociations omits ResourceShareAssociation.statusMessage for a PRINCIPAL association in ASSOCIATING state.
AWS: Smithy — ResourceShareAssociation has member `statusMessage` ("A message about the status of the association"). The recording shows AWS populating it for a PRINCIPAL association whose status is ASSOCIATING, explaining that the service is waiting for the principal to accept or reject the resource-share invitation (recorded text is an example of that rule, not a required string).
Emulator: the association is returned with resourceShareArn, resourceShareName, associatedEntity, associationType, status, creationTime, lastUpdatedTime and external, but no statusMessage member at all — `$.resourceShareAssociations[0]: missing (expected {... 'statusMessage': ...}, got {... no statusMessage ...})`.
Why: a client that surfaces or branches on the association's status message (the only field explaining why a cross-account association is still pending) gets nothing, and cannot distinguish an invitation awaiting acceptance from an association with no explanation.
Fix: the association state/serializer for GetResourceShareAssociations — set and emit statusMessage when an association is in a non-terminal state such as ASSOCIATING.
Traces: ram-principal-association-workflow#task#3, ram-principal-association-workflow#task#6.

Cause 8 of 8: the not-found error message for an unknown resource-share ARN does not follow AWS's template.
AWS: Smithy — UnknownResourceException (errorKind client, httpError 400, awsQueryError InvalidResourceShareArn.NotFound) with a required `message`. The recording shows the message built from the request's ARN using one fixed template across four different operations (GetResourceShares, UpdateResourceShare, DeleteResourceShare, AssociateResourceShare): "ResourceShare <requested-arn> could not be found." That the same template is produced by four independent operations is what makes the rule — ARN echoed into a fixed sentence — rather than one recording's text.
Emulator: status 400 and the error code match; only the message differs — `$.message: expected 'ResourceShare <arn> could not be found.', got "no resource share with the ARN '<arn>' exists"`.
Why: a client (or human operator) matching on the documented wording, or parsing the ARN out of the message, mis-handles the error; severity here is low because status and code are correct.
Fix: the shared RAM not-found error constructor used by these handlers — render the message from the requested ARN with AWS's template.
Traces: ram-resource-share-not-found-errors#task#1, ram-resource-share-not-found-errors#task#2, ram-resource-share-not-found-errors#task#3, ram-resource-share-not-found-errors#task#4.

No open issue covers any of these; the two currently open issues are for compute-optimizer-automation and ecr. Hosts, ports, generated share ids/ARNs, tag values and timestamps in the citations above are examples from the recording, not requirements.

## Evidence
Replay of trace set simparity-wire-aws-v1 (15 fixtures, 90 steps, 9 passed, score 10.0%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0) produced 14 divergence clusters, all service=ram, none stub-blocked and none owned by an open issue.

Cause 1 — expected $.resourceShare.resourceShareConfiguration = {'exclusiveAccountAccess': False, 'retainSharingOnAccountLeaveOrganization': False}; actual: member absent (None). Same expectation on GetResourceShares at $.resourceShares[name=...].resourceShareConfiguration; actual None. Status 200 both sides.
Cause 2 — expected $.resourceShare.tags = []; actual: member absent (None). Status 200 both sides.
Cause 3 — expected $.resourceShares[name=...].tags[1] = {'key': 'cost-center', 'value': '42'} after TagResource; actual None (tag list still holds only the original pair). Status 200 both sides.
Cause 4 — after UntagResource, expected $.resourceShares[name=...].tags[0] = {'key': 'cost-center', 'value': '42'}; actual {'key': 'team', 'value': 'platform'} (reported missing+extra at index 0), i.e. the removed key is still present. Status 200 both sides.
Cause 5 — expected $.resourceShares[0] = a full ResourceShare with 'status': 'DELETED' (plus name, resourceShareArn, owningAccountId, featureSet, allowExternalPrincipals, tags, resourceShareConfiguration); actual None — the share is not listed at all. Status 200 both sides.
Cause 6 — DeleteResourceShare: status expected 200, got 400 (divergence_key service=ram|DeleteResourceShare|status=200->400|keys=status).
Cause 7 — expected $.resourceShareAssociations[0] to include 'statusMessage' ('Waiting for the principal to either accept or reject the resource share invitation.') alongside associationType PRINCIPAL / status ASSOCIATING; actual object carries no statusMessage member. Status 200 both sides.
Cause 8 — four clusters, status 400 expected and 400 actual with the matching error code; only $.message differs: expected 'ResourceShare arn:aws:ram:us-west-2:000000000000:resource-share/00000000-0000-0000-0000-000000000000 could not be found.', got "no resource share with the ARN 'arn:aws:ram:...:resource-share/00000000-0000-0000-0000-000000000000' exists" — identical template divergence on GetResourceShares, UpdateResourceShare, DeleteResourceShare and AssociateResourceShare.

Model evidence: smithy_query on services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json — ResourceShare{resourceShareConfiguration, tags, status: ResourceShareStatus[PENDING|ACTIVE|FAILED|DELETING|DELETED], statusMessage}; ResourceShareAssociation{statusMessage}; DeleteResourceShare http code 200; GetResourceShares input resourceShareStatus; TagResource/UntagResource empty outputs; UnknownResourceException httpError 400 / InvalidResourceShareArn.NotFound.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) — all 14 clusters below reproduce.
3. Cause 1: fixture ram-resource-share-tagging step setup#1 (CreateResourceShare) and ram-resource-share-lifecycle step task#2 (GetResourceShares) — inspect resourceShareConfiguration in the response.
4. Cause 2: fixture ram-principal-association-workflow step setup#1 — CreateResourceShare with no tags; inspect $.resourceShare.tags.
5. Cause 3: fixture ram-resource-share-tagging step task#3 — TagResource a share, then GetResourceShares and inspect tags.
6. Cause 4: fixture ram-resource-share-tagging step task#5 — UntagResource a key, then GetResourceShares and inspect tags.
7. Cause 5: fixture ram-resource-share-lifecycle step task#6 — delete a share, then GetResourceShares and look for the DELETED record.
8. Cause 6: fixture ram-resource-share-lifecycle step teardown#7 — DeleteResourceShare against the already-deleted share; observe 400 instead of 200.
9. Cause 7: fixture ram-principal-association-workflow step task#3 — GetResourceShareAssociations with associationType PRINCIPAL while the association is ASSOCIATING; inspect statusMessage.
10. Cause 8: fixture ram-resource-share-not-found-errors steps task#1..task#4 — call GetResourceShares/UpdateResourceShare/DeleteResourceShare/AssociateResourceShare with an unknown resource-share ARN and compare the message against AWS's template.
