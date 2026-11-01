// SPDX-License-Identifier: CC0-1.0

//! `UpdateCustomDetectionRuleAssociation`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. An update SETS the mode of the association the two URI labels identify —
//!    "updates the mode of an existing custom detection rule association"
//!    (`smithy_query(query="doc",shape="UpdateCustomDetectionRuleAssociation")`) — and
//!    `GetCustomDetectionRuleAssociation` then reports the new mode while the minted
//!    `AssociationId`, the `Arn`, the `AccountId`, the `CreatedAt` and the tags the
//!    create recorded all stand; the association is reported by its new mode and by
//!    no other.
//! 2. A change moves `UpdatedAt` past the create instant; an update asking for the
//!    mode the association is already in changes nothing, which is what `@idempotent`
//!    on this PUT (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")`)
//!    means observably.
//! 3. A rule carries at most ONE association, so a second create for a rule the
//!    account already associated is refused with the modelled 409
//!    `ConflictException` whatever mode it names, while an update moves that one
//!    association between the modes freely.
//! 4. An `(AssociationId, RuleId)` pair the account holds no association for —
//!    an id never minted, a real id under another published rule, or an association
//!    made through another Region's endpoint — is refused with the modelled 404
//!    `ResourceNotFoundException`, never a hollow success and never an upsert.
//! 5. A `mode` outside the `AssociationMode` enum is refused by the modelled
//!    constraint with HTTP 400, and no refusal changes what a later read reports.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call_in, error_code, error_shape, new_router, published_rule_ids, Router,
};

/// `CreateCustomDetectionRuleAssociation` and `ListCustomDetectionRuleAssociations`
/// are both bound at `/custom-detection-rule/association`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,ListCustomDetectionRuleAssociations")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The read, the update and the delete of an association are all bound at
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`).
/// The catalogue itself is read through the shared harness's
/// [`published_rule_ids`].
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the regional
/// scope of an association is asserted rather than assumed.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The modelled error shapes this operation answers with
/// (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")`).
const NOT_FOUND: &str = "ResourceNotFoundException";
/// See [`NOT_FOUND`].
const CONFLICT: &str = "ConflictException";

/// A well-formed `AssociationId` (`^[a-zA-Z0-9_-]{1,64}$`, `len 1..64` —
/// `smithy_query(shape="AssociationDetail")`) that no create ever minted.
const ABSENT_ASSOCIATION_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so one page of
/// this size holds every association a case makes.
const MAX_RESULTS_MAX: i64 = 100;

/// The other `AssociationMode` member, so a case names the mode it moves TO without
/// repeating the enum.
fn other_mode(mode: &str) -> &'static str {
    if MODES[0] == mode {
        MODES[1]
    } else {
        MODES[0]
    }
}

/// Enroll `region`'s account in `rule_id` in `mode`, returning the `AssociationId`.
async fn associate_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    mode: &str,
    tags: Option<Value>,
) -> String {
    let mut body = json!({"ruleId": rule_id, "mode": mode});
    if let (Some(object), Some(tags)) = (body.as_object_mut(), tags) {
        object.insert("tags".to_string(), tags);
    }
    let (status, response) = call_in(router, region, "POST", ASSOCIATIONS_PATH, Some(body)).await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {response:?}"
    );
    response["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-association {rule_id}/{mode}: no associationId {response:?}")
        })
        .to_string()
}

/// [`associate_in`] through [`HOME_REGION`]'s endpoint.
async fn associate(router: &Router, rule_id: &str, mode: &str, tags: Option<Value>) -> String {
    associate_in(router, HOME_REGION, rule_id, mode, tags).await
}

async fn get_association_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    association_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
        None,
    )
    .await
}

async fn update_association_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    association_id: &str,
    body: Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "PUT",
        &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
        Some(body),
    )
    .await
}

/// [`update_association_in`] through [`HOME_REGION`]'s endpoint, with the mode the
/// case names.
async fn update_association(
    router: &Router,
    rule_id: &str,
    association_id: &str,
    mode: &str,
) -> (u16, Value) {
    update_association_in(
        router,
        HOME_REGION,
        rule_id,
        association_id,
        json!({"mode": mode}),
    )
    .await
}

