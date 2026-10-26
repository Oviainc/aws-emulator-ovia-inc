Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.concurrent-account-setting-writes ecs.exec-protection-agent-ops-on-missing-cluster-not-found
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.concurrent-account-setting-writes|_error#31|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.exec-protection-agent-ops-on-missing-cluster-not-found|run#36|AssertionError|assert code == "InvalidParameterException", f"SubmitAttachmentStateChanges control: got {code}"
Issue-Commit: 3a93fe113853c8e105663669e53b4ef887b2d408

# ecs: DeleteAccountSetting never refuses a re-delete of an absent override, and SubmitAttachmentStateChanges is an unimplemented scaffold stub (2 causes)

> _Note: this issue was identified against the code base at commit `3a93fe113853c8e105663669e53b4ef887b2d408`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAccountSetting

## Description
Two net-new behavioral root causes in the ECS emulator, both grounded in failing checks of the suite. Neither is marked ALREADY FILED or OWNED in the replay report, and no open ECS issue covers either operation (the closest open items — 20261024T081305Z-ecs-is-overly-permissive-ten-validations-authorizations-aws.md and 20261025T112916Z-ecs-refusal-messages-use-emulator-internal-wording-createservice.md — enumerate other operations and do not mention DeleteAccountSetting or SubmitAttachmentStateChanges; the per-operation stub issues filed for ECS cover ListServices, CreateExpressGatewayService, CreateTaskSet, UpdateTaskProtection, DescribeServiceDeployments, DeleteAttributes, CreateDaemon and UpdateServicePrimaryTaskSet, not SubmitAttachmentStateChanges).

=== Cause 1 of 2: DeleteAccountSetting on a principal that holds NO override returns 200 with the inherited account default instead of refusing with InvalidParameterException ===
Failing check: ecs.concurrent-account-setting-writes

