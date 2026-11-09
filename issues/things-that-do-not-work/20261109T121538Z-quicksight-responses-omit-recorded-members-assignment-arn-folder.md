Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-iam-policy-assignment-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-iam-policy-assignment-lifecycle quicksight-folder-lifecycle-and-duplicate-rejected quicksight-data-source-lifecycle-create-update-delete
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|DescribeIAMPolicyAssignment|2xx->2xx|$.IAMPolicyAssignment.Arn
Issue-Key: quicksight|UpdateIAMPolicyAssignment|2xx->2xx|$.Arn
Issue-Key: quicksight|UpdateIAMPolicyAssignment|2xx->2xx|$.Identities
Issue-Key: quicksight|DeleteIAMPolicyAssignment|2xx->2xx|$.Arn
Issue-Key: quicksight|DescribeFolder|2xx->2xx|$.Folder.FolderPath
Issue-Key: quicksight|DescribeDataSource|2xx->2xx|$.DataSource.CatalogEnabled
Issue-Key: quicksight|DescribeDataSource|2xx->2xx|$.DataSource.SslProperties
Issue-Key: quicksight|DescribeDataSource|2xx->2xx|$.DataSource.TIPEnabled
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# QuickSight responses omit recorded members: assignment Arn, Folder.FolderPath, data-source SSL/catalog flags

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIAMPolicyAssignment

## Description
Five net-new root causes with one shared fault: the emulator's success bodies are missing members AWS always returns for these resources (and in one case emits an empty collection AWS omits), so a client that reads those members gets null. This bundle has 5 causes; all five must be closed.

Cause 1 of 5 — DescribeIAMPolicyAssignment omits IAMPolicyAssignment.Arn (fixture quicksight-iam-policy-assignment-lifecycle, step task#3; the cluster also covers task#5).
AWS: DescribeIAMPolicyAssignment (Smithy: GET /accounts/{AwsAccountId}/namespaces/{Namespace}/iam-policy-assignments/{AssignmentName}; output IAMPolicyAssignment). The recorded response carries the assignment's ARN inside the IAMPolicyAssignment structure; this model revision declares IAMPolicyAssignment with AwsAccountId, AssignmentId, AssignmentName, PolicyArn, Identities, AssignmentStatus and no Arn, so the member is recorded behavior only. Its template is arn:aws:quicksight::<AwsAccountId>:assignment/<AssignmentName> — fixed text "arn:aws:quicksight::", ":assignment/", an empty region field, and two variable parts taken from the request path labels.
Emulator: returns the assignment without any Arn member. Replay: "$.IAMPolicyAssignment.Arn: missing (expected 'arn:aws:quicksight::000000000000:assignment/<name>', got None)".
Why: a caller that identifies the assignment by ARN (to tag it, reference it from a policy, or compare it against a create response) reads null and cannot construct the identifier itself without knowing QuickSight's region-less assignment ARN form.
Fix: DescribeIAMPolicyAssignment's response serializer / the assignment record in state (services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/describe_iam_policy_assignment.rs) — derive and emit the assignment ARN from the account and assignment name.

Cause 2 of 5 — UpdateIAMPolicyAssignment omits Arn and emits an empty Identities map (same fixture, step task#4).
AWS: UpdateIAMPolicyAssignment (Smithy: PUT .../iam-policy-assignments/{AssignmentName}; output members AssignmentName, AssignmentId, PolicyArn, Identities, AssignmentStatus, RequestId, Status). The recorded response carries the same assignment Arn described in cause 1, and omits Identities entirely when the assignment has no identities — AWS does not serialize an empty IdentityMap.
Emulator: no Arn, and Identities serialized as {}. Replay: "$.Arn: missing (expected 'arn:aws:quicksight::000000000000:assignment/<name>', got None); $.Identities: type_mismatch (expected None, got {})".
Why: as cause 1 for the ARN; additionally a client that distinguishes "no identities declared" (absent) from "identities present" sees a present-but-empty map, which flips `if 'Identities' in response` style checks.
Fix: UpdateIAMPolicyAssignment's response serializer — emit the assignment ARN and suppress the Identities member when the map is empty.

Cause 3 of 5 — DeleteIAMPolicyAssignment omits Arn (same fixture, step task#6).
AWS: DeleteIAMPolicyAssignment (Smithy: DELETE /accounts/{AwsAccountId}/namespace/{Namespace}/iam-policy-assignments/{AssignmentName}; modeled output AssignmentName, RequestId, Status). The recorded delete response also carries the deleted assignment's Arn in the same region-less form (recorded behavior only).
Emulator: returns the delete response without Arn. Replay: "$.Arn: missing (expected 'arn:aws:quicksight::000000000000:assignment/<name>', got None)".
Why: a caller that logs or reconciles on the ARN returned by the delete has nothing to key on.
Fix: DeleteIAMPolicyAssignment's response serializer — emit the ARN of the assignment it removed.

Cause 4 of 5 — DescribeFolder omits Folder.FolderPath (fixture quicksight-folder-lifecycle-and-duplicate-rejected, step task#2).
AWS: DescribeFolder (Smithy: GET /accounts/{AwsAccountId}/folders/{FolderId}; output Folder, whose FolderPath member targets Path, "An array of ancestor ARN strings for the folder"). AWS always serializes FolderPath for a described folder; for a folder with no parent it is the empty list, which is what the recording shows.
Emulator: FolderPath absent from the Folder structure. Replay: "$.Folder.FolderPath: missing (expected [], got None)".
Why: a client walking the folder hierarchy must distinguish "root folder, no ancestors" (empty list) from "unknown"; receiving null forces a null-guard AWS never requires and breaks code that iterates the ancestor list directly.
Fix: DescribeFolder's response serializer / the folder record (ops/describe_folder.rs) — always emit FolderPath, as the ancestor ARN list, empty for a root folder.

Cause 5 of 5 — DescribeDataSource omits SslProperties and the catalog/TIP flags (fixture quicksight-data-source-lifecycle-create-update-delete, step task#5; the cluster also covers task#7).
AWS: DescribeDataSource (Smithy: GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}; output DataSource, which declares SslProperties -> SslProperties{DisableSsl: Boolean, default false}). AWS returns SslProperties for the described data source with DisableSsl defaulted to false when the create did not supply it, and the recording additionally shows two boolean members this model revision does not declare, CatalogEnabled and TIPEnabled, both false by default (recorded behavior only).
Emulator: none of the three members is present. Replay: "$.DataSource.SslProperties: missing (expected {'DisableSsl': False}, got None); $.DataSource.CatalogEnabled: missing (expected False, got None); $.DataSource.TIPEnabled: missing (expected False, got None)".
Why: a client reading the data source's effective SSL setting gets null instead of the service-applied default and cannot tell whether SSL is disabled; the two feature flags likewise read as unknown rather than false.
Fix: DescribeDataSource's response serializer and the data-source record's defaults (ops/describe_data_source.rs and the data-source state struct) — persist and echo SslProperties with its defaulted DisableSsl, and the catalog/TIP flags, for every described data source.

Traces: quicksight-iam-policy-assignment-lifecycle#task#3 (and task#5), #task#4, #task#6; quicksight-folder-lifecycle-and-duplicate-rejected#task#2; quicksight-data-source-lifecycle-create-update-delete#task#5 (and task#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 (all steps returned HTTP 200 as expected; only the bodies diverge):

1) DescribeIAMPolicyAssignment — quicksight-iam-policy-assignment-lifecycle#task#3 (also task#5)
   Expected: IAMPolicyAssignment.Arn = "arn:aws:quicksight::<account>:assignment/<AssignmentName>".
   Actual: member absent.

2) UpdateIAMPolicyAssignment — quicksight-iam-policy-assignment-lifecycle#task#4
   Expected: Arn = "arn:aws:quicksight::<account>:assignment/<AssignmentName>"; no Identities member.
   Actual: Arn absent; Identities = {}.

3) DeleteIAMPolicyAssignment — quicksight-iam-policy-assignment-lifecycle#task#6
   Expected: Arn = "arn:aws:quicksight::<account>:assignment/<AssignmentName>". Actual: absent.

4) DescribeFolder — quicksight-folder-lifecycle-and-duplicate-rejected#task#2
   Expected: Folder.FolderPath = [] (empty ancestor-ARN list). Actual: absent.

