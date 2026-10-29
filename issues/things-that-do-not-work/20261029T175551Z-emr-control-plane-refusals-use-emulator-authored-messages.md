Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.block-public-access-configuration-overlapping-ranges-rejected emr.op-block-public-access-rejects-inverted-port-range emr.delete-security-configuration-then-describe-fails emr.op-security-configuration-delete-is-not-idempotent emr.op-duplicate-security-configuration-name-fails emr.op-malformed-security-configuration-json-fails emr.security-configuration-malformed-json-rejected emr.op-describe-release-label-unknown-fails emr.op-list-supported-instance-types-unknown-release-fails emr.get-session-invalid-session-id-error-semantics emr.get-session-unknown-id-invalid-request emr.op-managed-scaling-policy-ops-reject-unknown-cluster emr.set-unhealthy-node-replacement-error-not-swallowed
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.block-public-access-configuration-overlapping-ranges-rejected|run#12|AssertionError|assert message == expected_message, f"unexpected rejection message: {message!r}"
Issue-Key: emr.op-block-public-access-rejects-inverted-port-range|run#11|AssertionError|assert message == expected_message, f"unexpected rejection message: {message!r}"
Issue-Key: emr.delete-security-configuration-then-describe-fails|run#8|AssertionError|assert message == f'Security configuration with name "{name}" does not exist.', message
Issue-Key: emr.op-security-configuration-delete-is-not-idempotent|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: emr.op-duplicate-security-configuration-name-fails|run#8|AssertionError|assert message == f"SecurityConfiguration with name '{name}' already exists.", message
Issue-Key: emr.op-malformed-security-configuration-json-fails|run#6|AssertionError|assert message == "Invalid security configuration; Malformed Json string.", (
Issue-Key: emr.security-configuration-malformed-json-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: emr.op-describe-release-label-unknown-fails|run#9|AssertionError|assert message == "The release label isn't valid.", message
Issue-Key: emr.op-list-supported-instance-types-unknown-release-fails|run#6|AssertionError|assert message == "The supplied release label is invalid: emr-99.99.0.", (
Issue-Key: emr.get-session-invalid-session-id-error-semantics|run#6|AssertionError|assert code == "ValidationException", f"CODE={code!r}"
Issue-Key: emr.get-session-unknown-id-invalid-request|run#12|AssertionError|assert msg_unknown_session == f"Session id '{fake_session_id}' is not valid.", (
Issue-Key: emr.op-managed-scaling-policy-ops-reject-unknown-cluster|_assert_rejections#4|AssertionError|assert not wrong, (
Issue-Key: emr.set-unhealthy-node-replacement-error-not-swallowed|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# EMR control-plane refusals use emulator-authored messages (and one wrong error code) for 10 distinct validation paths

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBlockPublicAccessConfiguration

## Description
This issue bundles 10 net-new root causes, all in the EMR (elasticmapreduce) crate: each is one refusal path whose HTTP status and error code are right (except Cause 7, where the code is wrong too) but whose `Error.Message` is text the emulator invented rather than the message real EMR words for that error class. A client that matches on the message (the AWS CLI surfaces it, and SDK retry/branch logic and operator runbooks key on it) cannot recognise the refusal.

For every cause below, the fixed text of the message is the requirement and the variable parts are taken from the request/state, as marked.

