Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: service-quotas.get-service-quota-unknown-quota-code-not-found service-quotas.unknown-service-code-returns-not-found service-quotas.invalid-pagination-token-rejected service-quotas.list-service-quotas-invalid-pagination-token-rejected service-quotas.non-increasing-desired-value-rejected service-quotas.request-quota-increase-not-greater-than-current-rejected service-quotas.non-adjustable-quota-increase-rejected service-quotas.duplicate-pending-quota-increase-request-rejected service-quotas.get-requested-service-quota-change-invalid-request-id-rejected service-quotas.start-auto-management-invalid-opt-in-level-rejected service-quotas.auto-management-exclusion-list-invalid-quota-rejected service-quotas.quota-utilization-report-pagination-consistency
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas.get-service-quota-unknown-quota-code-not-found|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.unknown-service-code-returns-not-found|run#6|AssertionError|assert get_message == expected_get, f"expected message {expected_get!r}, got {get_message!r}"
Issue-Key: service-quotas.invalid-pagination-token-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.list-service-quotas-invalid-pagination-token-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.non-increasing-desired-value-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.request-quota-increase-not-greater-than-current-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.non-adjustable-quota-increase-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.duplicate-pending-quota-increase-request-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.get-requested-service-quota-change-invalid-request-id-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: service-quotas.start-auto-management-invalid-opt-in-level-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: service-quotas.auto-management-exclusion-list-invalid-quota-rejected|run#6|AssertionError|assert code == "IllegalArgumentException", code
Issue-Key: service-quotas.quota-utilization-report-pagination-consistency|run#8|AssertionError|assert field in item, f"quota utilization record missing {field!r}: {item}"
Issue-Commit: 910629c5b211cd374f458a2d82cf0881197b5097

# service-quotas: 9 net-new behavioral root causes — refusal messages are emulator-authored, StartAutoManagement misclassifies two rejections, Smithy validation text lacks AWS's "N validation error detected" prefix, and QuotaUtilizationInfo omits Utilization

> _Note: this issue was identified against the code base at commit `910629c5b211cd374f458a2d82cf0881197b5097`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RequestServiceQuotaIncrease

