Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-nonexistent-entity-returns-nosuchentity
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.get-nonexistent-entity-returns-nosuchentity|_expect#10|AssertionError|assert got_message == message, f"{label}: expected message {message!r}, got {got_message!r}"
Issue-Commit: 07a4d64ba40427af2b547e51fdb7bd6bf6527f85

# iam: GetPolicy's NoSuchEntity message says "does not exist." instead of AWS's "was not found."

> _Note: this issue was identified against the code base at commit `07a4d64ba40427af2b547e51fdb7bd6bf6527f85`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
ONE net-new root cause (Cause 1 of 1) in the bootstrapped `iam` service, grounded in a failing behavioral check the replay report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: GetPolicy's NoSuchEntityException message is the emulator's own generic not-found wording, not AWS's GetPolicy wording ===

AWS: GetPolicy against a well-formed, account-owned managed-policy ARN that the account holds no policy under is refused with NoSuchEntityException. Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): com.amazonaws.iam#GetPolicy input `PolicyArn` @required target arnType @length 20..2048; errors InvalidInputException httpError 400 / awsQueryError {code InvalidInput, 400}, NoSuchEntityException httpError 404 / awsQueryError {code NoSuchEntity, 404}, ServiceFailureException httpError 500 / awsQueryError {code ServiceFailure, 500}. NoSuchEntityException is documented "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." — the model does not state the sentence, so the message itself is recorded behavior only: the recording this check carries shows AWS answering HTTP 404, code `NoSuchEntity`, message `Policy <PolicyArn> was not found.` Its FIXED text is `Policy ` and ` was not found.`; its single VARIABLE part is the PolicyArn echoed exactly as the request supplied it (the check composes that ARN from the account id the target itself reports via sts GetCallerIdentity, so no account id, policy name or random suffix from one run is part of the requirement).

Emulator: the status and the error code are already correct (404 / `NoSuchEntity`) — only the message body diverges, because the handler mints a generic not-found sentence of its own. Replay shows `Policy arn:aws:iam::<account>:policy/<name> does not exist.` where AWS says `Policy arn:aws:iam::<account>:policy/<name> was not found.` The three legs asserted immediately before this one in the same check — GetUser, GetGroup and GetRole on nonexistent entities — all PASSED with their exact AWS messages (`The user with name <n> cannot be found.`, `The group with name <n> cannot be found.`, `The role with name <n> cannot be found.`), which localises the defect to the GetPolicy handler's own message constant rather than to a service-wide error-message convention. Because the check asserts sequentially and stopped here, the legs after it (GetPolicyVersion, GetInstanceProfile, GetOpenIDConnectProvider, GetSAMLProvider, GetServerCertificate, GetLoginProfile, GetMFADevice, GetSSHPublicKey) and the GetPolicy/GetPolicyVersion positive controls were never exercised — this issue claims nothing about them.

Why: the error message is part of the contract. A client, test harness, CDK/Terraform-style wrapper or operator runbook that keys on AWS's `was not found.` wording for a missing managed policy — and anything that surfaces the message to a human — sees text AWS never emits, so an otherwise entirely correct 404/NoSuchEntity refusal reads as an unknown failure and the "create the policy then retry" branch is not taken.

Fix: the NoSuchEntityException construction in the GetPolicy handler — the final fallthrough for an account-owned ARN the store holds no policy under — must use AWS's wording for this error class instead of the generic `does not exist.` template. Related but DIFFERENT handlers and DIFFERENT messages, so none of these covers this cause: open issue 20261026T004230Z-iam-3-net-new-behavioral-root-causes-iam.md Cause 2 reports the same `does not exist.` vs `was not found.` divergence in src/ops/tag_policy.rs (TagPolicy), and open issue 20261025T050617Z-iam-2-net-new-root-causes-listentitiesforpolicy-s.md Cause 1 reports src/ops/list_entities_for_policy.rs dropping the `or is not attachable.` clause; neither names GetPolicy, and that second issue's prose in fact asserts GetPolicy uses the plain 'does not exist.' wording, which this recording contradicts — so the templates must be sited per operation. Also not a duplicate of the fixed issue 20261022T131025Z-iam-has-no-aws-managed-policy-catalog-getpolicy.md: that one concerns ARNs in the `arn:aws:iam::aws:policy/*` namespace, whereas this check uses an account-owned ARN for which 404/NoSuchEntity is the correct outcome and only the sentence is wrong.

Traces: iam.get-nonexistent-entity-returns-nosuchentity.

