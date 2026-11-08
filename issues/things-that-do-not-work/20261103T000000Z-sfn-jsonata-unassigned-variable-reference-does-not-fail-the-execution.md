Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.choice-jsonata-assign-is-scoped-to-the-selected-rule|run#7|AssertionError|assert desc["status"] == "FAILED", (
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn: a JSONata expression that reads a variable NOTHING in scope assigned evaluates to nothing instead of failing the execution (1 cause)

> _This cause was uncovered by fixing the `Choice`-rule `Assign` scoping of
> `20261029T183117Z-sfn-interpreter-and-data-fidelity-five-causes-still-open.md`
> (now under `issues/things-that-now-work/`). That fix is what makes a matched rule
> leave a variable only the STATE level declares unassigned; the recorded check then
> gets past the scoping assertion and diverges on this one, which is a rule about
> reading an unassigned variable rather than about `Choice` at all._

## Operation
StartExecution

## Description

AWS: recorded behavior only (the ASL query language is not in the Smithy model). An
expression of a JSONata state that reads `$name` for a variable NO state in scope has
assigned does not evaluate to nothing — the execution FAILS. The recorded check builds
a `Choice` whose state level declares `tag` and whose matched rule does not, so nothing
assigns `tag` on the path taken, and then reads `$tag` in a later state's `Output`; it
asserts `status == "FAILED"`.

Emulator: the expression evaluates as plain JSONata does — an unbound variable is
`undefined`, so the member it feeds is simply omitted from the document the `Output`
builds and the execution SUCCEEDS. With the recorded definition it answers
`status=SUCCEEDED output='{"branch":"first"}'` where AWS answers `FAILED`.

Why: a workflow that misspells a variable name, or that reads a variable only one
branch of a `Choice` assigns, runs to completion with that member silently missing
instead of failing where AWS fails. No client can tell the two apart from the response.

Fix: the evaluation seam in
`services/service-com-amazonaws-sfn-awsstepfunctions/src/state/jsonata.rs` — the
`Bindings` an expression is evaluated with know the scope (`variables`), so the
reference has to be checked against it before or while the expression is evaluated, and
a reference to a name the scope does not hold raised as the language's own
`States.QueryEvaluationError` (`asl::QUERY_EVALUATION_ERROR`).

The hard part is telling a VARIABLE reference apart from a name that is not one. The
crate already extracts the `$name` terms of an expression lexically for
`DescribeStateMachine`'s `variableReferences`
(`asl::scan_expression`/`asl::variable_references`), and that scan already excludes
`$$`, `$` alone, the reserved `$states` term and a function call (`$name(`). What it
does NOT exclude is a name the expression itself BINDS — `$x := …` and a
`function($x) { … }` parameter — so reusing it as-is would fail an expression that
binds its own variables, which `tests/all/jsonata_states.rs` already drives
(`{% ( $x := [1..1000]; … ) %}`, `{% ( $f := function($x) { $f($x + 1) }; $f(0) ) %}`).
Either that scan grows an understanding of the two binding forms, or the check is asked
of the maintained interpreter behind `service_common::jsonata` rather than of a lexical
scan here — the latter is preferable, because the binding forms belong to the language
the engine implements and not to this crate.

## Evidence
`AssertionError: with only a state-level Assign, a MATCHED rule must leave $tag
undefined and the execution must fail; input {'n': 50} gave status=SUCCEEDED
output='{"branch":"first"}'`.

## Steps to Reproduce
CreateStateMachine with `QueryLanguage=JSONata` and a `Choice` whose state level
declares `Assign: {"tag": "from-state-level"}` and whose first (matching) rule declares
an `Assign` that does NOT declare `tag`, transitioning to a `Pass` whose `Output` reads
`{% $tag %}`; StartExecution and read `DescribeExecution.status`. AWS answers `FAILED`;
the emulator answers `SUCCEEDED` with the member absent.

## Notes
The rule is not specific to `Choice`: every member a JSONata state evaluates (`Output`,
`Assign`, `Arguments`, `Items`, a `Choice` rule's `Condition`, a `Wait`'s `Seconds`, a
`Fail`'s `Error`/`Cause`) reads variables the same way, so the check belongs at the one
seam all of them go through rather than at any state type.
