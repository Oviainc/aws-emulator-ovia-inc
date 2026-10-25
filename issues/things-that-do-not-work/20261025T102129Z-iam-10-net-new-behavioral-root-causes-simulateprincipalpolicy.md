Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.asgeni-simulate-secrets-only-role-eni-actions-denied iam.op-contract-listRoles-missing-permission-denied iam.team-role-deny-simulates-reads-explicit-delivery-implicit iam.simulate-principal-policy-attached-boundary-caps-allow iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch iam.sep-policy-composition-sourceip-condition-gates-real-call-denied-outside-cidr iam.sep-policy-composition-action-wildcards-match-exactly-the-documented-set-denied-outside iam.minted-identifier-prefixes-and-arn-paths iam.op-contract-listGroups-path-prefix iam.invalid-policy-arn-on-attach-detach-rejected iam.tag-instance-profile-reserved-prefix-rejected
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.asgeni-simulate-secrets-only-role-eni-actions-denied|run#7|AssertionError|assert len(sources) == 1 and sources[0].endswith("secrets-read"), (
Issue-Key: iam.op-contract-listRoles-missing-permission-denied|run#29|AssertionError|assert sources == {f"user_{user}_{_DENY}"}, (
Issue-Key: iam.team-role-deny-simulates-reads-explicit-delivery-implicit|run#9|AssertionError|assert got == ("explicitDeny", [src]), f"{action} on {res!r}: {got!r}"
Issue-Key: iam.simulate-principal-policy-attached-boundary-caps-allow|_poll#20|Timeout|time.sleep(interval_s)
Issue-Key: iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch|_settle#16|Timeout|time.sleep(interval_s)
Issue-Key: iam.sep-policy-composition-sourceip-condition-gates-real-call-denied-outside-cidr|_settle#12|Timeout|time.sleep(interval_s)
Issue-Key: iam.sep-policy-composition-action-wildcards-match-exactly-the-documented-set-denied-outside|run#16|AssertionError|assert real == {"iam:GetUser": "OK", "iam:ListAccessKeys": "OK", "iam:ListSSHPublicKeys": "OK",
Issue-Key: iam.minted-identifier-prefixes-and-arn-paths|_assert_id#35|AssertionError|assert value.startswith(prefix), (
Issue-Key: iam.op-contract-listGroups-path-prefix|run#15|AssertionError|assert code == "ValidationError", f"unexpected error code {code}"
Issue-Key: iam.invalid-policy-arn-on-attach-detach-rejected|run#12|AssertionError|assert code == "ValidationError", f"{op_name}: expected ValidationError for a malformed ARN, got {code}"
Issue-Key: iam.tag-instance-profile-reserved-prefix-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# iam: 10 net-new behavioral root causes — SimulatePrincipalPolicy never attributes matched statements or honours the stored permissions boundary, request-time authorization carries no condition context, and four refusal/minting contracts diverge

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SimulatePrincipalPolicy

## Description
Ten distinct net-new root causes in the bootstrapped `iam` service, each grounded in a failing behavioral check of this suite that the replay report marks neither ALREADY FILED nor OWNED. A fixer must close all ten before submitting; the per-cause check ids below are what to re-replay.

