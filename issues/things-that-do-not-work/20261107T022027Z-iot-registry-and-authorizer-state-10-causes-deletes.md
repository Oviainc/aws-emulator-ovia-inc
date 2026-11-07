Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorizer-active-delete-conflict-then-recover
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorizer-active-delete-conflict-then-recover iot-authorizer-error-paths iot-authorizer-lifecycle iot-custom-authorizer-lifecycle iot-mitigation-action-lifecycle-thing-group iot-mitigation-action-thing-group-quarantine-lifecycle iot-thing-group-hierarchy iot-thing-group-hierarchy-and-duplicate-error iot-thing-and-group-lifecycle iot-register-thing-with-certificate iot-vpce-traj-rollback-order-refusals
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DeleteAuthorizer|4xx->2xx|$
Issue-Key: iot|DeleteAuthorizer|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteAuthorizer|4xx->2xx|status
Issue-Key: iot|DescribeAuthorizer|2xx->4xx|$.authorizerDescription
Issue-Key: iot|DescribeAuthorizer|2xx->4xx|status
Issue-Key: iot|DeleteAuthorizer|2xx->4xx|status
Issue-Key: iot|DescribeAuthorizer|2xx->2xx|$.authorizerDescription.status
Issue-Key: iot|DescribeThingGroup|4xx->2xx|status
Issue-Key: iot|DescribeThingGroup|4xx->2xx|$.message
Issue-Key: iot|DescribeThingGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeThingGroup|4xx->2xx|$
Issue-Key: iot|DescribeThingGroup|2xx->2xx|$.thingGroupProperties.thingGroupDescription
Issue-Key: iot|DescribeThingGroup|2xx->2xx|$.version
Issue-Key: iot|DescribeThing|2xx->4xx|$.attributes
Issue-Key: iot|DescribeThing|2xx->4xx|$.defaultClientId
Issue-Key: iot|DescribeThing|2xx->4xx|$.thingArn
Issue-Key: iot|DescribeThing|2xx->4xx|$.thingId
Issue-Key: iot|DescribeThing|2xx->4xx|$.thingName
Issue-Key: iot|DescribeThing|2xx->4xx|$.version
Issue-Key: iot|DescribeThing|2xx->4xx|status
Issue-Key: iot|DeleteThing|4xx->2xx|$
Issue-Key: iot|DeleteThing|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteThing|4xx->2xx|status
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot registry and authorizer state: 10 causes — deletes leave resources readable, updates never apply, delete-gating absent

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAuthorizer

## Description
The emulator's IoT control-plane state does not track mutations the way AWS does: a DELETE that AWS refuses succeeds and destroys the resource, a DELETE that AWS performs leaves the resource readable, a status/property UPDATE is accepted but never applied, and a resource created through a provisioning path is never readable. The common fault is that these handlers acknowledge the request without transacting it against state (or without the precondition AWS checks first). 10 net-new causes, all in service iot.

Cause 1 of 10: DeleteAuthorizer does not refuse an authorizer in ACTIVE status.
AWS: Smithy: DeleteAuthorizer declares InvalidRequestException (httpError 400) and DeleteConflictException (409) among its errors. Recorded behavior: deleting an ACTIVE authorizer is refused with 400, x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', and the message 'Cannot delete authorizer ' + <authorizerName> + ' in ACTIVE status. Update status to INACTIVE and delete.' — fixed text 'Cannot delete authorizer ', ' in ACTIVE status. Update status to INACTIVE and delete.'; variable part the authorizer name.
Emulator: 200 with an empty body — the authorizer is deleted.
Why: this is a destructive divergence, not a message one: a caller that relies on AWS refusing the delete of a live authorizer loses it, and a caller testing the INACTIVE-then-delete sequence sees step order it must not rely on.
Fix: the DeleteAuthorizer handler's precondition check against the authorizer's status in state.
Traces: iot-authorizer-active-delete-conflict-then-recover#task#3 (also iot-authorizer-error-paths#task#6).

Cause 2 of 10: DescribeAuthorizer cannot read the authorizer the previous step should not have deleted.
AWS: Smithy: DescribeAuthorizer output member authorizerDescription targets AuthorizerDescription. Recorded behavior: 200 with authorizerDescription populated (authorizerArn, authorizerFunctionArn, authorizerName, creationDate, lastModifiedDate, enableCachingForHttp, signingDisabled, status ACTIVE, tokenKeyName, tokenSigningPublicKeys).
Emulator: 404, authorizerDescription absent.
Why: the downstream read of cause 1 — its own wire divergence (wrong status, missing output shape) and the step that confirms the authorizer survived.
Fix: follows from cause 1; verify under the DeleteAuthorizer precondition.
Traces: iot-authorizer-active-delete-conflict-then-recover#task#4 (also #task#6).

