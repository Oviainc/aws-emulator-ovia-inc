Issue-Source: trace-sweep-divergence
Issue-Trace: iam-account-password-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-account-password-policy-lifecycle iam-account-password-policy-update iam-account-settings-alias-and-password-policy iam-account-summary-and-authorization-details iam-pg-login-profile-password-policy-violation iam-account-settings-lifecycle iam-account-properties-role-manager-toggle iam-security-token-service-preferences iam-credential-report-generation-and-retrieval
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetAccountPasswordPolicy|2xx->4xx|$.GetAccountPasswordPolicyResponse
Issue-Key: iam|GetAccountPasswordPolicy|2xx->4xx|status
Issue-Key: iam|DeleteAccountPasswordPolicy|2xx->4xx|$.DeleteAccountPasswordPolicyResponse
Issue-Key: iam|DeleteAccountPasswordPolicy|2xx->4xx|status
Issue-Key: iam|GetAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|GetAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|GetAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetAccountPasswordPolicy|4xx->4xx|status
Issue-Key: iam|DeleteAccountPasswordPolicy|4xx->4xx|status
Issue-Key: iam|DeleteAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: iam|DeleteAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: iam|DeleteAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetAccountPasswordPolicy|2xx->4xx|$
Issue-Key: iam|GetAccountSummary|2xx->4xx|$.GetAccountSummaryResponse
Issue-Key: iam|GetAccountSummary|2xx->4xx|status
Issue-Key: iam|GenerateCredentialReport|2xx->4xx|$.GenerateCredentialReportResponse
Issue-Key: iam|GenerateCredentialReport|2xx->4xx|status
Issue-Key: iam|GetCredentialReport|2xx->4xx|$
Issue-Key: iam|GetCredentialReport|2xx->4xx|$.GetCredentialReportResponse
Issue-Key: iam|GetCredentialReport|2xx->4xx|status
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Account-level IAM actions answer InvalidAction instead of being routed

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountPasswordPolicy

## Description
Three modelled IAM operation families are not dispatched at all: the request is answered 400 ValidationError-class body with Code InvalidAction and Message 'unknown action: <Action>'.

Cause 1 of 3 - account password policy reads/deletes. AWS: GetAccountPasswordPolicy takes no input and returns GetAccountPasswordPolicyResult/PasswordPolicy (Smithy: output member PasswordPolicy required; errors NoSuchEntity 404, ServiceFailure 500). When no policy is set AWS answers 404 NoSuchEntity with Message 'The Password Policy with domain name <accountId> cannot be found.' (fixed text, with the 12-digit account id of the caller substituted). DeleteAccountPasswordPolicy returns an empty DeleteAccountPasswordPolicyResult on success and 404 NoSuchEntity 'The account policy with name PasswordPolicy cannot be found.' when none is set. Emulator: both actions are unrouted - 400 / Code InvalidAction / 'unknown action: GetAccountPasswordPolicy' (resp. DeleteAccountPasswordPolicy), so the success path, the NoSuchEntity path and the policy written by UpdateAccountPasswordPolicy are all unreachable. Why: a client cannot read back the password policy it just set, and cannot distinguish 'no policy' from 'bad request'. Fix: route both actions in the IAM action dispatcher and implement their handlers over the account password-policy state UpdateAccountPasswordPolicy writes.
Traces: iam-account-password-policy-lifecycle#task#3, #task#4, #task#5; iam-account-password-policy-update#setup#1, #teardown#4; iam-account-settings-alias-and-password-policy#teardown#17; iam-account-settings-lifecycle#task#6, #task#7; iam-pg-login-profile-password-policy-violation#task#3.

Cause 2 of 3 - GetAccountSummary. AWS: takes no input, returns GetAccountSummaryResult/SummaryMap as a map of entry/key/value (Smithy: output SummaryMap of type map; only error ServiceFailure 500), so it cannot fail for a well-formed call. Emulator: 400 InvalidAction 'unknown action: GetAccountSummary'. Why: console-style clients and quota checks that read the account summary get a hard 400 on a call AWS never fails. Fix: route GetAccountSummary and serialise the account's quota/usage counters as a SummaryMap.
Traces: iam-account-properties-role-manager-toggle#task#8; iam-account-settings-lifecycle#task#1.

Cause 3 of 3 - credential report generation/retrieval. AWS: GenerateCredentialReport returns GenerateCredentialReportResult with State and Description ('No report exists. Starting a new report generation task' on the first call). GetCredentialReport returns Content (base64 text/csv blob), ReportFormat 'text/csv' and GeneratedTime, and signals progress with the modelled ReportInProgress 404 / ReportNotPresent 410 / ReportExpired 410 errors (Smithy: CredentialReportNotReadyException httpResponseCode 404 code ReportInProgress, CredentialReportNotPresentException 410 ReportNotPresent, CredentialReportExpiredException 410 ReportExpired). Emulator: both are unrouted - 400 InvalidAction - so the fixtures' poll-until-200 loop cannot converge and ends 'poll ended: 400 InvalidAction cannot satisfy until_status [200]'. Why: credential-report consumers never get a report and never get one of the modelled not-ready/expired codes they are written to retry on. Fix: route both actions and add credential-report state (generate marks a report ready, get returns the CSV blob).
Traces: iam-account-settings-lifecycle#task#2, #task#3; iam-credential-report-generation-and-retrieval#setup#1.

Note: open issue 20261029T105408Z-iam-updateaccountpasswordpolicy-stub covers UpdateAccountPasswordPolicy's stub only; the read/delete side and the two other families above are not covered by it. Open issue 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four describes 'four modeled operation families answer InvalidAction' behaviorally but carries no replay key for these fixture steps.

## Evidence
Cause 1: expected (recorded AWS) HTTP 200 GetAccountPasswordPolicyResponse/GetAccountPasswordPolicyResult/PasswordPolicy{MinimumPasswordLength, RequireSymbols, ...}; actual HTTP 400 ErrorResponse/Error{Type Sender, Code InvalidAction, Message 'unknown action: GetAccountPasswordPolicy'}. On the not-set path, expected HTTP 404 Code NoSuchEntity Message 'The Password Policy with domain name 000000000000 cannot be found.' - fixed text with the caller's account id substituted; actual HTTP 400 Code InvalidAction. DeleteAccountPasswordPolicy expected 404 NoSuchEntity 'The account policy with name PasswordPolicy cannot be found.'; actual 400 InvalidAction 'unknown action: DeleteAccountPasswordPolicy'.
Cause 2: expected HTTP 200 GetAccountSummaryResponse/GetAccountSummaryResult/SummaryMap/entry{key,value}; actual HTTP 400 Code InvalidAction.
Cause 3: expected HTTP 200 GenerateCredentialReportResult{Description 'No report exists. Starting a new report generation task', State} and HTTP 200 GetCredentialReportResult{ReportFormat 'text/csv', GeneratedTime, Content <base64 csv>}; actual HTTP 400 Code InvalidAction for both, and the fixture poll reports 'poll ended: 400 InvalidAction cannot satisfy until_status [200] after 1 attempt(s)'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iam-account-password-policy-lifecycle: task#1-2 set a password policy, task#3 GetAccountPasswordPolicy -> 400 InvalidAction instead of 200, task#4 DeleteAccountPasswordPolicy -> 400, task#5 GetAccountPasswordPolicy on the now-empty policy -> 400 instead of 404 NoSuchEntity.
3. Replay iam-account-settings-lifecycle#task#1 (GetAccountSummary), #task#2 (GenerateCredentialReport), #task#3 (GetCredentialReport) - each answers 400 InvalidAction 'unknown action: <Action>'.
