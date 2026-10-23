Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-catalog-list-test-templates
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-catalog-list-test-templates
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|ListTestTemplates|2xx->2xx|$.testTemplates[name="Dependency validation"].description
Issue-Key: resiliencehubv2|ListTestTemplates|2xx->2xx|$.testTemplates[name="Multi-Region
Issue-Commit: 19d5759cd5b8d02c9adaac518282e5ba9a5b7da1

# ListTestTemplates serves paraphrased test-template descriptions instead of the service-owned catalog text

> _Note: this issue was identified against the code base at commit `19d5759cd5b8d02c9adaac518282e5ba9a5b7da1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTestTemplates

## Description
The emulator's built-in ListTestTemplates catalog carries its own paraphrased wording for the `description` member of three service-owned test templates, so the response body diverges from AWS even though the status, the template set and every `name` match.

Cause 1 of 1: ListTestTemplates returns the wrong `description` text for the service-owned templates "Dependency validation", "Multi-Region: isolation" and "Multi-Region: recovery" (fixture resiliencehubv2-catalog-list-test-templates, step task#1).

AWS: recorded behavior only. Smithy (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore): ListTestTemplates is GET /v2/list-test-templates, output ListTestTemplatesResponse.testTemplates (required) of TestTemplateSummary, whose members testTemplateArn (target ServiceOwnedArn), name and description are all @required; the model documents description only as "A description of the test template." and constrains no text. The model therefore does not fix the strings, but the recording does: this is a service-owned catalog (the ARNs are ServiceOwnedArn, i.e. AWS-owned, not account-scoped), so the catalog rows are fixed, reproducible AWS content rather than per-account or per-run generated values. The recording shows HTTP 200 with, for example, "Dependency validation" described as 'Validate how your service responds when its dependencies within the Region are impaired.', "Multi-Region: isolation" as 'Validate your service can operate independently from a Region when connectivity to another Region is unavailable.' and "Multi-Region: recovery" as 'Validate your service can recover and serve clients from another Region within your recovery objectives when a Region is impaired.'

Emulator: answers HTTP 200 with the same template names and list shape, but with independently worded descriptions — 'Validate your service degrades gracefully when a dependency it calls is impaired.', 'Validate your service in one Region keeps serving when another Region it shares resources with is isolated.' and 'Validate your service can recover in another Region after a Regional impairment.' respectively. Expected status 200, actual status 200; only the three `description` values differ.

Why: ListTestTemplates is the discovery call a client makes to present the AWS-owned test catalog to a user or to match a template by its documented wording. A client that renders or snapshot-compares the catalog (console-style listings, golden-file tests, docs generation, or template selection keyed on description text) gets text AWS never returns, so catalog content is not faithful and any client assertion over the catalog fails against the emulator while passing against AWS. Because the catalog is static service-owned data, there is no environment-dependent reason for the difference.

Fix: the seeded catalog data behind the ListTestTemplates handler in the resiliencehubv2 service crate (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore) — the static TestTemplateSummary rows it serves. The descriptions must be carried verbatim from the service-owned catalog rather than re-worded; the handler, serializer and state machinery are otherwise correct (status, list shape, required members and names all match).

No existing open issue covers this service or operation (list_existing_issues for 'resiliencehubv2' returns none), and the divergence reports no stub_path and no owned_by.

Traces: resiliencehubv2-catalog-list-test-templates#task#1 (ListTestTemplates, verdict mismatch, 3 value_mismatch keys under $.testTemplates[*].description).

## Evidence
Fixture resiliencehubv2-catalog-list-test-templates, step task#1, operation ListTestTemplates, verdict=mismatch, expected_status=200, actual_status=200, affected_count=1.

Replay detail (JSON-path keyed):
  $.testTemplates[name="Dependency validation"].description: value_mismatch
    expected: 'Validate how your service responds when its dependencies within the Region are impaired.'
    actual:   'Validate your service degrades gracefully when a dependency it calls is impaired.'
  $.testTemplates[name="Multi-Region: isolation"].description: value_mismatch
    expected: 'Validate your service can operate independently from a Region when connectivity to another Region is unavailable.'
    actual:   'Validate your service in one Region keeps serving when another Region it shares resources with is isolated.'
  $.testTemplates[name="Multi-Region: recovery"].description: value_mismatch
    expected: 'Validate your service can recover and serve clients from another Region within your recovery objectives when a Region is impaired.'
    actual:   'Validate your service can recover in another Region after a Regional impairment.'

Model evidence: Smithy TestTemplateSummary.description is @required (target String); testTemplateArn targets ServiceOwnedArn, confirming the catalog is AWS-owned static content and not account-scoped or generated per run. ListTestTemplates http binding: GET /v2/list-test-templates, success code 200.

## Steps to Reproduce
1. Boot the emulator built from this checkout (resiliencehubv2 is bootstrapped and serving).
2. Replay fixture resiliencehubv2-catalog-list-test-templates, step task#1 — i.e. issue GET /v2/list-test-templates against the resiliencehubv2 endpoint with no request body (ListTestTemplatesRequest has no members).
3. Observe HTTP 200 and the testTemplates list.
4. Compare each element's `description` against the recorded response: the entries named "Dependency validation", "Multi-Region: isolation" and "Multi-Region: recovery" come back with re-worded descriptions, while every `name` and the overall list shape match. The step is reported as a mismatch (steps_mismatched=1 of steps_total=1, score_pct=0.0).
