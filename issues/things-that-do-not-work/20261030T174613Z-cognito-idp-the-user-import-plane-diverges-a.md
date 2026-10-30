Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.e2e-user-import-job-csv-header-to-active-users cognito-idp.e2e-user-import-job-csv-to-listusers cognito-idp.create-user-import-job-missing-trust-policy-rejected cognito-idp.get-csv-header-matches-user-pool-schema cognito-idp.get-csv-header-without-permission-denied
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.e2e-user-import-job-csv-header-to-active-users|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cognito-idp.e2e-user-import-job-csv-to-listusers|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cognito-idp.create-user-import-job-missing-trust-policy-rejected|run#22|AssertionError|assert code == "PreconditionNotMetException", (
Issue-Key: cognito-idp.get-csv-header-matches-user-pool-schema|run#9|AssertionError|assert not missing and not extra, f"CSVHeader diff: missing={missing} extra={extra} full={sorted(header)}"
Issue-Key: cognito-idp.get-csv-header-without-permission-denied|_poll_for_denial#15|AssertionError|raise AssertionError(
Issue-Commit: fdbc3a0ffd43a28553464e8afd6021169d0df514

# cognito-idp: the user-import plane diverges — a started import job never imports the CSV, StartUserImportJob skips the CloudWatch-Logs role trust check, and GetCSVHeader omits password_hash and is not IAM-authorized (4 causes)

> _Note: this issue was identified against the code base at commit `fdbc3a0ffd43a28553464e8afd6021169d0df514`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartUserImportJob

## Description
Four distinct root causes in the cognito-idp user-import surface, all grounded in failing behavioral checks of this suite. None is marked ALREADY FILED or OWNED in the replay report. Related-but-different open issues (named here so a fixer does not conflate them): 20261029T184956Z-cognito-idp-identity-provider-defaults-resource-server-scope.md covers only the FORM of a minted import-job id; 20261024T114943Z-cognito-idp-getcsvheader-stub.md / 20261024T114943Z-cognito-idp-createuserimportjob-stub.md were about those operations being unimplemented scaffolds — both now answer, so neither covers the four behaviors below; 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md is an iam-service issue and does not carry Cause 4's cognito-idp replay key.

=== Cause 1 of 4: a started user import job never imports the uploaded CSV and never reaches a terminal status ===
AWS: StartUserImportJob "Instructs your user pool to start importing users from a CSV file that contains their usernames and attributes" (Smithy: StartUserImportJob doc; output UserImportJob -> UserImportJobType). UserImportJobType.Status is documented as the progression Created -> Pending -> InProgress -> Succeeded (and Stopping/Stopped/Failed/Expired), and UserImportJobType carries ImportedUsers, SkippedUsers, FailedUsers and CompletionMessage, which AWS fills in when the job completes. On AWS each CSV row becomes a real user in the pool, in UserStatus RESET_REQUIRED, with the row's attributes readable through AdminGetUser and ListUsers.
Emulator: the job's status is computed from stored timestamps only — Created, then Pending, then InProgress one span later — and there is no Succeeded (or Failed) transition at all, so a started job stays InProgress forever; the three counters are hard-coded (imported_users: 0, skipped_users: 0, failed_users: 0) and CompletionMessage is always absent; the uploaded CSV body is only counted in bytes (has_uploaded_csv checks csv_uploaded_at/csv_bytes) and is never parsed, so no user is ever created. Both e2e checks therefore poll DescribeUserImportJob for a terminal status until the harness kills them ("Check timed out after 60.0s"); the imported usernames never appear in ListUsers/AdminGetUser.
Why: a client that imports users through a job sees the job hang in InProgress, reads ImportedUsers=0, and finds none of the imported users in the pool — every downstream step (AdminSetUserPassword on the imported user, ADMIN_USER_PASSWORD_AUTH sign-in, ListUsers) fails against a user that AWS would have created.
Fix: the StartUserImportJob handler plus the user-import-job state machine must actually consume the CSV uploaded to the job's PreSignedUrl: parse the header row against the pool's CSV header, create one user per row (UserStatus RESET_REQUIRED, row attributes stored), advance the job Pending -> InProgress -> Succeeded, and report ImportedUsers/SkippedUsers/FailedUsers and CompletionMessage from that run instead of constants.
Traces: cognito-idp.e2e-user-import-job-csv-header-to-active-users, cognito-idp.e2e-user-import-job-csv-to-listusers (both time out waiting for a terminal job status). Code sites: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/start_user_import_job.rs (handle: job.start(now) is the only effect) and src/state.rs (UserImportJob::status, UserImportJob::to_user_import_job_type — the hard-coded zero counters and the status ladder with no Succeeded).

