Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.request-quota-increase-support-case-disallowed-not-approved service-quotas.get-requested-service-quota-change-invalid-request-id-rejected service-quotas.template-operations-require-organization-membership-rejected
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas.request-quota-increase-support-case-disallowed-not-approved|run#12|AssertionError|assert final["Status"] == "NOT_APPROVED", f"unexpected terminal status: {final['Status']!r}"
Issue-Key: service-quotas.get-requested-service-quota-change-invalid-request-id-rejected|run#7|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Key: service-quotas.template-operations-require-organization-membership-rejected|run#7|AssertionError|assert code == expected_code, (
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# Service Quotas ignores SupportCaseAllowed=False, misclassifies a malformed RequestId, and leaves template operations unimplemented

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestServiceQuotaIncrease

## Description
Three independent divergences on the quota-increase request plane: the request state machine ignores the one input that is documented to change its outcome, a pattern-violating RequestId is rejected under the wrong error code, and the quota-request-template operations answer NotImplemented instead of the modeled Organizations/Region gate. Three causes.

Cause 1 of 3 — RequestServiceQuotaIncrease with SupportCaseAllowed=False still resolves the request to APPROVED.
AWS: Smithy documents RequestServiceQuotaIncreaseRequest.SupportCaseAllowed: 'By default, this flag is set to True ... You can set this flag to False if you do not want a support case created when you request a quota increase. If you set the flag to False, Amazon Web Services does not open a support case and updates the request status to Not approved.' So the terminal RequestStatus is NOT_APPROVED (Smithy: RequestStatus enum PENDING, CASE_OPENED, APPROVED, DENIED, CASE_CLOSED, NOT_APPROVED, INVALID_REQUEST), no CaseId is present, and the account's applied quota value is unchanged.
Emulator: the request is created PENDING and then settled to APPROVED, with the DesiredValue put in force as the applied value. The handler states this outright: services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/request_service_quota_increase.rs, module doc lines 9-15 — 'No support case is opened, so SupportCaseAllowed does not change the outcome' — and handle() (same file, lines 117-244) never reads input.support_case_allowed before calling state::record_request.
Why: a client that sets SupportCaseAllowed=False is deliberately opting out of Support review and expects the request to be refused, not granted. Here it is granted and the quota is raised, so the emulator reports headroom the account does not have and the opt-out has no observable effect at all.
Fix: the RequestServiceQuotaIncrease handler and the request-settlement logic in src/state.rs — a request submitted with SupportCaseAllowed=false must settle to NOT_APPROVED without putting DesiredValue in force.
Traces: service-quotas.request-quota-increase-support-case-disallowed-not-approved.

Cause 2 of 3 — GetRequestedServiceQuotaChange rejects a pattern-violating RequestId with IllegalArgumentException instead of ValidationException.
AWS: GetRequestedServiceQuotaChangeRequest.RequestId is @required, @length 1-128, @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$ (Smithy). A value that fails that pattern is refused by the service front end with HTTP 400 and error code ValidationException — a code the operation does not model at all (Smithy: GetRequestedServiceQuotaChange errors are IllegalArgumentException and NoSuchResourceException), so this is recorded behavior only; the recorded message names the member and its constraint ('Value at \'requestId\' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}') and that construction rule, not the exact text, is the requirement.
Emulator: HTTP 400 with error code IllegalArgumentException (message "Value '!!!not-a-valid-request-id!!!' at 'requestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$").
Why: botocore and the AWS SDKs expose the code, and clients branch on it — a caller catching ValidationException for malformed input (and treating IllegalArgumentException as a semantic refusal it should surface differently) takes the wrong branch, and modelled-error-only error factories never produce the ValidationException class the caller catches.
Fix: the constraint-validation layer that rejects pattern violations for this operation (ops/get_requested_service_quota_change.rs and the shared input-validation path) must emit ValidationException for a front-end constraint violation and keep IllegalArgumentException for semantic refusals.
Traces: service-quotas.get-requested-service-quota-change-invalid-request-id-rejected.