## Description
Nine distinct root causes, all in the service-quotas (com.amazonaws.servicequotas#ServiceQuotasV20190624, awsJson1_1) emulator, surfaced by 12 failing behavioral checks. In seven of them the emulator picks the right HTTP status and error code but composes its own diagnostic sentence instead of the service's message; in two it answers the wrong error class entirely; in one a modeled response member is never serialized. All nine share one service token, so they are bundled here; each is numbered with its own failing check ids.

Note on existing issues: the open WIRE issue 20261029T192055Z-servicequotas-service-quotas-refusal-messages-are-emulator-authored-instead.md describes the same emulator-authored-message habit for GetServiceQuota (its cause 1) and RequestServiceQuotaIncrease below-default (its cause 3) from the wire corpus; it carries no replay key for ANY of the behavioral checks below (the suite reported no OWNED-by and no ALREADY-FILED for all 12), so causes 1 and 3 here are filed as net-new and a fixer should close them against these check ids. 20261029T033319Z-service-quotas-getquotautilizationreport-pagination.md (NextToken not checked for an incomplete report) and 20261028T055821Z-service-quotas-listservicequotas-error-response.md are about different code paths than cause 9 and cause 2 respectively.

Cause 1 of 9 — GetServiceQuota: the not-found refusal message is emulator prose that echoes the request.
AWS: HTTP 404/NoSuchResourceException (Smithy: GetServiceQuota errors IllegalArgumentException [httpError 400] and NoSuchResourceException [httpError 404], each a single Message member of ExceptionMessage; doc 'The specified resource does not exist.'). Recorded behavior: the Message for an unknown ServiceCode and/or an unknown-but-well-formed QuotaCode is the FIXED sentence 'The request failed because the specified quota and service do not exist.' — it has no variable parts; neither the ServiceCode nor the QuotaCode appears in it.
Emulator: right error code, but Message "no quota with ServiceCode '<ServiceCode>' and QuotaCode '<QuotaCode>' exists" — its own lower-case sentence with both request members interpolated (observed as "no quota with ServiceCode 'vpc' and QuotaCode 'L-00000000' exists" and "no quota with ServiceCode 'sim2real-nonexistent-service-zzz' and QuotaCode 'L-00000000' exists"; those values are this run's inputs, not part of the template).
Why: a client that logs, presents or branches on the message text sees a string AWS never sends, and interpolating caller input makes the message non-comparable between calls. It also fails to tell the caller that the SERVICE may be the thing that does not exist.
Fix: the GetServiceQuota handler's not-found refusal construction — take the message from the crate's single wording source for this error class instead of formatting the request members.
Traces: service-quotas.get-service-quota-unknown-quota-code-not-found, service-quotas.unknown-service-code-returns-not-found.

Cause 2 of 9 — ListServiceQuotas: the invalid-pagination-token refusal message is emulator prose.
AWS: HTTP 400/InvalidPaginationTokenException (Smithy: ListServiceQuotas errors include InvalidPaginationTokenException [httpError 400]; NextToken is @length 0..2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$, so a base64-alphabet token passes syntax validation and must then fail resolution). Recorded behavior: Message is the FIXED sentence 'You specified an invalid value for nextToken. You must get the value from the response to the previous API call.' — no variable parts.
Emulator: right error code, but Message 'NextToken does not name a position in this listing'.
Why: the message is the only part of this refusal a caller can read, and AWS's wording tells the caller where a valid token comes from; the emulator's wording is text no AWS SDK or log-matcher will ever see in production.
Fix: the ListServiceQuotas pagination-token validation path (the shared pagination-token resolver the listings use) must emit the service's fixed sentence for this error class.
Traces: service-quotas.invalid-pagination-token-rejected, service-quotas.list-service-quotas-invalid-pagination-token-rejected.

Cause 3 of 9 — RequestServiceQuotaIncrease: the too-low DesiredValue refusal states the wrong rule and the wrong wording.
AWS: HTTP 400/IllegalArgumentException. Recorded behavior: AWS compares the refused DesiredValue against the quota's AWS DEFAULT value first and, if it falls below it, answers 'You must provide a quota value greater than the default quota value of ' + the quota's default value rendered as a decimal (the fixed text is everything up to and including 'of '; the only variable part is the quota's default); if the value is at or above the default but not above the applied value, the message is the fixed sentence 'You must provide a quota value greater than the current quota value'. (Smithy: RequestServiceQuotaIncrease.DesiredValue required, QuotaValue double @range 0..10000000000.)
Emulator: right error code, but Message "DesiredValue <n> is not an increase on the current value <m> of QuotaCode '<QuotaCode>'" (observed "DesiredValue 4 is not an increase on the current value 5 of QuotaCode 'L-F678F1CE'"; 4/5/L-F678F1CE are this run's values). It names the CURRENT value and the QuotaCode, neither of which is in AWS's message, and never distinguishes the below-default bound from the below-current bound.
Why: the caller is told a different retry rule from the one the service enforces and is handed a number formatted differently from AWS's decimal, so automatic retry-with-a-higher-value logic computes the wrong next value.
Fix: the RequestServiceQuotaIncrease handler's value-validation branch — order the two bounds (default first, then applied) and emit each bound's fixed sentence, filling only the default value where AWS does.
Traces: service-quotas.non-increasing-desired-value-rejected, service-quotas.request-quota-increase-not-greater-than-current-rejected.

Cause 4 of 9 — RequestServiceQuotaIncrease: the non-adjustable-quota refusal message is emulator prose.
AWS: HTTP 400/IllegalArgumentException with the FIXED Message 'The request failed because the specified quota is not adjustable.' — no request values are interpolated. (Smithy: the Quota/ServiceQuota representation carries QuotaAdjustable (boolean, default false); RequestServiceQuotaIncrease declares IllegalArgumentException.)
Emulator: right error code, but Message "QuotaCode '<QuotaCode>' of ServiceCode '<ServiceCode>' is not adjustable" (observed "QuotaCode 'L-B2AA0F47' of ServiceCode 'lambda' is not adjustable").
Why: same message-contract break; a client matching on AWS's sentence to decide 'this quota can never be raised, open a support case instead' does not match.
Fix: the RequestServiceQuotaIncrease handler's Adjustable=false guard — use the service's fixed sentence from the crate's single wording source.
Traces: service-quotas.non-adjustable-quota-increase-rejected.

Cause 5 of 9 — RequestServiceQuotaIncrease: the duplicate-open-request refusal message is emulator prose and leaks a generated request id.
AWS: HTTP 400/ResourceAlreadyExistsException with the FIXED Message 'Only one open service quota increase request is allowed per quota.' — entirely fixed text; no request id, QuotaCode or ServiceCode appears in it. (Smithy: RequestServiceQuotaIncrease declares ResourceAlreadyExistsException.)
Emulator: right error code, but Message "request <generated request id> for QuotaCode '<QuotaCode>' is still awaiting a decision" (observed "request 42212a80bcb3f9503af211c4806e74d3 for QuotaCode 'L-1216C47A' is still awaiting a decision"; the hex id is a per-run value and no part of any template).
Why: besides the wording break, the emulator's message content varies on every run (it carries a freshly generated id), so no client assertion or log signature over this refusal is stable.
Fix: the RequestServiceQuotaIncrease handler's one-open-request-per-quota guard — emit the fixed sentence and stop interpolating the stored request's id.
Traces: service-quotas.duplicate-pending-quota-increase-request-rejected.

Cause 6 of 9 — GetRequestedServiceQuotaChange: the Smithy constraint-violation message omits AWS's 'N validation error detected: ' prefix.
AWS: HTTP 400/ValidationException for a RequestId that fails its pattern. Recorded behavior: the message is the standard AWS constraint-violation envelope '1 validation error detected: Value at 'requestId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}'. The fixed parts are the count-prefixed 'N validation error(s) detected: ' envelope and 'failed to satisfy constraint: Member must satisfy regular expression pattern: '; the variable parts are the violation count, the member's wire name ('requestId') and the pattern taken from the model (Smithy: GetRequestedServiceQuotaChangeRequest.RequestId -> ReportId/RequestId string @length 1..128 @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$).
Emulator: right error code and right clause, but the message starts at 'Value at 'requestId' failed to satisfy constraint: ...' — the '1 validation error detected: ' prefix is missing, so the count envelope that AWS always emits (and which is what makes the multi-violation form possible) is absent.
Why: every AWS SDK/tooling that parses or asserts on the constraint envelope (including anything that counts the reported violations) sees a different string; the single-violation and multi-violation forms become indistinguishable.
Fix: the crate's shared Smithy constraint-validation error builder — wrap the per-member clauses in the 'N validation error(s) detected: ' envelope (joining multiple clauses as AWS does) rather than emitting a bare clause.
Traces: service-quotas.get-requested-service-quota-change-invalid-request-id-rejected.

Cause 7 of 9 — StartAutoManagement: an OptInLevel outside the modeled enum is refused with the wrong error class.
AWS: HTTP 400/ValidationException with Message '1 validation error detected: Value at 'optInLevel' failed to satisfy constraint: Member must satisfy enum value set: [ACCOUNT]' — the fixed parts are the 'N validation error detected: ' envelope and 'failed to satisfy constraint: Member must satisfy enum value set: '; the variable parts are the member's wire name ('optInLevel') and the enum's value set, both from the model (Smithy: StartAutoManagementRequest.OptInLevel required, target OptInLevel enum with values [ACCOUNT]). An unmodeled enum value is a client-side constraint violation, not a service-semantics error.
Emulator: answers IllegalArgumentException instead of ValidationException for OptInLevel='REGION'.
Why: a client that distinguishes 'my request was malformed' (ValidationException, never retry, fix the input) from 'the service refused this argument' (IllegalArgumentException) is routed down the wrong branch, and the enum's valid value set is never reported back to the caller.
Fix: StartAutoManagement input validation — enum-membership failures must be raised by the crate's Smithy constraint validator (ValidationException, with cause 6's envelope), before the handler's own argument checks.
Traces: service-quotas.start-auto-management-invalid-opt-in-level-rejected.

Cause 8 of 9 — StartAutoManagement: an ExclusionList entry naming a nonexistent quota code is refused with the wrong error class and status.
AWS: HTTP 400/IllegalArgumentException with Message "Service code '<exclusion-map key>' with limit code '<exclusion-list entry>' does not exist" — the fixed text is "Service code '" / "' with limit code '" / "' does not exist", and the two variable parts are the map key and the offending list entry taken from the request. (Smithy: StartAutoManagement declares IllegalArgumentException [httpError 400] and NoSuchResourceException [httpError 404]; ExclusionList is a map of ExcludedService -> ExcludedQuotaList, both @pattern ^[A-Za-z0-9-_ /]{1,128}$, so a bogus-but-well-formed quota code passes syntax and must be refused on lookup.)
Emulator: answers NoSuchResourceException (modeled httpError 404) for ExclusionList={'vpc': ['L-00000000']}, i.e. a different error code and a different status class from the recorded refusal.
Why: a client gets a 404-class resource-not-found where AWS reports a 400-class bad argument, so retry/backoff and error-routing logic built on the status class behaves differently; the message also never names which exclusion entry was rejected, which is the only actionable part of this refusal.
Fix: the StartAutoManagement handler's ExclusionList resolution — classify an unresolvable service/quota pair in the exclusion map as IllegalArgumentException and build the message naming the offending service code and limit code (the rejection must remain side-effect-free: the check confirms the configuration is unchanged).
Traces: service-quotas.auto-management-exclusion-list-invalid-quota-rejected.

Cause 9 of 9 — GetQuotaUtilizationReport: QuotaUtilizationInfo records omit the modeled Utilization member.
AWS: each element of GetQuotaUtilizationReportResponse.Quotas is a QuotaUtilizationInfo carrying QuotaCode, ServiceCode, QuotaName, Namespace, Utilization (UtilizationPct, double), DefaultValue, AppliedValue, ServiceName and Adjustable (Smithy: QuotaUtilizationInfo as above; QuotaUtilizationInfoList @length 0..1000). Utilization is the single member the whole report exists to deliver.
Emulator: a COMPLETED report's records are serialized without Utilization (and without Namespace) — observed record: {'QuotaCode': 'AnalyzersPerRegion', 'ServiceCode': 'access-analyzer', 'QuotaName': 'Analyzers per Region', 'DefaultValue': 1.0, 'AppliedValue': 1.0, 'ServiceName': 'Access Analyzer', 'Adjustable': False}. The same omission appears in both the unpaginated read and the MaxResults-paged walk of the same report.
Why: a client reading the utilization report to find quotas nearing exhaustion gets no utilization figure at all; typed SDKs surface an absent/None field where AWS always returns a double, so any threshold comparison either throws or silently treats every quota as 0% used.
Fix: the GetQuotaUtilizationReport serializer/state — populate Utilization (and Namespace where the quota has a metric namespace) on every QuotaUtilizationInfo it emits, on both the paginated and unpaginated response paths.
Traces: service-quotas.quota-utilization-report-pagination-consistency.

## Evidence
Suite: simparity-behavioral-aws-v1, 28/40 passed (70%), 12 mismatches, all in service service-quotas; none marked ALREADY FILED and none OWNED by an open issue. Replay details, copied from the report:

Cause 1:
- service-quotas.get-service-quota-unknown-quota-code-not-found: AssertionError: expected message 'The request failed because the specified quota and service do not exist.', got "no quota with ServiceCode 'vpc' and QuotaCode 'L-00000000' exists" (error code NoSuchResourceException matched).
- service-quotas.unknown-service-code-returns-not-found: AssertionError: expected message 'The request failed because the specified quota and service do not exist.', got "no quota with ServiceCode 'sim2real-nonexistent-service-zzz' and QuotaCode 'L-00000000' exists".

Cause 2:
- service-quotas.invalid-pagination-token-rejected (ListServiceQuotas ServiceCode=ec2, NextToken='A'*40+'=='): expected message 'You specified an invalid value for nextToken. You must get the value from the response to the previous API call.', got 'NextToken does not name a position in this listing' (code InvalidPaginationTokenException matched).
- service-quotas.list-service-quotas-invalid-pagination-token-rejected (ServiceCode=vpc, NextToken='QUJDREVGRw==', MaxResults=5): identical expected/actual pair.

Cause 3:
- service-quotas.non-increasing-desired-value-rejected and service-quotas.request-quota-increase-not-greater-than-current-rejected: expected message 'You must provide a quota value greater than the default quota value of 5.0', got "DesiredValue 4 is not an increase on the current value 5 of QuotaCode 'L-F678F1CE'" (code IllegalArgumentException matched). The 5.0 is this run's default for vpc/L-F678F1CE; the check derives it from GetAWSDefaultServiceQuota, and its sibling branch expects the fixed 'You must provide a quota value greater than the current quota value' when the refused value is at or above the default.

Cause 4:
- service-quotas.non-adjustable-quota-increase-rejected: expected message 'The request failed because the specified quota is not adjustable.', got "QuotaCode 'L-B2AA0F47' of ServiceCode 'lambda' is not adjustable".

Cause 5:
- service-quotas.duplicate-pending-quota-increase-request-rejected: expected message 'Only one open service quota increase request is allowed per quota.', got "request 42212a80bcb3f9503af211c4806e74d3 for QuotaCode 'L-1216C47A' is still awaiting a decision" (code ResourceAlreadyExistsException matched).

Cause 6:
- service-quotas.get-requested-service-quota-change-invalid-request-id-rejected (RequestId='!!!not-a-valid-request-id!!!'): expected message "1 validation error detected: Value at 'requestId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}", got "Value at 'requestId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-zA-Z][a-zA-Z0-9-]{1,128}" — identical except for the missing count envelope (code ValidationException matched).

Cause 7:
- service-quotas.start-auto-management-invalid-opt-in-level-rejected (OptInLevel='REGION', OptInType='NotifyOnly'): AssertionError: expected ValidationException for an OptInLevel outside the supported enum, got 'IllegalArgumentException'. The check's expected message is "1 validation error detected: Value at 'optInLevel' failed to satisfy constraint: Member must satisfy enum value set: [ACCOUNT]".

Cause 8:
- service-quotas.auto-management-exclusion-list-invalid-quota-rejected (StartAutoManagement OptInLevel='ACCOUNT', OptInType='NotifyOnly', ExclusionList={'vpc': ['L-00000000']}): AssertionError: NoSuchResourceException where IllegalArgumentException was expected; the check's expected message is "Service code 'vpc' with limit code 'L-00000000' does not exist".

Cause 9:
- service-quotas.quota-utilization-report-pagination-consistency: AssertionError: quota utilization record missing 'Utilization': {'QuotaCode': 'AnalyzersPerRegion', 'ServiceCode': 'access-analyzer', 'QuotaName': 'Analyzers per Region', 'DefaultValue': 1.0, 'AppliedValue': 1.0, 'ServiceName': 'Access Analyzer', 'Adjustable': False}. The report reached COMPLETED and the unpaginated read returned no NextToken, so the failure is purely the absent member, not pagination.

Smithy evidence (services/service-com-amazonaws-servicequotas-servicequotasv20190624/smithy-model/com-amazonaws-servicequotas-servicequotasv20190624.json, awsJson1_1, sdkId 'Service Quotas'):
- IllegalArgumentException errorKind client httpError 400; NoSuchResourceException errorKind client httpError 404; InvalidPaginationTokenException errorKind client httpError 400 — each a single Message member (ExceptionMessage).
- GetServiceQuota errors [IllegalArgumentException, NoSuchResourceException]; ListServiceQuotas errors [IllegalArgumentException, InvalidPaginationTokenException, NoSuchResourceException]; RequestServiceQuotaIncrease errors [DependencyAccessDeniedException, IllegalArgumentException, InvalidResourceStateException, NoSuchResourceException, QuotaExceededException, ResourceAlreadyExistsException]; StartAutoManagement errors [IllegalArgumentException, NoSuchResourceException].
- StartAutoManagementRequest.OptInLevel required -> OptInLevel enum values [ACCOUNT]; OptInType enum values [NotifyOnly, NotifyAndAdjust]; ExclusionList map ExcludedService(@pattern ^[A-Za-z0-9-_ /]{1,128}$) -> list of ExcludedLimit(@pattern ^[A-Za-z0-9-_ /]{1,128}$).
- GetQuotaUtilizationReportRequest.ReportId required @length 1..128 @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$; NextToken @length 0..2048 @pattern ^[a-zA-Z0-9/+]*={0,2}$; MaxResults @range 1..1000. QuotaUtilizationInfo members: QuotaCode, ServiceCode, QuotaName, Namespace, Utilization (UtilizationPct, double), DefaultValue, AppliedValue, ServiceName, Adjustable (default false).
- RequestId pattern for GetRequestedServiceQuotaChange: @length 1..128 @pattern ^[0-9a-zA-Z][a-zA-Z0-9-]{1,128}$ — exactly the pattern echoed in the expected validation message.

Fixed text vs variable parts: in causes 1, 2, 4 and 5 the AWS message is entirely fixed for its error class, so every ServiceCode/QuotaCode/request id the emulator interpolates is an addition, not a substitution. In cause 3 the only variable part is the quota's AWS default value, rendered as a decimal. In causes 6 and 7 the variable parts are the violation count, the member's wire name and the model-derived pattern/enum value set. In cause 8 the variable parts are the exclusion map key and the offending limit code from the request. Concrete ids, hex request ids and numeric values quoted above are this run's values and are not part of any template.

## Steps to Reproduce
1. git_clone the repo, build and boot the emulator from the checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite (service service-quotas); 12 checks mismatch.
3. Cause 1: GetServiceQuota(ServiceCode='vpc', QuotaCode='L-00000000') and GetServiceQuota(ServiceCode='sim2real-nonexistent-service-zzz', QuotaCode='L-00000000') — the error code is NoSuchResourceException as expected; diff $.Message against 'The request failed because the specified quota and service do not exist.'
4. Cause 2: ListServiceQuotas(ServiceCode='ec2', NextToken='A'*40+'==') and ListServiceQuotas(ServiceCode='vpc', NextToken='QUJDREVGRw==', MaxResults=5) — InvalidPaginationTokenException with the emulator's own sentence; diff against 'You specified an invalid value for nextToken. You must get the value from the response to the previous API call.'
5. Cause 3: read the vpc/L-F678F1CE default via GetAWSDefaultServiceQuota, then RequestServiceQuotaIncrease(ServiceCode='vpc', QuotaCode='L-F678F1CE', DesiredValue=current-1) — diff $.Message against 'You must provide a quota value greater than the default quota value of <default>'.
6. Cause 4: ListServiceQuotas over lambda/vpc/ec2/... to find a quota with Adjustable=false, then RequestServiceQuotaIncrease on it with DesiredValue=Value+1 — diff $.Message against 'The request failed because the specified quota is not adjustable.'
7. Cause 5: leave one open (PENDING/CASE_OPENED) increase request on an adjustable ec2 vCPU quota, then RequestServiceQuotaIncrease on the same quota with a different DesiredValue — diff $.Message against 'Only one open service quota increase request is allowed per quota.'
8. Cause 6: GetRequestedServiceQuotaChange(RequestId='!!!not-a-valid-request-id!!!') — ValidationException whose message is missing the leading '1 validation error detected: '.
9. Cause 7: StartAutoManagement(OptInLevel='REGION', OptInType='NotifyOnly') — observe IllegalArgumentException where ValidationException (with the enum-value-set envelope) is required.
10. Cause 8: StartAutoManagement(OptInLevel='ACCOUNT', OptInType='NotifyOnly', ExclusionList={'vpc': ['L-00000000']}) — observe NoSuchResourceException (404 class) where IllegalArgumentException (400 class) with "Service code 'vpc' with limit code 'L-00000000' does not exist" is required; GetAutoManagementConfiguration confirms the rejected call left the configuration untouched.
11. Cause 9: StartQuotaUtilizationReport, poll GetQuotaUtilizationReport(ReportId=...) until Status=COMPLETED, then inspect $.Quotas[*] both unpaginated and with MaxResults=5 — no record carries Utilization (or Namespace).