Cause 1 of 10: SimulatePrincipalPolicy never reports MatchedStatements, so no decision can be attributed to the statement that produced it.
AWS: Smithy (com.amazonaws.iam) SimulatePrincipalPolicy returns SimulatePolicyResponse{EvaluationResults: EvaluationResultsListType}; each EvaluationResult carries MatchedStatements (StatementListType) alongside EvalDecision, and each Statement carries SourcePolicyId and SourcePolicyType. AWS populates MatchedStatements for every allowed and every explicitDeny decision with the policy that matched (recorded behavior for the exact contents; the model states the members). An implicitDeny legitimately matches nothing.
Emulator: the member is hard-coded absent. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs sets `matched_statements: None` on the EvaluationResult (and again on each ResourceSpecificResult), with a comment that the shared evaluator does not track source positions. The decision itself is returned, so callers see `EvalDecision: 'allowed'` / `'explicitDeny'` with no MatchedStatements key at all.
Why: a client cannot answer "which policy caused this?" — the whole point of the simulator for least-privilege review. Tooling that reads SourcePolicyId/SourcePolicyType to show the deciding document, or that asserts a deny comes from the intended policy and from nothing else, sees an empty list and concludes no policy is involved.
Fix: the operation's handler — services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs — must collect the matching statements from the shared evaluator (services/common/src/iam_authorization.rs must return which statement matched, not just the Decision) and populate MatchedStatements with SourcePolicyId and SourcePolicyType.
Traces: iam.asgeni-simulate-secrets-only-role-eni-actions-denied, iam.op-contract-listRoles-missing-permission-denied, iam.team-role-deny-simulates-reads-explicit-delivery-implicit. Two open issues name the same omission for other service tokens and do NOT cover these checks (their trace ids are different units): 20261025T094445Z-iam-s3-simulateprincipalpolicy-omits-matchedstatements-for-an-allow-reached-through.md and 20261025T085808Z-iam-kms-simulateprincipalpolicy-ignores-the-simulated-principal-when-evaluating-resourcepolicy.md.

