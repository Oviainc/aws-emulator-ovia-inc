Issue-Source: trace-sweep-divergence
Issue-Trace: backup-audit-framework-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-audit-framework-lifecycle backup-framework-error-paths
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|CreateFramework|4xx->2xx|$.Message
Issue-Key: backup|CreateFramework|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|CreateFramework|4xx->2xx|status
Issue-Key: backup|DescribeFramework|4xx->2xx|$.Message
Issue-Key: backup|DescribeFramework|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|DescribeFramework|4xx->2xx|status
Issue-Key: backup|CreateFramework|4xx->4xx|$.Message
Issue-Key: backup|DescribeFramework|4xx->4xx|$.Message
Issue-Key: backup|DescribeFramework|4xx->2xx|$
Issue-Commit: b74f8bdf7dc8d00213b4902066e9bbcc7a280a48

# AWS Backup audit-framework refusals diverge: control validation missing, deleted and absent frameworks readable, emulator-authored messages

> _Note: this issue was identified against the code base at commit `b74f8bdf7dc8d00213b4902066e9bbcc7a280a48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFramework

## Description
Five distinct causes in the AWS Backup audit-framework surface (CreateFramework / DescribeFramework). All five must be closed before this issue is submitted.

Cause 1 of 5: CreateFramework accepts an unsupported control name.
AWS: Smithy: CreateFramework input FrameworkControls (required, list of FrameworkControl whose ControlName is required); modeled error InvalidParameterValueException (errorKind client, httpError 400, members Code/Message/Type/Context), documented as "Indicates that something is wrong with a parameter's value". AWS validates ControlName against the catalog of supported AWS Backup audit controls and refuses an unknown one with HTTP 400 InvalidParameterValueException. Message template (fixed text, with the submitted control name substituted): "You can't create the framework control <ControlName> because it is unsupported." — here <ControlName> is the unsupported value taken verbatim from the request's FrameworkControls[*].ControlName. The recorded response also carries header x-amzn-errortype: "InvalidParameterValueException:http://internal.amazon.com/coral/com.amazonaws.services.cryo/" (the error shape name plus the service's coral namespace).
Emulator: answers HTTP 200 and creates the framework. No $.Message, no x-amzn-errortype header; the unsupported control is stored as-is.
Why: a client that probes control support, or that mistypes/uses a control its account cannot use, gets a successful create and believes an audit framework is evaluating a control that does not exist. Nothing ever reports the configuration as invalid, so the caller's error-handling path for unsupported controls is never exercised and the account state diverges from AWS permanently.
Fix: CreateFramework's input validation in the backup crate's framework handler — add a supported-ControlName check ahead of state mutation that raises InvalidParameterValueException (400) with the message template above and the coral-suffixed x-amzn-errortype header.
Traces: backup-audit-framework-lifecycle#task#1.

Cause 2 of 5: DescribeFramework returns a framework that was never (validly) created.
AWS: Smithy: DescribeFramework GET /audit/frameworks/{FrameworkName}, modeled error ResourceNotFoundException (client, httpError 400). A framework name that does not exist in the account is refused 400 ResourceNotFoundException with message template "Framework <FrameworkName> not found in account <accountId>." — fixed text "Framework " / " not found in account " / ".", with <FrameworkName> taken from the request's path label and <accountId> the caller's account from state. Recorded header x-amzn-errortype: "ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.services.cryo/".
Emulator: answers HTTP 200 with a full framework body, because the preceding CreateFramework (cause 1) was wrongly accepted. No $.Message and no x-amzn-errortype header.
Why: the not-found read path for frameworks is unreachable in exactly the case AWS exercises it; a client polling for a framework whose creation AWS rejected sees it as ACTIVE.
Fix: same handler family — the framework not-found read path plus the create-side validation of cause 1; DescribeFramework must emit ResourceNotFoundException (400) with the message template above when the name is absent from state.
Traces: backup-audit-framework-lifecycle#task#2.

Cause 3 of 5: duplicate-framework refusal uses the emulator's own sentence.
AWS: creating a framework whose controls and configuration duplicate an existing framework is refused 400 with message template "A framework with these controls and configurations already exists in this account <accountId>. To create new framework <FrameworkName>, change the number or scope of the controls." — the two sentences are fixed text; <accountId> comes from the caller's account in state and <FrameworkName> from the request's FrameworkName. (Smithy: CreateFramework errors AlreadyExistsException and InvalidParameterValueException, both client/400.) The recorded status 400 and error type match; only the message diverges.
Emulator: HTTP 400 with $.Message "Framework with the same name already exists" — wording the service never produces, and it attributes the conflict to the name rather than to the controls/configuration.
Why: clients and tests that match on AWS's message (or that follow its remediation hint, "change the number or scope of the controls") mis-handle the conflict; the emulator's sentence also misstates the rule AWS enforces.
Fix: CreateFramework's conflict path in the backup crate's framework handler/serializer — render AWS's two-sentence template from the request name and the account in state.
Traces: backup-framework-error-paths#task#2.

Cause 4 of 5: framework not-found refusal uses the emulator's own sentence.
AWS: DescribeFramework on an absent name is refused 400 ResourceNotFoundException with message template "Framework <FrameworkName> not found in account <accountId>." (fixed text as in cause 2; variable parts are the requested name and the caller's account).
Emulator: HTTP 400, correct x-amzn-errortype, but $.Message "Cannot find Framework" — a crate-authored string that names neither the framework nor the account.
Why: the message is a documented part of the refusal contract; a client that surfaces or matches it cannot tell which framework or account was missing.
Fix: the framework read path's error rendering (message template), not the status classification, which is already right.
Traces: backup-framework-error-paths#task#3.

Cause 5 of 5: a deleted framework stays readable forever.
AWS: after DeleteFramework succeeds, DescribeFramework for that name converges to 400 ResourceNotFoundException (same shape and message template as causes 2/4). The fixture polls for that refusal.
Emulator: DescribeFramework keeps answering 200 with the full framework body (FrameworkStatus ACTIVE, DeploymentStatus COMPLETED) for the whole 60s poll window — the poll caps after 61 attempts with status 200. Related open issue 20261030T001950Z-backup-deleteframework-answers-501-notimplemented-so-the-audit.md reports that DeleteFramework itself answers 501, which is the likely upstream reason (Hypothesis: the delete never removes the record, so the read path has nothing to refuse); that issue does not carry this fixture step, so the read-side convergence is filed here and must be re-replayed after the delete path lands.
Why: teardown and idempotency logic that waits for a framework to disappear never terminates against the emulator, and an account's framework list/read keeps resources AWS has removed.
Fix: framework state — delete must remove the record so the DescribeFramework read path refuses it with ResourceNotFoundException (400).
Traces: backup-framework-error-paths#teardown#7.

## Evidence
Cause 1 (backup-audit-framework-lifecycle#task#1, CreateFramework): expected status 400, got 200; $.Message expected "You can't create the framework control NOT_A_REAL_BACKUP_CONTROL because it is unsupported.", got None; $headers.x-amzn-errortype expected 'InvalidParameterValueException:http://internal.amazon.com/coral/com.amazonaws.services.cryo/', got None.
Cause 2 (backup-audit-framework-lifecycle#task#2, DescribeFramework): expected status 400, got 200; $.Message expected 'Framework <name> not found in account <accountId>.', got None; $headers.x-amzn-errortype expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazonaws.services.cryo/', got None.
Cause 3 (backup-framework-error-paths#task#2, CreateFramework): status 400 both sides; $.Message expected 'A framework with these controls and configurations already exists in this account <accountId>. To create new framework <name>, change the number or scope of the controls.', got 'Framework with the same name already exists'.
Cause 4 (backup-framework-error-paths#task#3, DescribeFramework): status 400 both sides; $.Message expected 'Framework <name> not found in account <accountId>.', got 'Cannot find Framework'.
Cause 5 (backup-framework-error-paths#teardown#7, DescribeFramework): expected status 400, got 200; poll capped at 60s — 'status 200 not in [400] after 61 attempt(s)', body still the live framework with FrameworkStatus ACTIVE / DeploymentStatus COMPLETED.
(Per-run values in the quoted expectations — generated framework names, account id, timestamps, idempotency tokens, ARNs — are the variable parts of the templates, not requirements.)

## Steps to Reproduce
1. Replay fixture backup-audit-framework-lifecycle: step task#1 calls CreateFramework with a FrameworkControls entry whose ControlName is not a supported AWS Backup control; the emulator answers 200 instead of 400 InvalidParameterValueException. Step task#2 then calls DescribeFramework for that name and gets 200 instead of 400 ResourceNotFoundException.
2. Replay fixture backup-framework-error-paths: step task#2 calls CreateFramework again with the same controls/configuration (expect AWS's two-sentence already-exists message, observe 'Framework with the same name already exists'); step task#3 calls DescribeFramework for an absent framework name (expect 'Framework <name> not found in account <accountId>.', observe 'Cannot find Framework'); step teardown#7 deletes the framework and polls DescribeFramework for 400, which never arrives (200 for 61 attempts).
3. Compare status, x-amzn-errortype and $.Message/$.Code against each recorded response.
