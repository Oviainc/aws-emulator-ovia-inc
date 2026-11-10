Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.wildcard-principal-scoped-by-principal-account-is-not-public-and-admits-a-same-account-role
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secrets-manager|PutResourcePolicy|PublicPolicyException
Issue-Commit: 1349203334c72f1c0ca71c4b073e11dccaca8841

# PutResourcePolicy refuses a wildcard-principal policy scoped by aws:PrincipalAccount as public, so a non-public resource policy cannot be attached under BlockPublicPolicy=True

> _Note: this issue was identified against the code base at commit `1349203334c72f1c0ca71c4b073e11dccaca8841`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
REOPENS 20261024T190718Z-secretsmanager-putresourcepolicy-stub.md: its fix left the call refused. (The same operation was also claimed by the fixed 20261024T030130Z-lambda-rds-secretsmanager-secrets-manager-putresourcepolicy-is-an-unimplemented-scaffold-stub.md.) PutResourcePolicy now exists and stores documents, but its public-access test is value-blind to conditions and refuses a document AWS accepts.

One root cause. Cause count: 1.

=== Cause 1 of 1: the BlockPublicPolicy public-access test treats any Principal "*" as public, ignoring a Condition that confines it to the account ===
AWS: BlockPublicPolicy=true refuses only a resource policy that actually grants broad (public) access; the analysis takes the statement's Condition block into account. A statement {Effect: Allow, Principal: "*", Action: secretsmanager:GetSecretValue, Resource: "*"} with Condition {StringEquals: {"aws:PrincipalAccount": "<the secret's own account>"}} cannot be satisfied by any principal outside the account, so it is NOT public: PutResourcePolicy(SecretId=..., ResourcePolicy=<that document>, BlockPublicPolicy=True) is accepted and answers HTTP 200 with the modelled output ARN and Name (Smithy: PutResourcePolicyResponse = ARN + Name; input BlockPublicPolicy is a BooleanType). The refusal is reserved for the unconditioned wildcard, which is refused with PublicPolicyException on HTTP 400 — Smithy documents PublicPolicyException as "The BlockPublicPolicy parameter is set to true, and the resource policy did not prevent broad access to the secret." and the service's fixed wire text is "You can not attach a resource policy that grants Public access to your secret." (constant string; no request-derived parts). GetResourcePolicy must then return the accepted document with its Condition intact.
Emulator: the account-scoped document is refused exactly like the unconditioned one — HTTP 400 PublicPolicyException with the message "You can not attach a resource policy that grants Public access to your secret." The status, code and message text are all AWS's here; the DECISION is what diverges. The same check shows the emulator correctly refusing the unconditioned wildcard just beforehand (that assertion passes, including the message), so the public test fires on the Principal value alone and never looks at Condition.
Why: scoping a wildcard principal with aws:PrincipalAccount (or aws:PrincipalOrgID) is the documented way to write a secret policy that applies to every identity in the account without granting anything outside it, and it is the pattern that BlockPublicPolicy is supposed to allow through. A client relying on AWS's behavior cannot attach that policy at all: the guardrail that exists to catch genuinely public policies reports a safe policy as public, and the refusal is indistinguishable from the one a truly public document earns, so the author has no signal about what to change.
Fix: the PutResourcePolicy handler's public-access test (the predicate behind PublicPolicyException) — when a statement's Principal is the wildcard, consult its Condition block and treat the statement as non-public when a condition confines it to the account or organisation (aws:PrincipalAccount, aws:PrincipalOrgID, aws:PrincipalArn and the equivalent source-account keys); only an effectively unconstrained wildcard Allow may raise PublicPolicyException. ValidateResourcePolicy's BlockPublicPolicyCheck must reach the same verdict for the same document.
Traces: secretsmanager.wildcard-principal-scoped-by-principal-account-is-not-public-and-admits-a-same-account-role. Note that this check's remaining half — the stored account-scoped policy then admitting a same-account role's GetSecretValue — additionally depends on resource-policy Allows being enforced on the read path, which is Cause 3 of the issue filed alongside this one ("Secrets Manager authorization over-refuses: conditioned identity grants and resource-policy Allows never admit a caller AWS permits"); this issue is only about the attach being refused. The open issue 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md lists this check under its Cause 2, but from the opposite symptom (the read succeeding without authorization) and it does not carry this check's replay key; 20261025T031105Z-secretsmanager-secrets-manager-resource-based-policies-and-the-blockpublicpolicy.md covers the BlockPublicPolicy CONDITION KEY and the refusal MESSAGE text, not this mis-classification.

## Evidence
Check secretsmanager.wildcard-principal-scoped-by-principal-account-is-not-public-and-admits-a-same-account-role [error]:

botocore.errorfactory.PublicPolicyException: An error occurred (PublicPolicyException) when calling the PutResourcePolicy operation: You can not attach a resource policy that grants Public access to your secret.

The raising call is the UNGUARDED put of the account-scoped document (the preceding public-document put is wrapped by the check's _error helper and its PublicPolicyException + message assertion passes):
  sm.put_resource_policy(SecretId=<secret arn>, ResourcePolicy=json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "secretsmanager:GetSecretValue", "Resource": "*", "Condition": {"StringEquals": {"aws:PrincipalAccount": "<this account>"}}}]}), BlockPublicPolicy=True)

Expected (AWS): HTTP 200 with ARN and Name, and GetResourcePolicy returning the document whose Statement[0].Condition is {"StringEquals": {"aws:PrincipalAccount": "<this account>"}}.
Observed (emulator): HTTP 400 PublicPolicyException, nothing attached.
Contrast within the same check: the identical statement WITHOUT the Condition is refused with PublicPolicyException and that fixed message, which is correct and passes — so the only difference between the accepted and refused cases on AWS (the Condition) has no effect on the emulator's verdict.

## Steps to Reproduce
1. sts GetCallerIdentity to read <account>.
2. secretsmanager CreateSecret(Name=<n>, SecretString="account-scoped-value").
3. PutResourcePolicy(SecretId=<arn>, ResourcePolicy={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"secretsmanager:GetSecretValue","Resource":"*"}]}, BlockPublicPolicy=True) -> correctly refused with 400 PublicPolicyException "You can not attach a resource policy that grants Public access to your secret.", and GetResourcePolicy shows no ResourcePolicy.
4. PutResourcePolicy(SecretId=<arn>, ResourcePolicy=<the same statement plus "Condition": {"StringEquals": {"aws:PrincipalAccount": "<account>"}}>, BlockPublicPolicy=True).
   Expected: HTTP 200 with ARN/Name; GetResourcePolicy returns the document with the Condition preserved.
   Observed: HTTP 400 PublicPolicyException with the same public-access message; nothing attached.
5. (Downstream, blocked by step 4 and tracked in the companion authorization issue) assume a same-account role with no identity policy and call GetSecretValue(SecretId=<arn>): the account-scoped policy alone must admit it.
