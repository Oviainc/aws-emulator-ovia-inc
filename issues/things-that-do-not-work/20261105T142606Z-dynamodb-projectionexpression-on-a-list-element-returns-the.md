Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-projection-nested-document-paths
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-projection-nested-document-paths
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.rows.L[*]
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.rows.L[*].M.b
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.deep.M.inner.L[*]
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.deep.M.inner.L[*].M.y
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# DynamoDB ProjectionExpression on a list element returns the element at its original index instead of a re-indexed one-element list

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
Two net-new root causes, both in how the emulator materialises a ProjectionExpression that addresses an element of a list-typed attribute. This bundle contains 2 causes; both must be closed.

Cause 1 of 2 — GetItem with a document path into a top-level list attribute (e.g. `rows[<i>].<member>`) returns a list padded to the original index.
AWS: Smithy: GetItem.ProjectionExpression is a plain string with no modeled constraints and GetItem.Item targets AttributeMap, so list-projection semantics are prose-only in the model — recorded behavior only. The recording shows that when a projection selects element <i> of a list attribute, AWS returns that attribute as a DENSE list containing only the selected element(s), re-indexed from 0: the selected element appears at L[0] (carrying only the projected members of that element), and no placeholder entries are emitted for the unselected positions. AWS answers 200 in both cases; this is a response-shaping requirement, not an error-message one.
Emulator: 200, but the selected element is emitted at its ORIGINAL list index — L[0] of the returned list does not carry the projected member (reported missing) while an extra entry appears at L[1] holding the projected element. The list is left positional/padded rather than collapsed.
Why: a client that reads the projected attribute at index 0 — which is what AWS guarantees after a list-element projection, and what every SDK example and document-path tutorial relies on — reads nothing, or reads an unexpected extra entry and mis-counts the list. Code that iterates the projected list sees a different length against the emulator than against AWS, so list-element projections are unusable and off-by-one bugs are introduced by the emulator rather than caught by it.
Fix: the GetItem response projection/serializer — the component that applies a parsed ProjectionExpression document path to a stored item must rebuild list attributes densely from the matched elements instead of preserving their source indices.
Traces: dynamodb-projection-nested-document-paths#task#9 (1 step in cluster).

Cause 2 of 2 — the same defect at greater nesting depth: a projection through a map into a nested list (e.g. `deep.inner[<i>].<member>`) likewise keeps the source index.
AWS: recorded behavior only (same prose-only ProjectionExpression semantics). The recording shows the nested list is returned dense: deep.M.inner.L[0] carries the projected member of the selected element, with no padding entries.
Emulator: 200, deep.M.inner.L[0]'s projected member is missing and an extra entry appears at deep.M.inner.L[1] — the nested list is padded to the source index exactly as in cause 1, so the defect is not confined to top-level list attributes and a fix applied only at the top level leaves this path wrong.
Why: the same off-by-one/extra-element exposure as cause 1, on the nested document paths that projections are most often used for (reading one row out of a nested collection), so a client reading `deep.inner[0]` after projecting `deep.inner[<i>]` gets an empty or wrong element.
Fix: same component as cause 1 — the recursive step of the projection walker, applied at every level of the document path, not just the outermost attribute. Related open issue 20261105T114920Z-dynamodb-10-net-new-projection-select-causes-a.md covers projection/Select validation causes; it does not cover this list re-indexing defect.
Traces: dynamodb-projection-nested-document-paths#task#11 (1 step in cluster).

## Evidence
Cause 1 — GetItem, fixture dynamodb-projection-nested-document-paths step task#9 (cluster of 1):
  expected 200, actual 200
  $.Item.rows.L[0].M.b: missing (expected {"S": "b0"}, got None)
  $.Item.rows.L[1]: extra (expected None, got {"M": {"b": {"S": "b0"}}})
  i.e. AWS returns a one-element list with the projected element at index 0; the emulator returns the element at index 1 and nothing projected at index 0. ('b', 'b0' and the index are this recording's values; the requirement is the dense re-indexing, not these values.)

Cause 2 — GetItem, fixture dynamodb-projection-nested-document-paths step task#11 (cluster of 1):
  expected 200, actual 200
  $.Item.deep.M.inner.L[0].M.y: missing (expected {"S": "y0"}, got None)
  $.Item.deep.M.inner.L[1]: extra (expected None, got {"M": {"y": {"S": "y0"}}})
  same padding behaviour one level deeper, through a map into a nested list.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus).

Cause 1: fixture dynamodb-projection-nested-document-paths, step task#9 — PutItem an item with a list attribute of several map elements, then GetItem with a ProjectionExpression addressing one element and one of its members (`rows[1].b` in the recording). Expect Item.rows.L to be a single-element list with the projected member at L[0]; observe a list whose L[0] lacks the member and whose L[1] carries it.

Cause 2: same fixture, step task#11 — GetItem with a ProjectionExpression through a map into a nested list (`deep.inner[1].y` in the recording). Expect Item.deep.M.inner.L[0] to carry the projected member; observe it at L[1] with L[0] empty of it.
