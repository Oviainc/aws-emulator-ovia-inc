Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.view-definition-must-parse-as-sql
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.view-definition-must-parse-as-sql|run#11|AssertionError|assert message == (
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# CreateView discards the SQL parser's own error for an invalid view definition and serves a fixed generic sentence instead

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateView

## Description
One net-new root cause in cloudwatchomni.

Cause 1 of 1: CreateView refuses an unparseable `definition` with the right status and error code but the wrong message. It throws the parser's diagnosis away and substitutes a constant sentence.

AWS:      Smithy (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/smithy-model/com-amazonaws-cloudwatchomni-cloudwatchomnifrontend.json): CreateView.definition -> ViewDefinition, @required, @length(min 1, max 10000), doc "View definition (SQL query)"; CreateView models ValidationException (HTTP 400, "A parameter is specified incorrectly"). The model states nothing about the message text, so this is recorded behavior only: for a definition that the service's SQL parser cannot read as a statement, AWS answers HTTP 400 ValidationException whose message is the parser's own error rendered under a fixed prefix. Message template: fixed text `Invalid view definition: ` followed verbatim by the parser's error string, itself of the fixed form `sql parser error: ` + the parser's diagnosis, which carries the variable parts taken from the request text (the expectation, the offending token, and its 1-based line and column). The recorded example for definition="{}" is `Invalid view definition: sql parser error: Expected: an SQL statement, found: { at Line: 1, Column: 1` — here `Invalid view definition: `, `sql parser error: `, `Expected: an SQL statement, found: `, ` at Line: `, `, Column: ` are the fixed text, while `{`, `1` and `1` are values derived from the submitted definition and are NOT part of the requirement.

Emulator: answers HTTP 400 ValidationException with the constant message `The view definition must be a valid SQL query.` for every definition the parser rejects, regardless of what is wrong with it or where. The parser's `Err(_)` is matched and dropped, so no position or expectation ever reaches the client.

Why:     a client cannot tell WHY its view definition was refused or WHERE the syntax error is. Tooling that surfaces the service's parser diagnostics to a user (an editor marker at line/column, a test asserting the parser's expectation) gets one opaque sentence instead, and any client asserting on AWS's message text fails. The failure is also indistinguishable from the emulator's other, differently worded definition refusals, so a caller cannot classify the refusal class.

Fix:      CreateView's definition validation — the shared helper the create/update handlers call. It must carry the parser error into the refusal message (prefix `Invalid view definition: ` + the parser error's own rendering) instead of collapsing every parse failure onto one constant, and the handler that constructs the ValidationException must pass that message through unchanged. The sibling refusal constants for "not a query" / "more than one statement" are the same hand-rolled wording and should be re-derived from the parser's output on the same pass.

Traces:   failing check cloudwatchomni.view-definition-must-parse-as-sql (CreateView with definition="{}" expects the parser message, got the constant one). Code: services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/state/view_definition.rs lines 22-23 (NOT_A_QUERY constant) and lines 36-39 (`Err(_) => return Some(NOT_A_QUERY.to_string())`, the discarded parser error); consumed at services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend/src/ops/create_view.rs lines 178-187.

No open issue covers this: the only open cloudwatchomni issues are 20261029T175624Z-cloudwatchomni-createintegration-stub.md (CreateIntegration scaffold stub) and 20261107T031501Z-cloudwatchomni-createalert-refuses-an-un-assumable-access-profile-with.md (CreateAlert's trust-grant message), neither of which touches view definitions.

## Evidence
Operation: CreateView(name="view.<generated>", definition="{}")

Expected (AWS, recorded): HTTP 400, Error.Code=ValidationException, Error.Message=
  "Invalid view definition: sql parser error: Expected: an SQL statement, found: { at Line: 1, Column: 1"

Actual (emulator): HTTP 400, Error.Code=ValidationException, Error.Message=
  "The view definition must be a valid SQL query."

Harness output:
  File "check_cloudwatchomni_view_definition_must_parse_as_sql.py", line 53, in run
    assert message == (
  AssertionError: unexpected message 'The view definition must be a valid SQL query.'

Status and error code match; only the message diverges. Fixed text of the AWS message: "Invalid view definition: " + "sql parser error: " + the parser's diagnosis ("Expected: <what was expected>, found: <token> at Line: <line>, Column: <column>"). Variable parts (<token>, <line>, <column>, <what was expected>) come from the definition in the request; the recorded "{" / "1" / "1" are that one request's values, not requirements.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudwatchomni client at it.
2. Create a valid view to confirm the happy path: cwo.create_view(name="view.probe1", definition="SELECT 1", description="probe") -> succeeds, get_view(name="view.probe1") returns definition "SELECT 1" and type "USER".
3. Submit a definition that is not SQL: cwo.create_view(name="view.probe2", definition="{}").
4. Observe the ClientError: Code is ValidationException (correct) but Message is the constant "The view definition must be a valid SQL query." instead of AWS's "Invalid view definition: sql parser error: Expected: an SQL statement, found: { at Line: 1, Column: 1".
5. Vary the bad definition (e.g. definition="SELECT FROM", definition="   ") — the emulator returns the same constant sentence with no position or expectation, where AWS's message tracks the parser's diagnosis and the offending line/column.
