Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics cloudwatch__logs.obs-logs-2-metric-filter-apply-on-transformed-logs-matches-transformed-event
Issue-Key: cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected|_error#9|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics|run#23|AssertionError|assert invalid_errors == [("InvalidParameterException", _INVALID_MESSAGE)] * _N_INVALID, (
Issue-Key: cloudwatch__logs.obs-logs-2-metric-filter-apply-on-transformed-logs-matches-transformed-event|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudwatch__logs: PutTransformer accepts a transformer with no leading parser, the logGroupName validation message reports an anchored pattern, and transformed-log metric filters emit no datapoint (3 causes)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
Three root causes across the cloudwatch+logs behavioral group. Cause count: 3.

=== Cause 1 of 3: PutTransformer accepts a transformerConfig whose first processor is not a parser ===
AWS: Smithy PutTransformer takes logGroupIdentifier (@required) and transformerConfig -> Processors (@required, length 1-20) and models InvalidParameterException (400) and InvalidOperationException (400). A transformer must begin with a parser processor (parseJSON / parseVPC / grok / csv / ...); a configuration whose first processor is a mutation step is refused with a 400 client error. Recorded behavior: real AWS refuses this exact call.
Emulator: the call answers HTTP 200 with an empty body, installing a transformer AWS would never accept.
Why: a client gets a transformer the service accepted but whose semantics are undefined; the validation that protects the ingest pipeline from a malformed processor chain is absent, and the mistake surfaces later as missing or wrong extracted fields rather than at the API call.
Fix: PutTransformer's transformerConfig validation (first-processor-must-be-a-parser rule) in the logs PutTransformer handler.
Traces: cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected.

=== Cause 2 of 3: the logGroupName constraint-violation message reports the pattern with ^...$ anchors ===
AWS: a logGroupName that violates its pattern is refused with 400 InvalidParameterException and the request-derived message "1 validation error detected: Value at 'logGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\.\-_/#A-Za-z0-9]+" — the pattern is reported UNANCHORED. Smithy LogGroupName carries pattern ^[\.\-_/#A-Za-z0-9]+$ (length 1-512); AWS strips the anchors when it renders the message.
Emulator: the same status and code, but the message renders the raw modelled pattern with its anchors: "... Member must satisfy regular expression pattern: ^[\.\-_/#A-Za-z0-9]+$".
Why: the message is built from the request and the model, so its shape is a requirement, not an example. A client or fixture that matches the validation message diverges on every constraint violation in the service. Open issue 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md records CWL message divergences for other checks and does not carry this one.
Fix: the constraint-violation message renderer shared by the logs validation layer (strip ^/$ when echoing a pattern).
Traces: cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics.

=== Cause 3 of 3: a metric filter over transformed log events produces no CloudWatch datapoint ===
AWS: when a log group has a transformer installed, a metric filter is applied to the TRANSFORMED event, so a pattern matching a field the transformer extracted publishes a datapoint readable through CloudWatch.
Emulator: no datapoint ever materialises and the check runs to the suite's 60 s cap waiting for it (its poll returns on the first successful reading, so a correct emulator would end it in one iteration). This is the transformed-log instance of the metric-filter emission gap also filed against service `logs`.
Why: a client that transforms at ingest and then measures the transformed fields gets no metric at all, so the whole transform-then-measure pattern is unobservable.
Fix: the metric-filter evaluation stage of the logs ingest pipeline — it must run after the transformer and emit into the CloudWatch metric store.
Traces: cloudwatch__logs.obs-logs-2-metric-filter-apply-on-transformed-logs-matches-transformed-event.

## Evidence
Cause 1 — cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected: AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}. Expected 400 InvalidParameterException.
Cause 2 — cloudwatch__logs.obs-logs-3-service-usage-and-emf-policy-metrics: AssertionError: PutLogEvents into the ill-formed group name 'sim2real bad name!*' returned [('InvalidParameterException', "1 validation error detected: Value at 'logGroupName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\.\\-_/#A-Za-z0-9]+$"), ...x3]; each must fail InvalidParameterException "... pattern: [\\.\\-_/#A-Za-z0-9]+".
Cause 3 — cloudwatch__logs.obs-logs-2-metric-filter-apply-on-transformed-logs-matches-transformed-event: 'Check timed out after 60.0s'; the CloudWatch datapoint for the metric filter over the transformed event never appeared.

## Steps to Reproduce
Cause 1: CreateLogGroup; PutTransformer(logGroupIdentifier=<group>, transformerConfig=[{<a non-parser processor, e.g. addKeys/renameKeys>}, ...]) — returns 200 instead of 400 InvalidParameterException.
Cause 2: PutLogEvents(logGroupName='sim2real bad name!*', ...) — the InvalidParameterException message reports the pattern with ^ and $.
Cause 3: CreateLogGroup; PutTransformer with a leading parseJSON; PutMetricFilter whose pattern references a transformer-extracted field; PutLogEvents a matching record; poll cloudwatch GetMetricStatistics — no datapoint.