Cause 2 of 10: SimulatePrincipalPolicy ignores the permissions boundary STORED on the principal; only the boundary passed in the request is honoured.
AWS: Smithy documents PermissionsBoundaryPolicyInputList as "if a permissions boundary is attached to an entity and you pass in a different permissions boundary policy using this parameter, then the new permissions boundary policy is used for the simulation" — i.e. the attached boundary is in force when the parameter is absent. An action an attached identity policy allows but the boundary does not evaluates to implicitDeny, with PermissionsBoundaryDecisionDetail.AllowedByPermissionsBoundary = false; deleting the boundary restores allowed (recorded behavior for the decision rule).
Emulator: the handler reads the boundary only from the request (`let boundary = input.permissions_boundary_policy_input_list.unwrap_or_default();`) and derives `no_boundary = boundary.is_empty()`; nothing reads the entity's stored boundary out of state, and PermissionsBoundaryDecisionDetail is emitted only when the request supplied one. So after PutUserPermissionsBoundary with a read-only boundary, s3:DeleteObject stays `allowed` from the attached `s3:*` policy forever and no PermissionsBoundaryDecisionDetail appears.
Why: a client validating that a boundary caps an over-broad identity policy is told the boundary has no effect — it will either ship a policy it believes is capped when the simulator cannot see the cap, or (worse) conclude the boundary works because the simulator agreed with the uncapped answer.
Fix: the operation's handler — src/ops/simulate_principal_policy.rs — must resolve the PolicySourceArn's attached boundary from src/state.rs when PermissionsBoundaryPolicyInputList is absent, intersect it as a ceiling, and report PermissionsBoundaryDecisionDetail.
Traces: iam.simulate-principal-policy-attached-boundary-caps-allow (verdict mismatch; the check's poll for the capped decision never converges and the harness reports `Check timed out after 60.0s`).

Cause 3 of 10: request-time IAM authorization supplies NO condition context, so every conditioned Allow is undecidable and never grants.
AWS: IAM evaluates a statement's Condition against the request context, which AWS itself populates from the request and the calling principal — aws:PrincipalTag/<key> from the signing principal's tags, aws:RequestTag/<key> from the request body, aws:ResourceTag/<key> from the target resource, aws:SourceIp from the connection. An Allow on iam:ListUserPolicies conditioned on `StringEquals aws:PrincipalTag/team = <v>` therefore permits the call while the principal carries that tag, and is refused AccessDenied once the tag changes (recorded behavior only for the per-key sourcing; the operators are IAM's documented condition grammar).
Emulator: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs builds the request as `iam::Request::new(action, resource, &principals)` — the constructor documented as "a request with no condition context" (`context: &[]`), and `.with_context(..)` is never called. services/common/src/iam_authorization.rs then treats a clause over a key the context does not carry as UNDECIDED, and an undecided statement never grants. So a conditioned Allow can never permit any real signed IAM call, whatever the principal's tags are.
Why: every ABAC grant a client writes is silently inert on this emulator — the call is refused AccessDenied even though AWS permits it, so a tag-based least-privilege design cannot be validated at all, and a client that works around the refusal by widening the policy ships something more permissive than it tested.
Fix: the IAM request authorizer — src/state/authorization.rs — must assemble the condition context (at minimum aws:PrincipalTag/* from the resolved principal's stored tags, aws:RequestTag/* from the operation's tag input, aws:ResourceTag/* from the target entity, aws:SourceIp from the connection) and pass it via `Request::with_context`.
Traces: iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch (verdict mismatch; `Check timed out after 60.0s` — the check polls the signed ListUserPolicies call for success and it is refused forever). This is the identity/condition leg that remains after the authorization gate described as Cause 10 of the open issue 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md was partly stood up; that issue's replay keys are other check ids and it does not cover this one.

Cause 4 of 10: condition operators outside the string family leave a statement undecided, in both the simulator and the authorizer.
AWS: IAM's condition grammar includes the IP operators (IpAddress / NotIpAddress), the numeric, date, Boolean, binary and ARN operators, Null, and the ForAnyValue:/ForAllValues: set qualifiers. `IpAddress aws:SourceIp 192.0.2.0/24` is a decidable clause: SimulatePrincipalPolicy given an aws:SourceIp context entry inside the CIDR reports `allowed`, and widening the CIDR to 0.0.0.0/0 makes the real signed call succeed (recorded behavior for the decisions; the operator set is IAM's documented grammar).
Emulator: services/common/src/iam_authorization.rs decides ONLY `const STRING_OPERATORS: &[&str] = [StringEquals, StringNotEquals, StringEqualsIgnoreCase, StringNotEqualsIgnoreCase, StringLike, StringNotLike]`; `conditions_hold` returns `None` for any other operator ("Everything else — the numeric, date, IP, ARN and Null operators, and the ForAnyValue:/ForAllValues: set qualifiers — leaves the statement undecided"), and an undecided statement never grants. An Allow guarded by IpAddress is therefore permanently implicitDeny in the simulator and permanently AccessDenied on a real call, even when the supplied/actual address is inside the CIDR.
Why: a client cannot express or validate any network-, time- or quantity-scoped grant: the policy it writes denies everything rather than gating, so it is pushed toward removing the condition — exactly the over-permission the condition existed to prevent.
Fix: services/common/src/iam_authorization.rs — extend the operator set (IP/CIDR, numeric, date, Boolean, ARN, Null) and the set qualifiers, keeping the fail-closed reading only for operators that genuinely remain unimplemented; both src/ops/simulate_principal_policy.rs and src/state/authorization.rs then inherit it.
Traces: iam.sep-policy-composition-sourceip-condition-gates-real-call-denied-outside-cidr (verdict mismatch; `Check timed out after 60.0s`). Related and NOT covering it: Cause 10 of 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md lists the date-operator and set-qualifier checks under its own trace ids.

Cause 5 of 10: GetUserPolicy is served to a principal whose policies do not grant iam:GetUserPolicy.
AWS: IAM authorizes every IAM API call against the caller's identity policies. A user whose only grant is `Allow Action: [iam:GetUse?, iam:List*Keys]` is permitted iam:GetUser (the `?` wildcard matches exactly one character) and iam:ListAccessKeys / iam:ListSSHPublicKeys, and is refused iam:GetUserPolicy and iam:ListUserTags with error code `AccessDenied` (HTTP 403), because neither action is in the complement the two patterns name.
Emulator: the signed GetUserPolicy call returned success. The shared matcher is correct about `?` (services/common/src/iam_authorization.rs `wildcard_match`: "`?` (exactly one character)"), so the grant cannot be what permitted it. Hypothesis: the GetUserPolicy handler does not call `crate::state::authorization::authorize` at all, i.e. the authorization gate is applied per-operation and this operation is outside it.
Why: an action the caller was never granted succeeds, and that action discloses the content of an inline identity policy. Any least-privilege assertion a client makes about this operation passes vacuously here and fails in production the other way round — the emulator is strictly more permissive than AWS.
Fix: the operation's handler — src/ops/get_user_policy.rs — must authorize `iam:GetUserPolicy` on the user ARN through src/state/authorization.rs before reading state; better, the gate belongs ahead of the operation handlers (src/service.rs) so no operation can be added without one.
Traces: iam.sep-policy-composition-action-wildcards-match-exactly-the-documented-set-denied-outside (the check reports `'iam:GetUserPolicy': 'OK'` where `AccessDenied` is required).

Cause 6 of 10: ListSSHPublicKeys is an unimplemented 501 scaffold.
AWS: Smithy binds ListSSHPublicKeys to the service; it takes UserName/Marker/MaxItems and returns SSHPublicKeys plus IsTruncated, with NoSuchEntityException (404) as its only client error. There is no modeled "not implemented" outcome, so a request naming an existing user must get HTTP 200 with the (possibly empty) key list.
Emulator: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_ssh_public_keys.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4 and answers HTTP 501 with error code `NotImplemented` (message e.g. "operation ListSSHPublicKeys is scaffolded but not implemented in this emulator" — quoted as an example). No open issue names this operation (the filed SSH-key stub issue is for UploadSSHPublicKey), and the check caught the error internally so the replay report did not mark it.
Why: a client that enumerates a user's CodeCommit SSH keys gets an unmodeled 501 the SDKs surface as a hard non-retryable error, where AWS returns 200; and because the call is refused, it cannot serve as the positive leg of an authorization comparison.
Fix: the operation's handler — src/ops/list_ssh_public_keys.rs — must return the user's SSH public keys from src/state.rs (NoSuchEntityException when the user does not exist), and the SCAFFOLDED marker line must then be deleted.
Traces: iam.sep-policy-composition-action-wildcards-match-exactly-the-documented-set-denied-outside (reports `'iam:ListSSHPublicKeys': 'NotImplemented'` where `OK` is required).

Cause 7 of 10: long-term access key ids are minted with an `EMULATOR` prefix instead of AWS's `AKIA`.
AWS: accessKeyIdType is `^[\w]+$`, length 16..128; AWS's documented convention is that a long-term IAM user key id begins `AKIA` (a temporary STS credential begins `ASIA`) and is 20 characters. Recorded behavior confirms a 20-character AKIA-prefixed id.
Emulator: CreateAccessKey returned `EMULATORAKWJSZVDW65Z` — 20 characters, but prefixed `EMULATORAK`. The random suffix is not the complaint; the prefix rule is.
Why: SDK and credential tooling that classifies a credential by its prefix (long-term vs session) misroutes these keys, and anything asserting the documented identifier shape fails.
Fix: the id minting in src/ops/create_access_key.rs (or the shared id generator in src/state.rs). This is the same root cause as Cause 7 of the open issue 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md, whose trace ids are iam.iam-access-key-crud and iam.user-access-key-round-trips-status-and-never-used only; this check is a third unit it does not re-replay, which is why it is recorded here.
Traces: iam.minted-identifier-prefixes-and-arn-paths.

Cause 8 of 10: an invalid ListGroups Marker is refused with the error code `ValidationException`, which this protocol has no such code for.
AWS: ListGroups' Marker targets markerType (@length 1..320, pattern `^[\u0020-\u00FF]+$`) and the operation models only ServiceFailureException; IAM is an awsQuery service, so a Marker that fails request validation is refused with the query-protocol constraint code `ValidationError` at HTTP 400. `ValidationException` is not a code in this service's model at all.
Emulator: the refusal carried error code `ValidationException`.
Why: a client that branches on the error code — retry vs. reset pagination vs. report a bad token — never matches `ValidationError`, so an invalid continuation token is handled as an unknown failure; the HTTP status is right and only the code is wrong, which makes it silent.
Fix: the service's error channel / awsQuery error serializer (src/scaffolded.rs and the validation layer that raises it) must emit `ValidationError` for member-constraint violations on this protocol.
Traces: iam.op-contract-listGroups-path-prefix (`unexpected error code ValidationException`).

Cause 9 of 10: a malformed PolicyArn on AttachUserPolicy/DetachUserPolicy is refused `InvalidInput` instead of `ValidationError`.
AWS: AttachUserPolicy's PolicyArn targets arnType (@length 20..2048) and the operation models InvalidInputException (awsQuery code `InvalidInput`, 400), NoSuchEntityException (404), PolicyNotAttachable (400), LimitExceeded (409). A PolicyArn that violates the modeled member constraint is rejected before the handler, with the query-protocol constraint code `ValidationError` (400); `InvalidInput` is what AWS uses for a semantically bad but well-formed input.
Emulator: the refusal carried error code `InvalidInput`.
Why: a client cannot distinguish "this string is not an ARN" (fix the caller) from "this ARN is not acceptable here" (fix the policy), so attach/detach failures are misreported and retried pointlessly.
Fix: ARN member-constraint checking belongs in the request validation layer ahead of src/ops/attach_user_policy.rs and src/ops/detach_user_policy.rs, raising `ValidationError` 400; the handler keeps `InvalidInput` for its own semantic refusals.
Traces: iam.invalid-policy-arn-on-attach-detach-rejected (`AttachUserPolicy: expected ValidationError for a malformed ARN, got InvalidInput`).

Cause 10 of 10: the reserved-tag-prefix refusal replaces AWS's fixed sentence with generated prose that echoes the request.
AWS: TagInstanceProfile models InvalidInputException (`InvalidInput`, 400) and refuses any tag key beginning `aws:`. Recorded behavior: the message is the fixed sentence AWS emits for every reserved-prefix refusal, `Tag keys beginning with aws: are reserved for system use.` — a constant for the refusal class, not built from the request, which is why it is a requirement here rather than an example.
Emulator: status and error code are correct (`InvalidInput`), but the message is generated per request and names the offending key: `Tag key aws:reserved-marker is reserved: keys must not begin with aws:`.
Why: the smallest-impact cause in this bundle — a client that surfaces or matches AWS's reserved-prefix wording (log scrapers, user-facing validation messages, golden-response comparisons) does not recognise it.
Fix: the reserved-prefix refusal in src/ops/tag_instance_profile.rs (and the shared tag-key validator the other Tag* operations use) must emit the fixed sentence.
Traces: iam.tag-instance-profile-reserved-prefix-rejected.

## Evidence
Cause 1 — replay output:
  [mismatch] iam.asgeni-simulate-secrets-only-role-eni-actions-denied
  AssertionError: secretsmanager:DescribeSecret was allowed by [], expected the one inline policy
  [mismatch] iam.op-contract-listRoles-missing-permission-denied
  AssertionError: the deny must be attributed to the stored inline policy that carries it, and to nothing else: expected {'user_<user>_<deny-policy>'}, got []
  [mismatch] iam.team-role-deny-simulates-reads-explicit-delivery-implicit
  AssertionError: logs:FilterLogEvents on 'arn:aws:logs:<region>:<account>:log-group:/aws/vendedlogs/...': ('explicitDeny', [])
Expected (AWS): EvaluationResult.MatchedStatements present, each Statement carrying SourcePolicyId / SourcePolicyType. Actual (emulator): the key is absent — source, services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/simulate_principal_policy.rs line 629 `matched_statements: None,` (EvaluationResult) and line 829 `matched_statements: None,` (ResourceSpecificResult), with the comment "`MatchedStatements` reports the SOURCE POSITION (line and column) of each statement that matched, which the shared evaluator does not track ... the member is left absent." Note the decision itself was correct in all three (allowed / explicitDeny), so only the attribution is missing.

Cause 2 — replay output:
  [mismatch] iam.simulate-principal-policy-attached-boundary-caps-allow
  Error: Check timed out after 60.0s
The check attaches `Allow s3:* on *`, polls until s3:DeleteObject is `allowed`, then PutUserPermissionsBoundary with `Allow [s3:GetObject, s3:ListBucket] on *`, asserts GetUser echoes User.PermissionsBoundary.PermissionsBoundaryArn (it does), and polls for s3:DeleteObject to become `implicitDeny` with PermissionsBoundaryDecisionDetail.AllowedByPermissionsBoundary = false. That poll can never converge. Source, src/ops/simulate_principal_policy.rs: `let boundary = input.permissions_boundary_policy_input_list.unwrap_or_default();` then `decide(&prepared, &prepared_boundary, boundary.is_empty(), ...)`; in `decide`, `let allowed_by_boundary = no_boundary || iam::is_allowed(...)` and `permissions_boundary_decision_detail: (!no_boundary).then_some(..)`. Nothing in the file reads the entity's stored boundary.

Cause 3 — replay output:
  [mismatch] iam.sep-policy-composition-principal-tag-condition-simulated-with-real-tags-denied-on-mismatch
  Error: Check timed out after 60.0s
The check creates the user with Tags=[{team: v1}], puts `Allow iam:ListUserPolicies on <user arn> Condition StringEquals aws:PrincipalTag/team = v1`, mints a key, and polls the signed ListUserPolicies call for success. Source, src/state/authorization.rs line 111: `let request = iam::Request::new(action, resource, &principals);` — `Request::new` is documented as "A request with no condition context" and sets `context: &[]`; `with_context` is never called anywhere in that file. services/common/src/iam_authorization.rs `conditions_hold` returns `None` when "a key the request context does not carry" is tested, and an undecided Allow never grants. Expected (AWS): the signed call succeeds while the tag matches and is refused with code `AccessDenied` after the retag. Actual: refused on every poll.

Cause 4 — replay output:
  [mismatch] iam.sep-policy-composition-sourceip-condition-gates-real-call-denied-outside-cidr
  Error: Check timed out after 60.0s
Source, services/common/src/iam_authorization.rs lines 64-74: "The string condition operators this evaluator decides. Everything else — the numeric, date, IP, ARN and `Null` operators, and the `ForAnyValue:`/`ForAllValues:` set qualifiers — leaves the statement undecided", `const STRING_OPERATORS: &[&str] = &["StringEquals", "StringNotEquals", "StringEqualsIgnoreCase", "StringNotEqualsIgnoreCase", "StringLike", "StringNotLike"];` and lines 468-470 `if !STRING_OPERATORS.contains(&operator.as_str()) { return None; }`. The module's own unit test asserts `{"DateLessThan":{"aws:CurrentTime":...}}` yields Decision::NotStated. Expected (AWS): with a ContextEntry for aws:SourceIp inside 192.0.2.0/24 the simulator reports `allowed`; after widening the policy to 0.0.0.0/0 the real signed ListAccessKeys succeeds. Actual: never `allowed`, never `OK`.

Cause 5 and 6 — replay output:
  [mismatch] iam.sep-policy-composition-action-wildcards-match-exactly-the-documented-set-denied-outside
  AssertionError: real signed calls under `iam:GetUse?` + `iam:List*Keys` returned {'iam:GetUser': 'OK', 'iam:ListAccessKeys': 'OK', 'iam:ListSSHPublicKeys': 'NotImplemented', 'iam:GetUserPolicy': 'OK', 'iam:ListUserTags': 'NotImplemented'}
Expected (AWS): {'iam:GetUser': 'OK', 'iam:ListAccessKeys': 'OK', 'iam:ListSSHPublicKeys': 'OK', 'iam:GetUserPolicy': 'AccessDenied', 'iam:ListUserTags': 'AccessDenied'}. GetUserPolicy must be refused with error code `AccessDenied` (403): `iam:GetUse?` matches exactly one trailing character, and services/common/src/iam_authorization.rs `wildcard_match` implements `?` as "exactly one character", so the grant does not cover it. ListSSHPublicKeys: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_ssh_public_keys.rs line 4 `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented``. (The ListUserTags 501 in the same row is the already-filed scaffold 20261025T000319Z-iam-listusertags-stub.md and is not part of this issue.)

Cause 7 — replay output:
  [mismatch] iam.minted-identifier-prefixes-and-arn-paths
  AssertionError: access key unique id must carry the documented 'AKIA' prefix, got 'EMULATORAKWJSZVDW65Z'
Expected: a 20-character id beginning `AKIA`. Actual: `EMULATORAK` + 10 characters.

Cause 8 — replay output:
  [mismatch] iam.op-contract-listGroups-path-prefix
  AssertionError: unexpected error code ValidationException
Expected: error code `ValidationError`, HTTP 400 (awsQuery constraint violation on markerType). Actual: `ValidationException`, a code absent from com.amazonaws.iam.

Cause 9 — replay output:
  [mismatch] iam.invalid-policy-arn-on-attach-detach-rejected
  AssertionError: AttachUserPolicy: expected ValidationError for a malformed ARN, got InvalidInput
Expected: `ValidationError` 400 for a PolicyArn violating arnType (@length 20..2048). Actual: `InvalidInput` 400 (the handler's semantic refusal code).

Cause 10 — replay output:
  [mismatch] iam.tag-instance-profile-reserved-prefix-rejected
  AssertionError: expected message 'Tag keys beginning with aws: are reserved for system use.', got 'Tag key aws:reserved-marker is reserved: keys must not begin with aws:'
The error code (`InvalidInput`) and status (400) already match; only the fixed refusal sentence diverges.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (the `iam` service is bootstrapped and serving).
2. Cause 1: CreateUser (or CreateRole), PutUserPolicy/PutRolePolicy with one inline document that Allows a named action on the entity's own ARN, then SimulatePrincipalPolicy(PolicySourceArn=<entity arn>, ActionNames=[that action], ResourceArns=[<entity arn>]). Observe EvalDecision `allowed` with no MatchedStatements member. Repeat with an inline Deny and observe `explicitDeny` with no MatchedStatements.
3. Cause 2: CreateUser; CreatePolicy `Allow s3:* on *` and AttachUserPolicy it; SimulatePrincipalPolicy for s3:GetObject and s3:DeleteObject — both `allowed`. CreatePolicy `Allow [s3:GetObject, s3:ListBucket] on *` and PutUserPermissionsBoundary with it; GetUser confirms User.PermissionsBoundary.PermissionsBoundaryArn. Simulate again with NO PermissionsBoundaryPolicyInputList: s3:DeleteObject must become `implicitDeny` with PermissionsBoundaryDecisionDetail.AllowedByPermissionsBoundary=false; it stays `allowed` with no such detail. DeleteUserPermissionsBoundary must restore `allowed`.
4. Cause 3: CreateUser with Tags=[{Key:team, Value:v1}]; PutUserPolicy `Allow iam:GetUser on <user arn>` and a second document `Allow iam:ListUserPolicies on <user arn> Condition {StringEquals: {"aws:PrincipalTag/team": "v1"}}`; CreateAccessKey and sign as that user. GetUser succeeds (unconditioned Allow); ListUserPolicies is refused `AccessDenied` although the tag matches — AWS permits it. Retag to v2 and AWS then refuses it.
5. Cause 4: on the same user, PutUserPolicy `Allow iam:ListAccessKeys on <user arn> Condition {IpAddress: {"aws:SourceIp": "0.0.0.0/0"}}` and call ListAccessKeys with that key — refused `AccessDenied` although every address is inside the CIDR. SimulatePrincipalPolicy with ContextEntries aws:SourceIp=any address likewise reports implicitDeny instead of allowed.
6. Cause 5/6: CreateUser; PutUserPolicy a single document `Allow Action: ["iam:GetUse?", "iam:List*Keys"] Resource: <user arn>`; CreateAccessKey and sign as that user. GetUserPolicy(UserName=<user>, PolicyName=<that policy>) succeeds — AWS refuses it `AccessDenied` 403. ListSSHPublicKeys(UserName=<user>) answers HTTP 501 `NotImplemented` — AWS returns 200.
7. Cause 7: CreateAccessKey for a fresh user and inspect AccessKeyId — it begins `EMULATORAK`, not `AKIA`.
8. Cause 8: ListGroups(Marker="not-a-marker-iam-ever-issued") — observe error code `ValidationException`; AWS answers `ValidationError` 400.
9. Cause 9: AttachUserPolicy(UserName=<user>, PolicyArn="not-an-arn") and the same for DetachUserPolicy — observe `InvalidInput`; AWS answers `ValidationError` 400.
10. Cause 10: CreateInstanceProfile, then TagInstanceProfile(Tags=[{Key:"aws:reserved-marker", Value:"x"}]) — the code is `InvalidInput` (correct) but the message is generated prose naming the key instead of AWS's fixed sentence `Tag keys beginning with aws: are reserved for system use.`
11. Equivalently: re-replay the check ids in trace_ids.
