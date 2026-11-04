Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-vscan-traj-send-command-validation-order
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-vscan-traj-send-command-validation-order ssm-run-command-execution
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|SendCommand|4xx->4xx|$.Message
Issue-Key: ssm|SendCommand|4xx->4xx|$.__type
Issue-Key: ssm|SendCommand|4xx->2xx|$.Message
Issue-Key: ssm|SendCommand|4xx->2xx|$.__type
Issue-Key: ssm|SendCommand|4xx->2xx|status
Issue-Key: ssm|GetCommandInvocation|4xx->4xx|$.Message
Issue-Key: ssm|GetCommandInvocation|4xx->4xx|$.__type
Issue-Commit: 685d7dc1b36320cc963844050fe238af8df0451a

# SSM Run Command validation diverges: AWS-managed documents are unknown and no instance-state check runs (4 causes)

> _Note: this issue was identified against the code base at commit `685d7dc1b36320cc963844050fe238af8df0451a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
The emulator's Run Command surface rejects and accepts the wrong requests: AWS-owned SSM documents are not in its document catalogue, so every SendCommand naming one is refused as a nonexistent document instead of reaching parameter validation; an unregistered instance id is accepted instead of refused; and GetCommandInvocation raises the wrong modelled error for an invocation that does not exist. Four causes, all in service `ssm`.

Cause 1 of 4 — SendCommand naming an AWS-managed document is refused InvalidDocument instead of validating parameters (representative; fixture ssm-vscan-traj-send-command-validation-order, step task#3).
AWS: Smithy: SendCommand.DocumentName (target DocumentARN, required, pattern ^[a-zA-Z0-9_\\-.:/]{3,128}$) and the modelled errors InvalidDocument ("The specified SSM document doesn't exist.") and InvalidParameters ("You must specify values for all required parameters in the Amazon Web Services Systems Manager document (SSM document). You can only supply values to parameters defined in the SSM document."). AWS pre-provisions its own public documents in every account, so SendCommand with an exactly-cased AWS-owned document name resolves the document and, when the supplied Parameters do not match its parameter definitions, refuses with status 400, __type 'InvalidParameters' and the fixed message 'Parameters provided in document are invalid or not supported.' (no request-derived values in that sentence).
Emulator: status 400 with __type 'InvalidDocument' and Message "Document <name> doesn't exist" — the AWS-owned document is absent from its catalogue, so the request never reaches parameter validation.
Why: every Run Command flow built on AWS-managed runbooks (patching, agent update, shell scripts) is unusable, and the refusal a client does get misattributes a parameter mistake to a missing document, so the caller 'fixes' the wrong thing.
Fix: the SSM document store must be seeded with the AWS-owned public documents, and the SendCommand handler's validation order must resolve the document and then validate Parameters against it.
Traces: ssm-vscan-traj-send-command-validation-order#task#3, #task#4, #task#7.

Cause 2 of 4 — the SendCommand InvalidDocument refusal carries a Message AWS omits (fixture ssm-vscan-traj-send-command-validation-order, step task#2).
AWS: Smithy: InvalidDocument.Message is an optional member. Recorded behavior only: for a document name that does not resolve (here, a wrongly-cased AWS document name — SSM document names are case-sensitive), AWS answers status 400 with __type 'InvalidDocument' and NO Message member in the body at all.
Emulator: emits the same status and __type but adds Message "Document <name> doesn't exist", a sentence the emulator authored.
Why: clients and SDK error mappers that branch on the presence of a message, or surface it to users verbatim, see text AWS never sends; the extra member also makes it impossible to distinguish the two refusal shapes AWS uses.
Fix: the SendCommand error serializer for InvalidDocument — do not synthesise a Message where AWS sends none.
Traces: ssm-vscan-traj-send-command-validation-order#task#2.

Cause 3 of 4 — SendCommand accepts an instance id that is not a valid managed node (fixture ssm-vscan-traj-send-command-validation-order, step task#8).
AWS: Smithy: SendCommand.InstanceIds (list, max 50) and the modelled error InvalidInstanceId, whose doc states the error is raised when, among other causes, "The managed node isn't in a valid state" or SSM Agent is not registered with the SSM endpoint. Recorded behavior: status 400, __type 'InvalidInstanceId', Message with the fixed text 'Instances not in a valid state for account' (no request-derived values).
Emulator: status 200 — the command is created and a Command payload returned for an instance id that is not a registered managed node, with no __type and no Message.
Why: a caller believes it dispatched a command to a node that does not exist; the invocation never completes and the failure surfaces much later (or never) instead of at the API call, and negative tests that assert the refusal pass vacuously.
Fix: the SendCommand handler's target validation — resolve each InstanceIds entry against managed-node state and refuse with InvalidInstanceId when it is absent or not in a valid state.
Traces: ssm-vscan-traj-send-command-validation-order#task#8.

Cause 4 of 4 — GetCommandInvocation raises InvalidCommandId instead of InvocationDoesNotExist (fixture ssm-run-command-execution, step task#5).
AWS: Smithy: GetCommandInvocation input CommandId (required, length exactly 36) and InstanceId (required, pattern ^(^i-(\\w{8}|\\w{17})$)|(^mi-\\w{17}$)$); modelled errors include both InvalidCommandId and InvocationDoesNotExist, the latter documented as "The command ID and managed node ID you specified didn't match any invocations." For a well-formed command id and instance id that match no invocation, the recorded response is status 400, __type 'InvocationDoesNotExist', Message 'Invocation not found for <commandId>, <instanceId>' — fixed text 'Invocation not found for ' then the CommandId and InstanceId taken from the request, comma-separated.
Emulator: status 400 but __type 'InvalidCommandId' and no Message member.
Why: the two errors mean different things — a malformed/unknown command versus a command that exists but has no invocation on that node — so retry and fan-out logic keyed on the error code takes the wrong branch, and the message identifying which (command, node) pair was missing is lost.
Fix: the GetCommandInvocation handler's lookup/error classification and its InvocationDoesNotExist message serialization.
Traces: ssm-run-command-execution#task#5.

No open issue owns these clusters (replay reported owned_by empty and stub_path empty for all four). The open issue 20261103T215115Z-ssm-control-plane-operations-answer-notimplemented-where-aws covers operations that answer NotImplemented, which is a different failure mode from the wrong-but-served refusals above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (service ssm, 4 net-new clusters):
- ssm-vscan-traj-send-command-validation-order#task#3 (SendCommand, 400->400): $.__type expected 'InvalidParameters', got 'InvalidDocument'; $.Message expected 'Parameters provided in document are invalid or not supported.', got "Document AWS-RunPatchBaseline doesn't exist". Also affects #task#4 and #task#7.
- ssm-vscan-traj-send-command-validation-order#task#2 (SendCommand, 400->400): $.Message unexpected_present — expected no Message member, got "Document aws-runpatchbaseline doesn't exist".
- ssm-vscan-traj-send-command-validation-order#task#8 (SendCommand, 400->200): expected status 400 with __type 'InvalidInstanceId' and Message 'Instances not in a valid state for account'; got 200 with no error members.
- ssm-run-command-execution#task#5 (GetCommandInvocation, 400->400): $.__type expected 'InvocationDoesNotExist', got 'InvalidCommandId'; $.Message expected 'Invocation not found for <commandId>, <instanceId>' (fixed prefix + request values), got None.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture ssm-vscan-traj-send-command-validation-order and inspect, in order: task#2 (SendCommand with a wrongly-cased AWS document name — an extra Message appears), task#3/#4/#7 (SendCommand naming an AWS-managed document with bad Parameters — InvalidDocument instead of InvalidParameters), task#8 (SendCommand with a valid document and an unregistered instance id — 200 instead of 400 InvalidInstanceId).
4. Replay fixture ssm-run-command-execution step task#5 — GetCommandInvocation with a well-formed 36-char command id and an i-<17 hex> instance id that match no invocation; compare __type and Message.
