Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# CreateWorkload never resolves the lenses it attaches (unknown alias and unpublished custom lens accepted)

_Filed: 2026-10-24T01:59:23Z_
_Operation: CreateWorkload | Class: validation_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkload` resolves `ReviewTemplateArns` against the store and refuses any `ProfileArns` entry, but the `Lenses` list is stored verbatim without ever being resolved to a lens. Consequences:

1. An alias that names NO lens at all (e.g. `"not-a-lens"`, or an ARN of a lens no account owns) is accepted and attached. The workload is then created with a review of a lens that does not exist: `GetWorkload`/`ListWorkloads` report it in `Lenses`, and `GetLensReview`/`ListAnswers` answer 200 for it. Real AWS resolves each `LensSummary$LensAlias` and refuses the create — `CreateWorkload` models `ResourceNotFoundException` (404) for exactly this kind of unresolved resource, alongside the review-template and profile ARNs the handler does resolve.
2. A CUSTOM lens ARN whose import has landed but which has never been published is accepted too, although `ImportLens` documents that "the lens cannot be applied to workloads … until it's published with CreateLensVersion". Nothing in this handler consults `store.lenses` or the published versions of the named lens.

Secondary symptom of the same class: `IndustryType` is documented as "If specified, must be one of the following: Agriculture | Automobile | … | Other" (`WorkloadIndustryType` doc) but any string within the 100-character length bound is stored, so a workload can be created with an industry type the service does not publish.

Expected: a `Lenses` entry that resolves to no AWS-official lens and to no published custom lens is refused (`ResourceNotFoundException`, or `ValidationException` naming `Lenses`) and no workload is created; an out-of-set `IndustryType` is refused with `ValidationException`. Actual: 200 plus a stored workload.

## Evidence

Spec: `CreateWorkloadInput$Lenses` — "The list of lenses associated with the workload. Each lens is identified by its LensSummary$LensAlias"; `LensAlias` doc distinguishes official aliases/ARNs from custom lens ARNs; `ImportLens` doc: "the lens cannot be applied to workloads … until it's published with CreateLensVersion"; `CreateWorkload` models `ResourceNotFoundException` (404). Handler: `src/ops/create_workload.rs` — profile ARNs refused at lines 272-281, review-template ARNs resolved at lines 390-417, but `let mut lenses = input.lenses;` (line 390) is written into `WorkloadRecord.lenses` (line 449) with no lookup in `store.lenses` and no version check anywhere in the file. Test coverage: `tests/all/workload_lifecycle.rs` only ever passes REAL AWS aliases (`wellarchitected`, `serverless`, `softwareasaservice`, lines 30, 64, 321) or an empty list; no test asserts anything about an alias that names no lens or about an unpublished custom lens, so nothing pins the current acceptance.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. Call CreateWorkload with Lenses=["not-a-lens"], a valid WorkloadName/Description/Environment/ReviewOwner and AwsRegions=["us-west-2"]
3. Observe HTTP 200 and a workload id; expected a 404 ResourceNotFoundException (or a 400 ValidationException naming Lenses) and no workload created
4. Call GetWorkload with the returned id and observe `Lenses` containing the bogus alias
5. Repeat with ImportLens (without CreateLensVersion) and pass the returned custom lens ARN as the only `Lenses` entry: the unpublished lens is attached anyway
6. Repeat with IndustryType="Not An Industry" and observe it is stored
