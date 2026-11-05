Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-block-public-access-configuration
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-block-public-access-configuration elasticmapreduce-cluster-scaling-and-policy-errors elasticmapreduce-interactive-session-error-paths elasticmapreduce-notebook-execution-error-paths elasticmapreduce-release-label-and-instance-type-catalog elasticmapreduce-run-job-flow-step-concurrency-level-out-of-range elasticmapreduce-security-configuration-crud
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|PutBlockPublicAccessConfiguration|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|PutManagedScalingPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|RemoveManagedScalingPolicy|4xx->4xx|$.message
Issue-Key: elasticmapreduce|GetSession|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|GetSession|4xx->4xx|$.message
Issue-Key: elasticmapreduce|GetClusterSessionCredentials|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|StartNotebookExecution|4xx->4xx|$.__type
Issue-Key: elasticmapreduce|StartNotebookExecution|4xx->4xx|$.message
Issue-Key: elasticmapreduce|DescribeReleaseLabel|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|DescribeReleaseLabel|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|ListSupportedInstanceTypes|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|ListSupportedInstanceTypes|4xx->4xx|$.Message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->4xx|$.message
Issue-Key: elasticmapreduce|CreateSecurityConfiguration|4xx->4xx|$.ErrorCode
Issue-Key: elasticmapreduce|CreateSecurityConfiguration|4xx->4xx|$.Message
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EMR validation refusals carry the wrong error code or an emulator-authored message

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBlockPublicAccessConfiguration

## Description
This issue bundles 10 net-new root causes in the EMR (elasticmapreduce) crate on INPUT-VALIDATION refusal paths. They share one fault with two faces: the emulator classifies and words validation failures itself instead of reproducing AWS's refusal, so on some paths the error TYPE is wrong (`InvalidRequestException` where AWS raises `ValidationException`, which flips a client's retry/branch decision), and on the rest the status and type are right but `Message` — and where modelled, `ErrorCode` — is emulator-authored text. Causes 2 and 3 are a third face: the right type with ANOTHER operation's message template.

For every cause the fixed text is the requirement and the variable parts are taken from the request, as marked; the concrete ids/labels in the recordings are per-run values.

Why (shared): a client distinguishes "your input is malformed, do not retry, fix the request" (`ValidationException`) from "your target does not resolve" (`InvalidRequestException`) by the error type, and distinguishes WHICH validation failed by `ErrorCode` and `Message`. With the type wrong the caller takes the wrong branch; with the message invented, no caller, CLI wrapper or runbook can classify the refusal at all.

Fix (shared): each operation's input-validation stage in the EMR crate's `src/ops/` handlers — the validation gate must run ahead of state lookup, raise the error type AWS raises, and emit AWS's fixed message text with only the modelled variable parts interpolated. Emulator-internal diagnostics (serde parser text, range-checker prose) must never reach the wire.

--- Cause 1 of 10: PutBlockPublicAccessConfiguration's inverted-port-range message is emulator-authored ---
AWS: Smithy: PutBlockPublicAccessConfiguration input `BlockPublicAccessConfiguration.PermittedPublicSecurityGroupRuleRanges` -> PortRange{MinRange, MaxRange}; modelled error `InvalidRequestException` (client, 400). The model does not state the ordering rule, so recorded behavior only: a range whose MinRange exceeds MaxRange is refused 400 with `The specified port range (<MinRange>, <MaxRange>) is not valid. Min Range must be less than or equal to Max Range.` Fixed text: `The specified port range (` / `, ` / `) is not valid. Min Range must be less than or equal to Max Range.`; variable parts: the two integers in request order from the offending PortRange.
Emulator: `PermittedPublicSecurityGroupRuleRanges carries the port range <min>-<max>: MaxRange must be at least MinRange` — a different sentence, a dash-joined pair instead of AWS's `(min, max)`, and it names the input member rather than stating the rule.
Fix: the PortRange ordering check in the PutBlockPublicAccessConfiguration handler's validation.
Traces: elasticmapreduce-block-public-access-configuration#task#2.

--- Cause 2 of 10: PutManagedScalingPolicy uses the Get-shaped cluster-id message for an unknown cluster ---
AWS: Smithy: PutManagedScalingPolicy(ClusterId required) models no operation-specific error. Recorded behavior only: an unknown cluster is refused 400 with the CONSTANT message `Specified job flow ID not valid.` — no variable part, no id echoed. (GetManagedScalingPolicy on the same id instead says `Cluster id '<ClusterId>' is not valid.`; the two are not interchangeable.)
Emulator: `Cluster id '<ClusterId>' is not valid.` — the Get-shaped template.
Fix: the unknown-cluster refusal in the PutManagedScalingPolicy handler — stop sharing GetManagedScalingPolicy's text.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#7.