Cause 3 of 10: the subsequent legitimate DeleteAuthorizer then 404s.
AWS: recorded behavior: 200 — after the status is moved to INACTIVE the delete succeeds.
Emulator: 404 — there is nothing left to delete because cause 1 already removed it.
Why: closes the cascade; a client's teardown reports failure for work it performed correctly.
Fix: follows from cause 1.
Traces: iot-authorizer-active-delete-conflict-then-recover#teardown#7 (also iot-authorizer-error-paths#teardown#8).

Cause 4 of 10: an authorizer status update to INACTIVE is accepted but the read still reports ACTIVE.
AWS: Smithy: AuthorizerDescription.status targets AuthorizerStatus (enum ACTIVE/INACTIVE); CreateAuthorizer/UpdateAuthorizer take a status member. Recorded behavior: after the status is set to INACTIVE, DescribeAuthorizer reports authorizerDescription.status = 'INACTIVE'.
Emulator: DescribeAuthorizer still reports 'ACTIVE'.
Why: the status field is the gate on cause 1's delete rule and on whether the authorizer is used for connections; a write that is acknowledged and then not reflected means no client can ever observe or rely on the authorizer's lifecycle state.
Fix: the authorizer state's status mutation (the update handler must persist status, and DescribeAuthorizer must read the persisted value rather than the creation-time one).
Traces: iot-authorizer-lifecycle#task#6 (also iot-custom-authorizer-lifecycle#task#6).

Cause 5 of 10: DescribeThingGroup still answers 200 for a deleted thing group.
AWS: Smithy: DescribeThingGroup declares ResourceNotFoundException (404). Recorded behavior: after DeleteThingGroup, DescribeThingGroup answers 404.
Emulator: 200 — the group is still readable.
Why: a deleted group remaining readable makes teardown unverifiable and lets a client re-attach things to a group AWS considers gone.
Fix: the DeleteThingGroup handler must remove the group from the thing-group state the describe path reads.
Traces: iot-mitigation-action-lifecycle-thing-group#teardown#8.

Cause 6 of 10: the same deleted-group read, with the not-found body also absent.
AWS: recorded behavior: 404 whose message is the account-qualified 'ThingGroup ' + <accountId> + ':' + <thingGroupName> + ' not found.' plus the x-amzn-errortype header.
Emulator: 200, no message member, no errortype header.
Why: distinct fixture family (quarantine/hierarchy groups) confirming the delete is not applied for nested groups either, and recording that the refusal body is entirely missing rather than merely reworded.
Fix: as cause 5, in the thing-group delete path.
Traces: iot-mitigation-action-thing-group-quarantine-lifecycle#teardown#8 (also iot-thing-group-hierarchy#teardown#5).

Cause 7 of 10: a deleted CHILD thing group remains readable even after a 30-attempt poll.
AWS: recorded behavior: 404 for the deleted child group.
Emulator: 200 for 30 consecutive polls, returning the child group with its full body including thingGroupMetadata.parentGroupName and rootToParentThingGroups.
Why: proves the divergence is not eventual-consistency lag — the delete never lands at all — and that the parent/child linkage keeps a deleted child alive in the hierarchy a client walks.
Fix: as cause 5, including the hierarchy index the describe path consults.
Traces: iot-thing-group-hierarchy-and-duplicate-error#teardown#6.

Cause 8 of 10: a thing-group property update is accepted but neither the description nor the version changes.
AWS: Smithy: DescribeThingGroup output carries thingGroupProperties and version (Version, long, default 0). Recorded behavior: after an update that sets a new thingGroupDescription, DescribeThingGroup reports that description and version 2 — the version counter increments on every accepted property update.
Emulator: thingGroupProperties.thingGroupDescription absent and version still 1.
Why: version is the optimistic-concurrency token for the registry (DeleteThing/UpdateThingGroup take expectedVersion); a counter that never advances makes conditional updates meaningless, and the dropped description means the write was silently discarded.
Fix: the thing-group update handler must persist thingGroupProperties and increment the stored version.
Traces: iot-thing-and-group-lifecycle#task#14.

Cause 9 of 10: a thing registered through the certificate-provisioning path is not readable.
AWS: Smithy: DescribeThing returns thingName, thingArn, thingId, attributes, defaultClientId, version. Recorded behavior: 200 with attributes {}, defaultClientId equal to the thing name, thingArn, thingId and version 1.
Emulator: 404 with every member absent.
Why: the provisioning path acknowledges registration but creates no registry entry, so nothing provisioned by certificate is ever addressable afterwards.
Fix: the RegisterThing/provisioning handler must write the thing into the registry state DescribeThing reads.
Traces: iot-register-thing-with-certificate#task#3.

