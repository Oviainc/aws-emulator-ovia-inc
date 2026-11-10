Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow
Issue-Key: kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow|_invoke_lambda#8|Timeout|resp = ctx.client("lambda").invoke(
Issue-Commit: 6c534d81bd78f33a29d39ee989f5affa38b22780

# kms__lambda: a KMS key-policy explicit Deny of kms:Decrypt for a Lambda execution role is never enforced, so the deny/restore lifecycle never converges and the check is killed at the 60 s cap

> _Note: this issue was identified against the code base at commit `6c534d81bd78f33a29d39ee989f5affa38b22780`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Decrypt

## Description
Cause 1 of 1: an in-function kms:Decrypt made with a Lambda execution role's credentials never observes the key's resource policy, so neither the "both keys decrypt" baseline nor the "the Deny reached the role" state is ever reported, and the check makes no progress until the harness kills it. Failing check ids for this cause (1): kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow.

AWS: KMS evaluates the key policy together with the caller's identity policy, and an explicit Deny in the key policy overrides an identity-policy Allow. Smithy (kms Decrypt, input CiphertextBlob/KeyId/EncryptionContext/GrantTokens; modelled errors DependencyTimeoutException 503, DisabledException 409, DryRunOperationException 412, IncorrectKeyException 400, InvalidCiphertextException 400, InvalidGrantTokenException 400, InvalidKeyUsageException 400, KeyUnavailableException 500, KMSInternalException 500, KMSInvalidStateException 409, NotFoundException 404) does not model the authorization refusal: KMS reports it with the service-common AccessDeniedException, HTTP 400, Error.Code `AccessDeniedException`. Its message is a template — fixed text `User: ` + ` is not authorized to perform: ` + ` on resource: ` + ` with an explicit deny in a resource-based policy`, with the variable parts taken from the request and state: the caller's assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<execution-role-name>/<function-name>), the action (`kms:Decrypt`), and the key ARN resolved from KeyId. The Deny is action-scoped and principal-scoped, so on AWS: kms:Encrypt on the SAME key still succeeds for that role; kms:Decrypt under an untouched control key with the identical identity policy still succeeds; the account's own root-delegated principal still decrypts under the denied key; and a PutKeyPolicy (Smithy: KeyId+Policy required, PolicyName pattern ^[\w]+$, errors MalformedPolicyDocumentException 400 / NotFoundException 404 / KMSInvalidStateException 409) that drops the Deny statement makes the identical Decrypt succeed again. Recorded behavior only for the managed python runtime's contents: AWS's python3.x runtimes ship a boto3/botocore whose data directory carries every public service model, so a handler's boto3.client("kms") always constructs and the handler observes KMS's own answer.

Emulator: the check never advances past its polls. Its first poll invokes the function every 5 s waiting for the handler to report decrypt_denied == False on BOTH keys (a plain, fully-authorized decrypt), and the run ends with `Check timed out after 60.0s` — i.e. a correctly-authorized in-function kms:Decrypt never reports success either, so the key-policy arm (AccessDeniedException with the explicit-deny clause) is never reached at all. Nothing in the run shows the emulator ever applying a key-policy Deny to a role-made Decrypt. Hypothesis: the in-function AWS client cannot reach KMS at all (the sibling check in this same service group fails by reporting a botocore client-construction error instead of an AccessDeniedException), so the predicate is unsatisfiable rather than merely slow; that is a hypothesis, not something this run's output states.

Why: a client cannot verify a bulk revocation control. An operator who adds an explicit Deny of kms:Decrypt for a function's execution role to a key policy gets no observable change — the emulator neither refuses the decrypt with AccessDeniedException nor keeps the action-scoped Encrypt and the control key working, so a security control that AWS enforces unconditionally appears to be a no-op (or indistinguishable from a broken invocation), and a review that would catch an over-permissive key policy passes against the emulator.

Fix: the KMS Decrypt/Encrypt authorization path — the component that resolves the calling principal and evaluates the key's resource policy before performing the cryptographic operation (the Decrypt operation handler in the kms service crate and the key-policy evaluation it must consult), plus the Lambda execution environment component that must let a handler construct a client for and dispatch a request to the emulated KMS endpoint with the execution role's credentials, so that the decision is observable from inside an invocation.

Related open work that the suite did NOT mark as owning this check: 20261106T143106Z-kms-lambda-a-handler-s-boto3-client-kms.md (names this check id under the UnknownServiceError symptom), 20261105T111951Z-kms-lambda-environment-variable-kms-enforcement-and-kms.md and 20261107T194121Z-kms-lambda-2-net-new-causes-disabling-a.md. None of them is recorded as carrying this check's current replay key, so it is filed here and those issues are named rather than duplicated.

Traces: kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow (see trace_ids).

