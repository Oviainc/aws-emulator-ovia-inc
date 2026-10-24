Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# CreateReviewTemplate stores its Lenses list without resolving any lens

_Filed: 2026-10-24T01:59:23Z_
_Operation: CreateReviewTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateReviewTemplate` validates the lens-list SIZE against an emulator quota and then stores `input.lenses` verbatim; no entry is ever resolved to a lens. A `Lenses` entry that names no AWS-official lens and no stored custom lens (e.g. `"not-a-lens"`), or a custom lens ARN that was imported but never published with `CreateLensVersion`, is accepted and recorded on the template.

That propagates: `CreateWorkload` applies a stored template's lenses to the new workload (`create_workload.rs` lines 390-417), so a bogus alias recorded here is attached to real workloads later, and `GetReviewTemplate`/`ListReviewTemplateAnswers` report a lens review for a lens that does not exist.

Expected: an unresolvable `Lenses` entry is refused — `CreateReviewTemplate` models `ResourceNotFoundException` (404) for a named resource that cannot be resolved (and `ValidationException` (400) for a bad member value) — and no template is stored. Actual: HTTP 200 with a `TemplateArn` and the bogus lens persisted.

## Evidence

Spec: `CreateReviewTemplate` input `Lenses` targets `ReviewTemplateLenses`, a list of `LensAlias` (`smithy_query(query="shape", shape="ReviewTemplateLenses")`); the operation models `ResourceNotFoundException` (404) and `ValidationException` (400) (`smithy_query(query="operation", operation="CreateReviewTemplate")`); `ImportLens` doc: a custom lens cannot be applied until published with `CreateLensVersion`. Handler: `src/ops/create_review_template.rs` — only `input.lenses.len()` is checked (lines 187-201) and the list is written straight into `ReviewTemplateRecord.lenses` (line 279); the file never touches `store.lenses`. Test coverage: `tests/all/review_template_lifecycle.rs` passes only real aliases (`serverless`, `softwareasaservice`, lines 159-162, 351, 496-527); no test covers an alias that resolves to nothing or an unpublished custom lens.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. Call CreateReviewTemplate with TemplateName/Description set and Lenses=["not-a-lens"]
3. Observe HTTP 200 with a TemplateArn; expected a 404 ResourceNotFoundException (or 400 ValidationException naming Lenses) and no template stored
4. Call GetReviewTemplate with that ARN and observe the bogus alias in `Lenses`
5. Call CreateWorkload with ReviewTemplateArns=[that ARN] and observe the bogus lens attached to the new workload
