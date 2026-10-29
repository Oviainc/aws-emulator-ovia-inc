Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-policy-catalog-browsing
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-policy-catalog-browsing mpa-policy-version-lookup-not-found-and-default-flag
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|ListPolicyVersions|2xx->2xx|$.PolicyVersions[Name="AWS Backup LAG Vault Protection Policy"]
Issue-Key: mpa|GetPolicyVersion|2xx->2xx|$.PolicyVersion.CreationTime
Issue-Key: mpa|GetPolicyVersion|2xx->2xx|$.PolicyVersion.Document
Issue-Key: mpa|GetPolicyVersion|2xx->2xx|$.PolicyVersion.LastUpdatedTime
Issue-Key: mpa|GetPolicyVersion|2xx->2xx|$.PolicyVersion.Name
Issue-Key: mpa|ListPolicyVersions|2xx->2xx|$.PolicyVersions[Arn="arn
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# mpa policy catalog answers diverge: missing AWS-managed entry, ARN-segment Name, synthesized Document, epoch-number timestamps

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPolicyVersions

## Description
Three net-new root causes in the mpa AWS-managed policy catalog that back ListPolicyVersions and GetPolicyVersion. All three come from the same place: the catalog is seeded from a one-entry table whose entry identity is derived from the ARN segments and whose version metadata is derived from the store's creation instant, so the catalog AWS publishes is neither complete, nor named as AWS names it, nor serialized in the modelled timestamp format.

Cause 1 of 4... (three causes total; numbered below)

Cause 1 of 3 — the catalog publishes only one policy, so a version listing for an AWS-managed policy AWS does publish comes back without it. Fixture mpa-policy-catalog-browsing, step task#2, operation ListPolicyVersions.
AWS: Smithy: ListPolicyVersions -> ListPolicyVersionsResponse$PolicyVersions : list<PolicyVersionSummary> (@length max 20), http POST /policies/{PolicyArn}/?List. Every member of PolicyVersionSummary (Arn, PolicyArn, VersionId, PolicyType, IsDefault, Name, Status, CreationTime, LastUpdatedTime) is @required. The set of AWS-managed MPA policies is not enumerated by the model, so recorded behavior only: the recording shows the catalog carrying an AWS_MANAGED, ATTACHABLE, IsDefault=true version-1 entry for the AWS Backup vault-protection managed policy (display Name "AWS Backup LAG Vault Protection Policy") alongside the approval-team policy.
Emulator: that entry does not exist at all. The replayed listing returned 200 with the entry missing ($.PolicyVersions[Name="AWS Backup LAG Vault Protection Policy"]: expected a PolicyVersionSummary, got None). state.rs seeds the catalog from a single-element table (SEEDED_POLICY_CATALOG = [("IdentityCenter", "MPAApprovalTeamPolicy", AwsManaged)]) and the module comment states outright that it "seeds the AWS-managed approval-team policy and invents no others".
Why: a client that resolves a managed policy by ARN or by name before attaching it to an approval team — which is the only way a caller discovers an attachable policy version ARN, since no request can create one — gets an empty/short listing and cannot proceed. The whole attach-a-managed-policy path is unreachable for every policy except the single seeded one.
Fix: the service's policy-catalog seed in state (the SEEDED_POLICY_CATALOG table and seeded_policy_catalog), which must carry every AWS-managed MPA policy the catalog publishes, each with its version set; ListPolicyVersions' handler already pages over whatever the store holds and needs no change.
Traces: mpa-policy-catalog-browsing#task#2.

Cause 2 of 3 — GetPolicyVersion reports the ARN's name segment as Name, a locally synthesized Document, and epoch-number timestamps. Fixture mpa-policy-catalog-browsing, step task#3, operation GetPolicyVersion.
AWS: Smithy: GetPolicyVersion -> GetPolicyVersionResponse$PolicyVersion (@required) : PolicyVersion, http GET /policy-versions/{PolicyVersionArn}. PolicyVersion$Name : PolicyName @length(max 64) — "Name of the policy" — is the policy's own display name and is independent of the `[a-zA-Z0-9_\.-]` ARN segment in UnqualifiedPolicyArn's pattern (a display name contains spaces, which that pattern forbids, so the two cannot be the same string). PolicyVersion$Document : PolicyDocument @length(max 400000), @sensitive, is the managed policy's real document. PolicyVersion$CreationTime / $LastUpdatedTime target IsoTimestamp with @timestampFormat("date-time"), i.e. an RFC 3339 / ISO-8601 string.
Emulator: for the same version ARN it answered 200 but with Name = the ARN's name segment instead of the policy's display name; Document = a locally synthesized one-statement document of the form {"Version":"2012-10-17","Statement":[{"Sid":"<alphanumerics of the policy name>v<versionId>","Effect":"Allow","Action":"mpa:*","Resource":"*"}]} instead of the managed policy's published document (which grants only the specific mpa:StartSession / mpa:CancelSession actions under the policy's own conditions); and CreationTime / LastUpdatedTime as bare epoch-seconds numbers (a JSON number such as 1793299589.2579482) rather than date-time strings.
Why: Name and Document are the two members a caller reads to decide whether a managed policy grants what it needs; a wildcard "Action":"mpa:*" document and an ARN-segment name make every policy look identical and over-broad, so policy-selection logic picks the wrong policy. The epoch numbers are a type divergence on two @required date-time members: a generated SDK deserializing GetPolicyVersionResponse expects a string there and either fails to parse the response or yields a wrong instant, so the call fails client-side even though the emulator returned 200.
Fix: the policy-catalog state (the catalog entry's display name and the document it reports — policy_version_document currently documents its contents as "NOT a claim about the text AWS serves", which is exactly the divergence) plus the GetPolicyVersion response serializer, which must emit IsoTimestamp members in the modelled date-time form.
Traces: mpa-policy-catalog-browsing#task#3.

Cause 3 of 3 — the same two faults on the list path: PolicyVersionSummary carries the ARN-segment Name and epoch-number timestamps. Fixture mpa-policy-version-lookup-not-found-and-default-flag, step task#3, operation ListPolicyVersions.
AWS: Smithy: PolicyVersionSummary$Name : PolicyName @length(max 64) @required; PolicyVersionSummary$CreationTime / $LastUpdatedTime target IsoTimestamp, @timestampFormat("date-time"), both @required. ListPolicyVersions returns these summaries for the PolicyArn in the httpLabel.
Emulator: the summary for the listed version ARN reported Name = the ARN's name segment rather than the policy's display name, and CreationTime / LastUpdatedTime as epoch-seconds numbers (type_mismatch against the expected date-time string). Because CatalogPolicyVersion stores creation_time / last_updated_time as the instant the store came into being and the serializer writes SystemTime straight out, every seeded version reports the emulator's boot time as a number.
Why: a client paging the catalog cannot match a version to the policy a human chose (names never match), and the two @required timestamp members fail SDK deserialization or decode to a nonsense instant, so ordering/recency logic over policy versions is wrong.
Fix: the ListPolicyVersions response serializer for the IsoTimestamp members, and the catalog state for the entry's display name — the same two components as cause 2, which is why they are bundled.
Traces: mpa-policy-version-lookup-not-found-and-default-flag#task#3.

Not covered by any open issue: the open issue 20261029T083022Z-mpa-listpolicyversions-error-response.md concerns the ORDER in which ListPolicyVersions judges the resource against the NextToken, and says nothing about catalog contents, Name, Document or timestamp serialization.

## Evidence
Replay of trace set simparity-wire-aws-v1 (11 fixtures, 35 steps, 30 passed, 5 mismatched).

Cause 1 — mpa-policy-catalog-browsing#task#2, ListPolicyVersions, expected 200 / actual 200:
  $.PolicyVersions[Name="AWS Backup LAG Vault Protection Policy"]: missing (expected a PolicyVersionSummary with PolicyType AWS_MANAGED, Status ATTACHABLE, IsDefault true, VersionId 1, got None)

Cause 2 — mpa-policy-catalog-browsing#task#3, GetPolicyVersion, expected 200 / actual 200:
  $.PolicyVersion.Name: expected the policy's display name ("AWS Backup LAG Vault Protection Policy"), got the ARN segment "MPAApprovalTeamPolicy"
  $.PolicyVersion.Document: expected the managed policy's published document (a single statement over the mpa:StartSession / mpa:CancelSession actions with its mpa:RequestedOperation / mpa:ProtectedResourceAccount / aws:ViaAWSService conditions), got the synthesized {"Version":"2012-10-17","Statement":[{"Sid":"MPAApprovalTeamPolicyv1","Effect":"Allow","Action":"mpa:*","Resource":"*"}]}
  $.PolicyVersion.CreationTime: expected a date-time string (recorded as "2025-06-17T00:00:00.000Z"), got the number 1793299589.2579482
  $.PolicyVersion.LastUpdatedTime: same divergence, got the number 1793299589.2579482

Cause 3 — mpa-policy-version-lookup-not-found-and-default-flag#task#3, ListPolicyVersions, expected 200 / actual 200, on the summary for Arn="arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/1":
  .CreationTime: type_mismatch — expected a date-time string, got the number 1793299589.9616995
  .LastUpdatedTime: type_mismatch — expected a date-time string, got the number 1793299589.9616995
  .Name: expected the display name, got the ARN segment "MPAApprovalTeamPolicy"

Source evidence in the checkout: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/state.rs lines 2591-2592 (SEEDED_POLICY_CATALOG holds exactly one entry), 2452-2469 (policy_version_document synthesizes a wildcard "mpa:*" document and states its contents are not AWS's), 2509-2528 (with_first_version copies the ARN name segment into CatalogPolicy.name and stamps both version timestamps with the store-creation instant), 2480-2488 (CatalogPolicyVersion holds them as SystemTime).

Constraints cited from services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json: PolicyVersion / PolicyVersionSummary member list and @required set; IsoTimestamp @timestampFormat("date-time"); UnqualifiedPolicyArn / QualifiedPolicyArn patterns.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture mpa-policy-catalog-browsing step task#2 — POST /policies/{PolicyArn}/?List for the AWS Backup vault-protection managed policy ARN and compare the returned PolicyVersions list against the recorded one; the summary whose Name is the policy's display name is absent.
3. Cause 2: replay fixture mpa-policy-catalog-browsing step task#3 — GET /policy-versions/{PolicyVersionArn} for that version and inspect $.PolicyVersion.Name, $.PolicyVersion.Document, $.PolicyVersion.CreationTime and $.PolicyVersion.LastUpdatedTime: the name is the ARN segment, the document is the synthesized "mpa:*" statement, and both timestamps are JSON numbers instead of date-time strings.
4. Cause 3: replay fixture mpa-policy-version-lookup-not-found-and-default-flag step task#3 — POST /policies/{PolicyArn}/?List and inspect the summary for version 1: Name is the ARN segment and both timestamps are JSON numbers.
5. All four of these steps must pass before this issue is closed.
