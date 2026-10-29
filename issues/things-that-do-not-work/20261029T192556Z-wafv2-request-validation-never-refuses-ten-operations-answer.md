Issue-Source: trace-sweep-divergence
Issue-Trace: wafv2-cloudfront-scope-outside-us-east-1-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wafv2-cloudfront-scope-outside-us-east-1-rejected wafv2-error-paths-and-validation wafv2-rate-based-rule-analytics-and-sampling wafv2-settlement-records-workflow wafv2-webacl-logging-configuration-setup wafv2-rulegroup-lifecycle-with-permission-policy-errors wafv2-webacl-resource-association-workflow
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2|ListWebACLs|4xx->2xx|$.Field
Issue-Key: wafv2|ListWebACLs|4xx->2xx|$.Parameter
Issue-Key: wafv2|ListWebACLs|4xx->2xx|$.Reason
Issue-Key: wafv2|ListWebACLs|4xx->2xx|$.__type
Issue-Key: wafv2|ListWebACLs|4xx->2xx|$.message
Issue-Key: wafv2|ListWebACLs|4xx->2xx|status
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|$.Field
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|$.Parameter
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|$.Reason
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|$.__type
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|$.message
Issue-Key: wafv2|ListAPIKeys|4xx->2xx|status
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|$.Field
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|$.Parameter
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|$.Reason
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|$.__type
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|$.message
Issue-Key: wafv2|CreateAPIKey|4xx->2xx|status
Issue-Key: wafv2|GetSampledRequests|4xx->2xx|$.__type
Issue-Key: wafv2|GetSampledRequests|4xx->2xx|$.message
Issue-Key: wafv2|GetSampledRequests|4xx->2xx|status
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|$.Field
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|$.Parameter
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|$.Reason
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|$.__type
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|$.message
Issue-Key: wafv2|ListSettlementRecords|4xx->2xx|status
Issue-Key: wafv2|PutLoggingConfiguration|4xx->2xx|$.Parameter
Issue-Key: wafv2|PutLoggingConfiguration|4xx->2xx|$.Reason
Issue-Key: wafv2|PutLoggingConfiguration|4xx->2xx|$.__type
Issue-Key: wafv2|PutLoggingConfiguration|4xx->2xx|$.message
Issue-Key: wafv2|PutLoggingConfiguration|4xx->2xx|status
Issue-Key: wafv2|DeletePermissionPolicy|4xx->2xx|$
Issue-Key: wafv2|DeletePermissionPolicy|4xx->2xx|status
Issue-Key: wafv2|AssociateWebACL|4xx->2xx|$
Issue-Key: wafv2|AssociateWebACL|4xx->2xx|status
Issue-Key: wafv2|GetWebACLForResource|4xx->2xx|$.Message
Issue-Key: wafv2|GetWebACLForResource|4xx->2xx|$.__type
Issue-Key: wafv2|GetWebACLForResource|4xx->2xx|status
Issue-Key: wafv2|DisassociateWebACL|4xx->2xx|$
Issue-Key: wafv2|DisassociateWebACL|4xx->2xx|status
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# wafv2 request validation never refuses: ten operations answer 200 where WAF returns a modeled 400

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWebACLs

## Description
Ten distinct wafv2 operations accept a request that AWS WAF rejects, and answer 200 with a success body (often an empty one) instead of raising the modeled client error. This is ONE issue with TEN causes; every cause below must be closed before the fix is submitted.

Cause 1 of 10: ListWebACLs with Scope=CLOUDFRONT from a region other than us-east-1.
AWS: Smithy: ListWebACLs.Scope is a required `Scope` enum and the operation models WAFInvalidParameterException (client, httpError 400). CLOUDFRONT scope is only servable from us-east-1, so WAF refuses with HTTP 400, __type `WAFInvalidParameterException`, and the detail members WAFInvalidParameterException carries (Field: `ParameterExceptionField`, Parameter, Reason). Fixed text of the message is `Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: ` followed by the rejected scope value from the request; Reason is the fixed sentence `The scope is not valid.`, Field is the fixed enum token `SCOPE_VALUE`, and Parameter is the request's Scope value.
Emulator: HTTP 200 with a normal ListWebACLs body; __type, Field, Parameter, Reason and message all absent.
Why: a client that relies on the region/scope guard sees a CLOUDFRONT listing succeed from a regional endpoint, so cross-region scope bugs in its own code never surface and it builds regional state under a global scope.
Fix: the scope precondition in the ListWebACLs handler's validation, shared with every other scope-taking operation.
Traces: wafv2-cloudfront-scope-outside-us-east-1-rejected#task#2.

