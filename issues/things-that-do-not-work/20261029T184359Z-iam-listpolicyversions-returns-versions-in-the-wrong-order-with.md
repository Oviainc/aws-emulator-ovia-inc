Issue-Source: trace-sweep-divergence
Issue-Trace: iam-err-delete-policy-blocked-by-versions
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-err-delete-policy-blocked-by-versions iam-list-policy-versions-default-and-error-contract iam-managed-policy-versioning-and-tags iam-managed-policy-lifecycle iam-pg-list-policy-versions-marker-walk
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListPolicyVersions|2xx->2xx|$.ListPolicyVersionsResponse.ListPolicyVersionsResult.Versions.member[*].IsDefaultVersion
Issue-Key: iam|ListPolicyVersions|2xx->2xx|$.ListPolicyVersionsResponse.ListPolicyVersionsResult.Versions.member[*].VersionId
Issue-Key: iam|ListPolicyVersions|2xx->2xx|$.ListPolicyVersionsResponse.ListPolicyVersionsResult.Versions.member.IsDefaultVersion
Issue-Key: iam|ListPolicyVersions|2xx->2xx|$.ListPolicyVersionsResponse.ListPolicyVersionsResult.Versions.member.VersionId
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# ListPolicyVersions returns versions in the wrong order with wrong IsDefaultVersion

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicyVersions

## Description
Cause 1 of 1: ListPolicyVersions returns a managed policy's versions oldest-first and attributes IsDefaultVersion to the wrong version.

AWS: the operation returns the policy's versions newest-first - the most recently created version is member[0] and v1 is last - and IsDefaultVersion is 'true' on exactly the version SetDefaultPolicyVersion last selected, 'false' on all others (Smithy: PolicyVersion.VersionId pattern ^v[1-9][0-9]*..., 'When a policy is created, the first policy version is v1'; PolicyVersion.IsDefaultVersion default false; the operation is paginated by Marker/MaxItems, so the order is also the page order). Creating a new version does not by itself move the default - after CreatePolicyVersion without SetAsDefault the default stays where it was.

Emulator: the list is emitted oldest-first (member[0] is v1) and IsDefaultVersion tracks position rather than the stored default - it is set on the version the emulator happens to order last/first instead of the selected default. Under pagination the same ordering defect shifts every page: a MaxItems=1 walk that AWS answers v5, v4, v3... the emulator answers v1, v2, v3..., so a caller walking markers never reaches the newest version where it expects it and the fixture's poll for v5 on page one exhausts its attempts.

Why: a client that reads the current policy document by taking member[0], or that finds the default by scanning for IsDefaultVersion='true', gets the wrong version - it edits or audits a stale policy body. A paginating client reads the pages in the opposite order from AWS.

Fix: the ListPolicyVersions handler - order the version list by descending creation (newest first) and derive IsDefaultVersion from the policy's stored default-version id rather than from list position.

Traces: iam-err-delete-policy-blocked-by-versions#task#4 (two-version policy: order and both flags inverted); iam-managed-policy-lifecycle#task#9 (five versions: the flag lands on the wrong member); iam-pg-list-policy-versions-marker-walk#task#6, #task#7, #task#8 (MaxItems=1 marker walk: each page returns the wrong version).

## Evidence
Expected (recorded AWS, two-version policy): Versions/member[0]{VersionId 'v2', IsDefaultVersion 'false'}, member[1]{VersionId 'v1', IsDefaultVersion 'true'}. Actual: member[0]{VersionId 'v1', IsDefaultVersion 'true'}, member[1]{VersionId 'v2', IsDefaultVersion 'false'} - both the order and the default flag inverted.
Five-version policy (iam-managed-policy-lifecycle#task#9): expected member[0].IsDefaultVersion 'true' and member[2].IsDefaultVersion 'false'; actual member[0] 'false' and member[2] 'true'.
Marker walk with MaxItems=1 (iam-pg-list-policy-versions-marker-walk): page 1 expected VersionId 'v5' with IsDefaultVersion 'true', actual 'v1'/'false' (and the fixture's poll reports "not in ['v5'] after 29 attempt(s)"); page 2 expected 'v4', actual 'v2'; page 3 expected member[1] 'v2' and member[2] 'v1' with IsDefaultVersion 'false', actual 'v4' and 'v5' with 'true'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-err-delete-policy-blocked-by-versions: create a policy, CreatePolicyVersion once, then task#4 ListPolicyVersions - the versions come back v1 then v2 and IsDefaultVersion is on v1.
3. Replay iam-pg-list-policy-versions-marker-walk#task#6..#task#8 with MaxItems=1 - each page returns the opposite version from the recording.
