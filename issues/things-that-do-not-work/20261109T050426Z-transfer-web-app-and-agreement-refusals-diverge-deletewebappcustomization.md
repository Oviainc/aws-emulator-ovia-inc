Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.delete-webapp-customization-conflict-blocked transfer.delete-server-referenced-by-agreement-succeeds-and-orphans-agreement
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.delete-webapp-customization-conflict-blocked|run#19|AssertionError|assert all(error == ("ConflictException", _CONFLICT_MESSAGE) for error in error_codes), (
Issue-Key: transfer.delete-server-referenced-by-agreement-succeeds-and-orphans-agreement|run#19|AssertionError|assert agreement_delete_code == "ResourceNotFoundException", (
Issue-Commit: 21f99eeabe4f0ab4efdf351ee564b8815702ec39

# Transfer web-app and agreement refusals diverge: DeleteWebAppCustomization's ConflictException carries emulator-authored prose, and DeleteAgreement on an agreement orphaned by DeleteServer answers 501 NotImplemented instead of the modeled 404

> _Note: this issue was identified against the code base at commit `21f99eeabe4f0ab4efdf351ee564b8815702ec39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebAppCustomization

## Description
TWO distinct root causes in the transfer emulator, each grounded in a failing behavioral check of this suite. Both must be closed before this issue is submitted.

=== Cause 1 of 2: the ConflictException a losing DeleteWebAppCustomization racer gets carries the emulator's own sentence instead of AWS's message ===
AWS: Smithy (com.amazonaws.transfer#TransferService) DeleteWebAppCustomization input WebAppId (@required, len 24..24, ^webapp-[0-9a-f]{17}$), output Unit, @idempotent; modeled errors AccessDeniedException 403, ConflictException 409, InternalServiceError 503, InvalidRequestException 400, ResourceNotFoundException 404, ThrottlingException 429. Recorded behavior: when several DeleteWebAppCustomization calls on one web app's customization overlap, a racer either succeeds or is refused with ConflictException whose Message is the fixed service text 'Unable to delete WebApp customization' — the whole message is fixed for this condition and carries no value from the request or the store.
Emulator: the code and status are right (409 ConflictException) but the Message is emulator-authored prose: "Another update of this web app's customization is in progress; wait for it to complete and try again." It is a crate constant, state/web_app_customization.rs::CUSTOMIZATION_WRITE_IN_PROGRESS, returned by write_conflict() and raised from ops/delete_web_app_customization.rs on Refusal::Conflict.
Why: the message is part of the requirement for this error class. A client (or an assertion, or an operator runbook) that matches AWS's sentence for a lost customization delete never matches, and the prose also mis-describes the condition it is raised for — it says 'another update' for a delete that lost the claim.
Fix: the refusal text for the delete path of the web-app customization write claim in the transfer crate (services/service-com-amazonaws-transfer-transferservice/src/state/web_app_customization.rs, raised by src/ops/delete_web_app_customization.rs) must be AWS's fixed text for this condition. Note the same write_conflict() helper also serves UpdateWebAppCustomization, whose own concurrency check currently passes, so the delete path's text must be corrected without regressing the update path.
Traces: transfer.delete-webapp-customization-conflict-blocked.

=== Cause 2 of 2: DeleteAgreement on an agreement whose server was deleted answers 501 NotImplemented, an error outside its modeled set ===
AWS: Smithy DeleteAgreement input AgreementId (@required, ^a-([0-9a-f]{17})$) and ServerId (@required, ^s-([0-9a-f]{17})$), output Unit, @idempotent; modeled errors InternalServiceError 503, InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException 503. Recorded behavior: DeleteServer succeeds even while an ACTIVE agreement references the server, and afterwards the orphaned agreement is genuinely gone on the mutation path as well as the read path — DeleteAgreement(ServerId=<deleted server>, AgreementId=<its agreement>) is refused with ResourceNotFoundException whose Message is the fixed generic not-found text 'Unknown agreement' (no identifier echoed), matching the 'Unknown server' answer DeleteServer/DescribeServer give for the deleted server.
Emulator: DescribeAgreement now correctly answers that 404, but DeleteAgreement answers code 'NotImplemented' (HTTP 501, 'operation DeleteAgreement is scaffolded but not implemented in this emulator'), because src/ops/delete_agreement.rs is still the scaffold stub. NotImplemented is not in the operation's modeled error set, so a client cannot classify it, and the orphaned agreement reads as gone while every mutation of it is unanswerable.
Why: a teardown/reconciliation loop that branches on ResourceNotFoundException to conclude 'already deleted' instead receives an unmodeled 501 and either aborts or retries forever; the read and mutate paths of the same resource disagree.
Fix: implement the handler in src/ops/delete_agreement.rs against the agreement state, resolving the ServerId before the AgreementId so a pair whose server no longer exists answers the modeled 404 ResourceNotFoundException with the service's fixed 'Unknown agreement' text, and remove that file's '//! SCAFFOLDED, NOT IMPLEMENTED:' marker line. Open issue 20261029T193150Z-transfer-deleteagreement-stub.md asks for the implementation and carries only the replay keys of transfer.agreement-create-describe-delete-404 and transfer.agreement-lifecycle-create-describe-delete; it does not carry this check's key nor the orphaned-agreement 404 requirement, so this check must be re-replayed too (the already-fixed 20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md covered only the DescribeAgreement half of it, via its Cause 3).
Traces: transfer.delete-server-referenced-by-agreement-succeeds-and-orphans-agreement.

## Evidence
Cause 1 — expected: a DeleteWebAppCustomization racer that loses the claim -> HTTP 409 ('ConflictException', 'Unable to delete WebApp customization'); actual: ('ConflictException', "Another update of this web app's customization is in progress; wait for it to complete and try again.") — AssertionError: a losing racer must get ConflictException with its exact message, got [('ConflictException', "Another update of this web app's customization is in progress; wait for it to complete and try again.")]. Source: state/web_app_customization.rs lines 38-47 (const CUSTOMIZATION_WRITE_IN_PROGRESS / write_conflict), raised at ops/delete_web_app_customization.rs lines 205-212.

Cause 2 — expected: after transfer.delete_server(ServerId=S) succeeds with an ACTIVE agreement A on S, transfer.delete_agreement(ServerId=S, AgreementId=A) -> HTTP 404 ('ResourceNotFoundException', 'Unknown agreement'); actual: AssertionError: expected DeleteAgreement on the orphaned agreement to fail, got 'NotImplemented' — HTTP 501 with code NotImplemented, which the operation does not model. DescribeServer and DescribeAgreement on the same pair correctly answered 404 ('Unknown server' / 'Unknown agreement') immediately before. Source: src/ops/delete_agreement.rs still returns crate::scaffolded::Scaffolded::not_implemented.

## Steps to Reproduce
Cause 1:
1. Boot the emulator; sso-admin.list_instances to get an Identity Center InstanceArn; iam.create_role trusted by transfer.amazonaws.com.
2. transfer.create_web_app(IdentityProviderDetails={'IdentityCenterConfig': {'InstanceArn': <arn>, 'Role': <role arn>}}).
3. transfer.update_web_app_customization(WebAppId=<id>, Title='x') and confirm DescribeWebAppCustomization reports it.
4. Issue 8 concurrent transfer.delete_web_app_customization(WebAppId=<same id>) calls from a thread pool.
5. Observe a losing racer refused 409 ConflictException with the message "Another update of this web app's customization is in progress; wait for it to complete and try again." instead of 'Unable to delete WebApp customization'.

Cause 2:
1. Create a server, a LOCAL profile, a PARTNER profile, an S3 bucket and a role; transfer.create_agreement(ServerId=S, LocalProfileId=..., PartnerProfileId=..., AccessRole=..., BaseDirectory='/<bucket>') and confirm DescribeAgreement reports Status 'ACTIVE'.
2. transfer.delete_server(ServerId=S) — it succeeds.
3. transfer.describe_agreement(ServerId=S, AgreementId=A) -> 404 ResourceNotFoundException 'Unknown agreement' (correct today).
4. transfer.delete_agreement(ServerId=S, AgreementId=A).
5. Observe HTTP 501 code 'NotImplemented' instead of HTTP 404 ResourceNotFoundException 'Unknown agreement'.
