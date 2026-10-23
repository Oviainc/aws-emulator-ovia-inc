Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation logs.deep-logs-1-logsql-sql-from-clause-overrides-log-group-identifiers logs.deep-logs-1-logsql-sql-group-by-having-matches-hand-computation logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected logs.deep-logs-2-logcmk-query-result-key-gates-insights logs.describe-queries-lists-inflight-and-completed logs.describe-queries-lists-running-and-completed logs.describe-queries-lists-running-and-finished logs.get-log-record-field-values-match-query-results-pointer logs.get-log-record-from-query-result logs.get-log-record-returns-full-event logs.insights-stats-aggregation-and-statistics logs.list-log-groups-for-query-scoped-results logs.list-log-groups-for-query-scopes-to-query logs.logs-insights-semantics-both-log-group-selectors-rejected logs.logs-insights-semantics-fields-rename-and-reserved-fields logs.logs-insights-semantics-filter-operators-select-exact-sets logs.logs-insights-semantics-multi-group-rows-attributed-via-log logs.logs-insights-semantics-no-matching-data-completes-with-empty-results logs.logs-insights-semantics-parse-glob-and-named-capture-extract-planted-values logs.logs-insights-semantics-semantic-error-rejected-at-start logs.logs-insights-semantics-sort-limit-dedup-display-arithmetic logs.logs-insights-semantics-stats-aggregates-by-field-match-hand-computation logs.logs-insights-semantics-stats-bin-one-minute-buckets-at-boundaries logs.logs-insights-semantics-status-progression-and-record-statistics logs.logs-insights-semantics-stop-running-query-reports-cancelled logs.logs-insights-semantics-syntax-error-rejected-at-start logs.logs-insights-semantics-unknown-log-group-rejected-at-start logs.logsquery-display-projects-only-the-named-fields logs.logsquery-limit-truncates-rows-while-recordsmatched-counts-all logs.logsquery-parse-glob-extracts-fields-from-unstructured-lines logs.logsquery-parse-then-numeric-aggregates-match-the-seeded-values logs.logsquery-sort-on-a-parsed-number-orders-numerically logs.logsquery-stats-group-key-absent-for-events-without-the-field logs.logsquery-time-window-excludes-events-outside-the-range logs.op-contract-stopQuery logs.query-execution-polling-states logs.query-lifecycle-state-ordering logs.stop-query-terminal-state-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected|run#14|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r} {msg!r}"
Issue-Key: cloudwatch-logs|StartQuery|NotImplemented
Issue-Key: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected|run#13|AssertionError|assert code == "MalformedQueryException", f"{having}: expected MalformedQueryException, got {code!r} {msg!r}"
Issue-Key: logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected|run#18|AssertionError|assert code == "InvalidParameterException", f"SQL mixed class: expected InvalidParameterException, got {code!r} {msg!r}"
Issue-Key: logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected|run#14|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r} {msg!r}"
Issue-Key: logs.deep-logs-2-logcmk-query-result-key-gates-insights|_poll#20|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting for {what}; last={last!r}")
Issue-Key: logs.logs-insights-semantics-both-log-group-selectors-rejected|run#12|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r} {msg!r}"
Issue-Key: logs.logs-insights-semantics-semantic-error-rejected-at-start|run#13|AssertionError|assert code == "MalformedQueryException", f"unknown function: expected MalformedQueryException, got {code!r} {msg!r}"
Issue-Key: logs.logs-insights-semantics-syntax-error-rejected-at-start|run#14|AssertionError|assert code1 == "MalformedQueryException", f"unknown command: expected MalformedQueryException, got {code1!r} {msg1!r}"
Issue-Key: logs.logs-insights-semantics-unknown-log-group-rejected-at-start|run#13|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r} {msg!r}"
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs StartQuery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
`StartQuery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

42 recorded unit(s) fail on their `StartQuery` call, so each one unblocks only when `StartQuery` works: `logs.deep-logs-1-logsql-cwli-query-string-as-sql-rejected`, `logs.deep-logs-1-logsql-ppl-backticked-fields-where-stats-match-hand-computation`, `logs.deep-logs-1-logsql-sql-from-clause-overrides-log-group-identifiers`, `logs.deep-logs-1-logsql-sql-group-by-having-matches-hand-computation`, `logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected`, `logs.deep-logs-1-logsql-sql-ppl-across-standard-and-infrequent-access-rejected`, `logs.deep-logs-1-logsql-sql-with-log-group-name-parameter-rejected`, `logs.deep-logs-2-logcmk-query-result-key-gates-insights`, `logs.describe-queries-lists-inflight-and-completed`, `logs.describe-queries-lists-running-and-completed`, `logs.describe-queries-lists-running-and-finished`, `logs.get-log-record-field-values-match-query-results-pointer`, `logs.get-log-record-from-query-result`, `logs.get-log-record-returns-full-event`, `logs.insights-stats-aggregation-and-statistics`, `logs.list-log-groups-for-query-scoped-results`, `logs.list-log-groups-for-query-scopes-to-query`, `logs.logs-insights-semantics-both-log-group-selectors-rejected`, `logs.logs-insights-semantics-fields-rename-and-reserved-fields`, `logs.logs-insights-semantics-filter-operators-select-exact-sets` (+22 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs`. Replaying the corpus, the emulator refused the `StartQuery` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_logs_deep_logs_1_logsql_cwli_query_string_as_sql_rejected.py", line 130, in run
    assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r} {msg!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterException, got 'NotImplemented' 'operation StartQuery is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the logs emulator
Call StartQuery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
