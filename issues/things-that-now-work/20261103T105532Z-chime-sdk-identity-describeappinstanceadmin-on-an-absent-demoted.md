Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.delete-app-instance-admin-demotes
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.delete-app-instance-admin-demotes|run#13|AssertionError|assert describe_code == "ForbiddenException", (
Issue-Commit: adb5576e5b4c8de09688914f068f290c40dc2de8

# chime-sdk-identity: DescribeAppInstanceAdmin on an absent/demoted admin grant returns BadRequestException(NotFound) instead of ForbiddenException

> _Note: this issue was identified against the code base at commit `adb5576e5b4c8de09688914f068f290c40dc2de8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppInstanceAdmin

## Description
Cause 1 of 1: DescribeAppInstanceAdmin misclassifies "this identity holds no admin grant on this AppInstance" as a 400 BadRequestException rather than the 403 ForbiddenException AWS answers with.

AWS: Smithy (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json) models DescribeAppInstanceAdmin with BOTH BadRequestException (HTTP 400, "The input parameters don't match the service's restrictions.") and ForbiddenException (HTTP 403, "The client is permanently forbidden from making the request."). Recorded behavior: after DeleteAppInstanceAdmin demotes an identity, a DescribeAppInstanceAdmin naming that (still well-formed, still existing) AppInstanceUser ARN under the same AppInstance is refused with error code ForbiddenException and the message "You are not allowed to make this request." — Chime models "no such admin grant" as an authorization refusal, not as a malformed-input or missing-resource one. The message is entirely fixed text with no variable parts: it interpolates neither the admin ARN nor the AppInstance ARN.

Emulator: returns BadRequestException (HTTP 400) with error code "NotFound" and a message templated as "<AppInstanceAdminArn> is not an administrator of AppInstance <AppInstanceArn>" (both values echoed from the request). The check fails at the first assertion on the error code, so the status/code divergence is what is observed; the message divergence follows from the same branch. Note the handler ALREADY answers ForbiddenException with the correct fixed message for the deleted-AppInstance case (crate::state::FORBIDDEN_REQUEST_MESSAGE) — only the absent-grant branch is misclassified.

Why: a client distinguishing "I may not ask" from "my request was malformed" sees the wrong class. botocore surfaces ForbiddenException vs BadRequestException as different exception types, so an except-clause or retry/permission-escalation path keyed on ForbiddenException never fires; worse, a 400 reads as a client-side bug in the caller's own request construction rather than as the expected answer for a demoted admin, which is a normal, non-exceptional state in admin-management code.

Fix: the DescribeAppInstanceAdmin handler's absent-grant branch — the `ok_or_else` on the admin-directory lookup should raise ForbiddenException carrying the service's fixed forbidden-request message (the same constant the deleted-AppInstance branch above it already uses) instead of BadRequestException with code "NotFound".

Traces: services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance_admin.rs lines 186-201 (the BadRequestException/"NotFound" branch); contrast lines 173-183, which construct the correct ForbiddenException. Failing check: chime-sdk-identity.delete-app-instance-admin-demotes.

## Evidence
Check chime-sdk-identity.delete-app-instance-admin-demotes, after CreateAppInstanceAdmin then DeleteAppInstanceAdmin on the same AppInstanceUser ARN:

Expected (AWS): DescribeAppInstanceAdmin(AppInstanceAdminArn=<demoted user arn>, AppInstanceArn=<instance arn>) -> HTTP 403, Error.Code == "ForbiddenException", Error.Message == "You are not allowed to make this request." (fixed text, no interpolated values).

Actual (emulator): HTTP 400, Error.Code == "BadRequestException" (handler sets the inner `code` field to "NotFound"), Error.Message == "<AppInstanceAdminArn> is not an administrator of AppInstance <AppInstanceArn>".

Assertion: AssertionError: expected ForbiddenException from DescribeAppInstanceAdmin on a demoted identity, got BadRequestException (check_chime_sdk_identity_delete_app_instance_admin_demotes.py line 57). The follow-on message assertion (line 63) is unreached but diverges for the same reason.

Everything before this point in the check passes: the grant is created, appears in ListAppInstanceAdmins, and is absent after DeleteAppInstanceAdmin — so state tracking of the demotion is correct and only the refusal classification diverges.

## Steps to Reproduce
1. client = boto3.client("chime-sdk-identity", region_name="us-east-1") against the emulator.
2. arn = client.create_app_instance(Name=<unique>, Metadata="", ClientRequestToken=uuid4().hex)["AppInstanceArn"].
3. user = client.create_app_instance_user(AppInstanceArn=arn, AppInstanceUserId=<unique>, Name="demoted-admin", ClientRequestToken=uuid4().hex)["AppInstanceUserArn"].
4. client.create_app_instance_admin(AppInstanceAdminArn=user, AppInstanceArn=arn) — succeeds, and the grant is listed by list_app_instance_admins.
5. client.delete_app_instance_admin(AppInstanceAdminArn=user, AppInstanceArn=arn) — succeeds, grant no longer listed.
6. client.describe_app_instance_admin(AppInstanceAdminArn=user, AppInstanceArn=arn).
   Observed: ClientError with Code "BadRequestException" (HTTP 400).
   Expected: ClientError with Code "ForbiddenException" (HTTP 403) and Message "You are not allowed to make this request.".
(The same divergence reproduces without step 4/5 at all — any DescribeAppInstanceAdmin for an identity that was never promoted.)
