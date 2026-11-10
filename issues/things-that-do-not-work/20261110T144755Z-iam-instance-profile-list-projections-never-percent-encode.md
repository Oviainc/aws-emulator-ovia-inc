Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-0-e3fd2c0d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-1-6142b56d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-2-672704ca"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-3-f86e1ed6"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-0-e3fd2c0d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-1-6142b56d"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-2-672704ca"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-59bf0dd2/sim2real-probe-iam-ipp-profile-3-f86e1ed6"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca

# iam instance-profile list projections never percent-encode Role.AssumeRolePolicyDocument

> _Note: this issue was identified against the code base at commit `a7d3bb56a8b20ad6b0cfb258ffd31cc1523a40ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
2 net-new causes, ONE shared fault: the embedded Role projection that IAM's instance-profile listing operations serialize emits Role.AssumeRolePolicyDocument as the stored raw JSON text instead of the percent-encoded (RFC 3986 / URL-encoded) form AWS returns for IAM policy-document response members. Both causes must be closed before this issue is submitted.

NOTE ON EXISTING ISSUES: the open issues 20261110T101948Z-iam-assumerolepolicydocument-is-not-percent-encoded-in-instance-profile.md, 20261110T050411Z-iam-instance-profile-listing-pages-serve-assumerolepolicydocument-unencoded.md, 20261109T145343Z-iam-instance-profile-listing-pages-return-assumerolepolicydocument-as.md and 20261109T134638Z-iam-query-responses-omit-or-mis-encode-modeled.md describe this same fault family, but each of their replay keys is pinned to the per-run instance-profile ARNs of an earlier recording (sim2real-probe-iam-ipp-a776f583 and earlier ids). This run's profiles live under sim2real-probe-iam-ipp-59bf0dd2, and the replay reports BOTH clusters below with stub_path empty and owned_by empty — no open issue carries these replay keys, so a fixer replaying those issues never exercises these steps. They are filed here and must be re-replayed against these keys to confirm the fix. That the fault keeps reappearing under a new key each run is itself evidence the encode step is absent from this projection, not merely mis-keyed.

Cause 1 of 2: ListInstanceProfiles, a paginated page (fixture iam-instance-profile-listing-pagination, step task#11).
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json) — ListInstanceProfiles output InstanceProfiles is a required instanceProfileListType of com.amazonaws.iam#InstanceProfile; InstanceProfile.Roles is a roleListType of com.amazonaws.iam#Role, and Role.AssumeRolePolicyDocument targets com.amazonaws.iam#policyDocumentType (@length min 1 max 131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$). The operation is @paginated (inputToken/outputToken Marker, pageSize MaxItems, items InstanceProfiles). The model does not state the transfer encoding, so this is recorded behavior plus AWS's documented rule for IAM policy-document response members: IAM returns the document URL-encoded per RFC 3986 and the caller must URL-decode it before parsing it as JSON. The requirement is the TRANSFORM, not any one document — percent-encode the stored JSON byte for byte, preserving whatever whitespace the caller submitted ('{' -> %7B, '"' -> %22, ':' -> %3A, ',' -> %2C, '[' -> %5B, ']' -> %5D, '*' -> %2A, ' ' -> %20) — and it applies to every InstanceProfiles.member.Roles.member of every page, not only the first. Status 200 on both sides; no error code and no error message are involved.
Emulator: serves the raw JSON text verbatim for each Roles.member.AssumeRolePolicyDocument on the page; the encode step is simply never applied in this projection. Every instance profile returned in the page reproduces it (4 profiles in this recording).
Why: a client that follows AWS's contract URL-decodes the member before json.loads(). Decoding an already-decoded document is a silent no-op only while the document contains no '%' and no '+'; a trust policy carrying either (a percent in a condition value, a '+' in a principal name or ARN) is corrupted by that decode — '+' becomes a space, a stray '%xx' becomes a different byte — so the parse fails or yields a policy the caller never wrote. Code that compares a round-tripped trust policy with the encoded form it submitted never matches, so a get/edit/put of a trust policy cannot be driven against the emulator the way it is against AWS. Because this is the paginated read, the breakage surfaces exactly when a caller enumerates instance profiles with Marker/MaxItems, which is the normal way to list them.
Fix: the IAM Query response serializer for policyDocumentType-targeted members, applied in the shared embedded-Role projection the instance-profile listings write out, so paginated listings and single-resource reads encode identically — one encode step in the serializer rather than per handler. Role-level reads (GetRole / ListRoles) do not diverge on this member anywhere in this corpus, so the encode exists elsewhere and is not reached from this projection.
Traces: iam-instance-profile-listing-pagination#task#11.

Cause 2 of 2: ListInstanceProfilesForRole (fixture iam-instance-profile-listing-pagination, step task#13).
AWS: identical contract — ListInstanceProfilesForRole output InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument targets the same com.amazonaws.iam#policyDocumentType and is returned percent-encoded for every profile on every page; the operation is @paginated the same way and models NoSuchEntityException (404, awsQueryError NoSuchEntity) for an unknown RoleName, which is not what diverges here. Status 200.
Emulator: the same raw-JSON passthrough for every profile returned for the role; status 200, only the member value diverges.
Why: same as Cause 1 — a client resolving which instance profiles a role is attached to, and reading the role's trust policy out of that response, gets a document in an encoding the documented contract says it is not in.
Fix: same component as Cause 1; both causes disappear with the single serializer edit and are listed separately only because each carries its own replay key.
Traces: iam-instance-profile-listing-pagination#task#13.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; /__admin/health answered 200 and the data listener accepts). Both causes are 200-vs-200 body divergences, verdict mismatch; the status line, the error code and the error message are not involved on either side. Both clusters are reported with stub_path empty and owned_by empty.

Cause 1 — ListInstanceProfiles, fixture iam-instance-profile-listing-pagination step task#11, expected status 200 / actual status 200:
  key: $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::<account>:instance-profile/<path>/<profile-name>"].Roles.member.AssumeRolePolicyDocument
  expected (recorded AWS): '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
  actual (emulator): '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'
  reported for all 4 profiles on the page (profile-0 .. profile-3 under the generated path sim2real-probe-iam-ipp-59bf0dd2). Expected and actual are the SAME policy document — only the encoding differs. The account id, the instance-profile path and the generated profile names in the key, and the trust-policy text itself, are per-run values of this recording; the percent-encoding of the member is the requirement.

Cause 2 — ListInstanceProfilesForRole, fixture iam-instance-profile-listing-pagination step task#13, expected status 200 / actual status 200:
  key: $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::<account>:instance-profile/<path>/<profile-name>"].Roles.member.AssumeRolePolicyDocument
  expected: the same '%7B%22Version%22%3A...%7D' percent-encoded text; actual: the same raw '{"Version": "2012-10-17", ...}' JSON, for every profile returned for the role.

Every other member of both pages agrees, so the divergence is purely the serialization of the policyDocumentType-targeted member.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay fixture iam-instance-profile-listing-pagination (it creates a role with an ec2.amazonaws.com trust policy and several instance profiles under one path, then walks them with Marker/MaxItems).
3. At step task#11 (ListInstanceProfiles, paginated page) read InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument: AWS returns the percent-encoded trust policy, the emulator returns raw JSON.
4. At step task#13 (ListInstanceProfilesForRole) repeat the comparison on InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument — same divergence.
5. Or directly: CreateRole with a trust policy, CreateInstanceProfile, AddRoleToInstanceProfile, then call ListInstanceProfiles (with MaxItems set so a page is returned) and ListInstanceProfilesForRole for that role. URL-decode the returned AssumeRolePolicyDocument as the documented contract requires: against the emulator the decode is a no-op, proving the member was never encoded. Use a trust policy containing a literal '+' or '%' to observe the corruption a raw document causes on decode.