--- Cause 1 of 10: PutBlockPublicAccessConfiguration accepts no AWS wording for an inverted port range ---
AWS: Smithy: PutBlockPublicAccessConfiguration input BlockPublicAccessConfiguration.PermittedPublicSecurityGroupRuleRanges -> PortRanges of PortRange{MinRange,MaxRange: Port @range min -1 max 65535}; modelled error InvalidRequestException (client, HTTP 400). The model does not state the ordering rule, so recorded behavior only: real EMR refuses a range whose MinRange exceeds MaxRange with InvalidRequestException/400 and the message `The specified port range (<MinRange>, <MaxRange>) is not valid. Min Range must be less than or equal to Max Range.` Fixed text: `The specified port range (` ... `) is not valid. Min Range must be less than or equal to Max Range.`; variable parts: the two integers, in request order, taken from the offending PortRange.
Emulator: returns InvalidRequestException/400 (code correct) with `PermittedPublicSecurityGroupRuleRanges carries the port range 200-100: MaxRange must be at least MinRange` (and `...90-20: ...` for the other check) — a different sentence, a different format for the pair, and it names the member instead of the range.
Why: a client/operator matching AWS's sentence (or parsing the `(min, max)` pair out of it) sees an unrecognised refusal and cannot tell an inverted-range rejection from any other InvalidRequestException.
Fix: the operation's validation in the EMR crate's PutBlockPublicAccessConfiguration handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/put_block_public_access_configuration.rs), where the PortRange ordering check builds its message.
Traces: emr.block-public-access-configuration-overlapping-ranges-rejected, emr.op-block-public-access-rejects-inverted-port-range.

--- Cause 2 of 10: the security-configuration not-found message quotes the name with the wrong quote characters ---
AWS: Smithy: DescribeSecurityConfiguration / DeleteSecurityConfiguration take Name (XmlString) and model InvalidRequestException (client, 400). Recorded behavior only for the text: AWS refuses a name that does not exist with InvalidRequestException/400 and the message `Security configuration with name "<Name>" does not exist.` — fixed text `Security configuration with name "` / `" does not exist.` with the name DOUBLE-quoted; variable part: the Name from the request.
Emulator: same code and status, but the message single-quotes the name: `Security configuration with name 'sim2real-behav-emr-secconf-del-19def777' does not exist.` (the name itself is a per-run value; the divergence is the quoting of the template).
Why: an exact-match or regex assertion on AWS's message fails, so callers that classify not-found by message (both of these checks do) mis-handle the refusal.
Fix: the shared not-found refusal in the EMR security-configuration handlers (describe_security_configuration.rs / delete_security_configuration.rs) or the state lookup helper they share.
Traces: emr.delete-security-configuration-then-describe-fails, emr.op-security-configuration-delete-is-not-idempotent.

--- Cause 3 of 10: CreateSecurityConfiguration's duplicate-name refusal is reworded ---
AWS: Smithy: CreateSecurityConfiguration(Name required, SecurityConfiguration required), error InvalidRequestException (client, 400). Recorded behavior only for the text: a name that already exists is refused with InvalidRequestException/400 and `SecurityConfiguration with name '<Name>' already exists.` — fixed text `SecurityConfiguration with name '` / `' already exists.` (note: the shape name `SecurityConfiguration`, single-quoted name); variable part: the Name from the request.
Emulator: `Security configuration with name 'sim2real-behav-emr-secconf-dup-2dc7ca4e' already exists.` — the leading noun phrase is spelled as prose (`Security configuration`) instead of the shape name AWS uses.
Why: callers that treat the duplicate-name message as the idempotency signal for create-or-adopt logic do not recognise it.
Fix: the duplicate-name branch of the CreateSecurityConfiguration handler in the EMR crate.
Traces: emr.op-duplicate-security-configuration-name-fails.

--- Cause 4 of 10: a malformed SecurityConfiguration document is refused with a parser diagnostic instead of AWS's message ---
AWS: Smithy: CreateSecurityConfiguration.SecurityConfiguration is a plain String; error InvalidRequestException (client, 400). Recorded behavior only for the text: AWS refuses a non-JSON document with InvalidRequestException/400 and the CONSTANT message `Invalid security configuration; Malformed Json string.` — it has no variable part and deliberately leaks nothing about the parser or the input.
Emulator: returns its own serde diagnostic, which varies with the input: `SecurityConfiguration is not valid JSON: expected ident at line 1 column 2` and `SecurityConfiguration is not valid JSON: key must be a string at line 1 column 2`.
Why: the message is not AWS's and is not even stable across inputs, so no client can match it; it also exposes emulator-internal parser text.
Fix: the JSON-validation branch of the CreateSecurityConfiguration handler — map every parse failure onto the single constant message.
Traces: emr.op-malformed-security-configuration-json-fails, emr.security-configuration-malformed-json-rejected.

