Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad

# Reserved `aws:`-prefixed tag keys are accepted and stored on the deployment parameter

_Filed: 2026-10-24T09:06:59Z_
_Operation: PutDeploymentParameter | Class: validation_

> _Note: this issue was identified against the code base at commit `c9f3f250c6ee93c7fb0ade0d9c6dfd55a63e86ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutDeploymentParameter$tags` (and, through the same shared merge, `TagResource$tags`) accepts tag keys that begin with the AWS-reserved `aws:` prefix, stores them on the record, reports them back in the `PutDeploymentParameter` response `tags` and lists them from `ListTagsForResource`.

Expected (well-established AWS tagging behavior, and specifically the behavior of the resource a deployment parameter materialises into — a Secrets Manager secret, whose tag rules the service inherits): a tagging request carrying a key in the reserved `aws:` namespace is refused with a 400-class input error (`ValidationException` for this service, which models it with `httpError 400` and `fieldName`), and nothing is stored. The `aws:` prefix is reserved for AWS-generated tags; a caller may not create, edit or delete one.

Actual: the request succeeds. `PutDeploymentParameter` answers HTTP 200 with the `aws:`-prefixed key echoed in the response `tags`, and the key is then permanently visible through `ListTagsForResource` — a tag state a real account can never reach. The emulator's only tag-key judgement is the modelled `TagsMap` character/length check, and `:` is inside `^[a-zA-Z0-9/_+=.:@-]+$`, so a reserved key sails through.

Note this is the opposite direction from the already-open `TagResource/validation` issue (which is about the emulator refusing keys the model leaves unconstrained): the reserved-prefix rule is not a character-class rule, it is a namespace AWS owns.

## Evidence

Model: `smithy_query(query="shape",shape="TagsMap")` — keys target `TagKey` with pattern `^[a-zA-Z0-9/_+=.:@-]+$`, so `aws:foo` is pattern-legal and the modelled constraint walk cannot refuse it; `ValidationException` (httpError 400, members `message` + required `fieldName`) is declared on `PutDeploymentParameter` and on all three tagging operations. Well-established AWS behavior: the `aws:` tag-key prefix is reserved AWS-wide (AWS tagging guidance; Secrets Manager — the resource a deployment parameter's secret lands in — documents that you cannot create tag keys beginning with `aws:`).

Handler: services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/ops/put_deployment_parameter.rs lines 337-347 call `crate::state::merge_tags(&BTreeMap::new(), &input.tags…)` on the create path; src/state.rs `merge_tags` (lines ~539-574) + `is_tag_key`/`is_tag_value` (lines ~517-525) check only length and `TAG_CHARACTER_CLASS` and the 50-entry cap — there is no prefix check anywhere in the crate (`grep -i "aws:"` over src/ finds no reserved-prefix handling).

Tests: no test in tests/all/ exercises a reserved tag key. tests/all/deployment_parameters.rs `a_tag_write_the_service_could_not_report_is_refused_and_stores_nothing` and tests/all/validation_messages.rs `the_tag_character_class_this_service_states_carries_no_anchors` cover only character-class/length/cap refusals, so this is uncovered rather than deliberately asserted.

## Steps to Reproduce

1. Seed an agreement for (catalog=AWSMarketplace, productId=prod-x, agreementId=agmt-x) — the out-of-band provisioning seam the service requires.
2. POST /catalogs/AWSMarketplace/products/prod-x/deployment-parameters with body {"agreementId":"agmt-x","deploymentParameter":{"name":"Reserved","secretString":"s"},"tags":{"aws:cloudformation:stack-name":"mystack"}}
3. Observe HTTP 200 and the response `tags` echoing the `aws:`-prefixed key, where AWS refuses the request with a 400-class ValidationException naming `tags` and stores nothing.
4. GET /tags/{percent-encoded resourceArn} and observe the reserved key is persisted and listed.
5. Repeat the same reserved key against POST /tags/{resourceArn} (TagResource) and observe HTTP 204 with the key stored, for the same missing check in the shared `merge_tags`.
