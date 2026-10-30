Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.empty-resource-listings-are-empty-not-errors
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.empty-resource-listings-are-empty-not-errors|run#22|KeyError|workflows = glue.list_workflows()["Workflows"]
Issue-Commit: 2078ff0f096505f914c344d63960c7f21729c8bc

# Glue ListWorkflows omits the Workflows member entirely when the account holds no workflows (AWS returns an empty list)

> _Note: this issue was identified against the code base at commit `2078ff0f096505f914c344d63960c7f21729c8bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkflows

## Description
Cause 1 of 1: `ListWorkflows` drops its `Workflows` output member instead of reporting an empty list when the account holds no workflows (and likewise for any page that lands past the end of the listing). Failing check: glue.empty-resource-listings-are-empty-not-errors.

AWS:      A Glue collection reader answers an empty collection, not a missing member and not an error. Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): `ListWorkflows` output is `ListWorkflowsResponse` with `Workflows` (target `WorkflowNames`, a list with @length min 1 / max 25) and `NextToken` (GenericString); the operation is @paginated with items=`Workflows`, outputToken=`NextToken`, pageSize=`MaxResults`. The @length min on `WorkflowNames` bounds a NON-EMPTY page's size; it is not an instruction to omit the member. Recorded behavior only for the empty case: the check reads `list_workflows()["Workflows"]` on a fresh account and asserts it is a list, i.e. real ListWorkflows answers HTTP 200 with `Workflows` present and empty. The same check first exercises eleven sibling empty listings against this emulator — get_ml_transforms/Transforms, list_ml_transforms/TransformIds, list_blueprints/Blueprints, list_crawlers/CrawlerNames, get_crawlers/Crawlers, get_classifiers/Classifiers, get_connections/ConnectionList, list_registries/Registries, get_security_configurations/SecurityConfigurations, list_data_quality_rulesets/Rulesets — and every one of them DOES return a present, empty list, so ListWorkflows is the outlier within this very service.

Emulator: The call succeeds with HTTP 200, but the serialized body carries no `Workflows` key at all (and no `NextToken`), so a client that reads the documented member off a successful response gets nothing. The handler builds `workflows: (!page.is_empty()).then_some(page)`, which maps the empty page to `None`, and the comment in the file states that choice explicitly, reasoning from the `@length 1..25` constraint.

Why:     Every client that treats a successful list response as carrying its items member breaks on an account that simply has no workflows: a key lookup raises (botocore `KeyError: 'Workflows'`, as the check shows), typed SDKs surface an unset/optional where AWS gives an empty collection, and "no workflows yet" becomes indistinguishable from a malformed response. This is the first-call path for any bootstrap or inventory flow, and because `Workflows` is the @paginated items member, a paginator walking a region with zero workflows sees no items member on the only page it gets.

Fix:     The `ListWorkflows` handler's output construction — it must always populate the `Workflows` member with the (possibly empty) page, exactly as the sibling name-listing handler in this same crate already does for `TriggerNames`. Nothing about the state read, the cursor arithmetic or the `NextToken` logic needs to change; only the member gating.

Traces:  glue.empty-resource-listings-are-empty-not-errors (verdict: error). Handler and the divergent line: services/service-com-amazonaws-glue-awsglue/src/ops/list_workflows.rs lines 211-217 (the `(!page.is_empty()).then_some(page)` gate and the comment justifying it). Correct sibling for comparison: services/service-com-amazonaws-glue-awsglue/src/ops/list_triggers.rs lines 249-252 (`trigger_names: Some(page)`, unconditional).

## Evidence
Check: glue.empty-resource-listings-are-empty-not-errors — verdict `error`.

Expected (AWS): `ListWorkflows` on an account with no workflows answers HTTP 200 with `Workflows` present and empty, i.e. `{"Workflows": []}`; `NextToken` is absent because nothing remains. No error code, no error message — this is a success path.

Actual (emulator): HTTP 200 with the `Workflows` member omitted from the body. The check fails at:

    File "check_glue_empty_resource_listings_are_empty_not_errors.py", line 42, in run
    workflows = glue.list_workflows()["Workflows"]
    KeyError: 'Workflows'

The failure is a missing member on a 200, not a status/code/message divergence, so there is no error wording at issue here.

Contrast inside the same check and the same service: the eleven empty listings called BEFORE this line all returned their items member present and empty against this emulator (Transforms, TransformIds, Blueprints, CrawlerNames, Crawlers, Classifiers, ConnectionList, Registries, SecurityConfigurations, Rulesets), which is why the check reached line 42 at all.

Source of the divergence (services/service-com-amazonaws-glue-awsglue/src/ops/list_workflows.rs, lines 211-217):

    Ok(ListWorkflowsOutput {
        // `Workflows` is `WorkflowNames`, "len 1..25", so the model admits no empty
        // list: an account holding no workflow, and a page past the end of the
        // listing, report no `Workflows` member at all rather than an empty one.
        workflows: (!page.is_empty()).then_some(page),
        next_token,
    })

The sibling name listing in the same crate does it the AWS way (services/service-com-amazonaws-glue-awsglue/src/ops/list_triggers.rs, lines 249-252): `trigger_names: Some(page)` — unconditional, empty page included.

## Steps to Reproduce
1. Boot the emulator from this checkout (the behavioral suite's standard bring-up).
2. Use a region/account namespace that holds NO Glue workflows (a fresh emulator state satisfies this; otherwise delete every workflow first, or request a page past the end of the listing with a `NextToken` that names the last stored workflow name).
3. Call Glue `ListWorkflows` with no parameters, e.g. `aws glue list-workflows` or botocore `glue.list_workflows()`.
4. Observe the response: HTTP 200 whose body has no `Workflows` key. Reading the documented member raises `KeyError: 'Workflows'` in botocore. AWS returns `Workflows` present and empty (`[]`) instead.
5. For the contrast, call `ListTriggers` in the same empty namespace: `TriggerNames` comes back present and empty, which is the behavior `ListWorkflows` must match.
6. Full replay: run the behavioral suite and re-run check `glue.empty-resource-listings-are-empty-not-errors`; it currently dies at `glue.list_workflows()["Workflows"]` and must proceed past it to the remaining listings (list_triggers, list_sessions, list_usage_profiles, list_data_quality_results, list_data_quality_rule_recommendation_runs, list_data_quality_ruleset_evaluation_runs, get_catalogs, get_data_catalog_encryption_settings, batch_get_blueprints).
