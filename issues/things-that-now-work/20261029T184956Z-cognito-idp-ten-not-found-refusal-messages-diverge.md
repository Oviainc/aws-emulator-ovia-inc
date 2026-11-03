Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-user-signup-confirm-and-mfa
Issue-Step: teardown#19
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-app-client-crud-and-secret-lifecycle cognito-idp-app-client-error-paths cognito-idp-app-client-secret-lifecycle cognito-idp-risk-configuration-lifecycle cognito-idp-user-pool-and-client-lifecycle cognito-idp-user-pool-client-domain-and-replica-configuration cognito-idp-user-session-mfa-and-refresh cognito-idp-client-secret-error-paths cognito-idp-invalid-inputs-and-missing-resources cognito-idp-log-delivery-configuration-lifecycle cognito-idp-ui-customization-crud cognito-idp-start-import-job-not-found-error cognito-idp-user-import-job-error-paths cognito-idp-user-import-job-errors cognito-idp-user-import-job-lifecycle cognito-idp-user-import-readiness-and-lookup cognito-idp-stop-import-job-not-started-error cognito-idp-user-pool-domain-errors cognito-idp-user-pool-replica-errors cognito-idp-user-pool-replica-notfound-errors
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DescribeUserPoolClient|4xx->4xx|$.message
Issue-Key: cognito-idp|DescribeUserPoolClient|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeUserPoolClient|4xx->4xx|status
Issue-Key: cognito-idp|ListUserPoolClientSecrets|4xx->4xx|$.message
Issue-Key: cognito-idp|ListUserPoolClientSecrets|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListUserPoolClientSecrets|4xx->4xx|status
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->4xx|$.message
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetLogDeliveryConfiguration|4xx->4xx|status
Issue-Key: cognito-idp|SetUICustomization|4xx->4xx|$.message
Issue-Key: cognito-idp|SetUICustomization|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetUICustomization|4xx->4xx|status
Issue-Key: cognito-idp|StartUserImportJob|4xx->4xx|$.message
Issue-Key: cognito-idp|StartUserImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|StartUserImportJob|4xx->4xx|status
Issue-Key: cognito-idp|StopUserImportJob|4xx->4xx|$.message
Issue-Key: cognito-idp|StopUserImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|StopUserImportJob|4xx->4xx|status
Issue-Key: cognito-idp|DescribeUserImportJob|4xx->4xx|$.message
Issue-Key: cognito-idp|DescribeUserImportJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeUserImportJob|4xx->4xx|status
Issue-Key: cognito-idp|CreateUserPoolDomain|4xx->4xx|status
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-ten-duplicate-resource-and-policy-refusal.md
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: ten not-found refusal messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPoolClient

## Description
Ten recorded not-found / wrong-state refusals whose message text is emulator-authored. For each, the AWS message is given with its fixed text and variable slots; two patterns recur — AWS sometimes does NOT interpolate the identifier (and the emulator does), and sometimes interpolates a DIFFERENT identifier than the emulator chooses (the client id plus the pool id, where the emulator names only the pool).

