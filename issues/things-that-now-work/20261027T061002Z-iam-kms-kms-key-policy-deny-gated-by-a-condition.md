Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied
Issue-Key: kms|Decrypt|AccessDeniedException
Issue-Commit: b76363ba3639c1fb1d2b9b08f3e303f98ad8a036

# KMS key-policy Deny gated by a Condition is applied to every principal: the account-root-delegated owner's Decrypt is refused AccessDeniedException on a key that denies only one named principal (1 cause)

> _Note: this issue was identified against the code base at commit `b76363ba3639c1fb1d2b9b08f3e303f98ad8a036`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
Cause 1 of 1 — a Deny statement in a KMS key policy whose Condition names ONE principal (StringEquals on aws:PrincipalArn) is enforced against every caller, so a principal the Deny does not name — here the account-root-delegated base identity that owns the key — is refused kms:Decrypt on that key.

AWS: a key policy statement applies only when its Condition block evaluates true for the request. A statement {Effect: Deny, Principal: {AWS: "*"}, Action: "kms:Decrypt", Resource: "*", Condition: {StringEquals: {"aws:PrincipalArn": "<one principal ARN>"}}} therefore refuses kms:Decrypt for that ONE principal only; every other principal is still governed by the document's other statements — in this key policy an Allow of kms:* to the account root (arn:aws:iam::<account>:root), which delegates to the account's IAM principals. So Decrypt issued by any principal OTHER than the ARN in the Condition must succeed: HTTP 200 with the request's KeyId and the exact plaintext bytes that were encrypted. Only the named principal is refused, with AccessDeniedException (HTTP 400) whose message fixed text is "User: <caller ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> with an explicit deny in a resource-based policy" — the variable parts are the caller ARN taken from the request's signing identity and the key ARN taken from state. Smithy (services/service-com-amazonaws-kms-trentservice): Decrypt models DisabledException, IncorrectKeyException, InvalidCiphertextException, InvalidGrantTokenException, InvalidKeyUsageException, KMSInvalidStateException and NotFoundException; the authorization refusal is the service-common AccessDeniedException, and the policy itself is server state (GetKeyPolicy/PutKeyPolicy round-trip it verbatim — this check asserts that and the assertion passed), so the Condition is evaluated server-side on each data-plane call. The per-principal scoping of a condition-gated Deny is documented AWS IAM/KMS policy-evaluation behavior; the recording shows the owner's Decrypt of its own ciphertext succeeding.

Emulator: the Deny is applied to a principal it does not name. The check's base (owner) credentials — arn:aws:iam::000000000000:user/emulator, which is NOT the ARN in the Condition (that is the freshly created sim2real-sep-denied-* user) — call Decrypt on a ciphertext they just produced with Encrypt under key A, and the emulator answers AccessDeniedException: "User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: kms:Decrypt on resource: arn:aws:kms:us-west-2:000000000000:key/<key A id> with an explicit deny in a resource-based policy". The same credentials' Encrypt under key A and GetKeyPolicy on key A succeed, so the key and the credentials are fine; it is the Deny's Condition that is not being evaluated. Hypothesis (not stated by the model or the recording): the key-policy evaluator ignores the Condition block of a Deny statement, treating Principal "*" as matching every caller; an equivalent explanation that fits the same evidence is that the evaluator attributes requests to a single default identity instead of the signing principal — either way the requirement is the one stated under AWS above.

Why: a client cannot express a per-principal Deny in a key policy at all. Scoping a Deny to one principal with aws:PrincipalArn is the standard way to withhold one identity's use of a CMK while key administrators and applications keep working; against this emulator that same document locks out every principal, including the key owner, so an operator who validates a least-privilege key policy here sees a total lockout that production does not have (and, conversely, cannot tell which principal the refusal is actually about, since the refusal message names a principal the policy never denies). The whole denied-vs-allowed separation this check grades is unobservable.