--- Cause 3 of 10: RemoveManagedScalingPolicy uses the Get-shaped cluster-id message for an unknown cluster ---
AWS: Smithy: RemoveManagedScalingPolicy(ClusterId required), no modelled op error. Recorded behavior only: 400 with the CONSTANT `Specified job flow ID not valid.`
Emulator: `Cluster id '<ClusterId>' is not valid.`
Fix: as Cause 2, in the RemoveManagedScalingPolicy handler.
Traces: elasticmapreduce-cluster-scaling-and-policy-errors#task#9.

--- Cause 4 of 10: GetSession raises InvalidRequestException and no message where AWS raises ValidationException with a sessionId message (WRONG ERROR CODE) ---
AWS: Smithy: GetSession(ClusterId required @length 0..256, SessionId required @length 0..256); modelled errors `InternalServerException` (500) and `InvalidRequestException` (400). `ValidationException` is not modelled, so recorded behavior only: real EMR shape-validates the SessionId BEFORE resolving the cluster and refuses a syntactically impossible SessionId with `__type` = `ValidationException` and the message `sessionId '<ClusterId>/session/<SessionId>' is not valid.` Fixed text: `sessionId '` / `/session/` / `' is not valid.`; variable parts: the ClusterId and SessionId exactly as sent.
Emulator: `__type` = `InvalidRequestException` and NO `message` member at all — it skips the input-shape gate, goes straight to cluster resolution, and serialises an error body with neither AWS's type nor any message.
Why (specific): this is a wrong error TYPE plus a missing message member — a client branching on `ValidationException` takes the `InvalidRequestException` branch, and with no `message` it cannot even report why.
Fix: add the SessionId shape validation ahead of cluster resolution in the GetSession handler, emitting `ValidationException` with the composite message.
Traces: elasticmapreduce-interactive-session-error-paths#task#3.

--- Cause 5 of 10: GetClusterSessionCredentials omits ErrorCode=InvalidClusterId ---
AWS: Smithy: GetClusterSessionCredentials models `InvalidRequestException` (client, 400) whose `ErrorCode` member (@length 1..256) is documented "The error code associated with the exception." Recorded behavior: an unresolvable cluster is refused 400 with `ErrorCode` = the constant `InvalidClusterId` — note this operation's code differs from the `MalformedClusterId` used by the cluster reads.
Emulator: 400 with the message but `$.ErrorCode` missing entirely.
Fix: populate `ErrorCode` in the GetClusterSessionCredentials handler's refusal.
Traces: elasticmapreduce-interactive-session-error-paths#task#6.

--- Cause 6 of 10: StartNotebookExecution raises InvalidRequestException and no message where AWS raises ValidationException (WRONG ERROR CODE) ---
AWS: Smithy: StartNotebookExecution(ExecutionEngine required, ServiceRole required), modelled errors `InternalServerException` (500) and `InvalidRequestException` (400). Recorded behavior only: when the ExecutionEngine names no usable EMR cluster, AWS refuses 400 with `__type` = `ValidationException` and the message `The EMR cluster Id <ExecutionEngine.Id> is not available` — fixed text `The EMR cluster Id ` / ` is not available` (no quoting around the id, no trailing period); variable part: the cluster id from `ExecutionEngine.Id`, which is rendered as the literal `null` when the member is absent.
Emulator: `__type` = `InvalidRequestException` with NO `message` member.
Why (specific): wrong error type plus a missing message, as Cause 4.
Fix: the execution-engine validation stage of the StartNotebookExecution handler — raise `ValidationException` and emit the message.
Traces: elasticmapreduce-notebook-execution-error-paths#task#2.

--- Cause 7 of 10: DescribeReleaseLabel's unknown-label refusal omits ErrorCode and rewords the message ---
AWS: Smithy: DescribeReleaseLabel(ReleaseLabel: String), error `InvalidRequestException` (client, 400). Recorded behavior only: an unknown release label is refused 400 with `ErrorCode` = the constant `InvalidInputParameter` and the CONSTANT message `The release label isn't valid.` — the message has NO variable part; AWS deliberately does not echo the label.
Emulator: `ErrorCode` missing, message `Invalid release label: <ReleaseLabel>` — reworded and echoing the value AWS omits.
Fix: the unknown-label branch of the DescribeReleaseLabel handler.
Traces: elasticmapreduce-release-label-and-instance-type-catalog#task#3.

