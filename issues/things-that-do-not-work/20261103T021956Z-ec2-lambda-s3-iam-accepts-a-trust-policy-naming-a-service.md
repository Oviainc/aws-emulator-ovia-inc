Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy
Issue-Key: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy|_err_code_and_message#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# IAM accepts a trust policy naming a service principal AWS refuses for role assumption (1 root cause)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
One net-new root cause replayed under the `ec2__lambda__s3` suite heading.

Cause 1 of 1: a trust policy whose Principal.Service names a service that cannot assume roles is accepted.
AWS: recorded behavior only. CreateRole with an AssumeRolePolicyDocument whose Statement.Principal.Service names the Neptune service principal is refused by IAM (MalformedPolicyDocument-class validation: IAM resolves the service principal and rejects one that is not permitted to assume roles), and no role is created. The loader role for Neptune must instead trust the RDS service principal, which the companion check exercises.
Emulator: the call SUCCEEDS and the role is created with that trust policy.
Why: IAM's service-principal validation at CreateRole time is the only point where this mistake is caught - the role is created, looks correct on read-back, and then every AssumeRole by the real service fails at runtime with an opaque access error. A client (or an IaC template) that relies on CreateRole to reject an unusable trust policy ships a role that can never be assumed, and the emulator reports the deployment as healthy.
Fix: service-principal validation of AssumeRolePolicyDocument in the IAM CreateRole handler (and UpdateAssumeRolePolicy, which takes the same document).
Traces: ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy

## Evidence
Replayed failing check under `### ec2__lambda__s3` (not marked ALREADY FILED / OWNED by):

ec2__lambda__s3.iam-rejects-neptune-service-principal-in-trust-policy -> AssertionError: the call succeeded; real AWS rejects it, so there is no error code

The companion check in the same group, ec2__lambda__s3.neptune-loader-role-trust-policy-round-trips-rds-principal, exercises the accepted (RDS) principal and the GetRole read-back of the same role, confirming the operation under test is IAM CreateRole / GetRole on the loader role's trust policy.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; the check appears under `### ec2__lambda__s3`, unmarked.
2. Standalone repro:
   - iam.create_role(RoleName='t', AssumeRolePolicyDocument=json.dumps({'Version':'2012-10-17','Statement':[{'Effect':'Allow','Principal':{'Service':'rds.amazonaws.com'},'Action':'sts:AssumeRole'}]})) must succeed.
   - The same call with Principal.Service set to the Neptune service principal must be REFUSED with IAM's malformed-policy-document validation, and iam.get_role must then report no such role.
