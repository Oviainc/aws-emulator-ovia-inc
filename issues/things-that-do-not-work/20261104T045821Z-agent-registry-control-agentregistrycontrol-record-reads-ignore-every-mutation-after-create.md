Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-control-registry-record-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-control-registry-record-lifecycle agent-registry-control-registry-record-approval-workflow
Issue-Shape-Id: com.amazonaws.agentregistrycontrol#AgentRegistryControl
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.displayName
Issue-Key: agent-registry-control|ListRegistryRecords|2xx->2xx|$.registryRecords[*].createdAt
Issue-Key: agent-registry-control|ListRegistryRecords|2xx->2xx|$.registryRecords[*].displayName
Issue-Key: agent-registry-control|ListRegistryRecords|2xx->2xx|$.registryRecords[*].updatedAt
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.status
Issue-Key: agent-registry-control|GetRegistryRecord|2xx->2xx|$.statusReason
Issue-Related: 20261104T045821Z-agent-registry-control-agentregistrycontrol-serializes-date-time-timestamps-as-epoch-numbers.md
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# AgentRegistryControl record reads ignore every mutation after create

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRegistryRecord

## Description
Registry-record reads return the record exactly as it was created: field updates and status transitions accepted earlier in the same fixture are absent from every later GetRegistryRecord/ListRegistryRecords response. This bundle has 4 causes and all 4 must be closed.

Cause 1 of 4 - UpdateRegistryRecord's displayName is not persisted, GetRegistryRecord (fixture agent-registry-control-registry-record-lifecycle, step task#6).
AWS: Smithy: UpdateRegistryRecord PATCH /registries/{registryId}/records/{recordId}, code 202, documented as PATCH semantics ("specify only the fields you want" to change) with an asynchronous apply; GetRegistryRecordResponse.displayName -> RegistryRecordDisplayName @length 1..255. After the update settles, a read returns the new displayName.
Emulator: GetRegistryRecord answers 200 but $.displayName is ABSENT, i.e. the record still carries no display name - the PATCH was acknowledged and discarded. (The same response also carries the numeric-timestamp defect filed separately.)
Why: a client that writes a field and reads it back sees its own write vanish, so any read-after-write workflow - console rendering, reconciliation loops, update-then-verify tests - concludes the resource does not hold the value it just set.
Fix: the UpdateRegistryRecord handler's state write in the agentregistrycontrol service crate's ops module - apply the PATCH members to the stored record (and bump updatedAt) rather than returning 202 without mutating state.
Traces: agent-registry-control-registry-record-lifecycle#task#6.

Cause 2 of 4 - the same lost update is visible through the list path, ListRegistryRecords (fixture agent-registry-control-registry-record-lifecycle, step task#7).
AWS: Smithy: ListRegistryRecords POST /registries/{registryId}/records-list returns record summaries that reflect the stored record, so registryRecords[i].displayName carries the updated value.
Emulator: $.registryRecords[0].displayName is missing from the 200 body, confirming the loss is in stored state rather than in one read path's projection; a fix limited to GetRegistryRecord would leave this step failing.
Why: list-based clients (pagination, inventory sync) never observe updated record attributes.
Fix: same stored-record write as Cause 1; verify the list projection reads the mutated record.
Traces: agent-registry-control-registry-record-lifecycle#task#7.

Cause 3 of 4 - a submitted record never reaches PENDING_APPROVAL (fixture agent-registry-control-registry-record-approval-workflow, step task#6).
AWS: Smithy: SubmitRegistryRecordForApproval POST /registries/{registryId}/records/{recordId}/submit-for-approval, code 202, documented as moving a DRAFT record into the registry's approval workflow; RegistryRecordStatus enum includes DRAFT and PENDING_APPROVAL. A read after the submit settles reports status PENDING_APPROVAL.
Emulator: GetRegistryRecord keeps reporting status DRAFT; the fixture's poll gave up after 10 attempts still seeing DRAFT.
Why: the approval workflow is unobservable - a client waiting for PENDING_APPROVAL polls forever, and any approval-gated automation stalls.
Fix: the SubmitRegistryRecordForApproval handler's state transition in the agentregistrycontrol service crate's ops module. Note the open issue 20261104T021320Z-agent-registry-control-submitregistryrecordforapproval-stub.md records that operation as an unimplemented scaffold; THIS cause is the wire-visible consequence on the read path and is not covered by that issue's replay key, so re-replay this fixture step after implementing the handler.
Traces: agent-registry-control-registry-record-approval-workflow#task#6.

Cause 4 of 4 - UpdateRegistryRecordStatus does not move the record to APPROVED or record statusReason (fixture agent-registry-control-registry-record-approval-workflow, step task#8).
AWS: Smithy: UpdateRegistryRecordStatus PATCH /registries/{registryId}/records/{recordId}/status, code 202, documented as updating a record's status in the curation workflow; RegistryRecordStatus includes APPROVED, and GetRegistryRecordResponse.statusReason (String) carries the reason supplied with the transition. A read after the transition reports status APPROVED and echoes the caller-supplied statusReason.
Emulator: GetRegistryRecord still reports status DRAFT and omits $.statusReason entirely; the poll for APPROVED exhausted 10 attempts.
Why: a curation client cannot tell an approved record from a draft one, and the human-readable reason it attached to the decision is lost.
Fix: the UpdateRegistryRecordStatus handler's state transition in the agentregistrycontrol service crate's ops module, plus the statusReason field on the stored record. The open issue 20261104T021320Z-agent-registry-control-updateregistryrecordstatus-stub.md records that operation as an unimplemented scaffold; this cause is the read-path consequence and carries its own replay key.
Traces: agent-registry-control-registry-record-approval-workflow#task#8.

## Evidence
Replay of simparity-wire-aws-v1 - all four steps return the expected HTTP status (200) with wrong record content:

1) GetRegistryRecord, agent-registry-control-registry-record-lifecycle#task#6: $.displayName: missing (expected 'sim2real display name', got None).
2) ListRegistryRecords, agent-registry-control-registry-record-lifecycle#task#7: $.registryRecords[0].displayName: missing (expected 'sim2real display name', got None).
3) GetRegistryRecord, agent-registry-control-registry-record-approval-workflow#task#6: $.status: value_mismatch (expected 'PENDING_APPROVAL', got 'DRAFT'); poll: not in ['PENDING_APPROVAL'] after 10 attempt(s).
4) GetRegistryRecord, agent-registry-control-registry-record-approval-workflow#task#8: $.status: value_mismatch (expected 'APPROVED', got 'DRAFT'); $.statusReason: missing (expected 'sim2real wire approval', got None); poll: not in ['APPROVED'] after 10 attempt(s).

The literals 'sim2real display name' and 'sim2real wire approval' are this run's request values; the requirement is that the read echoes whatever displayName/statusReason the caller supplied. Clusters 1, 2, 3 and 4 additionally report $.createdAt/$.updatedAt numeric-timestamp mismatches, filed as the separate date-time serialization issue.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Causes 1-2: CreateRegistry, CreateRegistryRecord, then PATCH /registries/{registryId}/records/{recordId} setting displayName; then GET the record (task#6) and POST /registries/{registryId}/records-list (task#7) - displayName is absent from both.
3. Cause 3: POST .../records/{recordId}/submit-for-approval, then poll GET on the record (agent-registry-control-registry-record-approval-workflow, task#6) - status stays DRAFT instead of becoming PENDING_APPROVAL.
4. Cause 4: PATCH .../records/{recordId}/status to APPROVED with a statusReason, then poll GET on the record (same fixture, task#8) - status stays DRAFT and statusReason is absent.