Cause 2 of 10: ListAPIKeys with Scope=CLOUDFRONT outside us-east-1.
AWS: Smithy: ListAPIKeys.Scope required, errors include WAFInvalidParameterException (400). Same refusal as Cause 1 — HTTP 400, __type `WAFInvalidParameterException`, Field `SCOPE_VALUE`, Parameter = the request's Scope, Reason `The scope is not valid.`, message `Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: <scope>`.
Emulator: HTTP 200 with an API-key listing body and no error members.
Why: same as Cause 1 — the guard is silently absent on the API-key surface too.
Fix: the scope precondition in the ListAPIKeys handler's validation.
Traces: wafv2-cloudfront-scope-outside-us-east-1-rejected#task#3.

Cause 3 of 10: CreateAPIKey with a token domain that is not a valid domain name.
AWS: Smithy: CreateAPIKey.TokenDomains is required, target APIKeyTokenDomains whose member TokenDomain is @length 1..253 @pattern ^[\w\.\-/]+$; the operation models WAFInvalidParameterException (400). A syntactically-shaped but non-registrable domain (a bare public suffix) is refused with HTTP 400, __type `WAFInvalidParameterException`, Field `TOKEN_DOMAIN`, Parameter = the offending domain string taken from the request, Reason = the fixed sentence `The parameter contains formatting that is not valid.`, message = `Error reason: The parameter contains formatting that is not valid., field: TOKEN_DOMAIN, parameter: ` + that domain.
Emulator: HTTP 200 and an issued APIKey; no error members at all.
Why: a client gets an API key bound to a domain that will never validate at the CAPTCHA/JS integration, and the failure moves from create-time to a silent runtime mismatch.
Fix: TokenDomains validation in the CreateAPIKey handler.
Traces: wafv2-error-paths-and-validation#task#6.

Cause 4 of 10: GetSampledRequests with a TimeWindow that is not strictly inside the last three hours.
AWS: Smithy: GetSampledRequests.TimeWindow required, errors include WAFInvalidParameterException (400). WAF only retains samples for a three-hour window and refuses an out-of-range window with HTTP 400, __type `WAFInvalidParameterException`, and the fixed message `Start time and end time must be strictly after current time - 3 hours` (no variable parts).
Emulator: HTTP 200 with a sample response; no __type and no message.
Why: the client believes it queried a window that WAF cannot serve and reads an empty/!fabricated sample set as "no matching traffic" rather than as a rejected request.
Fix: TimeWindow validation in the GetSampledRequests handler.
Traces: wafv2-rate-based-rule-analytics-and-sampling#task#8.

Cause 5 of 10: ListSettlementRecords with the same filter name supplied twice in Filters.
AWS: Smithy: ListSettlementRecords.Filters target MonetizationFilterList @length 1..20; errors include WAFInvalidParameterException (400). A repeated filter name is refused with HTTP 400, __type `WAFInvalidParameterException`, Field `SCOPE_VALUE`, Parameter = the duplicated filter name from the request, Reason = `Duplicate filter name '<name>' for ListSettlementRecords.` and message = `Error reason: Duplicate filter name '<name>' for ListSettlementRecords., field: SCOPE_VALUE, parameter: <name>` — fixed text `Duplicate filter name '` / `' for ListSettlementRecords.`, variable part the filter name taken from the request.
Emulator: HTTP 200 with a settlement-records body and no error members.
Why: the duplicate filter is silently dropped or double-applied, so the returned settlement set does not correspond to any filter the client asked for and billing reconciliation reads wrong numbers.
Fix: Filters validation in the ListSettlementRecords handler.
Traces: wafv2-settlement-records-workflow#task#2.