AWS: Smithy (com.amazonaws.ecs#DeleteAccountSetting) models input name (SettingName enum, required) + principalArn (String), output setting (Setting), and errors AccessDeniedException (403), ClientException (400), InvalidParameterException (400), ServerException (500). The operation 'Disables an account setting for a specified user, role, or the root user for an account' — i.e. it removes a principal-scoped override. Recorded behavior: when the named principal holds no override for that setting name (for example a second, identical DeleteAccountSetting immediately after a successful one), real AWS REFUSES the call with HTTP 400, error code InvalidParameterException and message 'Referenced setting doesn't exist'. That message is entirely fixed text for this error class — it carries no request- or state-derived variable part. The modeled InvalidParameterException ('The specified parameter isn't valid') is the error AWS classifies this under, so the refusal is modeled, not merely observed.

Emulator: the call SUCCEEDS. Replay recorded HTTP 200 with body {'setting': {'name': 'containerInsights', 'value': 'enhanced', 'principalArn': 'arn:aws:iam::<account>:root', 'type': 'user'}} for a DeleteAccountSetting whose override had already been removed — the preceding ListAccountSettings(effectiveSettings=False) correctly showed no override, so the emulator knows there is nothing to delete and answers 200 anyway. The handler's state lookup falls through to the account-wide default and reports that as the 'deleted' setting rather than returning a modeled error. Note the response is additionally misleading in two ways beyond the missing refusal: it reports the ACCOUNT DEFAULT as if it were the principal's deleted override, and it labels it type 'user'.

Why: a client cannot distinguish 'your override was removed' from 'there was no override'. Idempotency-sensitive teardown and reconciliation code (cleanup paths, CDK/Terraform-style converge loops, the harness's own restore step) uses that refusal to detect that state is already clean; against the emulator every re-delete looks like a fresh successful delete. Worse, the 200 body hands back a value that is NOT a principal override, so a client that writes the returned setting back — a common 'restore what I deleted' pattern — silently promotes the account default into a principal-scoped override that AWS would never have created.

Fix: the DeleteAccountSetting handler. The absent-override branch must return the modeled InvalidParameterException instead of synthesising a response from the effective/default setting; the fallback that resolves the account-wide default and returns it as `setting` is the defect. See the file/line citation under Traces.

=== Cause 2 of 2: SubmitAttachmentStateChanges answers 501 NotImplemented — it is an unimplemented scaffold stub ===
Failing check: ecs.exec-protection-agent-ops-on-missing-cluster-not-found

AWS: Smithy (com.amazonaws.ecs#SubmitAttachmentStateChanges) models input cluster (String) + attachments (AttachmentStateChanges, required), output acknowledgment (String), and errors AccessDeniedException (403), ClientException (400), ClusterNotFoundException (400), InvalidParameterException (400), ServerException (500). The operation is agent-facing: 'Sent to acknowledge that an attachment changed states.' Recorded behavior, two cases the check exercises:
  (a) existing cluster, attachment ARN that names no attachment -> HTTP 400, code InvalidParameterException, message 'Attachment with id <attachment-id> does not exist'. Fixed text: 'Attachment with id ' and ' does not exist'; the variable part is the attachment id taken from the request's attachments[].attachmentArn (the segment after the final '/'), not a literal from any one run.
  (b) cluster that does not exist -> HTTP 400, code ClientException, message 'Cluster not found.' — wholly fixed text. Note AWS does NOT use the modeled ClusterNotFoundException for the agent-facing Submit* family here; it surfaces ClientException, which is why the distinction matters.

Emulator: the request is refused with HTTP 501 and error code NotImplemented ('operation SubmitAttachmentStateChanges is scaffolded but not implemented in this emulator'). Replay asserted on case (a) and got code NotImplemented where InvalidParameterException was required, so neither the missing-attachment nor the missing-cluster branch exists. The handler is still the generated scaffold whose `handle` body returns `Scaffolded::not_implemented("SubmitAttachmentStateChanges")`, and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line.

Why: every caller of this operation — the ECS container agent itself, and any test or tool that simulates agent attachment acknowledgement — gets a 501 that no AWS client treats as retryable or as a modeled failure. A 501 is not in the operation's modeled error set, so SDK error handling cannot branch on it; code that expects ClientException('Cluster not found.') for a bad cluster or InvalidParameterException for a bad attachment id instead sees an unmodeled server-class failure. It also makes the whole cluster-existence error contract unverifiable for this operation: the sibling operations in the same family (ExecuteCommand, GetTaskProtection, UpdateTaskProtection, UpdateContainerAgent) pass their assertions in this same check, so SubmitAttachmentStateChanges is the only hole.

Fix: implement the SubmitAttachmentStateChanges handler — resolve `cluster` against the ECS cluster store first and refuse an unknown one with ClientException('Cluster not found.'), then resolve each attachments[].attachmentArn against attachment state and refuse an unknown id with InvalidParameterException carrying the attachment-id template above; on success return the modeled `acknowledgment`. Delete the SCAFFOLDED marker line once done. See the file/line citation under Traces.

Traces:
  Cause 1 — check ecs.concurrent-account-setting-writes (assertion at check_ecs_concurrent_account_setting_writes.py:179, 'expected a ClientError, but the call SUCCEEDED'). Handler: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/delete_account_setting.rs:207-220 (the `match guard.delete_account_setting(..)` None arm that resolves and returns the effective default).
  Cause 2 — check ecs.exec-protection-agent-ops-on-missing-cluster-not-found (assertion at check_ecs_exec_protection_agent_ops_on_missing_cluster_not_found.py:134, 'SubmitAttachmentStateChanges control: got NotImplemented'). Handler: services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/submit_attachment_state_changes.rs:168 (the `not_implemented` return), marker at line 4.

## Evidence
Cause 1 — ecs.concurrent-account-setting-writes
Expected (AWS): DeleteAccountSetting(name='containerInsights') repeated after the override was already removed -> HTTP 400, Error.Code = 'InvalidParameterException', Error.Message = "Referenced setting doesn't exist" (fixed text, no variable parts).
Actual (emulator): HTTP 200, no error raised. Response body: {'setting': {'name': 'containerInsights', 'value': 'enhanced', 'principalArn': 'arn:aws:iam::<account>:root', 'type': 'user'}} — the account-wide default echoed back as though it were the principal override just deleted. (Account id / timestamp / content-length in the recorded ResponseMetadata are per-run values and are not part of the requirement.)
Corroborating step in the same check: the immediately preceding ListAccountSettings(name='containerInsights', effectiveSettings=False) returned an EMPTY settings list and passed its assertion — so the emulator already knows no override exists when it answers 200.

Cause 2 — ecs.exec-protection-agent-ops-on-missing-cluster-not-found
Expected (AWS), existing cluster + nonexistent attachment ARN: HTTP 400, Error.Code = 'InvalidParameterException', Error.Message = 'Attachment with id <attachment-id> does not exist' where <attachment-id> is the final ARN segment of the request's attachments[0].attachmentArn.
Expected (AWS), nonexistent cluster: HTTP 400, Error.Code = 'ClientException', Error.Message = 'Cluster not found.' (fixed text).
Actual (emulator), both: HTTP 501, Error.Code = 'NotImplemented', Error.Message = 'operation SubmitAttachmentStateChanges is scaffolded but not implemented in this emulator'.
Assertion: 'AssertionError: SubmitAttachmentStateChanges control: got NotImplemented' (expected InvalidParameterException).
Scope note: the earlier assertions of this same check — ExecuteCommand (InvalidParameterException 'The specified task was not found. Specify a valid task ID and try again.' / ClusterNotFoundException 'Cluster not found.'), GetTaskProtection and UpdateTaskProtection (200 with failures[0].reason == 'MISSING' / ClusterNotFoundException), and UpdateContainerAgent (ClientException 'Referenced container instance <id> not found.' / ClusterNotFoundException) — all PASSED, isolating the divergence to SubmitAttachmentStateChanges.

## Steps to Reproduce
Cause 1 (DeleteAccountSetting re-delete):
1. ecs.put_account_setting(name='containerInsights', value='enabled')  — create a principal-scoped override for the authenticated caller.
2. ecs.delete_account_setting(name='containerInsights')  — removes it; succeeds on both AWS and the emulator.
3. ecs.list_account_settings(name='containerInsights', effectiveSettings=False)  — returns an empty settings list, confirming no override remains.
4. ecs.delete_account_setting(name='containerInsights')  — repeat the identical delete.
   AWS: ClientError InvalidParameterException, HTTP 400, message "Referenced setting doesn't exist".
   Emulator: HTTP 200 with a 'setting' body holding the inherited account default.
Full sequence: replay check ecs.concurrent-account-setting-writes.

Cause 2 (SubmitAttachmentStateChanges):
1. name = ecs.create_cluster(clusterName=<unique>)  — a real, empty cluster.
2. ecs.submit_attachment_state_changes(cluster=<that cluster>, attachments=[{'attachmentArn': 'arn:aws:ecs:<region>:<account>:attachment/00000000-0000-0000-0000-000000000000', 'status': 'ATTACHED'}]).
   AWS: HTTP 400 InvalidParameterException, 'Attachment with id 00000000-0000-0000-0000-000000000000 does not exist'.
   Emulator: HTTP 501 NotImplemented.
3. Repeat step 2 with cluster=<a cluster name that does not exist>.
   AWS: HTTP 400 ClientException, 'Cluster not found.'
   Emulator: HTTP 501 NotImplemented.
Full sequence: replay check ecs.exec-protection-agent-ops-on-missing-cluster-not-found.
