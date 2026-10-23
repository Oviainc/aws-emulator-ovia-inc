Issue-Severity: LOW
Issue-Gap-Class: service

# bcm-data-exports `CreateExport` / `UpdateExport` accept a `DataQuery.QueryStatement` that does not parse as SQL at all

_Filed: 2026-10-23T09:00:00Z_
_Operation: CreateExport (and UpdateExport) | Service: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports | Class: service_

## Description

`DataQuery.QueryStatement` is "an SQL statement. Data Exports only supports a limited
subset of the SQL syntax" (the modelled doc for `CreateExport`), so AWS parses the
statement and refuses one it cannot read with the modelled `ValidationException`
(HTTP 400).

The emulator reads the tables a statement names off the PARSED statement
(`state::query_tables`, `sqlparser`), and that is what the catalog check in
`state::effective_table_configurations` is driven from — a table the catalog does not
publish is refused, which is the rule the export-validation fixture pins. But a statement
the parser cannot read at all yields `None` there, which is deliberately treated as
"the tables of this statement are UNKNOWN" rather than as a refusal: this parser is a
general SQL grammar, not the service's own documented subset, so refusing everything it
cannot read would reject statements AWS accepts.

The consequence is the gap filed here: a `QueryStatement` that is not valid SQL is
PERSISTED and the create answers HTTP 200, where AWS refuses it at the create call.
`tests/all/data_query.rs::a_statement_whose_tables_cannot_be_read_is_not_refused_for_a_table`
pins the current accept, so the gap is visible in the suite rather than silent.

The same gap covers one more input class, for the same reason: a statement nested
deeper than the parser's recursion limit (`state::QUERY_RECURSION_LIMIT`) or longer
than `state::MAX_QUERY_STATEMENT_LENGTH` also yields no tables, so an unpublished
table named inside such a statement is not refused either. Those two bounds are
deliberate (caller-chosen nesting must not grow the stack, and the parse is bounded
by the statement's modelled ceiling); what is missing is the refusal for the input
they decline to read.

## Observable effect

`CreateExport` with `Export.DataQuery.QueryStatement` set to text that is not a SQL
statement (for example `not sql at all`):

- AWS: HTTP 400, `__type` `ValidationException`, a `Message` built from the request
  saying the query statement could not be parsed.
- emulator: HTTP 200 and an `ExportArn`; the definition is stored and read back by
  `GetExport` carrying the unparseable statement.

## Fix sketch

Refuse a statement the parser rejects with the modelled `ValidationException(400)`
naming `Export.DataQuery.QueryStatement` — but only once the accepted SQL subset is
pinned down well enough that the refusal cannot reject a statement the service accepts:
the published subset (a projection over one or more catalog tables, with a filter and a
join) is narrower than the generic dialect, so the safe direction is to validate against
that subset explicitly rather than to treat any parse failure as a client error.

## Steps to Reproduce

1. Boot the emulator.
2. `CreateExport` with a well-formed `Export` whose `DataQuery.QueryStatement` is not a
   SQL statement.
3. Observe HTTP 200 and the export readable through `GetExport`, where AWS answers 400
   `ValidationException`.
