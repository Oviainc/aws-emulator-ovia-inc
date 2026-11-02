Issue-Source: trace-sweep-divergence
Issue-Trace: iam-account-authorization-details-read
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-account-authorization-details-read
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetAccountAuthorizationDetails|2xx->2xx|$.GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member
Issue-Commit: 120b9066736a9aae86613aec18d737c516cfde8b

# iam GetAccountAuthorizationDetails serializes RoleDetailList as a single member object instead of a repeated member list

> _Note: this issue was identified against the code base at commit `120b9066736a9aae86613aec18d737c516cfde8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountAuthorizationDetails

## Description
1 cause.

Cause 1 of 1: GetAccountAuthorizationDetails answers 200 but the RoleDetailList member set in its body does not come across as a list of role-detail entries — it comes across as one role-detail object — so a client (or a fixture assertion) that iterates RoleDetailList.member cannot enumerate it.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508): GetAccountAuthorizationDetails output member RoleDetailList targets roleDetailListType, a list of RoleDetail; the operation is awsQuery, so the member is serialized as a repeated <member> element sequence inside <RoleDetailList> and is read by clients as a list, one entry per role the account holds (model doc: "A list containing information about IAM roles", and "Retrieves information about all IAM users, groups, roles, and policies in your ... account"). The recorded response for this step carries RoleDetailList.member as a LIST of role-detail entries, one of them being the role the fixture created (Path /sim2real-probe/, the probe role name, its account-scoped role ARN, and AttachedManagedPolicies holding the ReadOnlyAccess managed policy). Status is 200 on both sides; no error code or message is involved.

Emulator: for the same account state the emulator's RoleDetailList member value is a single role-detail OBJECT, not a list, and the replay verdict is `type_mismatch (expected [ {..role detail..} ], got {..role detail..})` on $.GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member. The entry's own asserted content (Path, RoleName, Arn, AttachedManagedPolicies.member.PolicyName/PolicyArn) agrees with the recording; only the container differs. Hypothesis: the account's role enumeration contributes exactly one entry here (the role the fixture created) where AWS's enumeration for the same account contributes more than one — a real account also holds the service-linked / service-role roles the emulator's store never seeds — so AWS's repeated-member sequence reads as a list while the emulator's one-entry sequence reads as a scalar object. The alternative hypothesis is that a one-entry roleDetailListType is not emitted as a repeated member sequence at all. Either way the observed defect is that RoleDetailList does not present as a list.

Why: a client that reads GetAccountAuthorizationDetails to snapshot account permissions walks RoleDetailList.member as a sequence. Against the emulator that walk either yields the object's own fields (Path, RoleName, ...) as if they were roles, or raises a type error; code written against AWS therefore mis-reads or fails on an account whose role set the emulator reports, and any role the emulator fails to report (service-linked roles present in every AWS account) is simply invisible in the snapshot.

Fix: the operation's handler/serializer for the role projection in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_account_authorization_details.rs — the `role_detail_list` accumulation and its `role_detail(...)` builder, plus whatever seeds `store.roles` for an account that has not created any role of its own. RoleDetailList must always be serialized as a repeated <member> sequence (list-shaped even with one entry) and must contain one entry per role the account holds.

Traces: iam-account-authorization-details-read#task#4 (the only member of this divergence cluster).

Not covered by an existing open issue: the open iam issues around this surface are 20261029T184359Z-iam-role-templates-are-not-provisioned-so-dependent-role.md (role TEMPLATES / AcquireRole 404s), 20261030T014928Z-iam-built-in-aws-reference-catalogs-diverge-from.md (managed-policy and Access Advisor catalogs) and 20261029T184359Z-iam-policy-documents-are-returned-as-raw-json.md (policy-document encoding); none of them carries this fixture step's replay key or this RoleDetailList shape divergence.

## Evidence
Replay of fixture iam-account-authorization-details-read, step task#4 (operation GetAccountAuthorizationDetails, service iam):
- expected status 200, actual status 200 (status agrees)
- divergence verdict: mismatch, divergence_key `service=iam|GetAccountAuthorizationDetails|status=200->200|keys=$.GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member`
- detail: `$.GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member: type_mismatch (expected [{'Path': '/sim2real-probe/', 'RoleName': '<probe role>', 'Arn': 'arn:aws:iam::<account>:role/sim2real-probe/<probe role>', 'AttachedManagedPolicies': {'member': {'PolicyArn': 'arn:aws:iam::aws:policy/ReadOnlyAccess', 'PolicyName': 'ReadOnlyAccess'}}}], got {'Path': '/sim2real-probe/', 'RoleName': '<probe role>', 'RoleId': '<generated>', 'Arn': 'arn:aws:iam::<account>:role/sim2real-probe/<probe role>', 'CreateDate': '<timestamp>', 'AssumeRolePolicyDocument': '{"Version": "2012-10-17", ...ec2.amazonaws.com sts:AssumeRole...}', 'InstanceProfileList': '', 'RolePolicyList': '', 'AttachedManagedPolicies': {'member': {'PolicyName': 'ReadOnlyAccess', 'PolicyArn': 'arn:aws:iam::aws:policy/ReadOnlyAccess'}}})`
- expected: RoleDetailList.member is a LIST of role details; actual: a single role-detail object. Generated ids, timestamps and the probe's generated role name above are per-run values, not requirements; the requirement is the list-shaped member sequence and one entry per role held.
- stub_path: empty (operation is implemented), owned_by: empty (no open issue carries this replay key), affected_count: 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture `iam-account-authorization-details-read` up to step `task#4`: the fixture creates a role under path /sim2real-probe/ with an ec2.amazonaws.com trust policy, attaches the AWS managed policy arn:aws:iam::aws:policy/ReadOnlyAccess to it, then calls iam GetAccountAuthorizationDetails (awsQuery POST /, Action=GetAccountAuthorizationDetails, Version=2010-05-08).
3. Parse the 200 body and look at GetAccountAuthorizationDetailsResponse.GetAccountAuthorizationDetailsResult.RoleDetailList.member: the emulator presents one role-detail object, where the recording presents a list of role-detail entries containing that role.
4. Compare: iterating RoleDetailList.member as a list of roles succeeds against the recording and fails (type mismatch) against the emulator.