Cause 1 of 10: DescribeUserPoolClient for an unknown client. AWS message: `User pool client does not exist.` — fixed text with NO client id. Emulator: `User pool client <clientId> does not exist.`. Fix: the DescribeUserPoolClient not-found branch. Fixture cognito-idp-admin-user-signup-confirm-and-mfa, step teardown#19 (8 recorded steps).
Cause 2 of 10: ListUserPoolClientSecrets with an unknown pool. AWS message: `User pool client <clientId> does not exist in user pool <userPoolId>.` — it names BOTH ids from the request. Emulator: `User pool <poolId> does not exist.`, naming only the pool. Fix: the ListUserPoolClientSecrets not-found branch must resolve the client-in-pool and use that message. Fixture cognito-idp-client-secret-error-paths, step task#4.
Cause 3 of 10: SetLogDeliveryConfiguration with an unknown pool. AWS message: `UserPool with ID: <userPoolId>does not exist.` — note the service's own text has no space between the id and `does` and uses the `UserPool with ID:` prefix. Emulator: `User pool <poolId> does not exist.`. Fix: the pool-resolution not-found message used by this handler. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#9 (2 recorded steps).
Cause 4 of 10: SetUICustomization with an unknown pool. AWS message: `User pool client <clientId> does not exist in user pool <userPoolId>.`. Emulator: `User pool <poolId> does not exist.`. Fix: the SetUICustomization not-found branch. Fixture cognito-idp-ui-customization-crud, step task#7.
Cause 5 of 10: StartUserImportJob for an unknown job. AWS message: `The import job <jobId> does not exist` — prefix `The import job `, no trailing period. Emulator: `User import job <jobId> does not exist.`. Fix: the import-job not-found message. Fixture cognito-idp-start-import-job-not-found-error, step task#2 (4 recorded steps).
Cause 6 of 10: StopUserImportJob for an unknown job — same AWS message, same emulator substitution. Fixture cognito-idp-user-import-job-error-paths, step task#3.
Cause 7 of 10: DescribeUserImportJob for an unknown job — same AWS message, same emulator substitution. Fixture cognito-idp-user-import-readiness-and-lookup, step task#4.
Cause 8 of 10: StopUserImportJob on a job that was never started. AWS message: `Job is not running. The specified job has a status of <Status>` — fixed text with the job's current status from state, and NO job id. Emulator: `User import job <jobId> is in status <Status> and is not running.`. Fix: the StopUserImportJob wrong-state branch. Fixture cognito-idp-stop-import-job-not-started-error, step task#4 (3 recorded steps).
Cause 9 of 10: CreateUserPoolDomain with an unknown pool. AWS message: `UserPool with ID: <userPoolId>does not exist.`. Emulator: `User pool <poolId> does not exist.`. Fix: the CreateUserPoolDomain not-found branch. Fixture cognito-idp-user-pool-domain-errors, step task#5.
Cause 10 of 10: CreateUserPoolReplica with an unknown pool. AWS message: `UserPool with ID: <userPoolId>does not exist.`. Emulator: `User pool <poolId> does not exist.`. Fix: the CreateUserPoolReplica not-found branch. Fixture cognito-idp-user-pool-replica-errors, step task#2 (2 recorded steps).

Related open issue that does not carry these replay keys: 20261029T182116Z-cognito-idp-not-found-and-duplicate-resource-error.md.

## Evidence
Each cause also diverges on status (404 instead of 400, filed separately) and on the missing x-amzn-errortype header; the message diffs are:
  DescribeUserPoolClient: expected 'User pool client does not exist.', got 'User pool client 8578238a5d314afa84532815d7 does not exist.'
  ListUserPoolClientSecrets: expected 'User pool client <clientId> does not exist in user pool <poolId>.', got 'User pool <poolId> does not exist.'
  SetLogDeliveryConfiguration: expected 'UserPool with ID: <poolId>does not exist.', got 'User pool <poolId> does not exist.'
  SetUICustomization: expected 'User pool client <clientId> does not exist in user pool <poolId>.', got 'User pool <poolId> does not exist.'
  StartUserImportJob / StopUserImportJob / DescribeUserImportJob: expected 'The import job <jobId> does not exist', got 'User import job <jobId> does not exist.'
  StopUserImportJob (not running): expected 'Job is not running. The specified job has a status of Created', got 'User import job <jobId> is in status Created and is not running.'
  CreateUserPoolDomain / CreateUserPoolReplica: expected 'UserPool with ID: <poolId>does not exist.', got 'User pool <poolId> does not exist.'

## Steps to Reproduce
1. Replay cognito-idp-admin-user-signup-confirm-and-mfa step teardown#19 and read $.message.
2. Replay each remaining fixture#step named in the causes and diff $.message character-for-character against the recording (including the missing space in 'UserPool with ID: <id>does not exist.' and the absent trailing period on the import-job text).
