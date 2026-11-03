Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected cognito-idp.auth-challenge-session-sequencing cognito-idp.code-delivery-failure-error cognito-idp.refarch-cog-forgot-password-unknown-client-rejected cognito-idp.user-import-job-sequential-state-progression cognito-idp.user-import-job-start-without-csv-rejected cognito-idp.user-import-job-start-wrong-state-rejected cognito-idp.update-provisioned-limit-exceeds-quota-rejected cognito-idp.update-provisioned-limit-excessive-value-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.auth-challenge-session-sequencing|run#18|AssertionError|assert code_replay_msg == 'Invalid session for the user, session can only be used once.', f"expected the exact {code_replay} message, got {code_replay_msg!r}"
Issue-Key: cognito-idp.code-delivery-failure-error|run#13|AssertionError|assert err1.get("Message") == expected1, (err1.get("Message"), expected1)
Issue-Key: cognito-idp.refarch-cog-forgot-password-unknown-client-rejected|run#9|AssertionError|assert message == "Username/client id combination not found.", (
Issue-Key: cognito-idp.user-import-job-sequential-state-progression|run#16|AssertionError|assert msg_stop_before_start == expected, (msg_stop_before_start, expected)
Issue-Key: cognito-idp.user-import-job-start-without-csv-rejected|run#14|AssertionError|assert message == f"No csv file was uploaded for {job_id}", message
Issue-Key: cognito-idp.user-import-job-start-wrong-state-rejected|run#11|AssertionError|assert message == f"{job_id} is not in the created state.", message
Issue-Key: cognito-idp.update-provisioned-limit-exceeds-quota-rejected|run#12|AssertionError|assert message.startswith(f"Requested value {absurd_value} exceeds Service Quota ceiling of "), message
Issue-Key: cognito-idp.update-provisioned-limit-excessive-value-rejected|run#10|AssertionError|assert message.startswith(f"Requested value {excessive_value} exceeds Service Quota ceiling of "), message
Issue-Commit: e56f1153b059bce6a929f0aeca74333a3bd5954e

# cognito-idp: eight refusal messages are emulator-authored prose instead of AWS's wording, and one refusal is raised for the wrong reason (8 causes)

