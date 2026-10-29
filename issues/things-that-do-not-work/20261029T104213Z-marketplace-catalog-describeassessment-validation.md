Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6bb9010ea97ecc711aeaca7897e143f375b8cacc

# AssessmentIdentifier's modelled @length(1..2048) is never enforced, so an over-long identifier answers 404 instead of a ValidationException

_Filed: 2026-10-29T10:42:13Z_
_Operation: DescribeAssessment | Class: validation_

> _Note: this issue was identified against the code base at commit `6bb9010ea97ecc711aeaca7897e143f375b8cacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeAssessmentRequest$AssessmentIdentifier` carries `skip_validation`, so the boundary validator does NOT apply the modelled constraints to it, and the handler is named as the replacement check. That replacement — `crate::state::assessment_id_of` / `is_assessment_id` — spells out only the SHAPE of the two accepted forms and places no bound on the length, while the modelled `AssessmentIdentifier` shape is `@length(min: 1, max: 2048)` as well as `@pattern`.

Consequence: an identifier of, say, 5 000 characters (`assessment-` followed by alphanumerics) is accepted as well formed, reaches the store lookup and is answered 404 `ResourceNotFoundException` — the answer this handler reserves for a WELL-FORMED identifier that names nothing — instead of the 422 `ValidationException` the handler's own comment says a malformed identifier gets ("a malformed identifier is a `ValidationException`, an unknown one a `ResourceNotFoundException`, and the two must not be confused").

This is also the crate contradicting itself: every other `skip_validation` member's hand-written predicate folds the modelled length into the published syntax it checks (`is_change_set_name`'s `{1,100}`, `is_client_request_token`'s `{1,64}`, `is_catalog_name`'s `{1,47}`, `is_change_name`'s `{1,72}`), and each reports that syntax in the refusal; `is_assessment_id` is the one that does not — so the pattern it quotes back to the caller (`ASSESSMENT_IDENTIFIER_PATTERN`) is stricter than what it actually enforces on length.

## Evidence

Model (smithy_query query="operation", operation="DescribeAssessment"): `"AssessmentIdentifier": {"target": "AssessmentIdentifier", "required": true, "length": {"min": 1, "max": 2048}, "pattern": "^(assessment-[a-zA-Z0-9]+|arn:[a-zA-Z0-9-]+:[a-zA-Z0-9-]+:[a-zA-Z0-9-]*:[0-9]*:[a-zA-Z0-9-]+/Assessment/assessment-[a-zA-Z0-9]+)$"}`.

The boundary is switched off for the member and the handler named as the replacement — `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/types/describe_assessment_types.rs:14-21`:
```
// `skip_validation` names `Catalog` and `NextToken` (see `ListAssessmentsInput`) plus
// `AssessmentIdentifier`, whose two accepted forms are narrower in shape and wider in
// namespace than the modelled `@pattern`, and whose fault quotes the syntax this service
// publishes — so the HANDLER judges it.
#[smithy_macros::smithy(
    shape = "com.amazonaws.marketplacecatalog#DescribeAssessmentRequest",
    skip_validation = ["Catalog", "AssessmentIdentifier", "NextToken"]
)]
```

The handler's only check — `src/ops/describe_assessment.rs:202-214`:
```
        // "You can provide either the assessment ID … or the full assessment ARN".
        // An identifier of neither form names no assessment at all, so it is
        // refused BEFORE the store is read — a malformed identifier is a
        // `ValidationException`, an unknown one a `ResourceNotFoundException`, and
        // the two must not be confused.
        let Some(assessment_id) =
            crate::state::assessment_id_of(&input.assessment_identifier, &input.catalog)
```
and the predicate it resolves through, with no length bound — `src/state.rs:2666-2671`:
```
pub fn is_assessment_id(value: &str) -> bool {
    match value.strip_prefix(ASSESSMENT_ID_PREFIX) {
        Some(tail) => !tail.is_empty() && tail.chars().all(|c| c.is_ascii_alphanumeric()),
        None => false,
    }
}
```
(the ARN alternative ends in the same predicate — `src/state.rs:2707-2708`: `let id = resource.strip_prefix(&format!("{catalog}/{ASSESSMENT_ARN_RESOURCE_TYPE}/"))?; is_assessment_id(id).then_some(id)`).

The sibling predicates for every other `skip_validation` member DO fold the modelled length in:
- `src/state.rs:3409-3411`: `pub fn is_change_set_name(value: &str) -> bool { (1..=CHANGE_SET_NAME_MAX).contains(&value.chars().count()) && …`
- `src/state.rs:3433-3438`: `const CLIENT_REQUEST_TOKEN_MAX: usize = 64;` … `pub fn is_client_request_token(value: &str) -> bool {`
- `src/state.rs:3110`: `if value.is_empty() || value.chars().count() > CATALOG_NAME_MAX { return false; }` (`is_catalog_name`)

Test coverage: `tests/all/assessments.rs:164-169` pins only a 211-character id — `("a long id", format!("assessment-{}", "a".repeat(200)), 404, "ResourceNotFoundException")` — i.e. well INSIDE the modelled 2048, so nothing pins an identifier past the bound.

## Steps to Reproduce

1. POST /DescribeAssessment with {"Catalog":"AWSMarketplace","AssessmentIdentifier":"assessment-" + "a"*5000, "MaxResults":50}.
2. Observe HTTP 404 ResourceNotFoundException ("Requested assessment '…' does not exist."), i.e. the identifier was accepted as well formed although it is 5011 characters against the member's modelled `@length(min: 1, max: 2048)`.
3. Expected: the 422 ValidationException this handler answers for an identifier outside the syntax it publishes, as the handler comment at describe_assessment.rs:202-206 requires ("a malformed identifier is a ValidationException, an unknown one a ResourceNotFoundException, and the two must not be confused").
4. Compare with an over-long ChangeSetName or ClientRequestToken on StartChangeSet (tests/all/request_validation.rs:345-390), which the crate's own predicates refuse with 422.
