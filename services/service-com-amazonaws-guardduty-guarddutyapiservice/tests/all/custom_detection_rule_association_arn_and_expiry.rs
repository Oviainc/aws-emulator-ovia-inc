// SPDX-License-Identifier: CC0-1.0

//! The ARN, the expiry and the one-association-per-rule rule of a custom detection
//! rule association, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A published rule's ARN is the AWS-MANAGED one — `detection-rule/custom/<ruleId>`
//!    with an EMPTY Region segment and the account segment `aws`, because the rule
//!    belongs to GuardDuty and not to the account reading it — while an association's
//!    ARN is the ACCOUNT's: it extends the same resource path with
//!    `/association/<associationId>` and carries the Region and account that own it.
//!    Both are the one `DetectionRuleArn`
//!    (`smithy_query(shape="RuleSummary,AssociationDetail")`) every operation that
//!    reports either resource derives, so the create, the detail read and both
//!    listings agree. Asserted over every published rule, in both modes and through
//!    two Regions.
//! 2. `ExpiresAt` is "the timestamp when the association expires"
//!    (`smithy_query(shape="AssociationDetail")`): an association in the mode that
//!    evaluates a rule without acting on it expires a fixed interval after it enters
//!    that mode, and one in the mode that keeps evaluating reports no expiry at all.
//!    The create, the detail read and the listing report the same instant, and an
//!    update that moves the mode moves the expiry with it.
//! 3. A rule carries at most ONE association per account and Region, so a second
//!    create naming a rule the account already associated is refused with the modelled
//!    409 `ConflictException` whatever mode either call names, where a rule the
//!    catalogue does not publish is refused with the modelled 404
//!    `ResourceNotFoundException`
//!    (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`); neither
//!    refusal changes what a later read reports.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, error_shape, new_router, Router};

/// `CreateCustomDetectionRuleAssociation` and `ListCustomDetectionRuleAssociations`
/// are both bound at `/custom-detection-rule/association` (POST and GET)
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,ListCustomDetectionRuleAssociations")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`,
/// `GetCustomDetectionRule` is `GET /custom-detection-rule/rule/{RuleId}` and the
/// association reads sit under
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="ListCustomDetectionRules,GetCustomDetectionRule,GetCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The member of [`MODES`] that evaluates a rule without acting on its findings, and
/// so is the mode an association expires out of.
const DRY_RUN: &str = "DRY_RUN";

/// Two Regions, so the Region an ARN names is asserted rather than assumed.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// How long after it enters [`DRY_RUN`] an association expires: fourteen days, in the
/// seconds a restJson1 timestamp is carried as.
const DRY_RUN_LIFETIME_SECONDS: f64 = 14.0 * 24.0 * 60.0 * 60.0;

/// A timestamp is serialized as epoch seconds with milliseconds after the point, so
/// two instants derived from the same millisecond compare within half a millisecond.
const INSTANT_TOLERANCE_SECONDS: f64 = 0.0005;

/// A well-formed `RuleId` (`^[a-z0-9]+(-[a-z0-9]+)*$`, `len 1..100`) the catalogue
/// does not publish, so no association can ever name it.
const ABSENT_RULE_ID: &str = "no-such-published-rule";

/// One published GuardDuty custom detection rule id, asserted to be in the catalogue
/// so the association surface is exercised over a rule GuardDuty itself publishes and
/// not only over whatever the catalogue happens to hold.
const PUBLISHED_RULE_ID: &str = "admin-policy-attached-to-user";

/// The ids the catalogue publishes, read over the wire so the test cannot drift from
/// it.
async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call_in(
        router,
        REGIONS[0],
        "POST",
        RULES_PATH,
        Some(json!({"maxResults": 100})),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules: expected 200, got {status} {body:?}"
    );
    let ids: Vec<String> = body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|rule| rule["ruleId"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.contains(&PUBLISHED_RULE_ID.to_string()),
        "the catalogue must publish {PUBLISHED_RULE_ID:?}, got {ids:?}"
    );
    ids
}

/// The `arn` each published rule is listed under through `region`'s endpoint, paired
/// with its `ruleId`.
async fn listed_rule_arns(router: &Router, region: &str) -> Vec<(String, String)> {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        RULES_PATH,
        Some(json!({"maxResults": 100})),
    )
    .await;
    assert_eq!(
        status, 200,
        "list-custom-detection-rules in {region}: expected 200, got {status} {body:?}"
    );
    body["rules"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            (
                rule["ruleId"].as_str().unwrap_or_default().to_string(),
                rule["arn"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create an association for `rule_id` in `mode` through `region`'s endpoint.
async fn create_association(
    router: &Router,
    region: &str,
    rule_id: &str,
    mode: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        ASSOCIATIONS_PATH,
        Some(json!({"ruleId": rule_id, "mode": mode})),
    )
    .await
}

/// Create an association that must succeed, returning the `AssociationDetail` the
/// create reported.
async fn associate(router: &Router, region: &str, rule_id: &str, mode: &str) -> Value {
    let (status, body) = create_association(router, region, rule_id, mode).await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"].clone()
}

/// The `AssociationDetail` `GetCustomDetectionRuleAssociation` reports.
async fn read_association(router: &Router, region: &str, rule_id: &str, id: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{RULES_PATH}/{rule_id}/association/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-association {rule_id}/{id} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"].clone()
}

/// The `AssociationSummary` the listing reports for `rule_id`, through `region`'s
/// endpoint.
async fn listed_association(router: &Router, region: &str, rule_id: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{ASSOCIATIONS_PATH}?maxResults=100&ruleId={rule_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-associations {rule_id} in {region}: expected 200, got {status} {body:?}"
    );
    let summaries = body["ruleAssociations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "list-associations {rule_id} in {region}: expected the rule's one association, got {body:?}"
    );
    summaries[0].clone()
}

/// The string member `member` of `value`, failing the test when it is absent.
fn text<'a>(value: &'a Value, member: &str, case: &str) -> &'a str {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {value:?}"))
}