/// The whole `GetCustomDetectionRuleAssociation` response for an association, failing
/// the test if the read did not succeed.
async fn read_association(router: &Router, rule_id: &str, association_id: &str, case: &str) -> Value {
    let (status, body) = get_association_in(router, HOME_REGION, rule_id, association_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-association expected 200, got {status} {body:?}"
    );
    body
}

/// A `Timestamp` member of `AssociationDetail` as epoch seconds: restJson1's default
/// timestamp format, so `createdAt`/`updatedAt` is a JSON number here
/// (`smithy_query(shape="AssociationDetail")` declares both as `Timestamp` with no
/// format override).
fn timestamp(detail: &Value, member: &str, case: &str) -> f64 {
    detail[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {detail:?}"))
}

/// Assert the update answered a bare success: `UpdateCustomDetectionRuleAssociationResponse`
/// declares no members (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")`).
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: update-association expected 200, got {status} {body:?}"
    );
    let members = body.as_object().map(|object| object.len()).unwrap_or(0);
    assert_eq!(
        members, 0,
        "case {case:?}: the response shape declares no members, got {body:?}"
    );
}

/// Assert a response is the modelled error SHAPE: the status, the shape in both
/// `__type` and the `x-amzn-errortype` header, and the shape repeated in the
/// exception's own `type` member.
fn assert_modelled_error(case: &str, status: u16, body: &Value, expected: &str, code: u16) {
    assert_eq!(
        status, code,
        "case {case:?}: expected HTTP {code}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        expected,
        "case {case:?}: expected {expected} in __type, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        expected,
        "case {case:?}: expected {expected} in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(expected),
        "case {case:?}: expected {expected} in the type member, got {body:?}"
    );
}