Cause 6 of 10: PutLoggingConfiguration with a malformed log-destination ARN.
AWS: Smithy: PutLoggingConfiguration.LoggingConfiguration required; errors include WAFInvalidParameterException (400). A destination ARN missing its account-id segment is malformed, and WAF refuses with HTTP 400, __type `WAFInvalidParameterException`, Parameter = the malformed ARN from the request, Reason = the fixed sentence `The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes.`, message = `Error reason: ` + that sentence + `, field: null, parameter: ` + the ARN. Note the recorded Field is the literal token `null` for this refusal, not an omitted member.
Emulator: HTTP 200 and the logging configuration is accepted; no error members.
Why: logging looks configured but no destination exists, so WAF log delivery silently never happens and the client has no signal that its ARN was wrong. (The open issue 20261026T023047Z-wafv2-logging-surface-putloggingconfiguration-accepts-a-destination-whose covers an unprefixed log-group name; it does not carry this malformed-ARN replay key.)
Fix: LoggingConfiguration.LogDestinationConfigs ARN validation in the PutLoggingConfiguration handler.
Traces: wafv2-webacl-logging-configuration-setup#task#5.

Cause 7 of 10: DeletePermissionPolicy for a rule group that has no permission policy.
AWS: Smithy: DeletePermissionPolicy.ResourceArn required; modeled errors are WAFInternalErrorException, WAFInvalidParameterException and WAFNonexistentItemException (client, 400). Deleting a policy that does not exist is refused with HTTP 400, __type `WAFNonexistentItemException` and Message = the fixed sentence WAF uses for that class, `AWS WAF couldn't perform the operation because your resource doesn't exist.` (as recorded, with the typographic apostrophe).
Emulator: HTTP 200 with an empty body `{}`.
Why: the call is idempotent-by-accident, so a client cannot distinguish "policy removed" from "there was never a policy" and cleanup/teardown assertions pass against a resource that was never in the expected state.
Fix: the existence check in the DeletePermissionPolicy handler against the rule group's stored permission policy.
Traces: wafv2-rulegroup-lifecycle-with-permission-policy-errors#task#7.

Cause 8 of 10: AssociateWebACL against a protected resource WAF cannot retrieve.
AWS: Smithy: AssociateWebACL takes required WebACLArn and ResourceArn (both ResourceArn, @length 20..2048) and models WAFUnavailableEntityException (client, 400) — documented as "WAF couldn't retrieve a resource that you specified for this operation". The recorded refusal is HTTP 400, __type `WAFUnavailableEntityException`, Message = the fixed sentence `AWS WAF couldn't retrieve the resource that you requested. Retry your request.`
Emulator: HTTP 200 with an empty body `{}` — the association is reported as made.
Why: the client records a protection that does not exist; every later GetWebACLForResource/enforcement expectation is built on an association WAF never created.
Fix: the protected-resource resolution step in the AssociateWebACL handler, before the association is written to state.
Traces: wafv2-webacl-resource-association-workflow#task#3.

Cause 9 of 10: GetWebACLForResource for a resource with no web ACL associated.
AWS: Smithy: GetWebACLForResource.ResourceArn required; models WAFNonexistentItemException (400). WAF refuses with HTTP 400, __type `WAFNonexistentItemException`, Message `AWS WAF couldn't perform the operation because your resource doesn't exist.` (fixed text, no variable parts).
Emulator: HTTP 200, no __type and no Message.
Why: "not associated" is indistinguishable from "associated with nothing", so a client's association check silently succeeds and it skips the association it needed to make.
Fix: the association lookup in the GetWebACLForResource handler.
Traces: wafv2-webacl-resource-association-workflow#task#4.

Cause 10 of 10: DisassociateWebACL for a resource with no association.
AWS: Smithy: DisassociateWebACL.ResourceArn required; models WAFNonexistentItemException (400). Refusal is HTTP 400, __type `WAFNonexistentItemException`, Message `AWS WAF couldn't perform the operation because your resource doesn't exist.`
Emulator: HTTP 200 with an empty body `{}`.
Why: same as Cause 7/9 — teardown cannot tell a real disassociation from a no-op, so an association that was never created is reported as removed.
Fix: the association existence check in the DisassociateWebACL handler.
Traces: wafv2-webacl-resource-association-workflow#task#5.

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, /__admin/health 200). All ten clusters are verdict=mismatch with expected_status 400 and actual_status 200.

