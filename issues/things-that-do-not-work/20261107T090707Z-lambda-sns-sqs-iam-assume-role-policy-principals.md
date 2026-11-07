Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.iam-role-trust-sns-principal
Issue-Key: lambda__sns__sqs.iam-role-trust-sns-principal|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83

# lambda__sns__sqs: IAM assume-role policy principals are not validated — CreateRole accepts a service name under the "AWS" key instead of MalformedPolicyDocument

> _Note: this issue was identified against the code base at commit `9ae19e0eca83af3a9e4c3dd8e20d1f4043458c83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
Cause 1 of 1: neither CreateRole nor UpdateAssumeRolePolicy validates the Principal element of the trust policy, so trust policies AWS refuses outright are accepted and a role AWS would never have created exists.
  AWS: Smithy: CreateRole.AssumeRolePolicyDocument is required (policyDocumentType, @length 1..131072); CreateRole models MalformedPolicyDocumentException (client, HTTP 400, awsQueryError code MalformedPolicyDocument), as does UpdateAssumeRolePolicy. AWS validates the trust policy's Principal element and refuses two forms this check exercises:
    - a service principal written under the "AWS" key — e.g. Principal {"AWS": "sns.amazonaws.com"} — with code MalformedPolicyDocument, HTTP 400, and the message `Invalid principal in policy: "AWS":"sns.amazonaws.com"`. Fixed text: `Invalid principal in policy: `; variable parts: the policy's principal KEY upper-cased and quoted, then `:` and the quoted principal VALUE exactly as the request wrote it.
    - a REGIONAL service name under the "Service" key — e.g. Principal {"Service": "sns.<region>.amazonaws.com"} — with the same code/status and the message `Invalid principal in policy: "SERVICE":"sns.<region>.amazonaws.com"`; note AWS renders the key as the upper-cased token `SERVICE`, not as the request's `Service`.
  A refused CreateRole must also be atomic: the role must not exist afterwards, so a following GetRole answers NoSuchEntity (HTTP 404, awsQueryError code NoSuchEntity). A refused UpdateAssumeRolePolicy must leave the role's existing, working trust document in force.
  Emulator: CreateRole with AssumeRolePolicyDocument Principal {"AWS": "sns.amazonaws.com"} returns 200 and mints the role (the check's `_error` helper reports `expected the call to be refused, but it succeeded: 'arn:aws:iam::<account>:role/<role-name>'`). No MalformedPolicyDocument is raised, so the subsequent GetRole-is-NoSuchEntity assertion and the regional-Service-name UpdateAssumeRolePolicy assertion in the same check are never reached either.
  Why: a client that relies on AWS rejecting a malformed trust policy gets the opposite verdict. Infrastructure code (and CDK/CloudFormation templates) carrying this very common mistake — a service principal pasted under "AWS", or a regionalized service name — deploys clean against the emulator and then fails at CreateRole against real AWS. Worse, the emulator ends up holding a role whose trust document can never be satisfied by any principal, so every downstream AssumeRole/PassRole scenario built on it diverges silently rather than at the point of the error.
  Fix: the IAM CreateRole and UpdateAssumeRolePolicy handlers' validation of AssumeRolePolicyDocument — a shared trust-policy Principal validator that rejects a service name under the "AWS" key and a regionalized `<service>.<region>.amazonaws.com` name under "Service" with MalformedPolicyDocument/400 and the message above, and that performs the validation BEFORE any state is written so a refused CreateRole leaves no role and a refused UpdateAssumeRolePolicy leaves the prior document in place.
  Related but NOT covering this (the check is not marked OWNED by either): 20261103T021956Z-ec2-lambda-s3-iam-accepts-a-trust-policy-naming-a-service.md and 20261103T031416Z-ec2-lambda-s3-2-net-new-iam-causes.md both concern a service principal that does not EXIST under the "Service" key; this cause is a valid service name placed under the wrong principal KEY, plus the regionalized-"Service" form, and its refusal message names the key.
  Traces: lambda__sns__sqs.iam-role-trust-sns-principal.

## Evidence
Check lambda__sns__sqs.iam-role-trust-sns-principal:
  expected: iam.create_role(AssumeRolePolicyDocument = trust policy with Principal {"AWS": "sns.amazonaws.com"}) refused with
            Error.Code    = "MalformedPolicyDocument"  (HTTP 400)
            Error.Message = 'Invalid principal in policy: "AWS":"sns.amazonaws.com"'
            and iam.get_role on that name then answering NoSuchEntity;
            then iam.update_assume_role_policy with Principal {"Service": "sns.<region>.amazonaws.com"} refused with
            Error.Code    = "MalformedPolicyDocument"
            Error.Message = 'Invalid principal in policy: "SERVICE":"sns.<region>.amazonaws.com"'
  actual:   File "check_lambda__sns__sqs_iam_role_trust_sns_principal.py", line 47, in run
            error = _error(lambda: _create_role_once(ctx, iam, aws_form, {"AWS": "sns.amazonaws.com"}))
            File "check_lambda__sns__sqs_iam_role_trust_sns_principal.py", line 85, in _error
            raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
            AssertionError: expected the call to be refused, but it succeeded: 'arn:aws:iam::000000000000:role/sim2real-iamtrust-awsform-c752d3259b08'
  (the role name/account in that string are per-run values; the divergence is the 200 + created role where AWS answers 400 MalformedPolicyDocument and creates nothing)

## Steps to Reproduce
1. iam:CreateRole with RoleName=<any> and AssumeRolePolicyDocument = {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"sns.amazonaws.com"},"Action":"sts:AssumeRole"}]}.
   AWS: 400 MalformedPolicyDocument, message 'Invalid principal in policy: "AWS":"sns.amazonaws.com"'. Emulator: 200 with a Role.Arn.
2. iam:GetRole on that RoleName. AWS: 404 NoSuchEntity (nothing was created). Emulator: 200, the role exists.
3. Create a role with a valid trust {"Service": ["lambda.amazonaws.com", "sns.amazonaws.com"]}, then iam:UpdateAssumeRolePolicy on it with Principal {"Service": "sns.<region>.amazonaws.com"}.
   AWS: 400 MalformedPolicyDocument, message 'Invalid principal in policy: "SERVICE":"sns.<region>.amazonaws.com"', and the role's previous trust document is unchanged.