=== Cause 2 of 4: StartUserImportJob does not verify that CloudWatchLogsRoleArn is assumable by cognito-idp.amazonaws.com ===
AWS: the CloudWatch Logs role a user import job names must trust cognito-idp.amazonaws.com; Cognito re-validates it when the job is started, and a role whose trust policy does not allow sts:AssumeRole for the cognito-idp.amazonaws.com service principal makes StartUserImportJob refuse with PreconditionNotMetException (Smithy: StartUserImportJob errors include PreconditionNotMetException, HTTP 400, "This exception is thrown when a precondition is not met"; CreateUserImportJob.CloudWatchLogsRoleArn is required and models the same error). The refusal persists for as long as the trust policy is wrong, and the identical call succeeds once the SAME role's trust policy is corrected in place.
Emulator: StartUserImportJob only looks up the pool and the job, checks the job status and whether any bytes were uploaded; it never resolves CloudWatchLogsRoleArn or inspects the role's trust policy. A job created with a role that trusts ec2.amazonaws.com starts successfully (no error — the check records error code None on its first attempt) while a correctly-trusted role on the identical wiring also starts, so the two are indistinguishable.
Why: a client with a misconfigured import role gets a job that appears to start on the emulator and fails on AWS, so the misconfiguration is only discovered in production; the negative control for import-role trust can never be reproduced.
Fix: the StartUserImportJob handler's precondition validation (and CreateUserImportJob's equivalent role check) must resolve the named role and refuse with PreconditionNotMetException when its trust policy does not allow sts:AssumeRole for the service principal cognito-idp.amazonaws.com. The message's fixed text is AWS's precondition sentence for this gate; it carries no per-run value from the request.
Traces: cognito-idp.create-user-import-job-missing-trust-policy-rejected (assertion: expected PreconditionNotMetException, got None). Code site: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/start_user_import_job.rs.

=== Cause 3 of 4: GetCSVHeader omits the password_hash column ===
AWS: Smithy models GetCSVHeader's output CSVHeader as ListOfStringTypes and says nothing about its contents, so: recorded behavior only. The recording shows AWS returning the pool's standard-attribute import columns (name, given_name, family_name, middle_name, nickname, preferred_username, profile, picture, website, email, email_verified, gender, birthdate, zoneinfo, locale, phone_number, phone_number_verified, address, updated_at, cognito:mfa_enabled, cognito:username), one custom:<name> column per custom schema attribute, AND a password_hash column that supports importing users with pre-hashed passwords.
Emulator: the returned header contains every standard column and the custom column, but no password_hash column at all (replay diff: missing={'password_hash'} extra=set()).
Why: a client that builds its import CSV from GetCSVHeader has no password_hash column, so the pre-hashed-password import path (CreateUserImportJob with PasswordHashingAlgorithm) cannot be driven from the pool's own template, and checks that strip or fill that column behave differently against the emulator than against AWS.
Fix: the GetCSVHeader handler / the pool's CSV-header derivation in state must append the password_hash column to the standard-plus-custom column set it renders.
Traces: cognito-idp.get-csv-header-matches-user-pool-schema. Code site: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/get_csv_header.rs (and the header derivation it reads out of src/state.rs).

=== Cause 4 of 4: GetCSVHeader performs no IAM authorization — an unpolicied principal is answered 200 ===
AWS: GetCSVHeader is an IAM-authorized control-plane call. A principal with no identity-based policy granting cognito-idp:GetCSVHeader is refused with error code AccessDeniedException and the message whose fixed text is "User: <principal ARN> is not authorized to perform: cognito-idp:GetCSVHeader on resource: <user pool ARN> because no identity-based policy allows the cognito-idp:GetCSVHeader action" — the fixed parts are "User: ", " is not authorized to perform: cognito-idp:GetCSVHeader on resource: " and " because no identity-based policy allows the cognito-idp:GetCSVHeader action"; the variable parts are the calling principal's ARN (from the request's credentials) and the pool's ARN (from state). Granting that same principal a policy scoped to that pool's ARN makes the identical call succeed. (Smithy models no AccessDeniedException on this operation — it is the service's common authorization refusal, so: recorded behavior for the code and message.)
Emulator: the restricted client's GetCSVHeader is answered HTTP 200 with the full CSVHeader list (recorded: {'UserPoolId': ..., 'CSVHeader': [...], 'ResponseMetadata': {'HTTPStatusCode': 200}}), i.e. no authorization is evaluated for this operation at all.
Why: an over-permissive control plane — a client validating least-privilege policies (or a test asserting a denial) sees success where AWS denies, so a policy gap in the caller's IAM configuration is invisible until production.
Fix: the GetCSVHeader handler must run the service's IAM authorization gate for cognito-idp:GetCSVHeader against the pool ARN before answering, and emit AccessDeniedException with AWS's message when no identity-based policy allows the action. The service-wide absence of this gate is tracked for the iam service in 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md; this cause carries the cognito-idp GetCSVHeader replay key.
Traces: cognito-idp.get-csv-header-without-permission-denied (assertion: expected AccessDeniedException but call succeeded).

## Evidence
Replay output (behavioral suite, service cognito-idp):

1) cognito-idp.e2e-user-import-job-csv-header-to-active-users — [mismatch] "Check timed out after 60.0s". The check polls DescribeUserImportJob for Status in (Succeeded, Failed, Stopped, Expired) and then asserts Status == 'Succeeded' and ImportedUsers == 1, then AdminGetUser(UserStatus == 'RESET_REQUIRED').
2) cognito-idp.e2e-user-import-job-csv-to-listusers — [mismatch] "Check timed out after 60.0s". Same shape: terminal status, ImportedUsers == 2, FailedUsers == 0, both usernames in ListUsers, email attribute readable on AdminGetUser.
   Emulator source confirming the cause: src/state.rs UserImportJob::status() returns only Stopped/Stopping/InProgress/Pending/Expired/Created (no Succeeded), and to_user_import_job_type() hard-codes imported_users: 0, skipped_users: 0, failed_users: 0, completion_message: None with the comment "No CSV was imported". src/ops/start_user_import_job.rs handle() only calls job.start(now).