--- Cause 8 of 10: ListSupportedInstanceTypes' unknown-label refusal omits ErrorCode and rewords the message ---
AWS: Smithy: ListSupportedInstanceTypes(ReleaseLabel required), error `InvalidRequestException` (client, 400). Recorded behavior only: 400 with `ErrorCode` = the constant `INVALID_RELEASE_LABEL` and the message `The supplied release label is invalid: <ReleaseLabel>.` — fixed text `The supplied release label is invalid: ` plus a trailing period; variable part: the ReleaseLabel from the request. Note this operation's wording and code BOTH differ from DescribeReleaseLabel's (Cause 7): they are not interchangeable.
Emulator: `ErrorCode` missing, message `Release label '<ReleaseLabel>' is not valid.`
Fix: the unknown-label branch of the ListSupportedInstanceTypes handler.
Traces: elasticmapreduce-release-label-and-instance-type-catalog#task#4.

--- Cause 9 of 10: RunJobFlow's StepConcurrencyLevel out-of-range message is emulator-authored ---
AWS: Smithy: RunJobFlow.StepConcurrencyLevel is a plain Integer with no @range trait, so recorded behavior only: a value outside the supported band is refused 400 with the CONSTANT message `Invalid step concurrency level. Allowed level is from 1 to 256` — no variable part (the offending value is NOT echoed) and no trailing period; the bound pair `1 to 256` is part of the fixed text.
Emulator: `StepConcurrencyLevel <value> is outside 1..256` — a different sentence, echoing the value, and rendering the bounds in Rust range syntax.
Fix: the StepConcurrencyLevel range check in the RunJobFlow handler's validation.
Traces: elasticmapreduce-run-job-flow-step-concurrency-level-out-of-range#task#5 (cluster also covers task#6 and task#7 of that fixture).

--- Cause 10 of 10: CreateSecurityConfiguration leaks a serde parser diagnostic instead of AWS's constant message, and omits ErrorCode ---
AWS: Smithy: CreateSecurityConfiguration(Name required, SecurityConfiguration required — a plain String), error `InvalidRequestException` (client, 400). Recorded behavior only: a non-JSON document is refused 400 with `ErrorCode` = the constant `INVALID_SECURITY_CONFIGURATION` and the CONSTANT message `Invalid security configuration; Malformed Json string.` — no variable part; AWS deliberately leaks nothing about the parser or the input.
Emulator: `ErrorCode` missing, message `SecurityConfiguration is not valid JSON: <serde diagnostic with line/column>` — emulator-internal parser text that also VARIES with the input, so it is not matchable even in principle.
Fix: the JSON-validation branch of the CreateSecurityConfiguration handler — map every parse failure onto the single constant message and set the ErrorCode.
Traces: elasticmapreduce-security-configuration-crud#task#6.

Note: the open behavioral issue 20261029T175551Z-emr-control-plane-refusals-use-emulator-authored-messages.md covers the behavioral-suite view of Causes 1, 2, 3, 4, 7, 8 and 10's message text, but it does not cover the missing `ErrorCode` member (Causes 5, 7, 8, 10), does not cover Causes 6 or 9 at all, and carries no wire replay key for any of these fixture steps.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator (service token `elasticmapreduce`). All ten clusters matched on status (400 expected, 400 actual); the error code and/or message members diverged.