## Evidence
Expected (AWS): with one symmetric CMK ("denied") and one untouched control CMK, and a Lambda execution role whose inline policy allows kms:Encrypt and kms:Decrypt on BOTH key ARNs, a RequestResponse Invoke of a handler that encrypts then decrypts under each key returns HTTP 200 with no X-Amz-Function-Error and a payload reporting, before any key-policy change, decrypt_denied == False and the exact plaintext for both keys. After PutKeyPolicy sets the key policy of the denied key to [the default EnableIAMUserPermissions root Allow, plus {"Sid":"DenyDecryptToLambdaRole","Effect":"Deny","Principal":{"AWS":"<execution-role-arn>"},"Action":"kms:Decrypt","Resource":"*"}], the same Invoke reports for the denied key: Error.Code `AccessDeniedException` (HTTP 400) with message `User: arn:aws:sts::<account>:assumed-role/<execution-role-name>/<function-name> is not authorized to perform: kms:Decrypt on resource: <denied-key-arn> with an explicit deny in a resource-based policy`, NO plaintext, encrypt still succeeding on that same key; and for the control key decrypt_denied == False with the exact plaintext. The account's own root-delegated credentials still Encrypt+Decrypt under the denied key. A second PutKeyPolicy removing the Deny statement makes the identical in-function Decrypt succeed again.

In the message above, the fixed text is `User: `, ` is not authorized to perform: `, ` on resource: ` and ` with an explicit deny in a resource-based policy`; the variable parts are the assumed-role session ARN (account id, execution-role name, function name), the action `kms:Decrypt` and the denied key's ARN — all taken from the request and the emulator's state. The concrete account id, role/function names and key ARNs of any single run are not requirements.

Actual (emulator): kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow fails with `Error: Check timed out after 60.0s`. The run never gets past the pre-change baseline poll (`_poll_until(lambda: _invoke_payload(ctx, fn['name'], event), lambda r: r.get('denied', {}).get('decrypt_denied') is False and r.get('control', {}).get('decrypt_denied') is False, timeout_s=120.0, interval_s=5.0)`), so no in-function kms:Decrypt ever reports success, no AccessDeniedException with the explicit-deny clause is ever produced for the denied key, and the restore arm is never exercised.

## Steps to Reproduce
1. kms CreateKey twice (symmetric, ENCRYPT_DECRYPT): one "denied" key, one "control" key.
2. iam CreateRole trusting lambda.amazonaws.com, with an inline policy allowing kms:Encrypt and kms:Decrypt on BOTH key ARNs (identical for the two keys, so the key policy is the only difference).
3. lambda CreateFunction Runtime=python3.12, Handler=lambda_function.lambda_handler, Role=<that role>, whose handler is:
       import boto3
       kms = boto3.client("kms")
       def lambda_handler(event, context):
           out = {}
           for label, key_arn in event["keys"].items():
               entry = {}
               try:
                   blob = kms.encrypt(KeyId=key_arn, Plaintext=b"probe")["CiphertextBlob"]
                   entry["encrypt_ok"] = True
               except Exception as exc:
                   err = getattr(exc, "response", None) or {}
                   entry["encrypt_ok"] = False
                   entry["encrypt_code"] = err.get("Error", {}).get("Code", type(exc).__name__)
                   out[label] = entry; continue
               try:
                   entry["plaintext"] = kms.decrypt(CiphertextBlob=blob, KeyId=key_arn)["Plaintext"].decode()
                   entry["decrypt_denied"] = False
               except Exception as exc:
                   err = getattr(exc, "response", None) or {}
                   entry["decrypt_denied"] = True
                   entry["decrypt_code"] = err.get("Error", {}).get("Code", type(exc).__name__)
                   entry["decrypt_message"] = err.get("Error", {}).get("Message", str(exc))
               out[label] = entry
           return out
   Wait for State == Active.
4. lambda Invoke(InvocationType=RequestResponse) with payload {"keys": {"denied": "<denied-key-arn>", "control": "<control-key-arn>"}}. On AWS the payload reports decrypt_denied False and the exact plaintext for both keys; on the emulator this never happens, which is where the check stalls.
5. kms PutKeyPolicy(KeyId=<denied key>, PolicyName="default", Policy = the default root Allow plus an explicit Deny of kms:Decrypt for the function's execution-role ARN) and Invoke again: expect the denied key to report AccessDeniedException with the message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: kms:Decrypt on resource: <denied-key-arn> with an explicit deny in a resource-based policy`, no plaintext, encrypt_ok True, and the control key still decrypting.
6. kms Encrypt+Decrypt under the denied key with the account's own credentials: must still succeed (the Deny is principal-scoped).
7. kms PutKeyPolicy again with only the root Allow, and Invoke a third time: the identical in-function Decrypt must succeed.
8. Re-replay kms__lambda.kms-key-policy-deny-overrides-lambda-identity-allow to confirm.
