Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms.sep-policy-composition-kms-key-policy-required-identity-policy-alone-denied
Issue-Key: iam__kms.sep-policy-composition-kms-key-policy-required-identity-policy-alone-denied|_settle#27|Timeout|time.sleep(interval_s)
Issue-Commit: df75a05586985baf68c72ff4d4d2cf208170da4a

# KMS performs no authorization on the data plane: a principal with no KMS grant anywhere is never refused AccessDeniedException, so the key-policy/identity-policy composition can never settle

> _Note: this issue was identified against the code base at commit `df75a05586985baf68c72ff4d4d2cf208170da4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Encrypt

## Description
One net-new root cause in the kms emulator's authorization path. The nearest open neighbour is 20261024T102105Z-iam-kms-a-key-policy-s-explicit-deny-does-not.md, which reports that a key policy's EXPLICIT Deny does not override an identity-policy allow; it is about a Deny statement being ignored and does not cover the absence of any refusal at all for a principal that no policy grants, nor KMS's opt-out from same-account union (the key policy must permit the principal even when the identity policy already does). The check here was not marked as owned by that issue.

Cause 1 of 1: KMS never refuses a request that no policy authorizes, so no poll for a denial ever settles.
AWS: Smithy kms#Encrypt (KeyId required, Plaintext required @length 1-4096) and kms#ListAliases model their service errors but, like every AWS operation, are additionally gated by authorization; the refusal is the standard AccessDeniedException. KMS deliberately opts OUT of the same-account identity-OR-resource union: the KEY POLICY must permit the principal, whatever its IAM identity policy says. Recorded behavior for the refusal: code `AccessDeniedException` with the message "User: <principal arn> is not authorized to perform: kms:Encrypt on resource: <key arn> because no resource-based policy allows the kms:Encrypt action" — fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the ", " action"; variable parts are the caller's principal ARN, the action, and the key ARN, all taken from the request and the caller's identity. A principal with no KMS permission at all is likewise refused AccessDeniedException on ListAliases. Adding the account delegation to the key policy (Principal: the account root, Action kms:*) makes the identity policy effective; withdrawing it restores the refusal.
Emulator: the check times out after 60s with no assertion failure, inside a poll loop (_settle) that calls one operation repeatedly until it returns the expected outcome on consecutive tries and gives up only at its own 150s budget. No outcome it waits for is ever observed. The two outcomes it waits for, in order, are (a) kms:Encrypt by the user the key policy names succeeding, and (b) ListAliases/Encrypt by a user whose identity policy allows kms:Encrypt but whom the key policy neither names nor delegates to being refused AccessDeniedException. The SimulatePrincipalPolicy legs that precede the first poll asserted successfully (the simulator reported allowed / matched in the Resource Policy), so the divergence is in ENFORCEMENT, not in simulation. Hypothesis: the emulator authorizes every KMS request unconditionally, so (b) can never be observed and the loop spins to the harness cap; the same unconditional allow is what the neighbouring explicit-Deny issue reports from the other direction.
Why: every KMS access-control expectation a client has is void. A principal with no key-policy grant can encrypt and decrypt under any key, so the emulator is strictly more permissive than the account it stands in for: code that is correct against this emulator will be refused in production, and code that handles AccessDeniedException from KMS (re-prompting for a grant, falling back to another key) has that branch silently dead. Because KMS's refusal is also the mechanism by which an account's key-policy delegation is observable, nothing about key-policy administration can be validated here. It also wedges callers: a client that polls for its own denial — the standard way to confirm a grant was withdrawn — never terminates.
Fix: the authorization gate for the kms data plane. Each KMS operation handler must evaluate the key's key policy (and, only where the key policy delegates to the account root, the caller's IAM identity policy) before acting, and answer AccessDeniedException with AWS's message template — filled from the caller's principal ARN, the action and the key ARN — when nothing authorizes the call, producing no ciphertext and no state change.
Traces: iam__kms.sep-policy-composition-kms-key-policy-required-identity-policy-alone-denied.

## Evidence
Suite simparity-behavioral-ga-prod-v1; iam and kms are both bootstrapped. [mismatch]:

  Error: Check timed out after 60.0s

There is no assertion failure and no exception from the emulator — the check is stuck in its own poll loop. That loop (_settle, streak of consecutive identical observations, interval 2s, its own budget 150s) exists because "IAM is eventually consistent and exposes no API that reports a grant as effective, so the operation is its own observable"; on a correct target each stage settles in a few seconds. Reaching 60s means an expected outcome was never observed even once.

What had already passed before the first poll (so is NOT the divergence): the SimulatePrincipalPolicy leg asserting that a user named only in the key policy is ("allowed", MatchedStatements SourcePolicyType == ["Resource Policy"]) for kms:Encrypt on that key ARN.

The outcomes polled for, and so the candidates for what the emulator never produces:
  1. kms:Encrypt(KeyId=<key arn>, Plaintext=<16 bytes>) by the user the key policy names -> "OK".
  2. A second user whose identity policy Allows kms:Encrypt on that exact key ARN, under a key policy that neither names it nor delegates to the account root -> ListAliases(Limit=1) and Encrypt refused with exactly:
       code "AccessDeniedException"
       message "User: <ident user arn> is not authorized to perform: kms:Encrypt on resource: <key arn> because no resource-based policy allows the kms:Encrypt action"
     Fixed text: "User: " / " is not authorized to perform: " / " on resource: " / " because no resource-based policy allows the " / " action". Variable parts: the caller's principal ARN, the action name, the key ARN — from the request and the caller's identity, never hardcoded.
  3. After PutKeyPolicy adds the account delegation, the same identity-only user's Encrypt -> "OK"; after it is withdrawn again, refused with exactly the message in 2 (the re-break leg, which rules out IAM propagation lag as the cause of the first refusal).

The emulator produces none of the awaited transitions within 60s. Given that AWS's own refusal is leg 2 and the neighbouring open issue already records that a KMS key policy's explicit Deny is not honoured either, the consistent reading is that no KMS operation is authorization-gated at all. Noted limitation: the check's internal poll budget (150s) exceeds the harness's 60s per-check cap, so the report shows a timeout rather than the specific stage's assertion text; the stage is identified above from the check source, and no stage of it can be slow on a local emulator that answers correctly.

## Steps to Reproduce
Boot the emulator and sign requests as a principal permitted to call IAM and KMS.
1. CreateKey, and PutKeyPolicy a policy whose only Allow names IAM user R (Principal: {"AWS": <R's user arn>}, Action kms:Encrypt, Resource *) and which does NOT delegate to the account root.
2. CreateUser R with NO identity policy; CreateAccessKey for R.
3. CreateUser I with an inline identity policy allowing kms:Encrypt on the exact key ARN; CreateAccessKey for I.
4. Signing as R, call Encrypt(KeyId=<key arn>, Plaintext=<16 bytes>). Expect success, and Decrypt of the ciphertext to return the exact plaintext under that KeyId.
5. Signing as I, call ListAliases(Limit=1) and Encrypt(KeyId=<key arn>, Plaintext=<16 bytes>). Expect BOTH to be refused AccessDeniedException; Encrypt's message must be "User: <I's arn> is not authorized to perform: kms:Encrypt on resource: <key arn> because no resource-based policy allows the kms:Encrypt action", and no ciphertext may be produced. Observe instead that the emulator does not refuse.
6. PutKeyPolicy a policy delegating to the account root (Principal: {"AWS": "arn:aws:iam::<account>:root"}, Action kms:*, Resource *). Signing as I, Encrypt now succeeds and Decrypt returns the plaintext.
7. PutKeyPolicy the non-delegating policy from step 1 again. Signing as I, Encrypt must be refused with exactly the message in step 5 again, while R's Encrypt still succeeds. Observe that it is not.
Replay: iam__kms.sep-policy-composition-kms-key-policy-required-identity-policy-alone-denied
