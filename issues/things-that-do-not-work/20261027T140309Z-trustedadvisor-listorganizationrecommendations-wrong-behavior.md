Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f39310ffb44e85959dc0d40e62557237e3e92192

# Organization recommendation reads report `standard` recommendations although every organization operation is documented as prioritized-only

_Filed: 2026-10-27T14:03:08Z_
_Operation: ListOrganizationRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f39310ffb44e85959dc0d40e62557237e3e92192`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole organization-level read surface of this crate serves organization recommendations whose `type` is `standard`, while the model documents each of those operations as supporting only prioritized recommendations.

The root is the seeded store: `ComAmazonawsTrustedadvisorTrustedadvisorState::default` creates ONE organization recommendation per `SEED` row and copies that row's `recommendation_type` into it, so the two `standard` rows (`amazon-ebs-snapshots`, `iam-access-key-rotation`) become `standard` organization recommendations. Consequently:

* `ListOrganizationRecommendations` reports them (and `?type=standard` selects them);
* `GetOrganizationRecommendation` resolves them and answers 200;
* `ListOrganizationRecommendationResources` and `ListOrganizationRecommendationAccounts` serve their resources/accounts — neither handler looks at the entry's `type` at all, they only check existence.

The crate already implements this exact documented rule once: `UpdateOrganizationRecommendationLifecycle` refuses a non-`priority` organization recommendation, and its own comment says the rule comes from the shared "only supports prioritized recommendations" sentence. So the identical model sentence is enforced on the one organization write path and ignored on all four organization read paths, against state the seed manufactures specifically to be read.

## Evidence

Model, `smithy_query(query="doc",shape="ListOrganizationRecommendations")`:
"List a filterable set of Recommendations within an Organization. This API only supports prioritized recommendations and provides global priority recommendations, eliminating the need to call the API in each AWS Region."

The same sentence on the other three organization reads:
* `GetOrganizationRecommendation`: "Get a specific recommendation within an AWS Organizations organization. This API supports only prioritized recommendations and provides global priority recommendations..."
* `ListOrganizationRecommendationResources`: "List Resources of a Recommendation within an Organization. This API only supports prioritized recommendations..."
* `ListOrganizationRecommendationAccounts`: "Lists the accounts that own the resources for an organization aggregate recommendation. This API only supports prioritized recommendations..."

The seed makes `standard` organization recommendations — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/state.rs:1592-1600:
```
            state.organization_recommendations.insert(
                organization_arn.clone(),
                OrganizationRecommendationEntry {
                    id: organization_recommendation_id,
                    arn: organization_arn,
                    text,
                    check_arn: Some(check_arn),
                    r#type: row.recommendation_type.to_string(),
```
with `recommendation_type: "standard"` on two rows (state.rs:1271 `amazon-ebs-snapshots`, state.rs:1378 `iam-access-key-rotation`).

The read handlers apply no type rule — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendations.rs:243-266 filters only on `type`/`status`/`pillar`/`awsService`/`source`/`checkIdentifier`/timestamps supplied by the caller; services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_organization_recommendation_accounts.rs:227-238 checks only existence:
```
        if !state.organization_recommendations.contains_key(identifier) {
            return Err(
                ListOrganizationRecommendationAccountsErrorModeled::ResourceNotFoundException(
```
(same shape at list_organization_recommendation_resources.rs:240-251 and get_organization_recommendation.rs:222-233).

The crate's own enforcement of the same sentence — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/update_organization_recommendation_lifecycle.rs:94-98 and 240-249:
```
// The lifecycle stage that resolves a recommendation, and the recommendation
// type whose lifecycle this operation can move — both shared with the
// account-level `UpdateRecommendationLifecycle`, which models the same enums
// and carries the same "only supports prioritized recommendations" sentence.
```
```
        if entry.r#type != RECOMMENDATION_TYPE_PRIORITY {
            return Err(
                UpdateOrganizationRecommendationLifecycleErrorModeled::ConflictException(
                    ConflictException {
                        message: format!(
                            "the lifecycle of the {} organization recommendation {} cannot be \
                             updated: this operation only supports prioritized recommendations",
```

Test coverage of the current behaviour: tests/all/organization_recommendations.rs:165 pins `("?type=standard", STANDARD_CHECKS)` and :163 pins the unfiltered list as all four `CHECK_IDS`; tests/all/organization_recommendation_accounts.rs:91-93 reads the accounts of "A `standard` check". Both pin it for mechanical reasons ("the list reports exactly the stored organization recommendations its filters keep", "so a row can assert the untouched case beside the grown one"), neither states a reason the organization surface should hold `standard` entries.

## Steps to Reproduce

1. Boot the emulator on an entitled support plan (EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business).
2. GET /v1/organization-recommendations?type=standard — two organization recommendations are reported (`.../organization-recommendation/amazon-ebs-snapshots-organization-recommendation` and `.../iam-access-key-rotation-organization-recommendation`), although the operation is documented as only supporting prioritized recommendations.
3. GET /v1/organization-recommendations/<that standard ARN> — 200 with `organizationRecommendation.type == "standard"`.
4. GET /v1/organization-recommendations/<that standard ARN>/accounts and /resources — 200 with rows, no type rule applied.
5. PUT /v1/organization-recommendations/<that same standard ARN>/lifecycle — 409 ConflictException "this operation only supports prioritized recommendations", i.e. the one handler that reads the shared doc sentence refuses the very entry the four read handlers serve.
