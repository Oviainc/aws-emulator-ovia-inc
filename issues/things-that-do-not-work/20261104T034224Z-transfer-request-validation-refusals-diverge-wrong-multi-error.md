Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-sftp-traj-update-server-logging-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-sftp-traj-update-server-logging-refusals transfer-sftp-traj-describe-server-id-mistakes
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|UpdateServer|4xx->4xx|$.message
Issue-Key: transfer|DescribeServer|4xx->4xx|$.message
Issue-Key: transfer|UpdateServer|4xx->4xx|$.__type
Issue-Key: transfer|UpdateServer|4xx->4xx|$.Message
Issue-Commit: b8482007ab5cef3cdc98ee3694cf4b4d8c9dc6a3

# Transfer request-validation refusals diverge: wrong multi-error ordering, null-value message prefix, non-modeled error code and a missing required Message (4 causes)

> _Note: this issue was identified against the code base at commit `b8482007ab5cef3cdc98ee3694cf4b4d8c9dc6a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateServer

## Description
FOUR net-new root causes in the transfer emulator's input-validation refusal paths, found by replaying the wire corpus simparity-wire-aws-v1 (score 85.4%). All four share one family of fault: the request is correctly refused with HTTP 400, but the refusal BODY is authored by the emulator (its own clause ordering, its own value prefix, its own error code, or no message at all) instead of reproduced as the service renders it. Each is a separate defect in a separate validation path and each must be closed before submitting. There are FOUR.

Related open issues that do NOT cover any cause below (named so a fixer can see the overlap): '20261029T184519Z-transfer-refusal-paths-diverge-nine-operations-answer-emulator.md' is the same class on a DIFFERENT set of wire steps (CreateProfile enum order, ImportHostKey, StartFileTransfer, TagResource, CreateServer, CreateWebApp, StartRemoteMove, DescribeAgreement) and owns those replay keys; '20261029T193150Z-transfer-refusal-fidelity-six-operations-answer-emulator-authored.md' grounds the same family in behavioral checks for UpdateWebApp/CreateConnector/CreateProfile/CreateServer/DeleteWorkflow/StartRemoteMove. Neither names DescribeServer's multi-violation ordering, the not-null message prefix, StructuredLogDestinations, or the LoggingRole refusal's missing Message.

Cause 1 of 4 - DescribeServer renders two violations of the SAME member in the wrong order.
AWS: Smithy: DescribeServer.ServerId required, target ServerId @length min 19 max 19, @pattern ^s-([0-9a-f]{17})$; the operation models InvalidRequestException (400) and ResourceNotFoundException (404), and modeled-constraint failures are refused by the framework as ValidationException (400) before the handler runs. Recorded behavior: a ServerId that is both too long and not matching the pattern is refused with HTTP 400 and the single message "2 validation errors detected: Value at 'serverId' failed to satisfy constraint: Member must satisfy regular expression pattern: s-([0-9a-f]{17}); Value at 'serverId' failed to satisfy constraint: Member must have length less than or equal to 19". Fixed text: the "<n> validation error(s) detected: " preamble, the per-violation clause "Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" / "Member must have length less than or equal to <max>", and the "; " join. Variable parts taken from the request and the model: the violation count, the member name in its lowerCamel wire spelling (serverId), the pattern source and the length bound. The ORDERING is part of the fixed rendering: for one member the pattern violation is reported BEFORE the length violation.
Emulator: identical status, count, clauses and member name, but the two clauses are emitted in the opposite order - "...Member must have length less than or equal to 19; ...Member must satisfy regular expression pattern: s-([0-9a-f]{17})".
Why: a client or test that exact-matches the service's refusal text (the whole message is one string; there is no per-violation structure on the wire) does not recognise it, and a caller that reads the FIRST reported violation to decide what to repair is pointed at the length when the service points at the format.
Fix: the shared modeled-constraint validation renderer for this service (the pre-handler ValidationException message builder) - it must order multiple violations of the same member pattern-before-length as the service does, not in the order the constraints happen to be evaluated.
Traces: transfer-sftp-traj-describe-server-id-mistakes#task#1 (same cluster: #task#3).

Cause 2 of 4 - the not-null refusal interpolates the (absent) value into AWS's fixed text.
AWS: Smithy: UpdateServer.ServerId is @required (target ServerId, @length 19..19, @pattern ^s-([0-9a-f]{17})$). Recorded behavior: omitting it is refused with HTTP 400 and message "1 validation error detected: Value at 'serverId' failed to satisfy constraint: Member must not be null". Fixed text: the whole clause; the only variable part is the member name in its lowerCamel wire spelling (serverId) and the violation count. For the not-null class the service emits "Value at '<member>'" with NO value token - there is no value to report.
Emulator: same status, count and constraint clause, but renders "1 validation error detected: Value null at 'serverId' failed to satisfy constraint: Member must not be null" - it splices the literal 'null' into the "Value ... at" slot that other constraint classes use for a supplied value.
Why: exact-match assertions on the refusal fail, and anything parsing the "Value <x> at '<member>'" form to extract the offending value reads the string 'null' as the value the caller sent rather than as an absent member.
Fix: the same pre-handler ValidationException message builder - the missing-required-member branch must emit the valueless "Value at '<member>'" form instead of reusing the with-value template.
Traces: transfer-sftp-traj-update-server-logging-refusals#task#1.

Cause 3 of 4 - a MODELED list-member constraint on StructuredLogDestinations is refused by the handler as InvalidRequestException instead of by the framework as ValidationException.
AWS: Smithy: UpdateServer.StructuredLogDestinations -> list StructuredLogDestinations (@length min 0 max 1) whose member targets Arn with @length min 20 max 1600 and @pattern ^arn:\S+$. Because the constraint is modeled on the member, a violating entry is refused before the handler with HTTP 400 and message "1 validation error detected: Value at 'structuredLogDestinations' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1600, Member must have length greater than or equal to 20, Member must satisfy regular expression pattern: arn:\S+]". Fixed text: the preamble, the "Value at '<member>' failed to satisfy constraint: Member must satisfy constraint: [...]" wrapper used for a collection whose MEMBER carries the constraints, and the bracketed clause list in the order max-length, min-length, pattern. Variable parts: the member name in its wire spelling (structuredLogDestinations), the two bounds and the pattern source, all taken from the model.
Emulator: HTTP 400 but __type InvalidRequestException with Message 'Invalid Member ARN' - a hand-written ARN check inside the handler, carrying emulator prose and the wrong error class, and omitting the bracketed constraint list entirely.
Why: the two classes mean different things to a caller - ValidationException says 'the request shape violates a declared constraint, repair it locally', InvalidRequestException says 'the service rejected the semantics'. A client branching on the error code mis-classifies the failure, retry/repair logic keyed on ValidationException never fires, and the constraint list that tells the caller the accepted length bounds and ARN pattern is not delivered. The field name differs too (ValidationException carries lowercase 'message', InvalidRequestException carries 'Message'), so SDK message extraction differs as well.
Fix: UpdateServer's validation - let the modeled member constraints on StructuredLogDestinations be enforced by the pre-handler constraint validation (which renders the composite ValidationException) and delete the handler's bespoke ARN check for this member.
Traces: transfer-sftp-traj-update-server-logging-refusals#task#2.

Cause 4 of 4 - UpdateServer's LoggingRole refusal omits the required Message member entirely.
AWS: Smithy: UpdateServer.LoggingRole -> NullableRole (@length 0..2048, @pattern ^(|arn:.*role/\S+)$); the operation models InvalidRequestException (errorKind client, httpError 400) and InvalidRequestException.Message is @required. Recorded behavior: a LoggingRole the service will not accept is refused with HTTP 400, code InvalidRequestException and Message 'Invalid LoggingRole ARN' - entirely fixed text for this class, with no variable slot (it does not echo the supplied value).
Emulator: the right status and the right error code, but the response body carries NO Message member at all (replay reports $.Message missing, expected 'Invalid LoggingRole ARN', got None), so the refusal is also not a valid rendering of the modeled error shape, whose Message is required.
Why: an SDK surfaces an empty error message, so the caller is told only 'InvalidRequestException' with no indication that the logging role is at fault; message-matching clients and operator logs lose the diagnostic, and a strict deserializer can fail on the absent required member.
Fix: UpdateServer's LoggingRole refusal path - populate InvalidRequestException.Message with the service's fixed string whenever that branch refuses.
Traces: transfer-sftp-traj-update-server-logging-refusals#task#3.

## Evidence
Replay of the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout: 309 steps, 264 passed, 38 mismatched, 7 blocked, score 85.4%; emulator_up=true (/__admin/health 200). Expected = the recorded AWS response, Actual = the emulator's.

Cause 1 - fixture transfer-sftp-traj-describe-server-id-mistakes, step task#1 (DescribeServer), verdict mismatch, status 400 -> 400, key $.message (cluster also covers task#3):
  expected: "2 validation errors detected: Value at 'serverId' failed to satisfy constraint: Member must satisfy regular expression pattern: s-([0-9a-f]{17}); Value at 'serverId' failed to satisfy constraint: Member must have length less than or equal to 19"
  actual:   "2 validation errors detected: Value at 'serverId' failed to satisfy constraint: Member must have length less than or equal to 19; Value at 'serverId' failed to satisfy constraint: Member must satisfy regular expression pattern: s-([0-9a-f]{17})"
  (only the clause ORDER differs; the fixed text, the count and the member name all match.)

Cause 2 - fixture transfer-sftp-traj-update-server-logging-refusals, step task#1 (UpdateServer), verdict mismatch, status 400 -> 400, key $.message:
  expected: "1 validation error detected: Value at 'serverId' failed to satisfy constraint: Member must not be null"
  actual:   "1 validation error detected: Value null at 'serverId' failed to satisfy constraint: Member must not be null"
  (the emulator inserts the token 'null' into the value slot of the template.)

Cause 3 - fixture transfer-sftp-traj-update-server-logging-refusals, step task#2 (UpdateServer), verdict mismatch, status 400 -> 400, keys $.__type and $.message:
  expected __type: ValidationException; actual __type: InvalidRequestException
  expected message: "1 validation error detected: Value at 'structuredLogDestinations' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 1600, Member must have length greater than or equal to 20, Member must satisfy regular expression pattern: arn:\\S+]"
  actual: 'Invalid Member ARN'

Cause 4 - fixture transfer-sftp-traj-update-server-logging-refusals, step task#3 (UpdateServer), verdict mismatch, status 400 -> 400, key $.Message:
  expected $.Message: 'Invalid LoggingRole ARN'; actual: member absent (None). The error code and status matched.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DescribeServer.ServerId and UpdateServer.ServerId are both required with @length 19..19 and @pattern ^s-([0-9a-f]{17})$; UpdateServer.StructuredLogDestinations is a list (@length 0..1) of Arn with @length 20..1600 and @pattern ^arn:\\S+$; UpdateServer.LoggingRole is NullableRole with @length 0..2048 and @pattern ^(|arn:.*role/\\S+)$; InvalidRequestException is errorKind client / httpError 400 with a REQUIRED Message member.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 against it.
3. Re-replay the four bound steps and compare status, error code (__type) and the full message against the recorded response:
   - transfer-sftp-traj-describe-server-id-mistakes#task#1 (DescribeServer with a ServerId that is both over 19 characters and not matching s-([0-9a-f]{17}); check the ORDER of the two clauses in $.message) - and #task#3 in the same cluster.
   - transfer-sftp-traj-update-server-logging-refusals#task#1 (UpdateServer with ServerId omitted; check $.message for the 'Value at' vs 'Value null at' prefix).
   - transfer-sftp-traj-update-server-logging-refusals#task#2 (UpdateServer with a StructuredLogDestinations entry violating the member's length/pattern constraints; check $.__type is ValidationException and $.message carries the bracketed composite constraint list).
   - transfer-sftp-traj-update-server-logging-refusals#task#3 (UpdateServer with an unacceptable LoggingRole; check the body carries $.Message = 'Invalid LoggingRole ARN').
