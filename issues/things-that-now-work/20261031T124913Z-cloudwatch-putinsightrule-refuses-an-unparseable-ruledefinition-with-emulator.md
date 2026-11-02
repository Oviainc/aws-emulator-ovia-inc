Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.missing-required-parameter-family
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.missing-required-parameter-family|run#17|AssertionError|assert error == ("InvalidParameterValue", "INVALID_RULE_BODY: The RuleBody could not be parsed."), (
Issue-Commit: 0751c91e0a2d4243baa985eebcbcef2867f2b65d

# CloudWatch PutInsightRule refuses an unparseable RuleDefinition with emulator-authored prose instead of AWS's 'INVALID_RULE_BODY: The RuleBody could not be parsed.'

> _Note: this issue was identified against the code base at commit `0751c91e0a2d4243baa985eebcbcef2867f2b65d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInsightRule

## Description
1 net-new root cause in the CloudWatch Contributor Insights write path, grounded in the behavioral check cloudwatch.missing-required-parameter-family (the only assertion in that check that failed; the DeleteInsightRules/Disable/EnableInsightRules empty-array refusals, the Start/StopMetricStreams empty-Names no-ops, the GetInsightRuleReport Period refusal, the PutManagedInsightRules/ListManagedInsightRules refusals and the DeleteMetricStream idempotent 200 all matched).

Cause 1 of 1: PutInsightRule with a syntactically valid but schema-invalid RuleDefinition (the minimal JSON body "{}", which passes botocore's client-side @length min 1 check and so reaches service validation).
  AWS: the call is refused 400 InvalidParameterValueException, which the awsQuery/JSON envelope surfaces as Error.Code 'InvalidParameterValue' (Smithy: PutInsightRule errors InvalidParameterValueException [client, httpError 400, awsQueryError code InvalidParameterValue], LimitExceededException, MissingRequiredParameterException; PutInsightRule.RuleDefinition targets InsightRuleDefinition @length(min 1, max 8192) @pattern ^[\x00-\x7F]+$ and is required). The message for this error class is 'INVALID_RULE_BODY: The RuleBody could not be parsed.' — recorded behavior for this class: the whole sentence is FIXED text with NO variable part, in the Contributor Insights '<REASON_CODE>: <sentence>' form (the reason code INVALID_RULE_BODY names what was wrong with the body; neither the rule name nor the submitted definition is echoed).
  Emulator: refuses with the right code (InvalidParameterValue) but its own prose message 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1' — the reason-code prefix is absent and the sentence is emulator-authored.
  Why: clients, SDK-level error handlers and teardown/retry logic branch on the reason-code prefix to tell an unparseable rule body (INVALID_RULE_BODY) apart from a rejected rule name (RULE_NAME), a bad pagination token (INVALID_TOKEN) or an over-limit rule; with the prefix replaced by free prose, every such branch falls through to a generic 'unknown InvalidParameterValue' path, and any assertion or log scrape on AWS's documented text fails.
  Fix: PutInsightRule's request validation / refusal-message construction in the CloudWatch crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801, the PutInsightRule handler's RuleDefinition parse check) — it must emit the service's fixed INVALID_RULE_BODY sentence for an unparseable body rather than describing the schema mismatch it detected.
  Traces: behavioral check cloudwatch.missing-required-parameter-family (assertion at check line 58).

Already-filed issue naming the same operation, which does NOT carry this behavioral check's replay key and so does not cover it: open issue 20261029T203422Z-monitoring-cloudwatch-contributor-insights-error-paths-use-emulator-authored.md (wire corpus simparity-wire-aws-v1, Cause 1 of 5) describes the same emulator-authored PutInsightRule message from the wire fixture cloudwatch-contributor-insights-error-cases#task#1. If that issue is fixed, re-replaying cloudwatch.missing-required-parameter-family is what confirms this behavioral check closes with it; it is recorded here because the behavioral replay still fails on mainline and this check id is not owned by any open issue.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1), check cloudwatch.missing-required-parameter-family, verdict mismatch:

  File "check_cloudwatch_missing_required_parameter_family.py", line 58, in run
    assert error == ("InvalidParameterValue", "INVALID_RULE_BODY: The RuleBody could not be parsed.")
  AssertionError: PutInsightRule: a schema-invalid RuleDefinition gave ('InvalidParameterValue', 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1')

Expected (AWS): HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'INVALID_RULE_BODY: The RuleBody could not be parsed.' (fixed text, no variable parts).
Actual (emulator): HTTP 400, Error.Code 'InvalidParameterValue' (correct), Error.Message 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1' (emulator-authored; mentions the parameter name and the CloudWatchLogRule schema version, neither of which appears in AWS's message for this class).

Every other operation exercised by the same check matched, so the divergence is isolated to PutInsightRule's unparseable-body refusal message.

## Steps to Reproduce
1. Boot the emulator from this branch and replay the behavioral suite (simparity-behavioral-aws-v1).
2. Or reproduce directly: with a cloudwatch client pointed at the emulator, call PutInsightRule(RuleName=<any 1-128 char printable-ASCII name>, RuleDefinition="{}").
3. Catch the ClientError and read response['Error']['Code'] and response['Error']['Message'].
4. Expected: ('InvalidParameterValue', 'INVALID_RULE_BODY: The RuleBody could not be parsed.'). Observed: ('InvalidParameterValue', 'The value for parameter RuleDefinition is not valid: its Schema is not the CloudWatchLogRule schema, version 1').
