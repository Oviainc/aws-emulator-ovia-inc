Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-insights-semantics-syntax-error-rejected-at-start logs.malformed-query-rejected logs.logsquery-parse-with-an-unclosed-regex-group-rejected logs.logsquery-redefining-a-parsed-field-rejected logs.op-contract-putQueryDefinition
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logs-insights-semantics-syntax-error-rejected-at-start|run#18|AssertionError|assert msg1 == expected1, f"unknown command: expected message {expected1!r}, got {msg1!r}"
Issue-Key: logs.malformed-query-rejected|run#15|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.logsquery-parse-with-an-unclosed-regex-group-rejected|run#17|AssertionError|assert code_msg == expected, (
Issue-Key: logs.logsquery-redefining-a-parsed-field-rejected|run#18|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.op-contract-putQueryDefinition|run#28|AssertionError|assert message == expected, (
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# CloudWatch Logs Insights query refusal messages diverge: missing the AWS service suffix, wrong offending symbol, no regex compile error, truncated PutQueryDefinition text (4 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Four net-new root causes in CloudWatch Logs' query-validation refusal messages. Each must be closed before submitting.

Cause 1 of 4 — MalformedQueryException messages omit AWS's trailing service/status/code/request-id clause.
AWS: Smithy (com.amazonaws.cloudwatchlogs) StartQuery, which models MalformedQueryException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: the refusal message ends with the clause ` (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <request id>; Proxy: null)`. Fixed text: ` (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: `, `; Proxy: null)`. Variable parts: the request id of the call, and the leading sentence (`unexpected symbol found <symbol> at line <line> and position <position>`) whose symbol/line/position come from the rejected queryString. In malformed-query-rejected the expected leading sentence is `unexpected symbol found | at line 1 and position <none>`: AWS names the offending symbol `|` with no position, where the emulator names the first token of the query and a position of 0.
Emulator: answers MalformedQueryException with the bare sentence only — `unexpected symbol found frobnicate at line 1 and position 58` / `unexpected symbol found this at line 1 and position 0` — dropping the whole service clause, and in the second case reporting a different symbol and position than the one AWS blames.
Why: that clause is how SDK/CLI users and log scrapers identify the service, status, error code and request id of a query refusal; stripping it breaks any handler or support workflow keyed on it, and misnaming the offending symbol/position sends a caller to the wrong part of its query.
Fix: the MalformedQueryException message construction in the logs crate's query-validation layer — append AWS's fixed clause with the call's request id, and blame the symbol AWS blames.
Traces: logs.logs-insights-semantics-syntax-error-rejected-at-start, logs.malformed-query-rejected

Cause 2 of 4 — a `parse` command's regex literal is not compiled, so an unclosed group is reported as an unexpected symbol instead of a compile error.
AWS: StartQuery, MalformedQueryException 400; recorded behavior only for the text. A `parse` with a regex literal whose group is not closed is refused with a compile-error message: `Compile Error - Invalid regular expression in query: '<regex with the query's named captures substituted>' — Unclosed group near index <index>` followed by the offending regex on its own line and then the service clause of cause 1. Fixed text: `Compile Error - Invalid regular expression in query: `, ` — Unclosed group near index `; variable parts: the regex (as rewritten with capture names) and the index at which the group is unclosed.
Emulator: answers `unexpected symbol found /lat=(?<lat>\d+/ at line 1 and position 55` — it treats the whole regex literal as an unparseable token rather than compiling it, so the refusal names neither the regular expression's fault nor where it is.
Why: the compile error is the only thing that tells the author which construct in the regex is broken and at what index; an "unexpected symbol" verdict implies the `parse /regex/` form itself is unsupported, which sends the caller to rewrite a valid query shape.
Fix: compile the regex literal of `parse` in the query parser and surface the compile failure as AWS's Compile Error message, in the logs crate's query-validation layer.
Traces: logs.logsquery-parse-with-an-unclosed-regex-group-rejected

Cause 3 of 4 — redefining an already-parsed ephemeral field is reported as a syntax error instead of AWS's duplicate-field refusal.
AWS: StartQuery, MalformedQueryException 400; recorded behavior only for the text. A query that defines an ephemeral field name a previous `parse` already produced is refused with `Ephemeral field is already defined: <field> <no span>` plus the service clause of cause 1. Fixed text: `Ephemeral field is already defined: ` and ` <no span>`; the variable part is the field name taken from the query.
Emulator: answers `unexpected symbol found lat at line 1 and position 135` — a syntax verdict that neither names the rule nor identifies the redefinition.
Why: the author is told the query does not parse when it parses fine and merely redefines a field, so the reported position points at a legal token and the real rule is never stated.
Fix: the ephemeral-field scope/duplicate check in the query semantic-validation layer, with AWS's sentence.
Traces: logs.logsquery-redefining-a-parsed-field-rejected

