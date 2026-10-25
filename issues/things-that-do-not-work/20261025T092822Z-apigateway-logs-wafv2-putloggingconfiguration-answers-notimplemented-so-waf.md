Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected
Issue-Key: apigateway__logs__wafv2|PutLoggingConfiguration|NotImplemented
Issue-Key: apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected|run#21|AssertionError|assert code == ERR_INVALID_PARAMETER, (
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# apigateway__logs__wafv2: PutLoggingConfiguration answers NotImplemented, so WAF log delivery and the aws-waf-logs- destination refusal are both unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLoggingConfiguration

## Description
Cause 1 of 1: wafv2 PutLoggingConfiguration is still a scaffold stub, so every WAFv2 logging-configuration behaviour reached through an API Gateway REST stage is unreachable — neither the delivered log record nor the destination-name refusal can be observed.

AWS: PutLoggingConfiguration enables one logging destination per web ACL and, once enabled, WAF delivers one record per inspected request to that destination with the configured RedactedFields redacted. The destination name is validated: AWS documents (and the model's own operation documentation states) that "The name that you give the destination must start with aws-waf-logs-", and a destination that does not is refused as a client error. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): PutLoggingConfiguration input LoggingConfiguration (required); modeled errors WAFInvalidParameterException (httpError 400), WAFLogDestinationPermissionIssueException (400), WAFNonexistentItemException (400), WAFOptimisticLockException (400), WAFInternalErrorException (500) — i.e. the refusal of an unacceptable LoggingConfiguration member is HTTP 400 WAFInvalidParameterException. On success the operation answers HTTP 200 echoing the stored LoggingConfiguration.

Emulator: every PutLoggingConfiguration call is refused with HTTP 501 and error code 'NotImplemented' ("operation PutLoggingConfiguration is scaffolded but not implemented in this emulator"). The delivery check dies on the botocore ClientError at the PutLoggingConfiguration call; the negative check reaches its assertion and reports: "a CloudWatch Logs destination whose group name lacks 'aws-waf-logs-' must be refused with 'WAFInvalidParameterException', got 'NotImplemented'". Note this is a CHANGE of failure mode from the earlier reports for this group — the two checks no longer hang at the stage, they now get as far as PutLoggingConfiguration and are refused there.

Why: a client cannot enable WAF logging at all, so the only audit trail of what a web ACL did to traffic in front of a REST stage does not exist; and a misconfigured destination that AWS refuses with HTTP 400 WAFInvalidParameterException is instead answered with a 501 'NotImplemented', which no SDK retry/handling path treats as a client configuration error.

Fix: the wafv2 PutLoggingConfiguration handler — it must validate the LoggingConfiguration (destination name prefix, web ACL existence, lock token), persist it as the web ACL's single logging configuration, echo it on a 200, and feed the record-delivery path to CloudWatch Logs. Related open issues that do NOT own these checks: 20261024T065754Z-wafv2-putloggingconfiguration-stub.md (names the stub but the suite did not mark either check ALREADY FILED against it), 20261023T210711Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks.md and 20261025T081755Z-apigateway-logs-wafv2-both-wafv2-logging-configuration-checks-2e88ae.md (both describe this pair hanging at the stage, a different failure mode from the one now observed).

Traces: apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields, apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected. Handler: services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/put_logging_configuration.rs:228 returns Scaffolded::not_implemented("PutLoggingConfiguration").

## Evidence
Check apigateway__logs__wafv2.wafv2-logging-configuration-delivers-a-blocked-request-record-with-redacted-fields [error]:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutLoggingConfiguration operation: operation PutLoggingConfiguration is scaffolded but not implemented in this emulator

Check apigateway__logs__wafv2.wafv2-put-logging-configuration-log-group-without-the-waf-prefix-rejected [mismatch]:
  AssertionError: a CloudWatch Logs destination whose group name lacks 'aws-waf-logs-' must be refused with 'WAFInvalidParameterException', got 'NotImplemented'

Expected (AWS): PutLoggingConfiguration with a destination log group named aws-waf-logs-<token> succeeds with HTTP 200 and the configuration is applied (one record per inspected request, RedactedFields redacted); PutLoggingConfiguration with a log group whose name lacks the aws-waf-logs- prefix is refused with HTTP 400, error code WAFInvalidParameterException.
Actual (emulator): both calls answer HTTP 501 with error code NotImplemented; no logging configuration is ever stored and no log record is ever delivered.

## Steps to Reproduce
1. Create a REST API with a working method/integration and deploy it to a stage.
2. wafv2 CreateWebACL (Scope=REGIONAL) with a blocking rule, then AssociateWebACL with the stage ARN.
3. logs CreateLogGroup aws-waf-logs-<token>; call wafv2 PutLoggingConfiguration with ResourceArn=<web ACL ARN>, LogDestinationConfigs=[<that log group ARN>] and RedactedFields set — observe HTTP 501 / NotImplemented instead of HTTP 200.
4. Call wafv2 PutLoggingConfiguration again with a log group whose name does NOT start with aws-waf-logs- — observe error code NotImplemented instead of HTTP 400 WAFInvalidParameterException.
5. Or replay the two check ids in trace_ids.