Cause 1 — elasticmapreduce-block-public-access-configuration#task#2, PutBlockPublicAccessConfiguration: `$.Message: value_mismatch (expected 'The specified port range (9000, 8000) is not valid. Min Range must be less than or equal to Max Range.', got 'PermittedPublicSecurityGroupRuleRanges carries the port range 9000-8000: MaxRange must be at least MinRange')`.
Cause 2 — elasticmapreduce-cluster-scaling-and-policy-errors#task#7, PutManagedScalingPolicy: `$.message: value_mismatch (expected 'Specified job flow ID not valid.', got "Cluster id 'j-3SD91U2P1PANK' is not valid.")`.
Cause 3 — elasticmapreduce-cluster-scaling-and-policy-errors#task#9, RemoveManagedScalingPolicy: same expected/actual pair as Cause 2.
Cause 4 — elasticmapreduce-interactive-session-error-paths#task#3, GetSession: `$.__type: value_mismatch (expected 'ValidationException', got 'InvalidRequestException')` and `$.message: missing (expected "sessionId 'j-3SD91U2P1PANK/session/e-3SD91U2P1PANK' is not valid.", got None)`.
Cause 5 — elasticmapreduce-interactive-session-error-paths#task#6, GetClusterSessionCredentials: `$.ErrorCode: missing (expected 'InvalidClusterId', got None)`.
Cause 6 — elasticmapreduce-notebook-execution-error-paths#task#2, StartNotebookExecution: `$.__type: value_mismatch (expected 'ValidationException', got 'InvalidRequestException')` and `$.message: missing (expected 'The EMR cluster Id null is not available', got None)`.
Cause 7 — elasticmapreduce-release-label-and-instance-type-catalog#task#3, DescribeReleaseLabel: `$.ErrorCode: missing (expected 'InvalidInputParameter', got None)` and `$.Message: value_mismatch (expected "The release label isn't valid.", got 'Invalid release label: emr-0.0.0')`.
Cause 8 — elasticmapreduce-release-label-and-instance-type-catalog#task#4, ListSupportedInstanceTypes: `$.ErrorCode: missing (expected 'INVALID_RELEASE_LABEL', got None)` and `$.Message: value_mismatch (expected 'The supplied release label is invalid: emr-0.0.0.', got "Release label 'emr-0.0.0' is not valid.")`.
Cause 9 — elasticmapreduce-run-job-flow-step-concurrency-level-out-of-range#task#5, RunJobFlow: `$.message: value_mismatch (expected 'Invalid step concurrency level. Allowed level is from 1 to 256', got 'StepConcurrencyLevel 0 is outside 1..256')`; affected_count 3 (task#5, task#6, task#7 of the same fixture, i.e. several out-of-range values all take the same wrong template).
Cause 10 — elasticmapreduce-security-configuration-crud#task#6, CreateSecurityConfiguration: `$.ErrorCode: missing (expected 'INVALID_SECURITY_CONFIGURATION', got None)` and `$.Message: value_mismatch (expected 'Invalid security configuration; Malformed Json string.', got 'SecurityConfiguration is not valid JSON: expected ident at line 1 column 2')`.

Smithy grounding: `InvalidRequestException` = {ErrorCode @length(1,256), Message}, client/400. GetSession and StartNotebookExecution model only InternalServerException(500) + InvalidRequestException(400) — `ValidationException` is unmodelled, hence "recorded behavior only" for Causes 4 and 6. RunJobFlow.StepConcurrencyLevel is an unconstrained Integer, hence "recorded behavior only" for Cause 9.

## Steps to Reproduce
Boot the emulator and replay these steps from wire trace set simparity-wire-aws-v1 (service token `elasticmapreduce`):
1. Fixture `elasticmapreduce-block-public-access-configuration`, step task#2 — PutBlockPublicAccessConfiguration with a PermittedPublicSecurityGroupRuleRanges entry whose MinRange > MaxRange; compare Error.Message.
2. Fixture `elasticmapreduce-cluster-scaling-and-policy-errors`, steps task#7 (PutManagedScalingPolicy) and task#9 (RemoveManagedScalingPolicy) against a nonexistent ClusterId; assert the constant `Specified job flow ID not valid.`
3. Fixture `elasticmapreduce-interactive-session-error-paths`, step task#3 — GetSession with a SessionId that cannot be well-formed; assert `__type == 'ValidationException'` and the `sessionId '<ClusterId>/session/<SessionId>' is not valid.` message. Step task#6 — GetClusterSessionCredentials on an unresolvable cluster; assert `Error.ErrorCode == 'InvalidClusterId'`.
4. Fixture `elasticmapreduce-notebook-execution-error-paths`, step task#2 — StartNotebookExecution whose ExecutionEngine names no usable cluster; assert `__type == 'ValidationException'` and a non-null message.
5. Fixture `elasticmapreduce-release-label-and-instance-type-catalog`, steps task#3 (DescribeReleaseLabel) and task#4 (ListSupportedInstanceTypes) with an unknown release label; assert both ErrorCode and Message.
6. Fixture `elasticmapreduce-run-job-flow-step-concurrency-level-out-of-range`, steps task#5, task#6 and task#7 — RunJobFlow with StepConcurrencyLevel outside 1..256.
7. Fixture `elasticmapreduce-security-configuration-crud`, step task#6 — CreateSecurityConfiguration with a SecurityConfiguration body that is not JSON; assert the constant message and the ErrorCode.
