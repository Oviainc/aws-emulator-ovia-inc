Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-pab-account-level-account-id-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-pab-account-level-account-id-mistakes
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetPublicAccessBlock|4xx->4xx|status
Issue-Key: s3control|PutPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|PutPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|PutPublicAccessBlock|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|PutPublicAccessBlock|4xx->4xx|status
Issue-Key: s3control|DeletePublicAccessBlock|4xx->2xx|$
Issue-Key: s3control|DeletePublicAccessBlock|4xx->2xx|status
Issue-Related: 20261029T192817Z-s3control-xml-error-envelopes-drop-requestid-hostid-and.md
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3control account-level PublicAccessBlock operations never deny the accountId host prefix

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicAccessBlock

## Description
4 net-new root causes, all the same fault, on the three ACCOUNT-LEVEL PublicAccessBlock operations: s3control authorizes the request's account before it does anything else, and the emulator instead shape-validates the account, looks the configuration up, or just serves the request. Every one of these operations carries its account in the `{AccountId}.` host prefix / `x-amz-account-id` header (Smithy: GetPublicAccessBlock/PutPublicAccessBlock/DeletePublicAccessBlock input member AccountId @required @hostLabel + httpHeader x-amz-account-id, @length max 64, @pattern ^\d{12}$, endpoint hostPrefix "{AccountId}."), so an account that is not the caller's — or that cannot be used as an account at all — is refused with HTTP 403 before any configuration is read or written. The modeled error set does not cover this: GetPublicAccessBlock declares only NoSuchPublicAccessBlockConfiguration (404) and Put/DeletePublicAccessBlock declare errors: [], so the 403 code and message below are recorded behavior only.

This is the same authorization gate that open issue 20261029T192817Z-s3control-ignores-the-accountid-host-prefix-so-cross.md reports for GetAccessPoint/ListAccessPoints; that issue's replay key does NOT cover any step below, so these four causes are filed separately and must be closed on their own fixtures. In AWS's recorded answer the fixed text is the code `AccessDenied` and the message `Access Denied`; the only variable part is the `<AccountId>` member, echoed from the request's host prefix/header. RequestId/HostId are per-run values and are not requirements.

--- Cause 1 of 4: GetPublicAccessBlock with an unusable accountId is a validation error instead of 403 ---
AWS: recorded behavior only — an account value that is not 12 digits is refused at authorization with HTTP 403 and `<ErrorResponse><Error><Code>AccessDenied</Code><Message>Access Denied</Message><AccountId>{the value from the request}</AccountId></Error><RequestId/><HostId/></ErrorResponse>`.
Emulator: answers HTTP 400 with Code `ValidationException` and message `1 validation error detected: Value '<value>' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$`, and omits the `AccountId` member from the error.
Why: a client sees a malformed-input error where AWS reports an authorization failure, so credential/role-assumption retry and security alerting keyed on 403 AccessDenied never fire, and a caller cannot tell it was denied rather than that it sent a bad field.
Fix: the s3control request-authorization layer (ahead of the operation handler and ahead of input validation) — an account that cannot be authorized is a 403, not a shape violation; the error serializer must also emit the `AccountId` member.
Traces: s3control-pab-account-level-account-id-mistakes#task#1.

--- Cause 2 of 4: GetPublicAccessBlock for a foreign account leaks configuration existence as 404 ---
AWS: recorded behavior only — authorization comes first, so a well-formed account that is not the caller's gets 403 AccessDenied / `Access Denied` with `<AccountId>` from the request, whether or not that account has a PublicAccessBlock configuration.
Emulator: answers HTTP 404 with Code `NoSuchPublicAccessBlockConfiguration` and message `The public access block configuration was not found`, with no `AccountId` member.
Why: the ordering difference is an existence oracle — a caller unauthorized for an account can distinguish "account has a PAB configuration" from "it does not", which AWS prevents by denying before lookup. It also means the emulator's account boundary is absent: the lookup ran against another account's state.
Fix: same authorization gate, evaluated before the GetPublicAccessBlock handler reads state.
Traces: s3control-pab-account-level-account-id-mistakes#task#2.