## Evidence
Replay output, the one surviving failing check of the bootstrapped service `iam` (marked neither ALREADY FILED nor OWNED):

  [mismatch] iam.get-nonexistent-entity-returns-nosuchentity
  File "check_iam_get_nonexistent_entity_returns_nosuchentity.py", line 80, in run
    _expect("GetPolicy", lambda: iam.get_policy(PolicyArn=fake_policy_arn), "NoSuchEntity", f"Policy {fake_policy_arn} was not found.")
  File "check_iam_get_nonexistent_entity_returns_nosuchentity.py", line 39, in _expect
    assert got_message == message, f"{label}: expected message {message!r}, got {got_message!r}"
  AssertionError: GetPolicy: expected message 'Policy arn:aws:iam::<account>:policy/sim2real-behav-nopolicy-<suffix> was not found.', got 'Policy arn:aws:iam::<account>:policy/sim2real-behav-nopolicy-<suffix> does not exist.'

Diverging response, quoted exactly:
  Expected (AWS): HTTP 404, awsQuery error code `NoSuchEntity`, message `Policy <PolicyArn> was not found.`
  Actual (emulator): HTTP 404, awsQuery error code `NoSuchEntity`, message `Policy <PolicyArn> does not exist.`
  Fixed text of the requirement: `Policy ` + ` was not found.` Variable part: the PolicyArn exactly as the request supplied it. The account id (000000000000 in this environment) and the random name suffix are per-run environment values and are NOT part of the requirement.

The assertion on the error CODE (`got_code == "NoSuchEntity"`) passed immediately before the message assertion, and the three preceding legs of the same check passed in full:
  GetUser     -> NoSuchEntity, 'The user with name <n> cannot be found.'   (PASS)
  GetGroup    -> NoSuchEntity, 'The group with name <n> cannot be found.'  (PASS)
  GetRole     -> NoSuchEntity, 'The role with name <n> cannot be found.'   (PASS)
  GetPolicy   -> NoSuchEntity, message body diverges                       (FAIL)
So this is a message-body-only defect in one handler, not a service-wide convention.

Corroborating code in this checkout:
  services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/get_policy.rs lines 206-211 — the fallthrough for "an account-owned ARN the account does not hold" returns NoSuchEntityException { message: Some(format!("Policy {} does not exist.", input.policy_arn)) }.
  Same file lines 176-183 — the InvalidInputException branch for a value that is not a managed-policy ARN at all (not exercised by this check; the ARN here is well formed).
  Same file lines 195-204 — the 501 branch for an uncarried arn:aws:iam::aws:policy/* ARN (not exercised by this check; the ARN here is account-owned).

Smithy, from services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json:
  GetPolicy input: PolicyArn (arnType, @length 20..2048, @required). Output: Policy (Policy).
  GetPolicy errors: InvalidInputException httpError 400, awsQueryError {code InvalidInput, httpResponseCode 400}; NoSuchEntityException httpError 404, awsQueryError {code NoSuchEntity, httpResponseCode 404}; ServiceFailureException httpError 500, awsQueryError {code ServiceFailure, httpResponseCode 500}.
  NoSuchEntityException doc: "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." The model does not fix the sentence, so the exact wording is recorded behavior only.

## Steps to Reproduce
Build and boot the emulator from this checkout (the `iam` service is bootstrapped and serving), then:

1. sts GetCallerIdentity and take the Account id it reports; call it <account>.
2. Confirm the three legs that already pass, so the divergence is localised to GetPolicy:
   - iam GetUser(UserName=<a name no user exists under>) -> HTTP 404, code NoSuchEntity, message `The user with name <that name> cannot be found.`
   - iam GetGroup(GroupName=<a name no group exists under>) -> HTTP 404, code NoSuchEntity, message `The group with name <that name> cannot be found.`
   - iam GetRole(RoleName=<a name no role exists under>) -> HTTP 404, code NoSuchEntity, message `The role with name <that name> cannot be found.`
3. iam GetPolicy(PolicyArn="arn:aws:iam::<account>:policy/<a name no policy was ever created under>"). The ARN must be well formed (it satisfies arnType @length 20..2048) and in the ACCOUNT's namespace, not `arn:aws:iam::aws:policy/*`.
   Expect: HTTP 404, awsQuery error code `NoSuchEntity`, message `Policy arn:aws:iam::<account>:policy/<that name> was not found.`
   Observe: HTTP 404, awsQuery error code `NoSuchEntity`, message `Policy arn:aws:iam::<account>:policy/<that name> does not exist.`
   The status and code are correct; only the sentence after the echoed ARN diverges.
4. Equivalently: re-replay the check id in this issue's trace ids — iam.get-nonexistent-entity-returns-nosuchentity. Note that the check asserts sequentially and currently stops at this leg, so once the message is fixed the subsequent legs (GetPolicyVersion, GetInstanceProfile, GetOpenIDConnectProvider, GetSAMLProvider, GetServerCertificate, GetLoginProfile, GetMFADevice, GetSSHPublicKey) and the two GetPolicy/GetPolicyVersion positive controls become reachable for the first time and must also hold for the check to pass.