--- Cause 5 of 10: DescribeReleaseLabel's unknown-release-label message is emulator-authored ---
AWS: Smithy: DescribeReleaseLabel(ReleaseLabel: String), error InvalidRequestException (client, 400). Recorded behavior only for the text: AWS answers an unknown release label with InvalidRequestException/400 and the CONSTANT message `The release label isn't valid.` (no variable part — it does NOT echo the label).
Emulator: `Invalid release label: emr-0.0.0-does-not-exist` — reworded and echoing the request value AWS omits.
Why: clients probing release-label validity by message (and the suite) cannot match it; echoing the input also diverges from AWS's deliberate non-echo.
Fix: the unknown-label branch of the DescribeReleaseLabel handler in the EMR crate.
Traces: emr.op-describe-release-label-unknown-fails.

--- Cause 6 of 10: ListSupportedInstanceTypes' unknown-release-label message is emulator-authored ---
AWS: Smithy: ListSupportedInstanceTypes(ReleaseLabel required), error InvalidRequestException (client, 400). Recorded behavior only for the text: `The supplied release label is invalid: <ReleaseLabel>.` — fixed text `The supplied release label is invalid: ` plus the requested label and a trailing period; variable part: the ReleaseLabel from the request. Note this operation's wording differs from DescribeReleaseLabel's (Cause 5): the two are not interchangeable.
Emulator: `Release label 'emr-99.99.0' is not valid.`
Why: a caller cannot distinguish an invalid release label from any other InvalidRequestException on this operation.
Fix: the unknown-label branch of the ListSupportedInstanceTypes handler in the EMR crate.
Traces: emr.op-list-supported-instance-types-unknown-release-fails.

--- Cause 7 of 10: GetSession never shape-validates SessionId, so a malformed id gets the cluster-lookup refusal (wrong CODE and message) ---
AWS: Smithy: GetSession(ClusterId required @length 0..256, SessionId required @length 0..256), modelled errors InternalServerException (500) and InvalidRequestException (400); ValidationException is not modelled, so recorded behavior only: real EMR validates the SessionId's SHAPE before resolving the cluster and refuses a syntactically impossible SessionId (spaces, slashes, control characters) with code `ValidationException` and the message `sessionId '<ClusterId>/session/<SessionId>' is not valid.` — fixed text `sessionId '` / `/session/` / `' is not valid.`; variable parts: the ClusterId and the SessionId exactly as sent. A well-formed-but-unknown SessionId on the same unknown cluster instead reaches lookup and is refused InvalidRequestException (the check's own positive control, which the emulator passes).
Emulator: returns `InvalidRequestException` with the cluster-lookup message for the malformed id too — i.e. it resolves the cluster first and has no input-shape gate at all, so the malformed case is indistinguishable from the unknown-cluster case.
Why: this is a wrong error CODE, not just wrong text: a client branching on ValidationException (bad input, do not retry, fix the request) vs InvalidRequestException (bad target) takes the wrong branch, and no client can detect a malformed session id.
Fix: add the SessionId shape validation ahead of cluster resolution in the GetSession handler of the EMR crate (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_session.rs), emitting ValidationException.
Traces: emr.get-session-invalid-session-id-error-semantics.

--- Cause 8 of 10: GetSession's unknown-session refusal invents a composite message ---
AWS: Smithy: GetSession errors include InvalidRequestException (client, 400). Recorded behavior only for the text: with a REAL ClusterId and an unknown SessionId, AWS refuses with InvalidRequestException/400 and `Session id '<SessionId>' is not valid.` — fixed text `Session id '` / `' is not valid.`; variable part: the SessionId from the request only. (With an unknown ClusterId it instead says `Cluster id '<ClusterId>' is not valid.`, which the emulator gets right.)
Emulator: `Session id 'is-ad6bbdf71f480' is not valid or does not exist on cluster 'j-0000000000001'.` — extra clause and a second interpolated value AWS does not include.
Why: callers matching the session-not-valid sentence (and anything parsing the id out of it) fail; the extra `or does not exist on cluster ...` clause is emulator-authored.
Fix: the unknown-session branch of the GetSession handler in the EMR crate.
Traces: emr.get-session-unknown-id-invalid-request.