> _Note: this issue was identified against the code base at commit `e56f1153b059bce6a929f0aeca74333a3bd5954e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminSetUserMFAPreference

## Description
Eight net-new root causes where the emulator refuses with the right class of failure but not AWS's message — and in Cause 1 not even AWS's reason, because the validation order differs. An HTTP status, an error code and an error message are all requirements, so each of these is a defect a client that matches on the message sees.

NOTE on adjacent issues: open issue 20261029T182116Z-cognito-idp-provisioned-limit-api-diverges-no-default.md describes the Cause 8 message as its own Cause 4, but under different replay keys; the checks below are still failing and are listed so the fixer re-replays them. Open issue 20261030T174613Z-cognito-idp-the-user-import-plane-diverges-a.md covers other user-import divergences but none of Causes 5-7. Fixed issues 20261025T084658Z-cognito-idp-three-causes-admincreateuser-accepts-an-sms...md and 20261025T192709Z-cognito-idp-setusermfapreference-answers-501-notimplemented-instead-of.md made those calls refuse at all; the refusals they produce now carry the wrong wording/reason, which is what Causes 1 and 3 are. None of these is a wrongly-refused call (AWS refuses too), so none is a REOPENS.

Cause 1 of 8 — AdminSetUserMFAPreference reports the wrong reason when two factors are both marked PreferredMfa.
AWS: marking more than one MFA factor PreferredMfa=True in a single call is refused with InvalidParameterException (Smithy: AdminSetUserMFAPreference errors = InvalidParameterException 400, NotAuthorizedException 403, OperationNotEnabledException 400, PasswordResetRequiredException 400, ResourceNotFoundException 404, UserNotConfirmedException 400, UserNotFoundException 404, InternalErrorException 500) and the message 'Two MFA types marked as preferred, only one is allowed.' — entirely fixed text, no request-derived part. The conflict is evaluated BEFORE the per-factor availability checks, so it is the answer even on a pool where software-token MFA has not been configured. No MFA settings are stored.
Emulator: InvalidParameterException with 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA' — a different rule's message, reached because the factor-availability check runs first.
Why: the caller is told to fix TOTP delivery configuration when the actual defect is its own request marking two factors preferred; following the emulator's message does not make the call succeed on AWS.
Fix: the validation order and message in the AdminSetUserMFAPreference handler (check the multiple-preferred conflict across SMSMfaSettings/SoftwareTokenMfaSettings/EmailMfaSettings/WebAuthnMfaSettings before any per-factor availability check); the same rule belongs to SetUserMFAPreference.
Traces: cognito-idp.admin-set-user-mfa-preference-conflicting-preferred-methods-rejected, cognito-idp.admin-set-user-mfa-preference-conflicting-settings-rejected.

Cause 2 of 8 — the replayed-challenge-session refusal drops the single-use clause.
AWS: re-using a challenge Session that was already consumed is refused with NotAuthorizedException (Smithy: RespondToAuthChallenge errors include NotAuthorizedException 403) and the message 'Invalid session for the user, session can only be used once.' — entirely fixed text. That clause is what distinguishes a replayed session from a merely malformed/expired one, which AWS reports as 'Invalid session for the user.'
Emulator: NotAuthorizedException 'Invalid session for the user.' for the replay case, conflating the two.
Why: a client cannot tell 'you already spent this session, start a new auth' from 'this session is not valid at all', so retry logic built on the distinction misbehaves.
Fix: the session-consumption branch of the RespondToAuthChallenge handler's refusal message.
Traces: cognito-idp.auth-challenge-session-sequencing.

Cause 3 of 8 — AdminCreateUser's SMS-without-phone_number refusal is reworded.
AWS: AdminCreateUser with DesiredDeliveryMediums ['SMS'] for a user carrying no phone_number attribute is refused at validation time with InvalidParameterException (Smithy: AdminCreateUser errors include InvalidParameterException 400) and the message 'No phone_number provided but desired delivery medium was SMS' — entirely fixed text, no trailing period.
Emulator: InvalidParameterException 'Cannot send the message by SMS. No registered phone_number'.
Why: the code is right but the message is not AWS's, so message-matching clients and fixtures diverge; the emulator's wording also reads like a delivery failure rather than a request-validation failure.
Fix: the DesiredDeliveryMediums validation message in the AdminCreateUser handler.
Traces: cognito-idp.code-delivery-failure-error.

Cause 4 of 8 — ForgotPassword names which half of the username/client-id pair was wrong.
AWS: ForgotPassword with a ClientId that does not exist is refused with the message 'Username/client id combination not found.' — entirely fixed text — deliberately refusing the COMBINATION rather than disclosing which half was wrong (Smithy: ForgotPassword errors include ResourceNotFoundException 404 and NotAuthorizedException 403).
Emulator: 'User pool client <ClientId> does not exist.' — emulator prose that interpolates the request's ClientId and discloses that the client, not the user, was the unknown half.
Why: it is an enumeration oracle AWS deliberately does not give (an attacker learns whether a client id is real), and the message does not match AWS for any client.
Fix: the not-found refusal message in the ForgotPassword handler.
Traces: cognito-idp.refarch-cog-forgot-password-unknown-client-rejected.

Cause 5 of 8 — StopUserImportJob's not-running refusal is reworded.
AWS: StopUserImportJob on a job that is not running is refused with the message 'Job is not running. The specified job has a status of <Status>' — fixed text 'Job is not running. The specified job has a status of ' plus the job's current status from the emulator's state (here 'Created'); the job id does not appear (Smithy: StopUserImportJob errors include PreconditionNotMetException 400).
Emulator: 'User import job <JobId> is in status Created and is not running.' — emulator prose that interpolates the job id.
Why: message-matching clients and the documented wording diverge; the job id in the text also makes the message unstable across runs.
Fix: the refusal message in the StopUserImportJob handler.
Traces: cognito-idp.user-import-job-sequential-state-progression.

Cause 6 of 8 — StartUserImportJob's no-CSV-uploaded refusal is reworded.
AWS: starting a job before any CSV has been uploaded to its pre-signed URL is refused with PreconditionNotMetException (Smithy: StartUserImportJob errors include PreconditionNotMetException 400) and the message 'No csv file was uploaded for <JobId>' — fixed text 'No csv file was uploaded for ' plus the request's JobId — and the job stays 'Created'.
Emulator: "No users are available for import. Upload the CSV file to the job's pre-signed URL before starting it." — emulator prose that names no job.
Why: clients keying on the documented message, or extracting the job id from it, do not match.
Fix: the no-CSV precondition message in the StartUserImportJob handler.
Traces: cognito-idp.user-import-job-start-without-csv-rejected.

Cause 7 of 8 — StartUserImportJob's wrong-state refusal is reworded.
AWS: starting a job that is no longer in the created state is refused with PreconditionNotMetException and the message '<JobId> is not in the created state.' — the variable part is the request's JobId, which LEADS the message, followed by the fixed ' is not in the created state.'
Emulator: 'User import job <JobId> is in status Pending and cannot be started.'
Why: same as above — the refusal is right, its text is not AWS's, so the two message shapes cannot both be parsed by one client.
Fix: the already-started precondition message in the StartUserImportJob handler.
Traces: cognito-idp.user-import-job-start-wrong-state-rejected.

Cause 8 of 8 — UpdateProvisionedLimit's over-ceiling refusal message.
AWS: a RequestedLimitValue above the account's Service Quotas ceiling is refused with ServiceQuotaExceededException (Smithy: UpdateProvisionedLimit errors include ServiceQuotaExceededException 400) and a message beginning 'Requested value <RequestedLimitValue> exceeds Service Quota ceiling of <ceiling>' — fixed text 'Requested value ' / ' exceeds Service Quota ceiling of ', variables are the request's RequestedLimitValue and the account's Service Quotas ceiling for that limit — and the provisioned value does not move.
Emulator: "The requested limit value <v> exceeds your account's maximum limit of <n> requests per second for this limit. To increase your limit, submit a Service Quotas increase request." — entirely different prose.
Why: callers that parse the ceiling out of the message to decide how much to request get nothing; fixtures recorded against AWS do not match.
Fix: the ServiceQuotaExceededException message in the UpdateProvisionedLimit handler.
Traces: cognito-idp.update-provisioned-limit-exceeds-quota-rejected, cognito-idp.update-provisioned-limit-excessive-value-rejected.

## Evidence
Verbatim from the replay (expected vs actual):
1. "expected message 'Two MFA types marked as preferred, only one is allowed.', got 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA'" (both AdminSetUserMFAPreference checks, on a user whose phone_number is verified up front).
2. "expected the exact NotAuthorizedException message, got 'Invalid session for the user.'" — expected 'Invalid session for the user, session can only be used once.'
3. "('Cannot send the message by SMS. No registered phone_number', 'No phone_number provided but desired delivery medium was SMS')" — (actual, expected); the error code InvalidParameterException matched.
4. "the message must refuse the username/client-id COMBINATION rather than naming which half was wrong; got 'User pool client abcdefghij0123456789klmnop does not exist.'" — expected 'Username/client id combination not found.'
5. "('User import job import-ba3a1e8e-... is in status Created and is not running.', 'Job is not running. The specified job has a status of Created')" — (actual, expected).
6. "AssertionError: No users are available for import. Upload the CSV file to the job's pre-signed URL before starting it." — expected f'No csv file was uploaded for {job_id}'.
7. "AssertionError: User import job import-385e4790-... is in status Pending and cannot be started." — expected f'{job_id} is not in the created state.'
8. "The requested limit value 2000000000 exceeds your account's maximum limit of 10000 requests per second for this limit. To increase your limit, submit a Service Quotas increase request." — expected a message starting 'Requested value 2000000000 exceeds Service Quota ceiling of ' (both UpdateProvisionedLimit over-ceiling checks).

## Steps to Reproduce
1. Create a pool and a user with phone_number verified; AdminSetUserMFAPreference with SMSMfaSettings={Enabled:true,PreferredMfa:true} AND SoftwareTokenMfaSettings={Enabled:true,PreferredMfa:true} -> expect InvalidParameterException 'Two MFA types marked as preferred, only one is allowed.'
2. Drive an auth challenge to a Session, answer it successfully, then RespondToAuthChallenge again with the same Session -> expect NotAuthorizedException 'Invalid session for the user, session can only be used once.'
3. AdminCreateUser with DesiredDeliveryMediums=['SMS'] and UserAttributes carrying only an email -> expect InvalidParameterException 'No phone_number provided but desired delivery medium was SMS'.
4. ForgotPassword with a syntactically valid but non-existent ClientId -> expect the message 'Username/client id combination not found.'
5. CreateUserImportJob, then StopUserImportJob before starting it -> expect 'Job is not running. The specified job has a status of Created'.
6. CreateUserImportJob, then StartUserImportJob with no CSV uploaded -> expect PreconditionNotMetException 'No csv file was uploaded for <JobId>'.
7. CreateUserImportJob, upload the CSV, StartUserImportJob, then StartUserImportJob again -> expect PreconditionNotMetException '<JobId> is not in the created state.'
8. UpdateProvisionedLimit (LimitClass API_CATEGORY, Category UserCreation) with RequestedLimitValue=2000000000 -> expect ServiceQuotaExceededException whose message starts 'Requested value 2000000000 exceeds Service Quota ceiling of '.
