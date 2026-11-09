Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-folder-lifecycle-and-duplicate-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-folder-lifecycle-and-duplicate-rejected quicksight-group-lifecycle-and-duplicate-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|DescribeFolder|2xx->2xx|$.Folder.Name
Issue-Key: quicksight|DescribeGroup|2xx->2xx|$.Group.Description
Issue-Key: quicksight|DescribeGroup|2xx->2xx|$.Group.PrincipalId
Issue-Related: 20261109T121538Z-quicksight-responses-omit-recorded-members-assignment-arn-folder.md
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# QuickSight Describe returns the pre-update folder/group record and a synthetic group PrincipalId

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFolder

## Description
Three net-new root causes with one shared fault: the record the emulator serves from state is not the record AWS would have — two update operations report success without persisting the changed field, and the group's PrincipalId is a generated opaque id rather than QuickSight's derived principal identifier. This bundle has 3 causes; all three must be closed.

Cause 1 of 3 — UpdateFolder does not persist the new folder Name (fixture quicksight-folder-lifecycle-and-duplicate-rejected, step task#4).
AWS: UpdateFolder followed by DescribeFolder (Smithy: GET /accounts/{AwsAccountId}/folders/{FolderId}; output Folder, whose Name targets FolderName, length 1..200, "A display name for the folder"). An accepted UpdateFolder is read-your-write: the next DescribeFolder returns Folder.Name equal to the Name the update supplied, and Folder.LastUpdatedTime advances. The recorded describe shows the updated name.
Emulator: the update returns success, but DescribeFolder still reports the name the folder was created with. Replay: "$.Folder.Name: value_mismatch (expected '<created name>-upd', got '<created name>')" — the "-upd" suffix is only how this fixture distinguishes the two names; the requirement is that the describe reflects whatever Name the update sent. (The same step also shows Folder.FolderPath missing, which is filed as a separate response-member-omission bundle.)
Why: an update silently does nothing while reporting success, so a client's rename is lost; any workflow that updates then reads back sees stale data and may retry forever or write downstream records under the wrong display name.
Fix: UpdateFolder's state mutation (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/update_folder.rs and the folder store) — apply the request's Name to the stored folder record (and advance LastUpdatedTime) so the describe path serves it.

Cause 2 of 3 — UpdateGroup does not persist the new group Description (fixture quicksight-group-lifecycle-and-duplicate-rejected, step task#4).
AWS: UpdateGroup followed by DescribeGroup (Smithy: GET /accounts/{AwsAccountId}/namespaces/{Namespace}/groups/{GroupName}; output Group, whose Description targets GroupDescription, length 1..512, "The group description"). An accepted UpdateGroup is read-your-write: the next DescribeGroup returns the Description the update supplied.
Emulator: DescribeGroup still returns the creation-time Description after a successful UpdateGroup. Replay: "$.Group.Description: value_mismatch (expected '<created description>-upd', got '<created description>')" — again the suffix is only the fixture's marker for "the value the update sent".
Why: same as cause 1 — a successful update that does not take effect; group metadata edits are lost and read-after-write checks diverge from AWS.
Fix: UpdateGroup's state mutation (ops/update_group.rs and the group store) — apply the request's Description to the stored group record.

Cause 3 of 3 — Group.PrincipalId is a generated opaque id, not QuickSight's derived principal identifier (fixture quicksight-group-lifecycle-and-duplicate-rejected, step task#2; the same mismatch recurs in the task#4 response).
AWS: DescribeGroup / CreateGroup output Group.PrincipalId (Smithy: Group.PrincipalId -> String, "The principal ID of the group"). The model does not constrain its form, so this is recorded behavior only: QuickSight returns a stable identifier derived from the group's identity in the namespace (of the form group/<Namespace>/<GroupName>), the same value for every read of that group, and the fixture asserts its shape rather than a literal — the recording's own value is not the requirement.
Emulator: returns a sequentially generated UUID from its id counter, which fails that shape assertion. Replay: "$.Group.PrincipalId: value_mismatch (expected '<shaped-not-asserted>', got '00000000-0000-4000-8000-000000000002')". The specific counter value is per-run; the divergence is that the value is a synthetic UUID instead of a namespace/group-derived principal id.
Why: a client that parses PrincipalId to recover the group's namespace and name, or that correlates a principal id across responses and services, gets an identifier with no relation to the group — and a UUID drawn from a shared counter is not stable against unrelated emulator activity.
Fix: the group record's principal-id derivation in state / the Group serializer shared by CreateGroup, DescribeGroup, UpdateGroup and ListGroups (ops/create_group.rs and the group store) — derive PrincipalId from the namespace and group name instead of allocating from the generic id counter.

Traces: quicksight-folder-lifecycle-and-duplicate-rejected#task#4, quicksight-group-lifecycle-and-duplicate-rejected#task#4, quicksight-group-lifecycle-and-duplicate-rejected#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (all three steps returned HTTP 200 as expected; the served records diverge):

1) DescribeFolder after UpdateFolder — quicksight-folder-lifecycle-and-duplicate-rejected#task#4
   Expected: Folder.Name = the Name supplied to UpdateFolder (fixture used "<id>-upd").
   Actual: Folder.Name = the creation-time name. (Folder.FolderPath also missing — covered by the response-member-omission bundle.)

2) DescribeGroup after UpdateGroup — quicksight-group-lifecycle-and-duplicate-rejected#task#4
   Expected: Group.Description = the Description supplied to UpdateGroup (fixture used "sim2real-probe-upd").
   Actual: Group.Description = the creation-time description ("sim2real-probe").

3) DescribeGroup after CreateGroup — quicksight-group-lifecycle-and-duplicate-rejected#task#2
   Expected: Group.PrincipalId of QuickSight's derived principal form (asserted by shape, not literal value).
   Actual: Group.PrincipalId = a counter-allocated UUID ("00000000-0000-4000-8000-000000000002" in this run).

Smithy citations: Folder.Name -> FolderName (length 1..200, display name for the folder); Group.Description -> GroupDescription (length 1..512); Group.PrincipalId -> String ("The principal ID of the group"), unconstrained — its form is recorded behavior only. The read-after-write requirement is the service contract for the Update* operations, which return success only when the record was changed.

## Steps to Reproduce
1. Replay fixture quicksight-folder-lifecycle-and-duplicate-rejected: CreateFolder(AwsAccountId, FolderId, Name=<n>), UpdateFolder(AwsAccountId, FolderId, Name=<n2>), then DescribeFolder(AwsAccountId, FolderId) and read Folder.Name — the emulator still returns <n> (step task#4).
2. Replay fixture quicksight-group-lifecycle-and-duplicate-rejected: CreateGroup(AwsAccountId, Namespace=default, GroupName, Description=<d>), UpdateGroup(..., Description=<d2>), then DescribeGroup(...) and read Group.Description — the emulator still returns <d> (step task#4).
3. In the same fixture, DescribeGroup right after the create and read Group.PrincipalId — the emulator returns a counter-allocated UUID instead of QuickSight's namespace/group-derived principal id (step task#2).
