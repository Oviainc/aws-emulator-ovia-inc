Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.generate-credential-report
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.generate-credential-report|run#16|AssertionError|assert restored_code == "OK", (
Issue-Commit: 4f520f1edeaf87046d1e59f82118bd1eb9149075

# REOPENS: IAM GenerateCredentialReport refuses a repeat call with 409 LimitExceeded inside its four-hour regeneration window, where AWS answers 200 with State

> _Note: this issue was identified against the code base at commit `4f520f1edeaf87046d1e59f82118bd1eb9149075`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateCredentialReport

## Description
REOPENS 20261029T192356Z-iam-acceptdelegationrequest-acquirerole-associatedelegationrequest-answer-notimplemented-and-generatecredentialreport.md: its fix left the call refused. That issue reported GenerateCredentialReport as not routed at all (awsQuery InvalidAction). The operation is now routed, but a caller that follows the documented poll loop is refused HTTP 409 LimitExceeded instead of being answered 200 with a State, so the operation is still unusable after the first report exists.

Cause 1 of 1: GenerateCredentialReport turns the account's four-hour report-regeneration rate into a hard refusal: once the account holds a report, every further call inside that window is refused the modeled LimitExceededException (HTTP 409, awsQuery code LimitExceeded) rather than reporting the state of the report the account already has.
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): GenerateCredentialReport takes no input (smithy.api#Unit) and answers GenerateCredentialReportResponse{State: ReportStateType (STARTED | INPROGRESS | COMPLETE), Description: ReportStateDescriptionType}; its only modeled errors are LimitExceededException (httpError 409, awsQueryError code LimitExceeded, documented "The request was rejected because it attempted to create resources beyond the current AWS account limits. The error message describes the limit exceeded.") and ServiceFailureException (500). The documented usage is to call the operation repeatedly until State is COMPLETE, so repetition is the normal client pattern, not a limit violation; LimitExceeded is an account-resource-limit error, not a regeneration-rate refusal. Recorded behavior: the operation keeps answering HTTP 200 with a State value for an account that already holds a report — a call inside the window reports the existing generation (COMPLETE once it is done) instead of failing — and it does so for ANY principal the call is authorized for, so a caller granted exactly iam:GenerateCredentialReport gets a 2xx. The authorization leg of the same check is already correct: an unprivileged caller is refused 403 AccessDenied with "User: <user-arn> is not authorized to perform: iam:GenerateCredentialReport on resource: * because no identity-based policy allows the iam:GenerateCredentialReport action" followed by AWS's authorization-details pointer.
Emulator: once store.credential_report exists and now < requested + CREDENTIAL_REPORT_REGENERATION_SECONDS, the handler returns LimitExceededException (HTTP 409, awsQuery code LimitExceeded) with crate::state::credential_report_rate_exceeded() as the message, and no State is ever reported again until that window ends. In the replay the account's first poll reached State=COMPLETE, GetCredentialReport returned a body, the zero-permission caller was correctly refused AccessDenied — and then, after the check granted that user iam:GenerateCredentialReport on Resource "*", the granted call came back LimitExceeded instead of a 2xx.
Why: a client cannot complete the documented "call until State is COMPLETE" loop, because the second and later calls are a 409 rather than a State. Any tool that regenerates and then reads a credential report (compliance scanners, credential-rotation reporting, Config/Security Hub style collectors) fails against the emulator even though it is fully authorized, and a caller probing whether a grant took effect reads the 409 as a quota problem with its own account instead of a success. It also makes the authorization result unverifiable: a grant that works cannot be distinguished from one that does not, because both answer non-2xx.
Fix: the regeneration-window arm of the handler in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/generate_credential_report.rs — it must report the state of the generation the account already holds (200 with State, the existing report staying readable) instead of returning LimitExceededException, and the message helper crate::state::credential_report_rate_exceeded() should stop being reachable from the rate path. Reserve LimitExceededException for the account-limit condition the model documents.
Traces: iam.generate-credential-report.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator; service heading in the suite report: ### iam.

[mismatch] iam.generate-credential-report:
  AssertionError: expected the grant to allow generate_credential_report once effective, got LimitExceeded

Sequence the check ran (all against the emulator):
1. iam.GenerateCredentialReport polled as the account principal -> reached State='COMPLETE' (passed).
2. iam.GetCredentialReport -> non-empty Content (passed).
3. A freshly created zero-permission IAM user calls GenerateCredentialReport -> Error.Code 'AccessDenied' with AWS's no-identity-based-policy message (passed).
4. iam.PutUserPolicy on that user granting {"Effect":"Allow","Action":"iam:GenerateCredentialReport","Resource":"*"}, then the same caller retries until the code is no longer AccessDenied -> the emulator answers error code 'LimitExceeded' (HTTP 409). Expected: a 2xx success (Error.Code absent; the harness records this as "OK"), carrying State from ReportStateType.

So the divergence is a status/error-code divergence on an authorized call: expected HTTP 200 with {State, Description}; got HTTP 409 with awsQuery <Code>LimitExceeded</Code> and the emulator-authored rate message from crate::state::credential_report_rate_exceeded().

Handler evidence: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/generate_credential_report.rs, the third match arm on store.credential_report — "The account has a report, and the published rate does not allow another one yet" — returns GenerateCredentialReportErrorModeled::LimitExceededException{ message: credential_report_rate_exceeded() } whenever now.secs() < held.requested.secs() + crate::state::CREDENTIAL_REPORT_REGENERATION_SECONDS.

Model evidence (smithy_query, query="operation", operation="GenerateCredentialReport"): input smithy.api#Unit; output {State: ReportStateType (enum), Description: ReportStateDescriptionType}; errors LimitExceededException (client, httpError 409, awsQueryError {code: LimitExceeded, httpResponseCode: 409}) and ServiceFailureException (server, 500).

Per-run values in the quoted lines (the generated user name, its ARN, the authorization id in the AccessDenied message, timestamps) are not part of the requirement; the requirement is that an authorized repeat call answers 200 with a State.

## Steps to Reproduce
1. iam.GenerateCredentialReport as the account principal, repeatedly at a few seconds' interval, until State == 'COMPLETE'.
2. iam.GetCredentialReport — expect a non-empty Content.
3. iam.CreateUser(UserName=<unique>) + iam.CreateAccessKey(UserName=<that user>); build an IAM client from those credentials.
4. As that user, iam.GenerateCredentialReport — expect 403 AccessDenied with "User: arn:aws:iam::<account>:user/<that user> is not authorized to perform: iam:GenerateCredentialReport on resource: * because no identity-based policy allows the iam:GenerateCredentialReport action" plus AWS's authorization-details pointer (this already matches).
5. iam.PutUserPolicy(UserName=<that user>, PolicyName=<unique>, PolicyDocument={"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:GenerateCredentialReport","Resource":"*"}]}).
6. As that user, retry iam.GenerateCredentialReport until the error code is no longer AccessDenied — expect HTTP 200 with a State value from {STARTED, INPROGRESS, COMPLETE}. The emulator answers HTTP 409 'LimitExceeded' because the account already generated a report in step 1, inside the four-hour regeneration window.
7. Re-replay iam.generate-credential-report to confirm the fix.