/// The `(ruleId, mode)` pairs the list reports through `region`'s endpoint, in page
/// order.
async fn listed_pairs(router: &Router, region: &str, case: &str) -> Vec<(String, String)> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{ASSOCIATIONS_PATH}?maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-associations expected 200, got {status} {body:?}"
    );
    body["ruleAssociations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ruleAssociations is @required, got {body:?}"))
        .iter()
        .map(|summary| {
            (
                summary["ruleId"].as_str().unwrap_or_default().to_string(),
                summary["mode"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1 + 2: the update sets the mode and nothing else, over a table spanning
// both modes, several published rules, a tagged and an untagged association, and
// a mode moved away and back again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_sets_the_mode_and_keeps_every_other_member() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;

    // One row per (rule, starting mode) pair over the first three published rules,
    // so the table spans both modes as a starting point and both as a target, and a
    // tagged association as well as an untagged one.
    struct Row {
        case: String,
        rule_id: String,
        from: &'static str,
        tags: Option<Value>,
    }
    let mut rows: Vec<Row> = Vec::new();
    for (index, rule_id) in rules.iter().take(3).enumerate() {
        for mode in MODES {
            rows.push(Row {
                case: format!("{rule_id}: {mode} -> {}", other_mode(mode)),
                rule_id: rule_id.clone(),
                from: mode,
                tags: (index % 2 == 0).then(|| json!({"team": "soc", "tier": "1"})),
            });
        }
    }

    for row in &rows {
        // Each row gets its own store, so a row's associations cannot collide with
        // another row's over the same published rule.
        let router = new_router();
        let case = row.case.as_str();
        let to = other_mode(row.from);
        let association_id =
            associate(&router, &row.rule_id, row.from, row.tags.clone()).await;
        let before = read_association(&router, &row.rule_id, &association_id, case).await;
        let detail_before = before["ruleAssociation"].clone();

        let (status, body) = update_association(&router, &row.rule_id, &association_id, to).await;
        assert_empty_success(case, status, &body);

        let after = read_association(&router, &row.rule_id, &association_id, case).await;
        let detail = after["ruleAssociation"].clone();
        assert_eq!(
            detail["mode"],
            json!(to),
            "case {case:?}: expected the mode the update named {to:?}, got {detail:?}"
        );
        for member in ["associationId", "arn", "ruleId", "accountId", "createdAt"] {
            assert_eq!(
                detail[member], detail_before[member],
                "case {case:?}: an update sets the mode alone, so {member} must stand, got {detail:?} after {detail_before:?}"
            );
        }
        assert_eq!(
            after["tags"], before["tags"],
            "case {case:?}: an update sets the mode alone, so the tags must stand, got {after:?}"
        );
        // A change is a mutation, so the association's last-updated instant moves
        // past the instant the create recorded.
        assert!(
            timestamp(&detail, "updatedAt", case) > timestamp(&detail, "createdAt", case),
            "case {case:?}: a change must move updatedAt past createdAt, got {detail:?}"
        );

        // The association is reported by its NEW mode and by no other: the list
        // names exactly one association, under the mode the update set.
        assert_eq!(
            listed_pairs(&router, HOME_REGION, case).await,
            vec![(row.rule_id.clone(), to.to_string())],
            "case {case:?}: the list must report the association under the mode the update set"
        );

        // Moving it BACK is the same rule applied again, so the round trip lands on
        // the mode the create made it in.
        let (status, body) =
            update_association(&router, &row.rule_id, &association_id, row.from).await;
        assert_empty_success(&format!("{case}: back"), status, &body);
        let back = read_association(&router, &row.rule_id, &association_id, case).await;
        assert_eq!(
            back["ruleAssociation"]["mode"],
            json!(row.from),
            "case {case:?}: a move back must land on the mode the create made, got {back:?}"
        );
        assert!(
            timestamp(&back["ruleAssociation"], "updatedAt", case)
                > timestamp(&detail, "updatedAt", case),
            "case {case:?}: the second change must move updatedAt again, got {back:?}"
        );

        // An update asking for the mode the association is already in changes
        // nothing: no member of the detail moves, `updatedAt` included.
        let (status, body) =
            update_association(&router, &row.rule_id, &association_id, row.from).await;
        assert_empty_success(&format!("{case}: replayed"), status, &body);
        let replayed = read_association(&router, &row.rule_id, &association_id, case).await;
        assert_eq!(
            replayed, back,
            "case {case:?}: an update to the mode already held must change nothing, got {replayed:?} after {back:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a rule carries at most one association, so the second CREATE is the
// modelled conflict and the update is what moves the one association's mode.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_create_conflicts_where_the_update_moves_the_mode() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;

    // Both starting modes over two published rules: whichever mode the account
    // associated a rule in, a create naming that rule again is the conflict and the
    // update is what lands the other mode.
    for rule_id in rules.iter().take(2) {
        for mode in MODES {
            let router = new_router();
            let to = other_mode(mode);
            let case = format!("{rule_id}: associated {mode}, create again as {to}");
            let association = associate(&router, rule_id, mode, None).await;
            let before = read_association(&router, rule_id, &association, &case).await;

            let (status, body) = call_in(
                &router,
                HOME_REGION,
                "POST",
                ASSOCIATIONS_PATH,
                Some(json!({"ruleId": rule_id, "mode": to})),
            )
            .await;
            assert_modelled_error(&case, status, &body, CONFLICT, 409);
            let message = body["message"].as_str().unwrap_or_default();
            assert_eq!(
                message,
                format!("Association already exists for rule {rule_id}"),
                "case {case:?}: the conflict must name the rule the request carried, got {message:?}"
            );
            assert!(
                !message.contains(association.as_str()),
                "case {case:?}: a refusal must not echo a value the caller never sent, got {message:?}"
            );

            // A refusal changes nothing: the association still reports the mode it
            // was made in.
            assert_eq!(
                read_association(&router, rule_id, &association, &case).await,
                before,
                "case {case:?}: a refused create must leave the association alone"
            );

            // The move the create may not make is exactly what the update makes.
            let (status, body) = update_association(&router, rule_id, &association, to).await;
            assert_empty_success(&case, status, &body);
            let moved = read_association(&router, rule_id, &association, &case).await;
            assert_eq!(
                moved["ruleAssociation"]["mode"].as_str(),
                Some(to),
                "case {case:?}: the update must set the mode, got {moved:?}"
            );
            assert_eq!(
                listed_pairs(&router, HOME_REGION, &case).await,
                vec![(rule_id.clone(), to.to_string())],
                "case {case:?}: the rule must still carry exactly one association"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an association the account does not hold is the modelled 404, never an
// upsert.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_the_account_does_not_hold_is_refused_as_not_found() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;
    let rule_id = rules[0].clone();
    let other_rule_id = rules[1].clone();
    // An association made HERE, so the cases below differ only in the identifier
    // they get wrong.
    let association_id = associate(&router, &rule_id, MODES[0], None).await;
    // An association made through another Region's endpoint: this one's ids are
    // well formed and real, and this endpoint still holds no such association.
    let elsewhere = associate_in(&router, OTHER_REGION, &other_rule_id, MODES[0], None).await;

    let cases: Vec<(String, String, String)> = vec![
        (
            "an associationId no create ever minted".to_string(),
            rule_id.clone(),
            ABSENT_ASSOCIATION_ID.to_string(),
        ),
        (
            "a real associationId under another published rule".to_string(),
            other_rule_id.clone(),
            association_id.clone(),
        ),
        (
            "an association made through another Region's endpoint".to_string(),
            other_rule_id.clone(),
            elsewhere.clone(),
        ),
    ];

    for (case, rule, id) in &cases {
        for mode in MODES {
            let case = format!("{case} ({mode})");
            let (status, body) = update_association(&router, rule, id, mode).await;
            assert_modelled_error(&case, status, &body, NOT_FOUND, 404);
            let message = body["message"].as_str().unwrap_or_default();
            // The `AssociationId` is what says WHICH association is missing, and it
            // came from the request, so the fixed not-found text carries it; the
            // rule the request named is not echoed beside it.
            assert_eq!(
                message,
                format!("Association not found: {id}"),
                "case {case:?}: expected the fixed not-found text carrying the association id, got {message:?}"
            );
            assert!(
                !message.contains(rule.as_str()),
                "case {case:?}: a refusal must not echo the rule the caller named, got {message:?}"
            );
            // Never an upsert: the refused pair is still unreadable afterwards.
            let (status, body) = get_association_in(&router, HOME_REGION, rule, id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused update must not create the association, got {status} {body:?}"
            );
        }
    }

    // The association this endpoint DOES hold is untouched by every refusal above.
    let held = read_association(&router, &rule_id, &association_id, "untouched").await;
    assert_eq!(
        held["ruleAssociation"]["mode"],
        json!(MODES[0]),
        "a refused update must leave the association the account holds alone, got {held:?}"
    );
    // And so is the one the other Region holds.
    let (status, body) = get_association_in(&router, OTHER_REGION, &other_rule_id, &elsewhere).await;
    assert_eq!(
        status, 200,
        "the other Region's association must still be readable there, got {status} {body:?}"
    );
    assert_eq!(
        body["ruleAssociation"]["mode"],
        json!(MODES[0]),
        "the other Region's association must keep its mode, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a `mode` outside the enum is refused by the modelled constraint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mode_outside_the_enum_is_refused_and_changes_nothing() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;
    let rule_id = rules[0].clone();
    let association_id = associate(&router, &rule_id, MODES[0], None).await;
    let before = read_association(&router, &rule_id, &association_id, "before").await;

    // `Mode` is a required member targeting the `AssociationMode` enum
    // (`smithy_query(operation="UpdateCustomDetectionRuleAssociation")`), so a value
    // outside it and an omitted one are both refused at the request boundary.
    let bodies = [
        ("a mode outside the enum", json!({"mode": "SILENT"})),
        ("a lowercase member", json!({"mode": "live"})),
        ("no mode at all", json!({})),
    ];
    for (case, body) in &bodies {
        let (status, response) =
            update_association_in(&router, HOME_REGION, &rule_id, &association_id, body.clone())
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            read_association(&router, &rule_id, &association_id, case).await,
            before,
            "case {case:?}: a refused update must change nothing"
        );
    }
}
