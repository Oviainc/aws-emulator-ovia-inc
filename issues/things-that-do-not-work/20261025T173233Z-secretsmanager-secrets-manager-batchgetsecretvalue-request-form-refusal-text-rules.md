Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.batch-get-secret-value-filters-form-and-pagination secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.batch-get-secret-value-filters-form-and-pagination|run#27|AssertionError|assert message == "You can't specify a secret ID list and filters in the same request.", (
Issue-Key: secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission|run#24|AssertionError|assert refused == expected, (
Issue-Key: secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: e730bd97e23027ce38421dab7cedb53db537ccde

# Secrets Manager BatchGetSecretValue: request-form refusal text/rules diverge and the operation performs no authorization, so denied secrets are returned as plaintext (3 causes)

> _Note: this issue was identified against the code base at commit `e730bd97e23027ce38421dab7cedb53db537ccde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecretValue

## Description
Three independent root causes in BatchGetSecretValue, all grounded in failing behavioral checks of the secretsmanager suite. None of them is covered by an open issue: the operation answers (it is no longer a 501 scaffold), and its three divergences are net-new. Related but NOT covering: 20261024T190718Z-secretsmanager-secrets-manager-enforces-no-authorization-and-never-consults.md and 20261025T031105Z-secretsmanager-secrets-manager-resource-based-policies-and-the-blockpublicpolicy.md describe the same absence of authorization for GetSecretValue/PutResourcePolicy; neither names BatchGetSecretValue, whose per-entry `Errors` partition is a response-shape requirement those issues do not state.

=== Cause 1 of 3: the mutually-exclusive-selection refusals use emulator-authored text, and MaxResults+SecretIdList is not refused at all ===
AWS: Smithy: BatchGetSecretValueRequest$SecretIdList/$Filters — "You must include Filters or SecretIdList, but not both"; BatchGetSecretValueRequest$MaxResults — "To use this parameter, you must also use the Filters parameter." Both violations are refused with the modeled error InvalidParameterException (HTTP 400, "The parameter name or value is invalid"). AWS's messages for these two classes are wholly fixed text with no request-derived parts: for both members supplied, "You can't specify a secret ID list and filters in the same request."; for MaxResults alongside SecretIdList, "Max results can only be used with filters."
Emulator: for SecretIdList+Filters it returns the right code InvalidParameterException with its own sentence, "You can't specify both SecretIdList and Filters in the same request." — the member names are spelled as model member identifiers instead of AWS's prose ("a secret ID list and filters"). For SecretIdList+MaxResults it refuses nothing: MaxResults is accepted and silently applied as a page size over the id-list form, so the call answers 200.
Why: a client that matches on the refusal message (log assertions, error-mapping tables, user-facing surfacing) sees a sentence real AWS never emits, and a client that accidentally sends MaxResults with SecretIdList is told its request is fine here while production rejects it — the bug surfaces only against real AWS.
Fix: the request-form validation at the head of the BatchGetSecretValue handler — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs (the `match (ids, filters)` arm and the page-size resolution that currently clamps MaxResults for both forms).
Traces: secretsmanager.batch-get-secret-value-filters-form-and-pagination (asserts the SecretIdList+Filters message, then the MaxResults+SecretIdList refusal).

=== Cause 2 of 3: the Filters form does not require secretsmanager:ListSecrets ===
AWS: Smithy: BatchGetSecretValue documentation — "Required permissions: secretsmanager:BatchGetSecretValue, and you must have secretsmanager:GetSecretValue for each secret. If you use filters, you must also have secretsmanager:ListSecrets." So the two request forms authorize differently: a principal allowed only BatchGetSecretValue + GetSecretValue on one secret is served by SecretIdList and must be refused the Filters form that selects the very same secret, with the identity-based denial AccessDeniedException (403).
Emulator: the Filters form succeeds for that principal and returns the secret's plaintext. Replay recorded ('ok', ([(<secret ARN>, <plaintext>)], [])) where a refusal was required. The handler evaluates no principal and no action at all — it enumerates the state store directly for the filter form.
Why: an emulated enumerate-and-read works for a role whose policy deliberately withholds ListSecrets, so a caller validates a least-privilege policy here that is denied in production; worse, it is the over-permissive direction — secret material is handed to a principal that must not be able to enumerate.
Fix: authorization of the Filters form in the BatchGetSecretValue handler (the filter-selection branch of services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs), which must evaluate secretsmanager:ListSecrets in addition to BatchGetSecretValue before enumerating.
Traces: secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission.

=== Cause 3 of 3: no per-entry AccessDeniedException — a resource-policy Deny is ignored and the denied secret's plaintext is returned ===
AWS: Smithy: BatchGetSecretValue documentation — "If Secrets Manager encounters errors such as AccessDeniedException while attempting to retrieve any of the secrets, you can see the errors in Errors in the response"; output members SecretValues (SecretValuesType) and Errors (APIErrorListType). The request therefore degrades PER ENTRY: asked for three secrets of which one carries a resource-policy Deny on secretsmanager:GetSecretValue, the call is a 200 carrying the two readable plaintexts in SecretValues plus exactly one Errors entry whose SecretId is the denied secret as requested, ErrorCode is AccessDeniedException, and Message is AWS's explicit-deny sentence. That sentence's fixed text is "User: <caller> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret> with an explicit deny in a resource-based policy", with the caller ARN taken from the request's signing identity and the resource ARN from the requested secret — never a literal from one run. The same Deny must make the single-secret GetSecretValue on that secret fail AccessDeniedException with the identical sentence.
Emulator: resource policies are never consulted on read. `entry_for` can only produce ResourceNotFoundException (missing secret / no AWSCURRENT) or InvalidRequestException (scheduled for deletion); AccessDeniedException is not reachable, and no principal is evaluated. Consequently the single-secret GetSecretValue on the denied secret keeps returning 200 with the plaintext, the check's bounded wait for the denial to become effective never converges, and the check is killed at the harness cap — recorded as "Check timed out after 60.0s". Hypothesis: the non-convergent wait is the 90 s poll for AccessDeniedException on GetSecretValue(arn2), which is the first bounded wait after the baseline batch read succeeds.
Why: the strongest possible over-permissive divergence — a secret whose resource policy explicitly denies reads is returned in full plaintext inside a 200. A client that relies on the Errors/SecretValues partition to detect and report partially-denied batches sees an all-success batch, so a real production denial appears to the client as data it has already consumed.
Fix: resource-policy evaluation on the read path, surfaced per entry rather than per request — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/batch_get_secret_value.rs (`entry_for`, which owns the per-entry error channel) together with the shared GetSecretValue authorization it must share.
Traces: secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret.

## Evidence
Suite: secretsmanager 84/183 (46%). All three checks below are in the failing list and are marked neither "ALREADY FILED" nor "OWNED by".

[mismatch] secretsmanager.batch-get-secret-value-filters-form-and-pagination
  AssertionError: unexpected refusal message for SecretIdList+Filters: "You can't specify both SecretIdList and Filters in the same request."
  Expected (AWS, fixed text): "You can't specify a secret ID list and filters in the same request."
  Code was correct in both cases: InvalidParameterException / HTTP 400.
  Second, unreached-but-certain divergence in the same check: it next asserts InvalidParameterException with "Max results can only be used with filters." for SecretIdList+MaxResults; the handler instead resolves MaxResults into a page size for the id-list form (services/.../src/ops/batch_get_secret_value.rs, the `page_size` computation) and answers 200.

[mismatch] secretsmanager.batch-get-secret-value-filters-form-denied-without-list-secrets-permission
  AssertionError: the Filters form without secretsmanager:ListSecrets must be refused; got ('ok', ([('arn:aws:secretsmanager:us-west-2:<account>:secret:<name>', '<plaintext>')], []))
  Expected: AccessDeniedException (403) for the Filters form; the SecretIdList form for the same principal and secret must still succeed (the check's own positive control).

[mismatch] secretsmanager.batch-get-secret-value-returns-partial-results-with-a-per-secret-error-for-a-denied-secret
  Error: Check timed out after 60.0s
  The check puts an explicit Deny resource policy on secret #2 of three, then polls GetSecretValue(secret #2) for AccessDeniedException with a 90 s budget before it can grade the batch partition. That denial never arrives.
  Expected: 200 with SecretValues = {secret#1, secret#3} carrying their real plaintexts, len(Errors) == 1, Errors[0].SecretId == the denied secret's requested ARN, Errors[0].ErrorCode == "AccessDeniedException", Errors[0].Message == the explicit-deny sentence above; and after DeleteResourcePolicy, 3 SecretValues with an empty Errors.
  Actual: the denied secret reads 200/plaintext both singly and in the batch; AccessDeniedException is not constructible on this path (services/.../src/ops/batch_get_secret_value.rs::entry_for emits only ResourceNotFoundException and InvalidRequestException).

## Steps to Reproduce
Cause 1:
1. CreateSecret three secrets sharing a name prefix, each with a SecretString.
2. BatchGetSecretValue(SecretIdList=[<one ARN>], Filters=[{Key:"name",Values:[<prefix>]}]) -> observe InvalidParameterException with the message "You can't specify both SecretIdList and Filters in the same request."; AWS returns "You can't specify a secret ID list and filters in the same request."
3. BatchGetSecretValue(SecretIdList=[<all three ARNs>], MaxResults=1) -> observe 200 with a one-entry page; AWS refuses InvalidParameterException "Max results can only be used with filters."

Cause 2:
1. CreateSecret one secret with a SecretString.
2. CreateRole + PutRolePolicy allowing exactly secretsmanager:BatchGetSecretValue on "*" and secretsmanager:GetSecretValue on that secret's ARN (NO secretsmanager:ListSecrets).
3. AssumeRole, then BatchGetSecretValue(SecretIdList=[<ARN>]) -> succeeds (correct).
4. As the same role, BatchGetSecretValue(Filters=[{Key:"name",Values:[<secret name>]}]) -> observe 200 with the plaintext; AWS refuses AccessDeniedException until secretsmanager:ListSecrets is added.

Cause 3:
1. CreateSecret three secrets A, B, C with distinct SecretStrings.
2. BatchGetSecretValue(SecretIdList=[A,B,C]) -> 3 SecretValues, empty Errors (baseline, passes today).
3. PutResourcePolicy on B with {"Effect":"Deny","Principal":{"AWS":"*"},"Action":["secretsmanager:GetSecretValue","secretsmanager:BatchGetSecretValue"],"Resource":"*"}.
4. GetSecretValue(SecretId=B) -> observe 200 with B's plaintext; AWS returns AccessDeniedException with the explicit-deny sentence.
5. BatchGetSecretValue(SecretIdList=[A,B,C]) -> observe 3 SecretValues and an empty Errors; AWS returns A and C in SecretValues and one AccessDeniedException entry for B in Errors.
6. DeleteResourcePolicy(SecretId=B); repeat step 5 -> AWS returns 3 SecretValues with an empty Errors (the positive control).