Cause 4 of 4 — PutQueryDefinition's unknown-queryDefinitionId refusal is truncated to its first sentence.
AWS: Smithy PutQueryDefinition, members name (required), queryDefinitionId, queryString (required); models ResourceNotFoundException with httpError 400. Model text only; the recorded refusal for a queryDefinitionId that does not resolve is ResourceNotFoundException with the message `Query definition id is invalid. If you are updating query definition, please provide correct query definition id; If you are creating new query definition, do not use --query-definition-id as a parameter (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <request id>; Proxy: null)`. All of that is fixed text except the request id.
Emulator: answers ResourceNotFoundException with the service clause intact but only `Query definition id is invalid.` before it — the whole actionable half of the sentence (what to do when updating versus creating) is missing.
Why: the dropped half is the part that tells a caller whether to supply a correct id or omit the parameter; without it a client cannot tell an update typo from a create that should not have sent the id at all.
Fix: the ResourceNotFoundException message for PutQueryDefinition in the logs crate's validation layer.
Traces: logs.op-contract-putQueryDefinition

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout. Error codes matched in every case; the messages diverged.

Cause 1 — logs-insights-semantics-syntax-error-rejected-at-start:
  expected 'unexpected symbol found frobnicate at line 1 and position 58 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
  actual   'unexpected symbol found frobnicate at line 1 and position 58'
  logs.malformed-query-rejected:
  expected 'unexpected symbol found | at line 1 and position <none> (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
  actual   'unexpected symbol found this at line 1 and position 0'

Cause 2 — logsquery-parse-with-an-unclosed-regex-group-rejected:
  expected "Compile Error - Invalid regular expression in query: 'lat=(?<capture000>(?s)\\d+' — Unclosed group near index 25\nlat=(?<capture000>(?s)\\d+ (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)"
  actual   'unexpected symbol found /lat=(?<lat>\\d+/ at line 1 and position 55'

Cause 3 — logsquery-redefining-a-parsed-field-rejected:
  expected 'Ephemeral field is already defined: lat <no span> (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
  actual   'unexpected symbol found lat at line 1 and position 135'

Cause 4 — op-contract-putQueryDefinition:
  expected 'Query definition id is invalid. If you are updating query definition, please provide correct query definition id; If you are creating new query definition, do not use --query-definition-id as a parameter (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <request id>; Proxy: null)'
  actual   'Query definition id is invalid. (Service: AWSLogs; Status Code: 400; Error Code: ResourceNotFoundException; Request ID: <request id>; Proxy: null)'

The request ids, field names ('lat'), symbols ('frobnicate') and positions above are this run's values filling the templates; the fixed text and the structure are the requirement.

## Steps to Reproduce
Boot the emulator from this checkout and create a log group g with some events.

Cause 1: aws logs start-query --log-group-name g --start-time <t0> --end-time <t1> --query-string 'fields @timestamp, @message | frobnicate' -> MalformedQueryException whose message ends at 'position 58'; AWS appends ' (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <id>; Proxy: null)'. Also try a query starting with a bare word followed by '|' and compare the symbol/position AWS blames.

Cause 2: --query-string 'fields @message | parse @message /lat=(?<lat>\d+/' -> 'unexpected symbol found /lat=.../'; AWS answers the Compile Error / Unclosed group message.

Cause 3: --query-string 'fields @message | parse @message "lat=*" as lat | parse @message "lon=*" as lat' -> 'unexpected symbol found lat ...'; AWS answers 'Ephemeral field is already defined: lat <no span> (...)'.

Cause 4: aws logs put-query-definition --name n --query-string 'fields @message' --query-definition-id 00000000-0000-4000-8000-000000000999 -> ResourceNotFoundException 'Query definition id is invalid. (Service: ...)'; AWS includes the update-versus-create guidance sentence.

Then re-replay: logs.logs-insights-semantics-syntax-error-rejected-at-start, logs.malformed-query-rejected, logs.logsquery-parse-with-an-unclosed-regex-group-rejected, logs.logsquery-redefining-a-parsed-field-rejected, logs.op-contract-putQueryDefinition
