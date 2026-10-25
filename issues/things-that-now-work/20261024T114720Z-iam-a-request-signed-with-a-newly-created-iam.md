Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record
Issue-Key: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record|run#28|AssertionError|assert who["Arn"] == created_user["User"]["Arn"], (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# A request signed with a newly created IAM user's access key resolves to the built-in 'emulator' principal, not to the user the key belongs to

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessKey

## Description
One cause. Grounded in the behavioral check cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record; the divergence itself is in IAM/credential resolution, which is why it is filed under iam rather than with the CloudTrail recording issues.

AWS: CreateAccessKey returns an AccessKeyId/SecretAccessKey pair that belongs to the named user (Smithy iam: CreateAccessKey, output AccessKey with required UserName, AccessKeyId, SecretAccessKey, Status, CreateDate). A request signed with that pair is authenticated AS that user: sts:GetCallerIdentity answers Arn 'arn:aws:iam::<account>:user/<the user the key was minted for>' and UserId equal to that user's id. Recorded behavior only for the latency: a fresh key may need a moment to propagate, which the check absorbs by retrying.

Emulator: the key is minted, but signing with it does not change the principal. GetCallerIdentity made with the new user's credentials answers 'arn:aws:iam::000000000000:user/emulator' instead of the minted user's ARN ('arn:aws:iam::000000000000:user/sim2real-behav-obsiam1sec-bbd8118650b8'), so every request resolves to one built-in principal regardless of which access key signed it. The check dies there and never reaches its real subject (that CreateAccessKey's secretAccessKey and CreateLoginProfile's password are omitted from the audit record).

Why: identity is the foundation every other behavior sits on. A client cannot act as more than one principal, cannot verify that credentials it just provisioned work, cannot test per-user resource ownership or isolation, and cannot produce a caller identity to correlate with - any multi-principal scenario (least privilege, delegation, per-user quotas, audit attribution) is unreachable. It also makes the absent-authorization defect already filed as 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md unfixable in isolation: a policy cannot be evaluated against a principal the request path never resolves. That issue does not carry this check or this cause.

Fix: the SigV4 credential-resolution seam in the server runtime must look the request's AccessKeyId up in IAM's access-key registry and set the resolved principal (user ARN/UserId, or the assumed-role session for STS-vended keys) for the rest of the request, and sts GetCallerIdentity must report that resolved principal rather than a fixed built-in identity. CreateAccessKey's handler must register the minted key against its user so that lookup can succeed.

Traces: cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record.

## Evidence
cloudtrail__iam.obs-iam-1-access-key-secret-and-login-password-are-omitted-from-the-audit-record - verdict 'mismatch': "AssertionError: the new key signs as 'arn:aws:iam::000000000000:user/emulator', expected the user it was minted for ('arn:aws:iam::000000000000:user/sim2real-behav-obsiam1sec-bbd8118650b8')", raised at 'assert who["Arn"] == created_user["User"]["Arn"]' after the check created the user, called CreateAccessKey for it, built a client from the returned AccessKeyId/SecretAccessKey and called sts GetCallerIdentity with it.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. iam CreateUser UserName=u-1; note User.Arn.
3. iam CreateAccessKey UserName=u-1; note AccessKey.AccessKeyId / AccessKey.SecretAccessKey.
4. Build a client from exactly those credentials and call sts GetCallerIdentity.
5. Expect Arn 'arn:aws:iam::<account>:user/u-1'; observe 'arn:aws:iam::000000000000:user/emulator' - the signing key does not select the principal.