3) cognito-idp.create-user-import-job-missing-trust-policy-rejected — [mismatch]
   AssertionError: expected the untrusted-role job's Start to stay rejected with PreconditionNotMetException for >= 30s (3x the 0.0s a correctly-trusted role took to clear on this same wiring), got None on attempt 1
   Expected: StartUserImportJob refused with PreconditionNotMetException (HTTP 400) while CloudWatchLogsRoleArn names a role trusting ec2.amazonaws.com; the job stays Status='Created'; after update_assume_role_policy puts cognito-idp.amazonaws.com back, the identical Start returns Pending/InProgress.
   Actual: the Start on the untrusted-role job returned success (no error code at all).

4) cognito-idp.get-csv-header-matches-user-pool-schema — [mismatch]
   AssertionError: CSVHeader diff: missing={'password_hash'} extra=set() full=['address', 'birthdate', 'cognito:mfa_enabled', 'cognito:username', 'custom:favoritecolor', 'email', 'email_verified', 'family_name', 'gender', 'given_name', 'locale', 'middle_name', 'name', 'nickname', 'phone_number', 'phone_number_verified', 'picture', 'preferred_username', 'profile', 'updated_at', 'website', 'zoneinfo']
   Expected: that same set PLUS 'password_hash'.

5) cognito-idp.get-csv-header-without-permission-denied — [mismatch]
   AssertionError: expected AccessDeniedException but call succeeded: {'UserPoolId': '<pool id>', 'CSVHeader': ['name', ..., 'cognito:username'], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
   Expected: error code AccessDeniedException with message "User: <iam user ARN> is not authorized to perform: cognito-idp:GetCSVHeader on resource: <user pool ARN> because no identity-based policy allows the cognito-idp:GetCSVHeader action", and success only after put_user_policy grants cognito-idp:GetCSVHeader on that pool ARN.
   Actual: HTTP 200 with the full header for a freshly created IAM user that has zero attached policies.

## Steps to Reproduce
Cause 1: CreateUserPool; GetCSVHeader; CreateUserImportJob(JobName, UserPoolId, CloudWatchLogsRoleArn=<role trusting cognito-idp.amazonaws.com>); PUT a CSV (header row from GetCSVHeader minus password_hash, one row with cognito:username + email + email_verified) to the job's PreSignedUrl; StartUserImportJob; poll DescribeUserImportJob. Observe Status never leaves Pending/InProgress, ImportedUsers stays 0, and AdminGetUser/ListUsers never see the imported username (AWS: Status Succeeded, ImportedUsers 1, user present in RESET_REQUIRED).

Cause 2: create an IAM role whose trust policy allows sts:AssumeRole for Principal Service ec2.amazonaws.com; CreateUserImportJob with that role as CloudWatchLogsRoleArn; upload the CSV to the PreSignedUrl; call StartUserImportJob repeatedly for ~30s. Observe it succeeds immediately (AWS: PreconditionNotMetException every time, job stays Created; then fix the same role's trust policy to cognito-idp.amazonaws.com and the identical Start succeeds).

Cause 3: CreateUserPool with Schema=[{Name: 'favoritecolor', AttributeDataType: 'String', Mutable: true, Required: false}]; GetCSVHeader(UserPoolId). Observe the returned list has no 'password_hash' entry.

Cause 4: CreateUserPool; create an IAM user with NO policies and an access key for it; call GetCSVHeader(UserPoolId) with those credentials. Observe HTTP 200 + full CSVHeader instead of AccessDeniedException; then put_user_policy allowing cognito-idp:GetCSVHeader on the pool ARN and observe the same success, so the allow is indistinguishable from the deny.
