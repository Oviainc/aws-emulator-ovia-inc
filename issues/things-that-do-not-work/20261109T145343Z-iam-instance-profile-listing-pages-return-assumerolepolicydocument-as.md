Issue-Source: trace-sweep-divergence
Issue-Trace: iam-instance-profile-listing-pagination
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-instance-profile-listing-pagination
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-0-125e3062"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-1-bacda056"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-2-1c215cd5"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfiles|2xx->2xx|$.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-3-f81404c0"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-0-125e3062"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-1-bacda056"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-2-1c215cd5"].Roles.member.AssumeRolePolicyDocument
Issue-Key: iam|ListInstanceProfilesForRole|2xx->2xx|$.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="arn:aws:iam::000000000000:instance-profile/sim2real-probe-iam-ipp-16445403/sim2real-probe-iam-ipp-profile-3-f81404c0"].Roles.member.AssumeRolePolicyDocument
Issue-Commit: 1e889af2b8755cf890ac6135dc3bfbe1a49f6ce1

# IAM instance-profile listing pages return AssumeRolePolicyDocument as raw JSON

> _Note: this issue was identified against the code base at commit `1e889af2b8755cf890ac6135dc3bfbe1a49f6ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstanceProfiles

## Description
Both causes are the same defect reached through the instance-profile listing operations when their results are walked as paginated pages: the embedded Role.AssumeRolePolicyDocument is emitted as the stored raw JSON text instead of the RFC-3986 percent-encoded form AWS returns. The two clusters below carry replay keys that no open issue holds, so neither is covered by the existing (similar, broader) open issues 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md and 20261109T134638Z-iam-query-responses-omit-or-mis-encode-modeled.md — those own the non-paginated members/pages of the same operations (task#10 / task#12 of this same fixture) but not these page keys.

Cause 1 of 2: ListInstanceProfiles (fixture iam-instance-profile-listing-pagination, step task#11).
AWS: IAM serialises every member whose target is com.amazonaws.iam#policyDocumentType URL-encoded. Smithy: Role.AssumeRolePolicyDocument targets com.amazonaws.iam#policyDocumentType (@length 1..131072, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$); the service documents the encoding on the same type ("The policy document returned in this structure is URL-encoded compliant with RFC 3986. You can use a URL decoding method to convert the policy back to plain JSON text."). The requirement is the transform, not any one document: percent-encode the stored JSON byte-for-byte, preserving whatever whitespace the caller submitted ('{' -> %7B, '"' -> %22, ':' -> %3A, ',' -> %2C, '[' -> %5B, ']' -> %5D, '*' -> %2A, ' ' -> %20). This applies to every InstanceProfiles.member.Roles.member element of every page, not only the first page. Status is 200 on both sides; only the member value diverges.
Emulator: returns the raw JSON text verbatim for each Roles.member.AssumeRolePolicyDocument in the page — e.g. {"Version": "2012-10-17", "Statement": [...]} where AWS returns %7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B...%5D%7D. Every profile in the page reproduces it.

Cause 2 of 2: ListInstanceProfilesForRole (fixture iam-instance-profile-listing-pagination, step task#13).
AWS: identical contract — ListInstanceProfilesForRoleResult.InstanceProfiles.member.Roles.member.AssumeRolePolicyDocument targets policyDocumentType and is URL-encoded on every page. Status 200.
Emulator: same raw-JSON passthrough on each profile of the page; status 200, value diverges.

Why: the documented contract tells a client to URL-decode this member. A client that does so against the emulator gets a document that was never encoded — decoding is a silent no-op for most inputs but mangles any literal '%' or '+' inside the policy, and strict clients that validate the member against the documented encoding reject the response. Get -> edit -> Put round-tripping of a trust policy therefore cannot be done against the emulator the way it is done against AWS, and because these are the PAGINATED reads, the breakage appears specifically when a caller walks instance profiles with Marker/MaxItems, which is the normal way to enumerate them.

Fix: the IAM Query response serializer for policyDocumentType-targeted members, applied where the Role projection is written out so it covers Roles embedded in list/page results as well as single-resource reads — one encode step in the serializer rather than per handler. Both causes vanish with the same edit; they are listed separately only because each carries its own replay key.

Traces: iam-instance-profile-listing-pagination#task#11 (ListInstanceProfiles, representative) and iam-instance-profile-listing-pagination#task#13 (ListInstanceProfilesForRole).

## Evidence
Cause 1 — ListInstanceProfiles, iam-instance-profile-listing-pagination#task#11, expected status 200 / actual status 200:
  key $.ListInstanceProfilesResponse.ListInstanceProfilesResult.InstanceProfiles.member[Arn="...instance-profile/<path>/<profile-name>"].Roles.member.AssumeRolePolicyDocument
  expected (recorded AWS): '%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D'
  actual (emulator): '{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}'
  reported for every profile in the page (4 members in this recording). The profile names, ARNs and account id in the key are per-run values; the requirement is the encoding of the document, not those values.

Cause 2 — ListInstanceProfilesForRole, iam-instance-profile-listing-pagination#task#13, expected status 200 / actual status 200:
  key $.ListInstanceProfilesForRoleResponse.ListInstanceProfilesForRoleResult.InstanceProfiles.member[Arn="..."].Roles.member.AssumeRolePolicyDocument
  expected: the same '%7B%22Version%22%3A...' percent-encoded text; actual: the same raw '{"Version": "2012-10-17", ...}' JSON.

No error code or message is involved: both sides answer 200 and agree on every other member of the page, so the divergence is purely the serialization of the policyDocumentType member.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture iam-instance-profile-listing-pagination, which creates a role and several instance profiles under one path and then walks them.
3. At step task#11 (ListInstanceProfiles, paginated page) compare InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument against the recording: AWS returns the percent-encoded document, the emulator returns raw JSON.
4. At step task#13 (ListInstanceProfilesForRole) repeat the comparison on InstanceProfiles.member[*].Roles.member.AssumeRolePolicyDocument — same divergence.
5. URL-decode the emulator's value as the documented contract requires: it is unchanged, confirming the member was never encoded.
