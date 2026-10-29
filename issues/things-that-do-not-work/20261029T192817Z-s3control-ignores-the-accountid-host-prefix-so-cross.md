Issue-Source: trace-sweep-divergence
Issue-Trace: s3c-account-id-host-prefix-is-authorized-before-the-request
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3c-account-id-host-prefix-is-authorized-before-the-request
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetAccessPoint|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|GetAccessPoint|4xx->2xx|status
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetAccessPoint|4xx->4xx|status
Issue-Key: s3control|ListAccessPoints|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|ListAccessPoints|4xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control ignores the accountId host prefix, so cross-account requests are never denied

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPoint

## Description
4 net-new root causes, all the same fault: s3control authorizes the request's account before it looks at the resource, and the emulator does neither — it serves (or validates) the request as if the caller's account always matched. AWS resolves the account from the `{AccountId}.` host prefix / `x-amz-account-id` header (Smithy: GetAccessPoint.AccountId @hostLabel + httpHeader x-amz-account-id, @pattern ^\d{12}$, endpoint hostPrefix "{AccountId}."), and refuses a mismatching or unusable account with HTTP 403 before any lookup. Smithy declares no errors for these operations (errors: []), so the codes and messages below are recorded behavior only.

--- Cause 1 of 4: GetAccessPoint for a foreign account is served instead of denied ---
AWS: recorded behavior only — a request whose account differs from the caller's is refused with HTTP 403, `<Error><Code>AccessDenied</Code><Message>Access Denied</Message><AccountId>..</AccountId></Error>` plus RequestId/HostId. Fixed text: code `AccessDenied` and message `Access Denied`; the variable part is `AccountId`, taken from the request.
Emulator: answers HTTP 200 and describes the access point.
Why: a cross-account read that AWS forbids succeeds, so a client (or a security test) concludes the account boundary does not apply; data from another account's control plane is exposed.
Fix: request authorization in the s3control crate — resolve the account from the host prefix/header and deny before dispatching to the operation handler.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#4.

--- Cause 2 of 4: GetAccessPoint for a foreign account and an absent name leaks existence as 404 ---
AWS: recorded behavior only — authorization comes first, so the answer is 403 AccessDenied / `Access Denied` with `<AccountId>` from the request, regardless of whether the named access point exists.
Emulator: answers 404 NoSuchAccessPoint with the message `The specified access point does not exist: <name>`.
Why: the ordering difference is an existence oracle — a caller unauthorized for an account can distinguish present from absent resources in it, which AWS prevents by denying first.
Fix: same authorization gate, evaluated ahead of resource lookup.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#5.

--- Cause 3 of 4: an unusable accountId is a validation error instead of 403 ---
AWS: recorded behavior only — a non-numeric account in the host prefix/header is refused with 403 AccessDenied / `Access Denied` carrying `<AccountId>` echoing the value from the request.
Emulator: answers 400 `ValidationException` with `1 validation error detected: Value '<value>' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$`.
Why: a client sees a malformed-input error where AWS reports an authorization failure, so retry/alerting logic keyed on 403 never fires; it also confirms that the emulator validates the account instead of authorizing it.
Fix: same authorization gate — an account that cannot be authorized is a 403, not a shape violation.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#7.

--- Cause 4 of 4: ListAccessPoints for a foreign account is served instead of denied ---
AWS: recorded behavior only — 403 AccessDenied / `Access Denied` with `<AccountId>` from the request.
Emulator: answers HTTP 200 with a listing.
Why: as cause 1, and worse for a list: another account's access points are enumerated.
Fix: same authorization gate, applied to the listing operations too.
Traces: s3c-account-id-host-prefix-is-authorized-before-the-request#task#8.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator, fixture s3c-account-id-host-prefix-is-authorized-before-the-request:
- task#4 GetAccessPoint: `status: expected 403, got 200`; `$.ErrorResponse: missing (expected {'Error': {'Code': 'AccessDenied', 'Message': 'Access Denied', 'AccountId': '<account from request>'}, ...})`.
- task#5 GetAccessPoint (foreign account, absent name): `status: expected 403, got 404`; `Code: expected 'AccessDenied', got 'NoSuchAccessPoint'`; `Message: expected 'Access Denied', got 'The specified access point does not exist: <name>'`; `Error.AccountId: missing`.
- task#7 GetAccessPoint (non-numeric accountId): `status: expected 403, got 400`; `Code: expected 'AccessDenied', got 'ValidationException'`; `Message: expected 'Access Denied', got "1 validation error detected: Value 'notanaccountid' at 'accountId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\d{12}$"`; `Error.AccountId: missing`.
- task#8 ListAccessPoints: `status: expected 403, got 200`; whole ErrorResponse missing.
Recorded RequestId/HostId values are per-run and not requirements.

## Steps to Reproduce
1. Boot the emulator and replay s3c-account-id-host-prefix-is-authorized-before-the-request#task#4: GET /v20180820/accesspoint/{Name} with the host prefix / x-amz-account-id of an account other than the signing one. The emulator answers 200; the recording answers 403 AccessDenied with the AccountId member.
2. task#5: same request for a name that does not exist — the emulator answers 404 NoSuchAccessPoint instead of 403.
3. task#7: send `notanaccountid` as the account — the emulator answers 400 ValidationException instead of 403 AccessDenied.
4. task#8: GET /v20180820/accesspoint for the foreign account — the emulator returns a listing instead of 403.
