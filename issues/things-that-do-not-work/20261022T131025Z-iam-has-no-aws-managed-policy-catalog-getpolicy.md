Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.iam-glue-role-attached-managed-policies-list-the-declared-set
Issue-Key: glue__iam__lakeformation__s3|GetPolicy|NoSuchEntity
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# IAM has no AWS-managed policy catalog: GetPolicy on arn:aws:iam::aws:policy/* answers 404 NoSuchEntity even while the policy is attached to a role

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
Cause 1 of 1: AWS-managed policies can be attached but cannot be read.
AWS: Smithy com.amazonaws.iam#GetPolicy takes PolicyArn (arnType, required, len 20..2048) and returns 200 with Policy (AttachmentCount, DefaultVersionId, Arn, PolicyName, Path, ...); NoSuchEntityException (HTTP 404, awsQueryError code NoSuchEntity) is modeled for a policy that does not exist. AWS-managed policy ARNs under arn:aws:iam::aws:policy/ exist in every account, so GetPolicy on them succeeds and GetPolicyVersion(DefaultVersionId) returns their document. Recorded behavior: ListAttachedRolePolicies on the Glue role returns exactly AWSGlueServiceRole (arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole), AmazonS3FullAccess and AWSLakeFormationDataAdmin; GetPolicy on arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin reports AttachmentCount >= 1 and a DefaultVersionId, and that version's document allows lakeformation:* while explicitly denying only lakeformation:PutDataLakeSettings.
Emulator: AttachRolePolicy with the AWS-managed ARNs succeeds and ListAttachedRolePolicies returns the right three entries (that assertion passed), but GetPolicy on arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin fails with HTTP 404, code NoSuchEntity, message 'Policy arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin does not exist.' The IAM state holds only customer-created policies (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_policy.rs reports the managed policy the ARN names 'as this service's state holds it'), so there is no AWS-managed policy catalog behind the aws:policy ARN namespace.
Why: the emulator accepts an attachment to a policy it then denies exists — self-inconsistent state. Any client that reads an AWS-managed policy's metadata or document (permission diagnosis, drift detection, policy-simulation input, 'which policy grants this') gets a 404 for a policy AWS always serves, and cannot tell a genuinely absent policy from a managed one.
Fix: back the arn:aws:iam::aws:policy/ namespace with a seeded AWS-managed policy catalog in the IAM service state (crate::state) — at minimum the policies the suite attaches (service-role/AWSGlueServiceRole, AmazonS3FullAccess, AWSLakeFormationDataAdmin, AWSLakeFormationDataAdmin's documented default version) — so GetPolicy returns Arn/PolicyName/Path/DefaultVersionId with an AttachmentCount that counts live attachments, and GetPolicyVersion returns the policy document; reserve NoSuchEntity for ARNs outside the catalog. Attaching a policy that is not in the catalog should itself be refused rather than silently accepted.
Traces: glue__iam__lakeformation__s3.iam-glue-role-attached-managed-policies-list-the-declared-set.

## Evidence
Expected: GetPolicy(PolicyArn='arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin') -> HTTP 200 with Policy.AttachmentCount >= 1 and Policy.DefaultVersionId, then GetPolicyVersion returning a document whose Allow actions include lakeformation:* and whose only Deny is lakeformation:PutDataLakeSettings. Actual: botocore.errorfactory.NoSuchEntityException: An error occurred (NoSuchEntity) when calling the GetPolicy operation: Policy arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin does not exist. (HTTP 404) — raised after ListAttachedRolePolicies had already returned that same ARN as attached to the role.

## Steps to Reproduce
1. Boot the emulator.
2. CreateRole with a glue.amazonaws.com trust policy and AttachRolePolicy for arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole, arn:aws:iam::aws:policy/AmazonS3FullAccess and arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin (all succeed).
3. ListAttachedRolePolicies(RoleName=<role>) -> the three entries are returned.
4. GetPolicy(PolicyArn='arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin') -> 404 NoSuchEntity instead of the policy metadata. Check: glue__iam__lakeformation__s3.iam-glue-role-attached-managed-policies-list-the-declared-set.
