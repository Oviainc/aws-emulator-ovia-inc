Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.op-contract-get-provisioned-limit cognito-idp.update-provisioned-limit-concurrent-writes-conflict cognito-idp.provisioned-limit-invalid-category-rejected cognito-idp.provisioned-limit-large-numeric-value-precision-preserved cognito-idp.provisioned-limit-update-concurrent-modification-conflict cognito-idp.provisioned-limit-update-reflected cognito-idp.provisioned-limit-exceeds-quota-rejected cognito-idp.update-provisioned-limit-over-quota-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.op-contract-get-provisioned-limit|run#9|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: cognito-identity-provider|GetProvisionedLimit|ResourceNotFoundException
Issue-Key: cognito-idp.provisioned-limit-invalid-category-rejected|run#8|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: cognito-idp.provisioned-limit-large-numeric-value-precision-preserved|run#9|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException for the unchanged value, got {code!r}"
Issue-Key: cognito-idp.provisioned-limit-update-concurrent-modification-conflict|run#9|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.provisioned-limit-update-reflected|run#9|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.provisioned-limit-exceeds-quota-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.update-provisioned-limit-over-quota-rejected|run#14|AssertionError|assert match, f"unexpected rejection message: {errors.get('message')!r}"
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: provisioned-limit API diverges — no default limits, wrong error class for an unknown category, no-op requests accepted, wrong quota-ceiling message (4 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProvisionedLimit

## Description
Four root causes in the Get/UpdateProvisionedLimit pair.

Cause 1 of 4 — GetProvisionedLimit has no limit to report for a valid category.
AWS: every valid API category has a provisioned limit in the account, so GetProvisionedLimit returns it; ResourceNotFoundException is modeled but not the answer for a valid category that was never explicitly provisioned.
Emulator: ResourceNotFoundException 'There is no limit for API category UserResourceRead in this account.'
Why: a caller cannot read the current value, which is the precondition for every update.
Fix: seed the account's default provisioned limits in the GetProvisionedLimit state.
Traces: cognito-idp.op-contract-get-provisioned-limit, cognito-idp.update-provisioned-limit-concurrent-writes-conflict.

Cause 2 of 4 — an invalid API category is reported as not-found.
AWS: InvalidParameterException (Smithy UpdateProvisionedLimit errors: InvalidParameterException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 400) — an unrecognised LimitDefinition category is a malformed request, not a missing resource.
Emulator: ResourceNotFoundException.
Fix: validate the LimitDefinition category before the lookup.
Traces: cognito-idp.provisioned-limit-invalid-category-rejected.

Cause 3 of 4 — a request equal to the current value is accepted.
AWS: UpdateProvisionedLimit with RequestedLimitValue equal to the current ProvisionedLimitValue is refused with InvalidParameterException (a redundant no-op).
Emulator: the call succeeds (and in one check returns no error at all where an error code was expected).
Why: callers cannot detect that their change was a no-op; conflict/idempotency tests pass against the emulator and fail against AWS.
Fix: the no-op check in the UpdateProvisionedLimit handler.
Traces: cognito-idp.provisioned-limit-large-numeric-value-precision-preserved, cognito-idp.provisioned-limit-update-concurrent-modification-conflict, cognito-idp.provisioned-limit-update-reflected.

Cause 4 of 4 — the over-quota refusal message.
AWS: 'Requested value <RequestedLimitValue> exceeds Service Quota ceiling of <ceiling>' — fixed text, variables are the requested value and the account's Service Quotas ceiling for that limit.
Emulator: "The requested limit value 2000000000 exceeds your account's maximum limit of 10000 requests per second for this limit. To increase your limit, submit a Service Quotas increase request."
Fix: the ServiceQuotaExceeded/InvalidParameter message in UpdateProvisionedLimit.
Traces: cognito-idp.provisioned-limit-exceeds-quota-rejected, cognito-idp.update-provisioned-limit-over-quota-rejected.

## Evidence
Verbatim from the replay:
- uncaught ResourceNotFoundException on GetProvisionedLimit: 'There is no limit for API category UserResourceRead in this account.'
- 'expected InvalidParameterException, got ResourceNotFoundException' (invalid category) and 'expected InvalidParameterException, got \'ResourceNotFoundException\'' (op-contract-get-provisioned-limit)
- 'UpdateProvisionedLimit accepted RequestedLimitValue=50 even though it already equals the current ProvisionedLimitValue -- real AWS rejects this as a no-op'
- 'a same-value UpdateProvisionedLimit request unexpectedly succeeded; real AWS rejects it with InvalidParameterException ("same as current")'
- expected 'Requested value 2000000000 exceeds Service Quota ceiling of 25', got "The requested limit value 2000000000 exceeds your account's maximum limit of 10000 requests per second for this limit. ..."

## Steps to Reproduce
1. GetProvisionedLimit for API category UserResourceRead on a fresh account -> expect the default limit, not ResourceNotFoundException.
2. UpdateProvisionedLimit with a bogus category -> expect InvalidParameterException.
3. GetProvisionedLimit, then UpdateProvisionedLimit with RequestedLimitValue equal to the returned ProvisionedLimitValue -> expect InvalidParameterException.
4. UpdateProvisionedLimit with RequestedLimitValue=2000000000 -> compare the refusal message with AWS's 'Requested value <v> exceeds Service Quota ceiling of <c>'.
