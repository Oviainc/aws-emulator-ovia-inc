Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields
Issue-Key: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields|_scan#27|Timeout|page = logs.filter_log_events(**kwargs)
Issue-Commit: ff813631b44d590ec7aaf15ff09fde7e2cd62f49

# apigateway__logs__wafv2: the WAF logging-configuration check still reaches no verdict at the 180 s cap — a BLOCK record with redacted fields is never delivered to the CloudWatch Logs destination

> _Note: this issue was identified against the code base at commit `ff813631b44d590ec7aaf15ff09fde7e2cd62f49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
Cause 1 of 1: a REGIONAL web ACL bound to a CloudWatch Logs destination by PutLoggingConfiguration and associated to a deployed API Gateway REST stage never delivers a log record for a request its rules BLOCKed, so neither the record's shape nor the redaction of the configured RedactedFields members is ever observable. The check makes no progress at all inside the harness cap and ends `Check timed out after 180.0s`.

AWS: recorded behavior only for the delivery semantics (the Smithy model for wafv2 PutLoggingConfiguration models the LoggingConfiguration shape — ResourceArn, LogDestinationConfigs, RedactedFields — and the refusal classes WAFInvalidParameterException for a CloudWatch Logs destination whose log-group name lacks the `aws-waf-logs-` prefix and WAFNonexistentItemException for an unknown web ACL, but says nothing about delivery). The recording shows: PutLoggingConfiguration binds the web ACL to the `aws-waf-logs-`-prefixed log group and GetLoggingConfiguration round-trips both LogDestinationConfigs and RedactedFields; a request matching a Block rule at the associated stage is answered 403 at the edge; and a JSON WAF log record for that request is then delivered to the destination log group, carrying `action: "BLOCK"`, `terminatingRuleId` equal to the name of the rule that terminated, an `httpRequest.uri` naming the requested path, each header named in RedactedFields replaced by the literal string `REDACTED`, and every header NOT named in RedactedFields carried verbatim. Delivery is asynchronous and is observed by polling, never by a deadline (the check budgets 420 s for AssociateWebACL's eventually-consistent WAFUnavailableEntityException window, 360 s for rule propagation to the edge and 480 s for delivery).

Emulator: the check reaches no verdict — `Check timed out after 180.0s`, with no assertion reached. Nothing in the chain converges: no BLOCK record arrives in the destination log group, so the record's action/terminatingRuleId/httpRequest and the per-header redaction are all unobservable.

Why: a client that relies on WAF logging gets no records at all. Audit, incident-response and compliance pipelines keyed on WAF log delivery (CloudWatch Logs subscription filters, metric filters, Insights queries over `aws-waf-logs-*`) are dead, and a caller cannot verify the one property PutLoggingConfiguration's RedactedFields exists to guarantee — that a sensitive request header (here `authorization`) is written as `REDACTED` and not in clear, while an unconfigured header is left untouched.

Fix: the wafv2 logging-delivery path together with the WAF evaluation hook on the protected API Gateway REST stage — a terminating BLOCK verdict at the edge must emit the WAF JSON log record to each destination in the web ACL's LoggingConfiguration, with the RedactedFields members redacted and all other headers verbatim, instead of leaving the delivery poll unsatisfiable.

Note: four open issues describe a similar hang for this family but none carries this check's replay key, so none covers it — 20261104T052719Z-apigateway-logs-wafv2-the-wafv2-logging-configuration-check.md (same check, filed when the cap was 60 s and bound to a different poll site), 20261023T210711Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks.md, 20261025T081755Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks-2e88ae.md and 20261026T010229Z-apigateway-logs-wafv2-waf-logging-is-now-accepted.md; 20261101T125407Z-wafv2-logging-an-accepted-s3-logging-destination-never.md is the S3-destination variant.

Traces: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields.

## Evidence
Failing check apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields:

```
[mismatch] Error: Check timed out after 180.0s
```

No assertion in the check was reached, so the divergence is that the whole scenario never converges — not a single wrong value.

The check's observation windows are all polls on observable conditions and are far more generous than the harness cap, which is what the cap pre-empts:

```python
_ASSOCIATE_BUDGET_S = 420.0    # AssociateWebACL's WAFUnavailableEntityException window
_PROPAGATION_BUDGET_S = 360.0  # rule change reaching the API Gateway edge
_LOG_DELIVERY_BUDGET_S = 480.0 # poll on the WAF record ARRIVING, never a delivery deadline
```

Expected, once the stage serves its MOCK integration 200 and the web ACL (one ByteMatchStatement on UriPath CONTAINS the marker path, Action Block, DefaultAction Allow) is associated and PutLoggingConfiguration has bound the `aws-waf-logs-`-prefixed log group with `RedactedFields=[{"SingleHeader": {"Name": "authorization"}}]`:
- GET of the marker path answers 403 on successive requests;
- FilterLogEvents on the destination group yields a JSON record whose `httpRequest.uri` contains the marker path, `action == "BLOCK"`, `terminatingRuleId` == the rule's name;
- that record's `authorization` header value is the literal `REDACTED` and the request's secret value appears nowhere in the record, while the unconfigured `x-waf-keep` header carries its request value verbatim.

Actual: the check produces none of this and is cut off at the 180 s cap; no WAF log record is delivered to the configured CloudWatch Logs destination.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields.
3. Manual repro: CreateLogGroup with a name starting `aws-waf-logs-`; create a REGIONAL REST API with a MOCK GET method under a marker path and deploy it to a stage; CreateWebACL (Scope=REGIONAL, DefaultAction Allow) with one rule whose ByteMatchStatement matches UriPath CONTAINS the marker path with Action Block; AssociateWebACL to arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage>; PutLoggingConfiguration binding that web ACL to the log group's ARN (without the trailing `:*`) with RedactedFields naming the `authorization` header; GET the marker path with an `authorization` header and a second, unconfigured header; then poll FilterLogEvents on the log group. Observed: no WAF record is ever delivered and the scenario never completes. Expected: 403 at the edge and a JSON WAF record in the group with action BLOCK, the terminating rule named, `authorization` written as `REDACTED`, and the unconfigured header verbatim.