5) DescribeDataSource — quicksight-data-source-lifecycle-create-update-delete#task#5 (also task#7)
   Expected: DataSource.SslProperties = {"DisableSsl": false}, DataSource.CatalogEnabled = false, DataSource.TIPEnabled = false.
   Actual: all three absent.

Smithy citations: Folder.FolderPath -> Path (list of ancestor ARN strings, length 1..10); DataSource.SslProperties -> SslProperties{DisableSsl: Boolean @default false}; IAMPolicyAssignment members = AwsAccountId, AssignmentId, AssignmentName, PolicyArn, Identities, AssignmentStatus (no Arn in this revision); UpdateIAMPolicyAssignmentResponse/DeleteIAMPolicyAssignmentResponse likewise declare no Arn. The assignment Arn and the CatalogEnabled/TIPEnabled flags are therefore recorded behavior only; the empty-list FolderPath and defaulted SslProperties are the model's own shapes.

## Steps to Reproduce
1. Replay fixture quicksight-iam-policy-assignment-lifecycle: CreateIAMPolicyAssignment, then DescribeIAMPolicyAssignment(AwsAccountId, Namespace=default, AssignmentName) and read IAMPolicyAssignment.Arn — absent (step task#3).
2. UpdateIAMPolicyAssignment on the same assignment and read Arn and Identities — Arn absent, Identities returned as {} where AWS omits it (step task#4).
3. DeleteIAMPolicyAssignment on the same assignment and read Arn — absent (step task#6).
4. Replay fixture quicksight-folder-lifecycle-and-duplicate-rejected: CreateFolder, then DescribeFolder(AwsAccountId, FolderId) and read Folder.FolderPath — absent where AWS returns [] (step task#2).
5. Replay fixture quicksight-data-source-lifecycle-create-update-delete: CreateDataSource, then DescribeDataSource(AwsAccountId, DataSourceId) and read DataSource.SslProperties / CatalogEnabled / TIPEnabled — all absent where AWS returns {"DisableSsl": false} / false / false (step task#5).