Cause 10 of 10: DeleteThing succeeds for a thing that still has attached principals.
AWS: Smithy: DeleteThing declares InvalidRequestException (400) and VersionConflictException (409). Recorded behavior: 400 whose message is 'Cannot delete. Thing ' + <thingName> + ' is still attached to one or more principals' — fixed text 'Cannot delete. Thing ' and ' is still attached to one or more principals'.
Emulator: 200 with an empty body — the thing is deleted with its principal attachments in place.
Why: AWS makes detach-before-delete mandatory; an emulator that deletes anyway lets a client build a teardown order AWS will reject, and leaves certificate-to-thing attachments dangling.
Fix: the DeleteThing handler's precondition — check attached principals before removing the thing.
Traces: iot-register-thing-with-certificate#teardown#4 (also iot-vpce-traj-rollback-order-refusals#task#6).

Related open issues that do NOT cover these: '20261106T201842Z-iot-updateauthorizer-stub.md', '20261106T201842Z-iot-updatethinggroup-stub.md' and '20261106T201842Z-iot-registry-plane-9-net-new-causes-creatething.md' carry no replay key for any of the ten fixture steps above; the replay reports no stub blocking these steps.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

1. DeleteAuthorizer, iot-authorizer-active-delete-conflict-then-recover#task#3 — status expected 400, got 200; body expected {'message': 'Cannot delete authorizer sim2real-probe-iot-authorizer-3e3f59f9 in ACTIVE status. Update status to INACTIVE and delete.'}, got {}; x-amzn-errortype expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got absent. Cluster of 2.
2. DescribeAuthorizer, iot-authorizer-active-delete-conflict-then-recover#task#4 — status expected 200, got 404; $.authorizerDescription expected populated (status ACTIVE, signingDisabled true, enableCachingForHttp false, ...), got absent. Cluster of 2.
3. DeleteAuthorizer, iot-authorizer-active-delete-conflict-then-recover#teardown#7 — status expected 200, got 404. Cluster of 2.
4. DescribeAuthorizer, iot-authorizer-lifecycle#task#6 — $.authorizerDescription.status expected 'INACTIVE', got 'ACTIVE'. Cluster of 2.
5. DescribeThingGroup, iot-mitigation-action-lifecycle-thing-group#teardown#8 — status expected 404, got 200.
6. DescribeThingGroup, iot-mitigation-action-thing-group-quarantine-lifecycle#teardown#8 — status expected 404, got 200; $.message expected 'ThingGroup 000000000000:sim2real-probe-iot-mit-quarantine-group-c58ff862 not found.', got absent; x-amzn-errortype expected present, got absent. Cluster of 2.
7. DescribeThingGroup, iot-thing-group-hierarchy-and-duplicate-error#teardown#6 — status expected 404, got 200 after 30 poll attempts; body returned the child group with thingGroupMetadata.parentGroupName set to the parent group.
8. DescribeThingGroup, iot-thing-and-group-lifecycle#task#14 — $.thingGroupProperties.thingGroupDescription expected 'updated description', got absent; $.version expected 2, got 1.
9. DescribeThing, iot-register-thing-with-certificate#task#3 — status expected 200, got 404; attributes, defaultClientId, thingArn, thingId, thingName, version all expected present, got absent.
10. DeleteThing, iot-register-thing-with-certificate#teardown#4 — status expected 400, got 200; body expected {'message': 'Cannot delete. Thing sim2real-probe-iot-provthing-cc3e64e2 is still attached to one or more principals'}, got {}; x-amzn-errortype expected present, got absent. Cluster of 2.

(Authorizer/thing/group names, account id and timestamps above are this recording's values; the requirements are the fixed message texts with request-derived names, the status codes, and the state rules described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the ten clusters listed under Evidence, starting with fixture iot-authorizer-active-delete-conflict-then-recover step task#3.
4. Equivalently by hand: create an authorizer with status ACTIVE then DELETE /authorizer/<n> (expect 400) and GET /authorizer/<n> (expect 200); set its status INACTIVE and GET again (expect status INACTIVE); create a thing group, DELETE it, then GET /thing-groups/<g> (expect 404); update a thing group's description and GET it (expect the new description and version 2); register a thing via the certificate-provisioning path and GET /things/<t> (expect 200); attach a principal to a thing and DELETE /things/<t> (expect 400).