--- Cause 9 of 10: Put/RemoveManagedScalingPolicy use the Get-shaped message for an unknown cluster ---
AWS: Smithy: PutManagedScalingPolicy / RemoveManagedScalingPolicy (ClusterId required) model NO errors, so recorded behavior only: all three managed-scaling operations refuse an unknown cluster with code `ValidationException`, but with DIFFERENT messages — GetManagedScalingPolicy says `Cluster id '<ClusterId>' is not valid.` (variable part: the ClusterId), while PutManagedScalingPolicy and RemoveManagedScalingPolicy say the CONSTANT `Specified job flow ID not valid.` (no variable part, no id echoed). An inverted ComputeLimits policy on an unknown cluster is still refused with the same constant.
Emulator: answers all three with the Get-shaped `Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.` (code ValidationException is correct). Only GetManagedScalingPolicy is right by accident.
Why: the per-operation message mapping is wrong, so a client cannot match the documented wording of the mutating calls.
Fix: the unknown-cluster refusal in put_managed_scaling_policy.rs / remove_managed_scaling_policy.rs of the EMR crate (stop sharing GetManagedScalingPolicy's text).
Traces: emr.op-managed-scaling-policy-ops-reject-unknown-cluster.

--- Cause 10 of 10: SetUnhealthyNodeReplacement uses the job-flow-shaped message where AWS uses the cluster-id one (the mirror of Cause 9) ---
AWS: Smithy: SetUnhealthyNodeReplacement(JobFlowIds required, UnhealthyNodeReplacement required) models only InternalServerError, so recorded behavior only: a well-formed but nonexistent JobFlowId is refused with code `ValidationException` and the message `Cluster id '<JobFlowId>' is not valid.` — fixed text `Cluster id '` / `' is not valid.`; variable part: the id from JobFlowIds. DescribeCluster on the same id independently refuses with InvalidRequestException and the SAME sentence.
Emulator: `Specified job flow ID not valid: 'j-5C980F9603334'` — i.e. the constant AWS reserves for the managed-scaling mutators, with an id appended that AWS does not append there.
Why: together with Cause 9 the two unknown-cluster sentences are swapped across operations, so neither matches AWS on the operation that uses it.
Fix: the unknown-id refusal in the SetUnhealthyNodeReplacement handler of the EMR crate.
Traces: emr.set-unhealthy-node-replacement-error-not-swallowed.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (service emr, 35/201 checks passing):

Cause 1 — emr.block-public-access-configuration-overlapping-ranges-rejected: expected InvalidRequestException + 'The specified port range (200, 100) is not valid. Min Range must be less than or equal to Max Range.'; got InvalidRequestException + 'PermittedPublicSecurityGroupRuleRanges carries the port range 200-100: MaxRange must be at least MinRange'. emr.op-block-public-access-rejects-inverted-port-range: expected '... (90, 20) ...'; got 'PermittedPublicSecurityGroupRuleRanges carries the port range 90-20: MaxRange must be at least MinRange'.

Cause 2 — emr.delete-security-configuration-then-describe-fails: AssertionError on `message == f'Security configuration with name "{name}" does not exist.'`; got "Security configuration with name 'sim2real-behav-emr-secconf-del-19def777' does not exist.". emr.op-security-configuration-delete-is-not-idempotent: expected message 'Security configuration with name "sim2real-behav-emr-secconf-twice-6e2db026" does not exist.', got "Security configuration with name 'sim2real-behav-emr-secconf-twice-6e2db026' does not exist.".

Cause 3 — emr.op-duplicate-security-configuration-name-fails: expected "SecurityConfiguration with name '<name>' already exists.", got "Security configuration with name 'sim2real-behav-emr-secconf-dup-2dc7ca4e' already exists.".

Cause 4 — emr.op-malformed-security-configuration-json-fails: expected 'Invalid security configuration; Malformed Json string.', got 'SecurityConfiguration is not valid JSON: expected ident at line 1 column 2'. emr.security-configuration-malformed-json-rejected: same expectation, got 'SecurityConfiguration is not valid JSON: key must be a string at line 1 column 2'.

Cause 5 — emr.op-describe-release-label-unknown-fails: expected "The release label isn't valid.", got 'Invalid release label: emr-0.0.0-does-not-exist'.

Cause 6 — emr.op-list-supported-instance-types-unknown-release-fails: expected 'The supplied release label is invalid: emr-99.99.0.', got "Release label 'emr-99.99.0' is not valid.".

Cause 7 — emr.get-session-invalid-session-id-error-semantics: GetSession(ClusterId='j-INVALIDCLUSTERID1', SessionId='not a valid !! session /// id \x01\x02') expected code ValidationException and message "sessionId 'j-INVALIDCLUSTERID1/session/not a valid !! session /// id \x01\x02' is not valid."; got code 'InvalidRequestException' (AssertionError: CODE='InvalidRequestException'). The check's positive control (well-formed unknown SessionId -> InvalidRequestException + "Cluster id 'j-INVALIDCLUSTERID1' is not valid.") passed, proving the emulator never shape-validates SessionId.

Cause 8 — emr.get-session-unknown-id-invalid-request: real cluster + unknown session expected "Session id '<SessionId>' is not valid."; got "Session id 'is-ad6bbdf71f480' is not valid or does not exist on cluster 'j-0000000000001'.". The unknown-CLUSTER leg of the same check passed.

Cause 9 — emr.op-managed-scaling-policy-ops-reject-unknown-cluster: expected (ValidationException, 'Specified job flow ID not valid.') for PutManagedScalingPolicy, PutManagedScalingPolicy-with-inverted-limits and RemoveManagedScalingPolicy; got (ValidationException, "Cluster id 'j-ZZZZZZZZZZZZZ' is not valid.") for all three. GetManagedScalingPolicy (which does expect the Cluster-id sentence) matched.

Cause 10 — emr.set-unhealthy-node-replacement-error-not-swallowed: expected (ValidationException, "Cluster id 'j-5C980F9603334' is not valid."); got message "Specified job flow ID not valid: 'j-5C980F9603334'".

## Steps to Reproduce
Boot the emulator and replay these checks from suite simparity-behavioral-aws-v1 (service token `emr`):
1. emr.block-public-access-configuration-overlapping-ranges-rejected and emr.op-block-public-access-rejects-inverted-port-range — PutBlockPublicAccessConfiguration with PermittedPublicSecurityGroupRuleRanges=[{MinRange:200,MaxRange:100}] (resp. 90/20) and compare Error.Message.
2. emr.delete-security-configuration-then-describe-fails / emr.op-security-configuration-delete-is-not-idempotent — CreateSecurityConfiguration, DeleteSecurityConfiguration, then DescribeSecurityConfiguration (resp. DeleteSecurityConfiguration again) on the same name.
3. emr.op-duplicate-security-configuration-name-fails — CreateSecurityConfiguration twice with the same Name and different bodies.
4. emr.op-malformed-security-configuration-json-fails / emr.security-configuration-malformed-json-rejected — CreateSecurityConfiguration with SecurityConfiguration='this-is-not-json' (resp. '{not valid json').
5. emr.op-describe-release-label-unknown-fails — DescribeReleaseLabel(ReleaseLabel='emr-0.0.0-does-not-exist').
6. emr.op-list-supported-instance-types-unknown-release-fails — ListSupportedInstanceTypes(ReleaseLabel='emr-99.99.0').
7. emr.get-session-invalid-session-id-error-semantics — GetSession with a malformed SessionId containing spaces/slashes/control characters; assert code ValidationException.
8. emr.get-session-unknown-id-invalid-request — launch a cluster, then GetSession(ClusterId=<real>, SessionId='is-0000000000000').
9. emr.op-managed-scaling-policy-ops-reject-unknown-cluster — Get/Put/RemoveManagedScalingPolicy with ClusterId='j-ZZZZZZZZZZZZZ'.
10. emr.set-unhealthy-node-replacement-error-not-swallowed — SetUnhealthyNodeReplacement(JobFlowIds=['j-<13 hex upper>'], UnhealthyNodeReplacement=true).
