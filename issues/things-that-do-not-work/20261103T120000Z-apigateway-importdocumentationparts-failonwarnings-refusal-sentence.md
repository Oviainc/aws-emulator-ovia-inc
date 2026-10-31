Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.import-documentation-parts-bulk-atomic
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.import-documentation-parts-bulk-atomic|run#21|AssertionError|assert blocked_message == (
Issue-Commit: 11c4079479e9b2b0b5a1a2a0fb2cb7a1f2a1c6a1

# API Gateway: the SENTENCE `ImportDocumentationParts`' `failOnWarnings` refusal states is still not sourced — the gate, the atomicity and the warning CONDITION now work, and the recorded check compares the whole message verbatim

> _Split out of `issues/things-that-now-work/20261027T101902Z-apigateway-2-causes-of-the-request-validator-issue-remain-open.md`, whose other cause (the four-mode request-validator check) is fixed and whose unit passes. This report carries the one replay key that does NOT pass, so nothing about it is claimed closed. Everything below is what that fix ESTABLISHED, including the recorded import body, which is new: a later fixer starts from here rather than from scratch._

## Operation
ImportDocumentationParts

## Description

Cause 1 of 1: the whole-message comparison of `apigateway.import-documentation-parts-bulk-atomic` still fails, because AWS's wording for the warning the refusal states cannot be read from any source in this repo.

AWS: with `mode=merge` and `failOnWarnings=true`, an import whose body earns a warning is refused with HTTP 400 `BadRequestException` and nothing is created; with `failOnWarnings=false` the same body commits and the warnings are reported in `DocumentationPartIds$warnings`.

Emulator: all of that now happens. What diverges is the TEXT of the refusal. The emulator answers

```
Warnings found during import:
	Reference to model 'DoesNotExist' not found. Ignoring.
```

and the check's assertion is `run#21 assert blocked_message == (…)` with the custom message "expected the unresolvable model reference to be named in the rejection, got …" — which quotes only the actual value, so the expectation is not readable from the replay.

What the fix ESTABLISHED, which is the useful part of this report:

* **The recorded import body, in full** (whitespace collapsed; read out of the replay a window at a time through a diagnostic refusal):

```json
{"openapi": "3.0.0", "info": {"title": "doc-import-warn", "version": "1"},
 "paths": {"/": {"get": {"responses": {"200": {"description": "200 response",
   "content": {"application/json": {"schema": {"$ref": "#/components/schemas/DoesNotExist"}}}}}}}},
 "x-amazon-apigateway-documentation": {"version": "1.0",
   "documentationParts": [{"location": {"type": "MODEL", "name": "Empty"},
                           "properties": {"description": "warn-part"}}]}}
```

  So the WARNING CONDITION is settled, and it is not what the parent report guessed: the document carries a `$ref` to `#/components/schemas/DoesNotExist` and declares no `components` at all, so the warning is about a model reference THE DOCUMENT does not resolve. The `MODEL`/`Empty` documentation part is the PAYLOAD being imported (and the thing the atomicity half asserts is not created), not the warning: `Empty` is a model every API has.
* The condition, the gate and the atomicity are implemented in
  `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/import_documentation_parts.rs`
  (`import_warnings`, sharing the definition importer's reference reader,
  `put_rest_api::model_reference_warnings`), and pinned over a table of documents in
  `tests/all/documentation_part_import_warnings.rs`. The recorded check gets PAST its own
  key (`_error#19`, "expected a ClientError, but the call SUCCEEDED") and onto `run#21`.
* The SENTENCE is all that is left. Twelve wordings have now been tried against the recorded
  unit and rejected. Each was tried inside the lead `"Warnings found during import:\n\t…"`
  this service's other two importers answer with, which is itself recorded:
  1. `Invalid model name specified: Empty`
  2. `Unable to resolve reference 'Empty'. Ignoring.`
  3. `Unable to create documentation part for the specified location: type 'MODEL', name 'Empty'.`
  4. `Unable to create documentation part for MODEL 'Empty'. Ignoring.`
  5. `Unable to resolve model reference 'Empty'. Ignoring.`
  6. `Reference to model 'Empty' not found. Ignoring.`
  7. `Reference to model 'DoesNotExist' not found. Ignoring.` (AWS's own recorded sentence for
     this condition — see below — and the one the tree now answers)
  8. 7 followed by `Invalid format for model application/json for method GET` (AWS's recorded
     SECOND warning for the same condition on the definition importer)
  9. `Unable to resolve reference '#/components/schemas/DoesNotExist'. Ignoring.`
  10. 9 followed by the `Invalid format …` line
  11. `Unable to resolve model reference '#/components/schemas/DoesNotExist'. Ignoring.`
  12. `Reference to model '#/components/schemas/DoesNotExist' not found. Ignoring.`
  13. 7 followed by `Reference to model 'Empty' not found. Ignoring.`
  14. 7 followed by the `Invalid format …` line and `No integration defined for method`
* Where sentence 7 comes from: `issues/things-that-do-not-work/20261029T200808Z-apigateway-api-gateway-tag-vpc-link-stage-and-import.md`
  (cause 10) records AWS answering an `ImportRestApi` of a document with a missing model
  reference with `warnings[0] = "Reference to model '<name>' not found. Ignoring."` and
  `warnings[1] = "Invalid format for model <contentType> for method <HTTPMETHOD>"`. That is the
  only AWS wording for this condition anywhere in this repo, so it is what the tree now answers
  — the model-name sentence from both importers, the format sentence from the definition
  importer only (a documentation import creates no methods, so it loses no format).
* The same unknown blocks a SIBLING check on the other importer:
  `issues/things-that-do-not-work/20261029T193654Z-apigateway-api-gateway-control-plane-is-over-permissive-6.md`
  cause 6 (`apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind`)
  compares `ImportRestApi`'s whole `failOnWarnings` refusal verbatim too. Whoever can read
  EITHER recorded response can settle both: the two refusals are one rendering
  (`put_rest_api::warnings_refusal`).

Fix: the refusal rendering and/or the warning sentence, taken from the recorded response for
`apigateway.import-documentation-parts-bulk-atomic` (`run#21`) or for the sibling
`apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind`
(`run#9`). A fixer who can read either closes this in one edit; guessing it is not a fix, and
fourteen guesses are now on the record as rejected.

Traces: apigateway.import-documentation-parts-bulk-atomic

## Evidence
Replay after the fix: `apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare` PASSES; `apigateway.import-documentation-parts-bulk-atomic` fails with

* `AssertionError: expected the unresolvable model reference to be named in the rejection, got "Warnings found during import:\n\tReference to model 'DoesNotExist' not found. Ignoring."`

and its key is reported as `still_failing` with `now_fails_on` the check's own `run#21` message
assertion — i.e. the import IS refused now, with the atomicity the check asserts next.

## Steps to Reproduce
1. `CreateRestApi`; `ImportDocumentationParts(restApiId, mode="merge", failOnWarnings=true, body=<the document above>)`: expect HTTP 400 `BadRequestException` whose message states the warning as AWS words it, and `GetDocumentationParts` unchanged; with `failOnWarnings=false` the same body must commit and report that warning. Observe the 400 above, whose message differs from the recorded one.
2. Then replay `apigateway.import-documentation-parts-bulk-atomic`.