/// The timestamp member `member` of `value`, failing the test when it is absent.
fn instant(value: &Value, member: &str, case: &str) -> f64 {
    value[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} instant, got {value:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1: the ARN of a rule, and of the association hanging off it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_rule_and_association_arn_carries_the_custom_rule_path() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;
    let account = ::engine::emulator_account_id();

    for region in REGIONS {
        let listed = listed_rule_arns(&router, region).await;
        assert_eq!(
            listed.len(),
            ids.len(),
            "case {region:?}: every Region lists the same catalogue, got {listed:?}"
        );

        for (index, (rule_id, rule_arn)) in listed.iter().enumerate() {
            let case = format!("{rule_id} in {region}");
            // The published rule's own ARN: the resource type and the `custom`
            // family it belongs to, then the rule's id — under the AWS-managed
            // empty-Region, `aws`-account form, because the rule is GuardDuty's and
            // not the account's.
            let expected_rule_arn =
                format!("arn:aws:guardduty::aws:detection-rule/custom/{rule_id}");
            assert_eq!(
                rule_arn, &expected_rule_arn,
                "case {case:?}: expected the rule ARN {expected_rule_arn:?}, got {rule_arn:?}"
            );

            // The detail read reports the listed ARN.
            let (status, detail) =
                call_in(&router, region, "GET", &format!("{RULES_PATH}/{rule_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-custom-detection-rule expected 200, got {status} {detail:?}"
            );
            assert_eq!(
                text(&detail["rule"], "arn", &case),
                expected_rule_arn,
                "case {case:?}: the detail read must report the listed ARN, got {detail:?}"
            );

            // The association's ARN extends its rule's resource path, whichever
            // mode it is in — but in the ACCOUNT's own namespace, because the
            // association is a resource this account created in this Region.
            let mode = MODES[index % MODES.len()];
            let created = associate(&router, region, rule_id, mode).await;
            let association_id = text(&created, "associationId", &case).to_string();
            let expected_association_arn = format!(
                "arn:aws:guardduty:{region}:{account}:detection-rule/custom/{rule_id}/association/{association_id}"
            );
            assert_eq!(
                text(&created, "arn", &case),
                expected_association_arn,
                "case {case:?}: expected the association ARN {expected_association_arn:?}, got {created:?}"
            );

            let read = read_association(&router, region, rule_id, &association_id).await;
            assert_eq!(
                read, created,
                "case {case:?}: the read must report what the create minted, got {read:?}"
            );
            let summary = listed_association(&router, region, rule_id).await;
            assert_eq!(
                text(&summary, "arn", &case),
                expected_association_arn,
                "case {case:?}: the listing must report the same ARN, got {summary:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the expiry belongs to the mode the association is in.
// ---------------------------------------------------------------------------

/// What `ExpiresAt` must be for an association in `mode` that entered it at
/// `since`: a fixed interval later for the mode that expires, and absent for the one
/// that does not.
fn assert_expiry(association: &Value, mode: &str, since: f64, case: &str) {
    let expires_at = association.get("expiresAt").and_then(Value::as_f64);
    if mode == DRY_RUN {
        let expected = since + DRY_RUN_LIFETIME_SECONDS;
        let actual = expires_at.unwrap_or_else(|| {
            panic!("case {case:?}: a {DRY_RUN} association must carry expiresAt, got {association:?}")
        });
        assert!(
            (actual - expected).abs() < INSTANT_TOLERANCE_SECONDS,
            "case {case:?}: expected expiresAt {expected}, got {actual}"
        );
    } else {
        assert!(
            expires_at.is_none(),
            "case {case:?}: an association in {mode} must report no expiresAt, got {association:?}"
        );
    }
}

#[tokio::test]
async fn the_expiry_follows_the_mode_the_association_is_in() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;

    // Each row: a published rule, the mode it is created in, and the mode an update
    // then moves it to — so the table spans a create and an update in both
    // directions, and a mode left where it was.
    let rows: Vec<(String, &str, &str)> = ids
        .iter()
        .take(4)
        .enumerate()
        .flat_map(|(index, rule_id)| {
            let from = MODES[index % MODES.len()];
            MODES.map(|to| (rule_id.clone(), from, to))
        })
        .collect();

    for (rule_id, from, to) in rows {
        let case = format!("{rule_id}: created {from}, updated to {to}");
        let router = new_router();

        let created = associate(&router, REGIONS[0], &rule_id, from).await;
        let created_at = instant(&created, "createdAt", &case);
        assert_expiry(&created, from, created_at, &case);
        let association_id = text(&created, "associationId", &case).to_string();

        // The detail read and the listing report the same expiry as the create.
        let read = read_association(&router, REGIONS[0], &rule_id, &association_id).await;
        assert_eq!(
            read.get("expiresAt"),
            created.get("expiresAt"),
            "case {case:?}: the read must report the create's expiresAt, got {read:?}"
        );
        let summary = listed_association(&router, REGIONS[0], &rule_id).await;
        assert_eq!(
            summary.get("expiresAt"),
            created.get("expiresAt"),
            "case {case:?}: the listing must report the create's expiresAt, got {summary:?}"
        );

        let (status, body) = call_in(
            &router,
            REGIONS[0],
            "PUT",
            &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
            Some(json!({"mode": to})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: update-association expected 200, got {status} {body:?}"
        );

        let moved = read_association(&router, REGIONS[0], &rule_id, &association_id).await;
        assert_eq!(
            text(&moved, "mode", &case),
            to,
            "case {case:?}: the update must set the mode, got {moved:?}"
        );
        // The expiry is counted from the instant the association entered the mode it
        // is now in: the create instant where nothing moved, the update instant
        // otherwise.
        let entered = if from == to {
            created_at
        } else {
            instant(&moved, "updatedAt", &case)
        };
        assert_expiry(&moved, to, entered, &case);
        assert_eq!(
            instant(&moved, "createdAt", &case),
            created_at,
            "case {case:?}: an update must not move createdAt, got {moved:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one association per rule — the conflict, and the absent rule.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_create_for_an_associated_rule_is_refused_as_a_conflict() {
    let router = new_router();
    let ids = published_rule_ids(&router).await;

    // Every (held mode, requested mode) combination over several published rules,
    // including the rule GuardDuty publishes that the catalogue is asserted to carry.
    let mut rules: Vec<String> = ids.iter().take(3).cloned().collect();
    rules.push(PUBLISHED_RULE_ID.to_string());

    for rule_id in &rules {
        for held in MODES {
            for requested in MODES {
                let case = format!("{rule_id}: associated {held}, create again as {requested}");
                let router = new_router();
                let created = associate(&router, REGIONS[0], rule_id, held).await;

                let (status, body) =
                    create_association(&router, REGIONS[0], rule_id, requested).await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected the modelled 409, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ConflictException",
                    "case {case:?}: expected the ConflictException class, got {body:?}"
                );
                assert_eq!(
                    error_shape(&body),
                    "ConflictException",
                    "case {case:?}: expected the ConflictException shape, got {body:?}"
                );
                assert_eq!(
                    body["type"].as_str(),
                    Some("ConflictException"),
                    "case {case:?}: expected the class in the body's type member, got {body:?}"
                );
                assert_eq!(
                    body["message"].as_str(),
                    Some(format!("Association already exists for rule {rule_id}").as_str()),
                    "case {case:?}: expected the conflict text naming the rule, got {body:?}"
                );

                // A rule the catalogue does not publish is the OTHER refusal, so the
                // two are never answered for one another.
                let (status, absent) =
                    create_association(&router, REGIONS[0], ABSENT_RULE_ID, requested).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: an unpublished rule expects the modelled 404, got {status} {absent:?}"
                );
                assert_eq!(
                    error_code(&absent),
                    "ResourceNotFoundException",
                    "case {case:?}: expected the ResourceNotFoundException class, got {absent:?}"
                );
                assert_eq!(
                    absent["message"].as_str(),
                    Some(format!("Rule not found: {ABSENT_RULE_ID}").as_str()),
                    "case {case:?}: expected the not-found text naming the rule, got {absent:?}"
                );

                // Neither refusal changed anything: the one association the rule
                // carries still reports what the create minted.
                let still = read_association(
                    &router,
                    REGIONS[0],
                    rule_id,
                    text(&created, "associationId", &case),
                )
                .await;
                assert_eq!(
                    still, created,
                    "case {case:?}: a refused create must leave the association alone, got {still:?}"
                );
                let summary = listed_association(&router, REGIONS[0], rule_id).await;
                assert_eq!(
                    text(&summary, "mode", &case),
                    held,
                    "case {case:?}: the rule must still carry its one association, got {summary:?}"
                );
            }
        }
    }

    // Another Region's endpoint holds its own associations, so a rule associated here
    // is still free there.
    let rule_id = &rules[0];
    let router = new_router();
    associate(&router, REGIONS[0], rule_id, MODES[0]).await;
    let elsewhere = associate(&router, REGIONS[1], rule_id, MODES[0]).await;
    assert!(
        text(&elsewhere, "arn", "across Regions").contains(REGIONS[1]),
        "the other Region's association must name its own Region, got {elsewhere:?}"
    );
}