1. ListWebACLs wafv2-cloudfront-scope-outside-us-east-1-rejected#task#2 — "status: expected 400, got 200; $.__type: missing (expected 'WAFInvalidParameterException', got None); $.Field: missing (expected 'SCOPE_VALUE'); $.Parameter: missing (expected 'CLOUDFRONT'); $.Reason: missing (expected 'The scope is not valid.'); $.message: missing (expected 'Error reason: The scope is not valid., field: SCOPE_VALUE, parameter: CLOUDFRONT')".
2. ListAPIKeys wafv2-cloudfront-scope-outside-us-east-1-rejected#task#3 — identical detail set.
3. CreateAPIKey wafv2-error-paths-and-validation#task#6 — expected Field 'TOKEN_DOMAIN', Parameter 'co.uk', Reason 'The parameter contains formatting that is not valid.', message 'Error reason: The parameter contains formatting that is not valid., field: TOKEN_DOMAIN, parameter: co.uk'; got 200 and none of them.
4. GetSampledRequests wafv2-rate-based-rule-analytics-and-sampling#task#8 — expected 400 WAFInvalidParameterException, message 'Start time and end time must be strictly after current time - 3 hours'; got 200.
5. ListSettlementRecords wafv2-settlement-records-workflow#task#2 — expected Reason "Duplicate filter name 'SettlementStatus' for ListSettlementRecords.", Field 'SCOPE_VALUE', Parameter 'SettlementStatus'; got 200.
6. PutLoggingConfiguration wafv2-webacl-logging-configuration-setup#task#5 — expected 400 with Reason "The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes." and message '... , field: null, parameter: arn:aws:firehose:us-west-2::deliverystream/aws-waf-logs-...'; got 200.
7. DeletePermissionPolicy wafv2-rulegroup-lifecycle-with-permission-policy-errors#task#7 — "$: empty_body (expected {'__type': 'WAFNonexistentItemException', 'Message': 'AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.'}, got {})".
8. AssociateWebACL wafv2-webacl-resource-association-workflow#task#3 — "$: empty_body (expected {'__type': 'WAFUnavailableEntityException', 'Message': 'AWS WAF couldn\u2019t retrieve the resource that you requested. Retry your request.'}, got {})".
9. GetWebACLForResource wafv2-webacl-resource-association-workflow#task#4 — expected 400 WAFNonexistentItemException + Message; got 200 with neither.
10. DisassociateWebACL wafv2-webacl-resource-association-workflow#task#5 — "$: empty_body (expected {'__type': 'WAFNonexistentItemException', 'Message': 'AWS WAF couldn\u2019t perform the operation because your resource doesn\u2019t exist.'}, got {})".

The ARNs, scope value, domain and filter name above are this corpus's request values; the requirement is the fixed text plus the value echoed from the request.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1/2: with a non-us-east-1 regional endpoint, call ListWebACLs and ListAPIKeys with Scope=CLOUDFRONT -> expect HTTP 400 WAFInvalidParameterException (Field SCOPE_VALUE); emulator returns 200.
3. Cause 3: CreateAPIKey with Scope=REGIONAL and TokenDomains containing a bare public suffix -> expect 400 WAFInvalidParameterException (Field TOKEN_DOMAIN); emulator issues a key with 200.
4. Cause 4: GetSampledRequests with TimeWindow start/end older than now-3h -> expect 400; emulator returns 200.
5. Cause 5: ListSettlementRecords with two Filters entries carrying the same filter name -> expect 400; emulator returns 200.
6. Cause 6: PutLoggingConfiguration with a LogDestinationConfigs ARN whose account-id segment is empty -> expect 400; emulator accepts with 200.
7. Cause 7: CreateRuleGroup, then DeletePermissionPolicy on its ARN without ever calling PutPermissionPolicy -> expect 400 WAFNonexistentItemException; emulator returns 200 {}.
8. Causes 8-10: AssociateWebACL to a protected-resource ARN that cannot be retrieved, then GetWebACLForResource and DisassociateWebACL on an unassociated resource ARN -> expect 400 WAFUnavailableEntityException / WAFNonexistentItemException / WAFNonexistentItemException; emulator returns 200 for all three.
