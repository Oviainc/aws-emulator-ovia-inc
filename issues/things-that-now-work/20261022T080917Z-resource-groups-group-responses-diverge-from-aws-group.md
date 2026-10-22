Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-error-config-query-conflict
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-error-config-query-conflict resource-groups-group-tags-crud resource-groups-tag-query-group-lifecycle resource-groups-error-group-lifecycle resource-groups-generic-group-configuration
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|CreateGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|DeleteGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|DeleteGroup|2xx->4xx|$.Group
Issue-Key: resource-groups|DeleteGroup|2xx->4xx|status
Issue-Key: resource-groups|GetGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|GetGroup|2xx->2xx|$.Group.Description
Issue-Key: resource-groups|DeleteGroup|2xx->2xx|$.Group.Description
Issue-Commit: 41ec0a3fe2217154a05a733932ceed67852d38f9

# resource-groups group responses diverge from AWS: Group.OwnerId omitted, configuration groups and UpdateGroup state missing

> _Note: this issue was identified against the code base at commit `41ec0a3fe2217154a05a733932ceed67852d38f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Bundle of 3 CAUSES in the AWS Resource Groups emulator (service `resource-groups`), all in the group-record / `Group` projection path. They cover 6 divergence clusters (17 replayed steps) over 4 fixtures of trace set simparity-wire-aws-v1. All three must be closed before this issue is submitted.

Cause 1 of 3 — `Group.OwnerId` is never serialized on any Group payload.
AWS: recorded behavior only. The on-disk Smithy model (services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json, shape com.amazonaws.resourcegroups#Group) models GroupArn, Name, Description, Criticality, Owner, DisplayName and ApplicationTag, and does NOT model OwnerId. The recorded AWS responses nonetheless carry `Group.OwnerId` — the 12-digit id of the account that owns the group — on CreateGroup (HTTP 200), GetGroup (200) and DeleteGroup (200); the recording shows it as the emulator's canonical account id. The requirement is that every Group structure reports the owning account id, not that literal value.
Emulator: the member is absent from every Group payload. Replay reports `$.Group.OwnerId: missing (expected '000000000000', got None)` on CreateGroup, on GetGroup and on DeleteGroup, with the HTTP status itself matching (200).
Why: a client that reads `Group.OwnerId` (to confirm which account owns a group, to build an ARN, or to key its own records by account) gets a null from the emulator where AWS always supplies a value, so account-aware logic silently takes the wrong branch instead of failing loudly.
Fix: the shared Group projection — `GroupRecord::to_group()` in services/service-com-amazonaws-resourcegroups-ardi/src/state.rs builds the structure with no owner_id and the generated `types::Group` carries no such member, so the field has to be added to the Group serializer and populated from the emulator's account id for CreateGroup, GetGroup and DeleteGroup alike.
Traces: resource-groups-error-config-query-conflict#setup#1 (CreateGroup, cluster of 3 steps); resource-groups-error-config-query-conflict#teardown#7 (DeleteGroup, cluster of 3); resource-groups-tag-query-group-lifecycle#task#2 (GetGroup); resource-groups-tag-query-group-lifecycle#task#5 (GetGroup); resource-groups-tag-query-group-lifecycle#task#8 (DeleteGroup).

Cause 2 of 3 — a group created with a service `Configuration` never exists, so deleting it answers 404 where AWS answers 200.
AWS: Smithy: CreateGroup input `Configuration` (GroupConfigurationList, @length 0..2) is the documented alternative to `ResourceQuery`; CreateGroup returns 200 with `Group` and `GroupConfiguration`, and the resulting group is an ordinary group — DeleteGroup (POST /delete-group) resolves it by name or group ARN and returns 200 with the deleted `Group` (DeleteGroupOutput$Group); NotFoundException/404 is modelled only for a group that does not exist. The recording shows DeleteGroup on the configuration group's ARN returning 200 with the Group structure.
Emulator: CreateGroup refuses the Configuration form outright with HTTP 501 NotImplemented (`Scaffolded::not_implemented("CreateGroup with a service Configuration")` in src/ops/create_group.rs), so no record is ever stored; the fixture's later DeleteGroup on that group's ARN returns HTTP 404 NotFoundException instead of 200, and `$.Group` is missing from the body entirely.
Why: configuration-backed (application) groups cannot be created, read or torn down at all — a client's create succeeds nowhere and its delete reports the group as nonexistent, so cleanup code either loops or treats a real resource as already gone.
Fix: CreateGroup's handler plus the group record — src/state.rs `GroupRecord` has no configuration field, so the Configuration must be stored on the record and reported back in `GroupConfiguration`, after which the group resolves through `ComAmazonawsResourcegroupsArdiState::lookup` and DeleteGroup answers 200. Hypothesis: the 404 is purely downstream of that 501 refusal rather than a defect in ARN resolution, because another DeleteGroup-by-identifier in the same fixture answered 200. Note the unimplemented CreateGroup-with-Configuration path is tracked by its own auto-filed stub issue; this cause is the separate wire divergence replay reported on DeleteGroup with no stub_path and no owning issue.
Traces: resource-groups-error-config-query-conflict#teardown#8 (DeleteGroup, expected 200 got 404), also resource-groups-generic-group-configuration#teardown#5.

Cause 3 of 3 — an updated group Description is never persisted, so read-after-write returns the stale value with a 200.
AWS: Smithy: UpdateGroup (POST /update-group) is documented as "Updates the description for an existing group", takes `Description` (@length 0..1024) and returns the updated `Group`; a subsequent GetGroup or DeleteGroup must report the new Description.
Emulator: src/ops/update_group.rs is still marked "SCAFFOLDED, NOT IMPLEMENTED" and its handler returns 501, so the stored GroupRecord keeps its create-time description. The following GetGroup and the final DeleteGroup answer 200 but report the old value: `$.Group.Description: value_mismatch (expected 'Updated probe group', got 'Probe tag query group')`.
Why: the mutation is lost silently — the group reads back successfully with its old description, so a client cannot distinguish "update applied" from "update dropped" and any workflow that keys off the description diverges from AWS.
Fix: implement UpdateGroup's handler to mutate the GroupRecord (description, criticality, owner, display_name) in the shared store and return the updated Group; these two steps are the wire evidence that the state change is missing, and they diverge on Description in addition to the OwnerId gap of Cause 1.
Traces: resource-groups-tag-query-group-lifecycle#task#5 (GetGroup) and resource-groups-tag-query-group-lifecycle#task#8 (DeleteGroup).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0; 41 steps, 4 passed, score 9.8%).

Cause 1 — CreateGroup resource-groups-error-config-query-conflict#setup#1: expected HTTP 200 with $.Group.OwnerId='000000000000', actual HTTP 200 with $.Group.OwnerId absent (got None). Same key missing on DeleteGroup (resource-groups-error-config-query-conflict#teardown#7, +2 affected steps), GetGroup (resource-groups-tag-query-group-lifecycle#task#2, #task#5) and DeleteGroup (resource-groups-tag-query-group-lifecycle#task#8). Model shape com.amazonaws.resourcegroups#Group models no OwnerId member; GroupRecord::to_group() in src/state.rs emits none.

Cause 2 — DeleteGroup resource-groups-error-config-query-conflict#teardown#8: expected HTTP 200 with $.Group={GroupArn: <configuration group ARN>, Name: <probe configuration group>, OwnerId: '000000000000'}; actual HTTP 404 with $.Group missing. Same cluster: resource-groups-generic-group-configuration#teardown#5. Upstream: src/ops/create_group.rs returns 501 NotImplemented for CreateGroup when `Configuration` is non-empty, so the group is never stored.

Cause 3 — GetGroup resource-groups-tag-query-group-lifecycle#task#5 and DeleteGroup #task#8: expected $.Group.Description='Updated probe group', actual 'Probe tag query group' (the create-time value), both with HTTP 200. Upstream: src/ops/update_group.rs is a SCAFFOLDED 501 stub, so the update never reaches the store.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) and read the resource-groups divergence clusters.
3. Cause 1: replay fixture resource-groups-error-config-query-conflict step setup#1 — CreateGroup with a Name, Description and a TAG_FILTERS_1_0 ResourceQuery — and inspect the 200 body: `Group` carries GroupArn and Name but no OwnerId. Repeat with GetGroup (resource-groups-tag-query-group-lifecycle#task#2) and DeleteGroup (resource-groups-error-config-query-conflict#teardown#7): the member is absent from every Group payload.
4. Cause 2: replay resource-groups-error-config-query-conflict through teardown#8 (or resource-groups-generic-group-configuration through teardown#5). CreateGroup with a non-empty `Configuration` answers 501, and the teardown DeleteGroup on that group's ARN then answers 404 NotFoundException instead of 200 with the deleted Group.
5. Cause 3: replay resource-groups-tag-query-group-lifecycle in order — CreateGroup (Description 'Probe tag query group'), UpdateGroup (Description 'Updated probe group'), then GetGroup at task#5 and DeleteGroup at task#8. Both answer 200 and report the original description; UpdateGroup itself answers 501.
