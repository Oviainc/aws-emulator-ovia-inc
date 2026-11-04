Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields
Issue-Key: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields|_poll#27|Timeout|time.sleep(interval_s)
Issue-Commit: 024100b485faef6e8a7d69224b42c4d112173d44

# apigateway__logs__wafv2: the WAFv2 logging-configuration check still never reaches a verdict — a blocked request's log record with redacted fields is never delivered

> _Note: this issue was identified against the code base at commit `024100b485faef6e8a7d69224b42c4d112173d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
Cause 1 of 1: a web ACL with a CloudWatch Logs logging configuration never delivers a log record for a request its rules blocked at the protected API Gateway stage.
AWS: PutLoggingConfiguration binds a REGIONAL web ACL to a CloudWatch Logs destination whose log-group name must start with `aws-waf-logs-` (a destination without that prefix is refused `WAFInvalidParameterException`; an unknown web ACL is `WAFNonexistentItemException`). Once bound and associated to an API Gateway stage via AssociateWebACL, a request that a rule BLOCKS is refused 403 at the edge AND a JSON log record for that request is delivered to the destination log group, with the members named in the configuration's `RedactedFields` replaced by the redaction marker rather than their request values. All of this is asynchronous but observable by polling the log group. recorded behavior only for the delivery semantics.
Emulator: the check ends `Check timed out after 60.0s`. Nothing in the chain reaches a verdict: no blocked-request record arrives in the destination log group, so neither the delivered record's shape nor the redaction of the configured fields is observable.
Why: a client that relies on WAF logs for audit, incident response or redaction compliance gets no records at all, and cannot verify that sensitive request members are redacted before they are written. Any pipeline keyed on WAF log delivery (subscription filters, metric filters) is dead.
Fix: the wafv2 logging-delivery path together with the WAF enforcement hook on the protected API Gateway stage — a BLOCK verdict at the edge must emit the WAF log record to the configured CloudWatch Logs destination with the `RedactedFields` members redacted, instead of leaving the poll unsatisfiable.
Note: two open issues describe a similar hang for this family but do not carry this check's replay key and so do not cover it — 20261023T210711Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks.md and 20261025T081755Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks-2e88ae.md, alongside 20261026T010229Z-apigateway-logs-wafv2-waf-logging-is-now-accepted.md and 20261101T125407Z-wafv2-logging-an-accepted-s3-logging-destination-never.md (S3 destination).
Traces: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields.

## Evidence
apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields:
`Error: Check timed out after 60.0s`

The check's own budgets show how generous the observation windows are before the harness cap intervenes: `_ASSOCIATE_BUDGET_S = 420.0`, `_PROPAGATION_BUDGET_S = 360.0`, `_LOG_DELIVERY_BUDGET_S = 480.0`, each a poll on an observable condition (the record ARRIVING). The check reached no verdict at all within the harness's 60 s cap, so no blocked-request log record and no redaction evidence was produced.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields.
3. Manual repro: create a CloudWatch log group named `aws-waf-logs-<suffix>`; CreateWebACL (Scope=REGIONAL) with a rule that BLOCKs the probe request; PutLoggingConfiguration binding the web ACL to that log group with RedactedFields naming a request header; AssociateWebACL to a deployed API Gateway REST stage; send the blocked request to the stage URL and poll the log group for the WAF record. Observed: nothing is delivered and the scenario never completes. Expected: a 403 at the edge and a JSON WAF log record in the group whose redacted members carry the redaction marker instead of their request values.