Fix: the key-policy authorization gate used by the Decrypt handler in services/service-com-amazonaws-kms-trentservice — its policy evaluator must evaluate a statement's Condition block (StringEquals / aws:PrincipalArn resolved from the request's signing principal) before the statement contributes an explicit Deny, and must resolve the caller from the request signature rather than a default identity; the AccessDeniedException serializer must then name that resolved caller. Adjacent open issues that do NOT carry this check's replay key and describe the opposite direction (a key-policy Deny never being applied, or no authorization being evaluated at all) are 20261027T005501Z-kms-4-net-new-root-causes-a-key.md (its Cause 1), 20261024T102105Z-iam-kms-a-key-policy-s-explicit-deny-does-not.md, 20261024T102105Z-kms-no-authorization-is-evaluated-putkeypolicy-has-no.md and 20261027T000203Z-iam-kms-kms-performs-no-authorization-on-the-data-plane.md; none of them covers a condition-gated Deny being over-applied to an unnamed principal.

Traces: iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied.

## Evidence
Cause 1 — iam__kms.sep-kms-key-scope-key-policy-deny-beats-identity-allow-on-that-key-only-denied [error]:
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the Decrypt operation: User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: kms:Decrypt on resource: arn:aws:kms:us-west-2:000000000000:key/b9a1d5f5-57f4-499f-9d28-8334d7fe0c14 with an explicit deny in a resource-based policy

State at the failing call (from the check body):
  key A policy = [ {Sid DelegateToAccount, Allow, Principal {AWS: arn:aws:iam::<account>:root}, Action kms:*, Resource *},
                   {Sid DenyDecryptForOnePrincipal, Deny, Principal {AWS: "*"}, Action kms:Decrypt, Resource *,
                    Condition {StringEquals {aws:PrincipalArn: arn:aws:iam::<account>:user/sim2real-sep-denied-<suffix>}} } ]
  The ARN in the Condition is the check's freshly created IAM user. The refused call is made with the BASE credentials (arn:aws:iam::000000000000:user/emulator), which the Condition does not name.

Calls that succeeded before it, same base credentials, same key A: CreateKey (with that policy), GetKeyPolicy (returned the document verbatim — the stored-policy assertion passed), Encrypt(KeyId=key A, Plaintext=b'sep-deny-scope-payload-A').

Expected: HTTP 200 from Decrypt(CiphertextBlob=<the blob just produced under key A>) with KeyId = key A's ARN and Plaintext = the exact bytes encrypted; AccessDeniedException is reserved for the single principal named in the Condition, with message "User: <that principal's ARN> is not authorized to perform: kms:Decrypt on resource: <key A ARN> with an explicit deny in a resource-based policy".
Actual: AccessDeniedException (HTTP 400) for a principal the policy does not deny, naming arn:aws:iam::000000000000:user/emulator.

Ungraded behind this stop (must also hold once the Condition is honoured): PutKeyPolicy removing only the Deny statement must let the named principal's Decrypt of the IDENTICAL ciphertext succeed and return that key's ARN plus the exact plaintext.

## Steps to Reproduce
Replay the behavioral suite and re-run the check id in trace_ids. Directly against the emulator, with the base (owner) credentials:

1. iam.create_user(UserName=U); user_arn = iam.get_user(UserName=U)['User']['Arn'].
2. kms.create_key(Policy=<document with (a) Allow kms:* / Resource * to Principal {AWS: arn:aws:iam::<account>:root} and (b) Deny kms:Decrypt / Resource * to Principal {AWS: "*"} with Condition StringEquals aws:PrincipalArn = user_arn>) -> key A. Create a second key B with only statement (a).
3. kms.get_key_policy(KeyId=key A, PolicyName='default') — the document comes back verbatim with exactly one Deny on kms:Decrypt (passes today).
4. ct_a = kms.encrypt(KeyId=key A, Plaintext=b'sep-deny-scope-payload-A') with the BASE credentials — succeeds.
5. kms.decrypt(CiphertextBlob=ct_a) with the SAME BASE credentials (a principal the Condition does not name) — observe AccessDeniedException "User: arn:aws:iam::<account>:user/emulator is not authorized to perform: kms:Decrypt on resource: <key A ARN> with an explicit deny in a resource-based policy" where AWS returns HTTP 200 with KeyId=<key A ARN> and Plaintext=b'sep-deny-scope-payload-A'.
6. Attach to U an identity policy allowing kms:Encrypt and kms:Decrypt on [key A, key B], create an access key for U, and with U's credentials: decrypt of key B's blob must succeed; decrypt of ct_a must be refused AccessDeniedException naming U's ARN; encrypt under key A must succeed.
7. kms.put_key_policy(KeyId=key A, PolicyName='default', Policy=<statement (a) only>) and repeat U's decrypt of ct_a — it must now return key A's ARN and the exact plaintext.