Cause 3 of 3 — the quota-request-template operations are scaffold stubs that answer NotImplemented instead of the modeled topology/Region gate.
AWS: PutServiceQuotaIncreaseRequestIntoTemplate and GetServiceQuotaIncreaseRequestFromTemplate model exactly this gate (Smithy: errors AWSServiceAccessNotEnabledException, DependencyAccessDeniedException, IllegalArgumentException, NoAvailableOrganizationException, NoSuchResourceException, TemplatesNotAvailableInRegionException, plus QuotaExceededException on the Put). Recorded behavior: from an account that belongs to no AWS Organization both calls are refused with NoAvailableOrganizationException (and a non-management member account gets AccessDeniedException, example message 'The request was called by a member account. To make this request, you must use the organization's master account.'); either way no template entry becomes observable and unrelated Service Quotas calls keep working.
Emulator: both calls return error code NotImplemented ('operation ... is scaffolded but not implemented in this emulator'), i.e. the operation is an unimplemented scaffold stub and the template gate is never evaluated. Note the suite did NOT mark this check as covered by an auto-filed scaffold issue, so it is filed here.
Why: a client cannot distinguish 'templates are not available for this account/Region' from 'this emulator is broken'; the NotImplemented code is not in the operation's error set, so SDK error handling for the template feature cannot be exercised, and provisioning code that treats the documented refusal as a normal standalone-account outcome aborts instead.
Fix: implement the template operations' gate in services/service-com-amazonaws-servicequotas-servicequotasv20190624/src/ops/put_service_quota_increase_request_into_template.rs and ops/get_service_quota_increase_request_from_template.rs (refuse with NoAvailableOrganizationException when the account has no organization, TemplatesNotAvailableInRegionException outside the us-east-1 home Region), with the template store in src/state.rs.
Traces: service-quotas.template-operations-require-organization-membership-rejected.

## Evidence
Cause 1: check service-quotas.request-quota-increase-support-case-disallowed-not-approved — RequestServiceQuotaIncrease(ServiceCode='ec2', an adjustable Elastic-IP quota, DesiredValue = 20x current + jitter, SupportCaseAllowed=False) is accepted PENDING; polling GetRequestedServiceQuotaChange to a terminal state gives AssertionError: "unexpected terminal status: 'APPROVED'" (expected NOT_APPROVED). Source confirmation: ops/request_service_quota_increase.rs lines 9-15 and 117-244 never consult SupportCaseAllowed.
Cause 2: check service-quotas.get-requested-service-quota-change-invalid-request-id-rejected — GetRequestedServiceQuotaChange(RequestId='!!!not-a-valid-request-id!!!') -> AssertionError: expected ValidationException for a RequestId that fails the pattern, got {'Code': 'IllegalArgumentException', 'Message': "Value '!!!not-a-valid-request-id!!!' at 'requestId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$"}.
Cause 3: check service-quotas.template-operations-require-organization-membership-rejected — PutServiceQuotaIncreaseRequestIntoTemplate(ServiceCode='vpc', QuotaCode='L-F678F1CE', AwsRegion=<region>, DesiredValue=1000000) against us-east-1 -> AssertionError: expected ... NoAvailableOrganizationException for a standalone account, got 'NotImplemented'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 suite.
2. Cause 1: POST RequestServiceQuotaIncrease for an adjustable quota with SupportCaseAllowed=false, then poll GetRequestedServiceQuotaChange until the status leaves PENDING/CASE_OPENED -> APPROVED (AWS: NOT_APPROVED) and GetServiceQuota shows the raised value.
3. Cause 2: POST GetRequestedServiceQuotaChange with RequestId '!!!not-a-valid-request-id!!!' -> 400 IllegalArgumentException (AWS: ValidationException).
4. Cause 3: POST PutServiceQuotaIncreaseRequestIntoTemplate (and GetServiceQuotaIncreaseRequestFromTemplate) from an account with no AWS Organization -> NotImplemented (AWS: NoAvailableOrganizationException).