--- Cause 3 of 4: PutPublicAccessBlock with an unusable accountId is a validation error instead of 403 ---
AWS: recorded behavior only — 403 AccessDenied / `Access Denied` with `<AccountId>` echoing the request's value, before the PublicAccessBlockConfiguration payload is considered.
Emulator: answers HTTP 400 `ValidationException` with `1 validation error detected: Value '<value>' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$`, again without the `AccountId` member.
Why: as cause 1, on a mutation: a client that treats 400 as "fix the request body and retry" retries a call AWS will never authorize, and never surfaces the denial.
Fix: same authorization gate, applied to the mutating PAB operations too.
Traces: s3control-pab-account-level-account-id-mistakes#task#3.

--- Cause 4 of 4: DeletePublicAccessBlock for a foreign account succeeds instead of being denied ---
AWS: recorded behavior only — 403 AccessDenied / `Access Denied` with `<AccountId>` from the request; nothing is deleted.
Emulator: answers HTTP 200 with an empty body, i.e. it accepts the delete for an account that is not the caller's.
Why: this is a cross-account MUTATION that AWS forbids. A client (or a security test) concludes the account boundary does not apply, and account-level public-access protection for another account can be removed through the emulator; any test asserting that a foreign-account delete is refused passes wrongly.
Fix: same authorization gate, evaluated before the DeletePublicAccessBlock handler touches state.
Traces: s3control-pab-account-level-account-id-mistakes#task#4.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator, fixture s3control-pab-account-level-account-id-mistakes (all four clusters have empty owned_by and empty stub_path, i.e. net-new):
- task#1 GetPublicAccessBlock (non-numeric account): `status: expected 403, got 400`; `$.ErrorResponse.Error.Code: expected 'AccessDenied', got 'ValidationException'`; `$.ErrorResponse.Error.Message: expected 'Access Denied', got "1 validation error detected: Value 'not-an-account' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$"`; `$.ErrorResponse.Error.AccountId: missing (expected the request's account value, got None)`.
- task#2 GetPublicAccessBlock (well-formed foreign account): `status: expected 403, got 404`; `Code: expected 'AccessDenied', got 'NoSuchPublicAccessBlockConfiguration'`; `Message: expected 'Access Denied', got 'The public access block configuration was not found'`; `Error.AccountId: missing`.
- task#3 PutPublicAccessBlock (non-numeric account): `status: expected 403, got 400`; `Code: expected 'AccessDenied', got 'ValidationException'`; `Message: expected 'Access Denied', got the same "1 validation error detected: ... 'accountId' ... ^\\d{12}$" text`; `Error.AccountId: missing`.
- task#4 DeletePublicAccessBlock (well-formed foreign account): `status: expected 403, got 200`; `$: empty_body (expected {'ErrorResponse': {'Error': {'Code': 'AccessDenied', 'Message': 'Access Denied', 'AccountId': <request's account>}, ...}}, got {})`.
Model evidence: Smithy s3control GetPublicAccessBlock/PutPublicAccessBlock/DeletePublicAccessBlock — AccountId @required @hostLabel, httpHeader x-amz-account-id, @pattern ^\d{12}$, endpoint hostPrefix "{AccountId}."; GetPublicAccessBlock errors [NoSuchPublicAccessBlockConfiguration 404], Put/DeletePublicAccessBlock errors []. Recorded RequestId/HostId values differ per run and are not requirements.

## Steps to Reproduce
1. Boot the emulator and replay fixture s3control-pab-account-level-account-id-mistakes.
2. task#1 — GET /v20180820/configuration/publicAccessBlock with a non-numeric account in the `{AccountId}.` host prefix / `x-amz-account-id` header. The emulator answers 400 ValidationException; the recording answers 403 AccessDenied / `Access Denied` with the `<AccountId>` member echoing the sent value.
3. task#2 — the same GET for a well-formed account that is not the caller's. The emulator answers 404 NoSuchPublicAccessBlockConfiguration; the recording answers 403 AccessDenied before any lookup.
4. task#3 — PUT /v20180820/configuration/publicAccessBlock with a non-numeric account and a valid PublicAccessBlockConfiguration payload. The emulator answers 400 ValidationException; the recording answers 403 AccessDenied.
5. task#4 — DELETE /v20180820/configuration/publicAccessBlock for a well-formed foreign account. The emulator answers 200 with an empty body (the delete is accepted); the recording answers 403 AccessDenied and deletes nothing.
