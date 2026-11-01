// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomDetectionRuleAssociation`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A delete removes the association the two URI labels identify — "disables a
//!    custom detection rule by deleting its association"
//!    (`smithy_query(query="doc",shape="DeleteCustomDetectionRuleAssociation")`) — so
//!    `GetCustomDetectionRuleAssociation` for it afterwards answers the modelled 404,
//!    `ListCustomDetectionRuleAssociations` no longer reports it, and the
//!    `(RuleId, Mode)` pair it held is free to associate again.
//! 2. It removes nothing else: the same rule's association in the other
//!    `AssociationMode`, another rule's association, one made through another Region's
//!    endpoint, and the account's ORGANIZATION configuration for the same pair are all
//!    left exactly as they were.
//! 3. An `(AssociationId, RuleId)` pair this account holds no association for — an id
//!    never minted, a real id under another published rule, an association held only by
//!    another Region's endpoint, a rule the catalogue does not publish, and a replayed
//!    delete — is refused with the modelled 404 `ResourceNotFoundException`, never a
//!    hollow success, and the refusal removes nothing.
//! 4. A label outside the length or pattern its member carries
//!    (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`) is refused at
//!    the request boundary with HTTP 400 — never a 501, never a hollow success — and
//!    the association it named is still there afterwards.
//! 5. The `ClientToken` a deleted association's create carried stops replaying to it,
//!    so a replayed create mints a new association rather than answering with an id the
//!    store no longer holds.

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
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// The organization configuration of a rule is `/custom-detection-rule/org-configuration`
/// (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the regional
/// scope of an association is asserted rather than assumed.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The modelled error shape this operation answers an absent association with
/// (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// A well-formed `AssociationId` (`^[a-zA-Z0-9_-]{1,64}$`, `len 1..64` —
/// `smithy_query(shape="AssociationDetail")`) that no create ever minted.
const ABSENT_ASSOCIATION_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so one page of
/// this size holds every association a case makes.
const MAX_RESULTS_MAX: i64 = 100;

/// The other `AssociationMode` member, so a case names the mode it leaves alone
/// without repeating the enum.
fn other_mode(mode: &str) -> &'static str {
    if MODES[0] == mode {
        MODES[1]
    } else {
        MODES[0]
    }
}

/// Enroll `region`'s account in `rule_id` in `mode`, returning the minted
/// `AssociationId`.
async fn associate_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    mode: &str,
    extra: Option<Value>,
) -> String {
    let mut body = json!({"ruleId": rule_id, "mode": mode});
    if let (Some(object), Some(extra)) = (body.as_object_mut(), extra.as_ref().and_then(Value::as_object))
    {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    let (status, response) = call_in(router, region, "POST", ASSOCIATIONS_PATH, Some(body)).await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {response:?}"
    );
    response["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create-association {rule_id}/{mode}: expected an associationId, got {response:?}")
        })
        .to_string()
}

/// [`associate_in`] through [`HOME_REGION`]'s endpoint.
async fn associate(router: &Router, rule_id: &str, mode: &str, extra: Option<Value>) -> String {
    associate_in(router, HOME_REGION, rule_id, mode, extra).await
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

async fn delete_association_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    association_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "DELETE",
        &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
        None,
    )
    .await
}

/// [`delete_association_in`] through [`HOME_REGION`]'s endpoint.
async fn delete_association(router: &Router, rule_id: &str, association_id: &str) -> (u16, Value) {
    delete_association_in(router, HOME_REGION, rule_id, association_id).await
}

/// The whole `GetCustomDetectionRuleAssociation` response for an association, failing
/// the test if the read did not succeed.
async fn read_association(
    router: &Router,
    rule_id: &str,
    association_id: &str,
    case: &str,
) -> Value {
    let (status, body) = get_association_in(router, HOME_REGION, rule_id, association_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-association expected 200, got {status} {body:?}"
    );
    body
}

/// The `AssociationId`s the list reports through `region`'s endpoint, in page order.
async fn listed_ids(router: &Router, region: &str, case: &str) -> Vec<String> {
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
        .map(|summary| summary["associationId"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// Assert the delete answered a bare success: `DeleteCustomDetectionRuleAssociationResponse`
/// declares no members (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`).
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: delete-association expected 200, got {status} {body:?}"
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

/// Assert the association resolves to nothing: the read, a replayed delete and the
/// list all say it is gone.
async fn assert_gone(router: &Router, rule_id: &str, association_id: &str, case: &str) {
    let (status, body) = get_association_in(router, HOME_REGION, rule_id, association_id).await;
    assert_modelled_error(
        &format!("{case}: the read after the delete"),
        status,
        &body,
        NOT_FOUND,
        404,
    );
    let (status, body) = delete_association(router, rule_id, association_id).await;
    assert_modelled_error(
        &format!("{case}: the replayed delete"),
        status,
        &body,
        NOT_FOUND,
        404,
    );
    let listed = listed_ids(router, HOME_REGION, case).await;
    assert!(
        !listed.contains(&association_id.to_string()),
        "case {case:?}: the list must not report a deleted association, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the delete removes the association the labels identify, and frees the
// pair.
// ---------------------------------------------------------------------------

/// One row of the delete table: which published rule the association binds, in which
/// mode, and what the create carried beside the pair.
struct Removal {
    case: String,
    rule_id: String,
    mode: &'static str,
    /// Members the create sends beside `ruleId` and `mode`.
    extra: Option<Value>,
}

/// A delete removes the association the two URI labels identify: the read afterwards
/// is the modelled 404, the list no longer reports it, a replayed delete is the 404
/// too, and the freed `(RuleId, Mode)` pair can be associated again — over rows
/// differing in the published rule, in the mode, in whether the association carried
/// tags, and in how many tags it carried.
#[tokio::test]
async fn a_delete_removes_the_association_the_labels_identify() {
    let catalogue = published_rule_ids(&new_router()).await;

    let mut rows: Vec<Removal> = Vec::new();
    for (index, rule_id) in catalogue.iter().take(3).enumerate() {
        for mode in MODES {
            let extra = match index % 3 {
                0 => None,
                1 => Some(json!({"tags": {"team": "soc"}})),
                _ => Some(json!({"tags": {"team": "soc", "tier": "1", "owner": "detections"}})),
            };
            rows.push(Removal {
                case: format!("{rule_id} in {mode}, extra members {extra:?}"),
                rule_id: rule_id.clone(),
                mode,
                extra,
            });
        }
    }

    for row in &rows {
        // Each row gets a store of its own, so a row cannot see what another one
        // associated over the same published rule.
        let router = new_router();
        let case = row.case.as_str();
        let association_id = associate(&router, &row.rule_id, row.mode, row.extra.clone()).await;
        let before = read_association(&router, &row.rule_id, &association_id, case).await;
        let listed = listed_ids(&router, HOME_REGION, case).await;
        assert!(
            listed.contains(&association_id),
            "case {case:?}: the list must report the association before the delete, got {listed:?}"
        );

        let (status, body) = delete_association(&router, &row.rule_id, &association_id).await;
        assert_empty_success(case, status, &body);

        assert_gone(&router, &row.rule_id, &association_id, case).await;

        // The pair is FREE afterwards: the create that would have collided with the
        // deleted association now succeeds, and what it writes is a NEW association
        // rather than the one the delete removed.
        let remade = associate(&router, &row.rule_id, row.mode, None).await;
        assert_ne!(
            remade, association_id,
            "case {case:?}: a re-create mints its own id, got {remade}"
        );
        let after = read_association(&router, &row.rule_id, &remade, case).await;
        assert!(
            timestamp(&after["ruleAssociation"], "createdAt", case)
                >= timestamp(&before["ruleAssociation"], "createdAt", case),
            "case {case:?}: a re-create is a new association, got {after:?} after {before:?}"
        );
        assert_eq!(
            after["tags"],
            json!({}),
            "case {case:?}: the re-create carried no tags, so the deleted association's must not survive it, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the delete removes nothing but the association it names.
// ---------------------------------------------------------------------------

/// With two rules associated through this Region's endpoint, one Region over from a
/// third association and an organization configuration for the target pair, deleting
/// one association leaves every other resource byte-for-byte as it was — asserted for
/// every association in turn, each over a store of its own.
#[tokio::test]
async fn a_delete_removes_only_the_association_it_names() {
    let catalogue = published_rule_ids(&new_router()).await;
    let rules = [catalogue[0].clone(), catalogue[1].clone()];

    for (target_index, target_rule) in rules.iter().enumerate() {
        // A rule carries at most one association, so each round associates every
        // rule in ONE mode; the offset rotates which mode each rule takes, so the
        // table still spans both modes for every target.
        for offset in 0..MODES.len() {
            let target_mode = MODES[(target_index + offset) % MODES.len()];
            let case = format!("deleting {target_rule}/{target_mode}");
            let router = new_router();

            // Both rules through this Region's endpoint, each with tags of its own
            // so a mix-up between them would show.
            let mut made: Vec<(String, &str, String)> = Vec::new();
            for (rule_index, rule_id) in rules.iter().enumerate() {
                let mode = MODES[(rule_index + offset) % MODES.len()];
                let association_id = associate(
                    &router,
                    rule_id,
                    mode,
                    Some(json!({"tags": {"slot": format!("{rule_index}-{offset}")}})),
                )
                .await;
                made.push((rule_id.clone(), mode, association_id));
            }
            // One more through ANOTHER Region's endpoint, over the pair this case
            // deletes: an association belongs to the Region that made it.
            let elsewhere =
                associate_in(&router, OTHER_REGION, target_rule, target_mode, None).await;
            // And the account's ORGANIZATION configuration for the same pair, which is
            // a different resource in a store of its own.
            let (status, body) = call_in(
                &router,
                HOME_REGION,
                "POST",
                ORG_CONFIGURATION_PATH,
                Some(json!({"ruleId": target_rule, "mode": target_mode})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: create-org-configuration expected 200, got {status} {body:?}"
            );

            let mut before: Vec<Value> = Vec::new();
            for (rule_id, _, association_id) in &made {
                before.push(read_association(&router, rule_id, association_id, &case).await);
            }
            let (status, org_before) = call_in(
                &router,
                HOME_REGION,
                "GET",
                &format!("{ORG_CONFIGURATION_PATH}/{target_rule}?mode={target_mode}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: get-org-configuration expected 200, got {status} {org_before:?}"
            );
            let (status, elsewhere_before) =
                get_association_in(&router, OTHER_REGION, target_rule, &elsewhere).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the other Region's association must be readable, got {status} {elsewhere_before:?}"
            );

            let target_id = made
                .iter()
                .find(|(rule_id, mode, _)| rule_id == target_rule && *mode == target_mode)
                .map(|(_, _, association_id)| association_id.clone())
                .unwrap_or_else(|| panic!("case {case:?}: the target pair was associated above"));
            let (status, body) = delete_association(&router, target_rule, &target_id).await;
            assert_empty_success(&case, status, &body);

            for ((rule_id, mode, association_id), recorded) in made.iter().zip(&before) {
                if association_id == &target_id {
                    assert_gone(&router, rule_id, association_id, &case).await;
                    continue;
                }
                let still = read_association(&router, rule_id, association_id, &case).await;
                assert_eq!(
                    &still, recorded,
                    "case {case:?}: {rule_id}/{mode} must be untouched, got {still:?}"
                );
            }

            // The other Region's association over the deleted pair stands, and so does
            // the organization configuration for that pair.
            let (status, elsewhere_after) =
                get_association_in(&router, OTHER_REGION, target_rule, &elsewhere).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a delete through one Region must leave another's association alone, got {status} {elsewhere_after:?}"
            );
            assert_eq!(
                elsewhere_after, elsewhere_before,
                "case {case:?}: the other Region's association must be untouched, got {elsewhere_after:?}"
            );
            let (status, org_after) = call_in(
                &router,
                HOME_REGION,
                "GET",
                &format!("{ORG_CONFIGURATION_PATH}/{target_rule}?mode={target_mode}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the organization configuration is a different resource, got {status} {org_after:?}"
            );
            assert_eq!(
                org_after, org_before,
                "case {case:?}: the organization configuration must be untouched, got {org_after:?}"
            );

            // The published rule itself is GuardDuty's catalogue entry rather than this
            // account's, so deleting an association never withdraws it.
            let still_published = published_rule_ids(&router).await;
            assert!(
                still_published.contains(target_rule),
                "case {case:?}: the catalogue must still publish the rule, got {still_published:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a pair with no association behind it is the modelled 404.
// ---------------------------------------------------------------------------

/// There is nothing to disable for an `(AssociationId, RuleId)` pair the account
/// holds no association for: the answer is the modelled 404 and nothing is removed,
/// over rows differing in WHY the pair resolves to nothing.
#[tokio::test]
async fn deleting_a_pair_with_no_association_is_the_modelled_404() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].clone();
    let other_rule = catalogue[1].clone();

    // One association this Region's endpoint holds, so every refusal below can be
    // shown to have left it alone.
    let association_id = associate(&router, &rule_id, MODES[0], Some(json!({"tags": {"k": "v"}}))).await;
    let before = read_association(&router, &rule_id, &association_id, "the baseline").await;

    // An association made through ANOTHER Region's endpoint: this Region has nothing
    // to delete for its id.
    let elsewhere = associate_in(&router, OTHER_REGION, &other_rule, MODES[1], None).await;

    for (case, path_rule_id, path_association_id) in [
        (
            "an associationId no create ever minted",
            rule_id.as_str(),
            ABSENT_ASSOCIATION_ID,
        ),
        (
            "a minted associationId under another published rule",
            other_rule.as_str(),
            association_id.as_str(),
        ),
        (
            "a well-formed ruleId the catalogue does not publish",
            "no-such-published-rule",
            association_id.as_str(),
        ),
        (
            "a minimal well-formed ruleId the catalogue does not publish",
            "r1",
            ABSENT_ASSOCIATION_ID,
        ),
        (
            "an association held only by another Region's endpoint",
            other_rule.as_str(),
            elsewhere.as_str(),
        ),
        (
            "a one-character associationId no create ever minted",
            rule_id.as_str(),
            "a",
        ),
        (
            "the longest associationId the member allows, never minted",
            rule_id.as_str(),
            // `len 1..64` (`smithy_query(shape="AssociationDetail")`): 64 characters
            // is inside the bound, so this reaches the handler.
            "a234567890123456789012345678901234567890123456789012345678901234",
        ),
    ] {
        let (status, body) = delete_association(&router, path_rule_id, path_association_id).await;
        assert_modelled_error(case, status, &body, NOT_FOUND, 404);
    }

    // Nothing above removed anything: this Region's association and the other
    // Region's are both still readable.
    let after = read_association(&router, &rule_id, &association_id, "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused delete must change nothing, got {after:?}"
    );
    let (status, body) = get_association_in(&router, OTHER_REGION, &other_rule, &elsewhere).await;
    assert_eq!(
        status, 200,
        "the other Region must still hold its association, got {status} {body:?}"
    );

    // And the well-formed delete against the association that IS there succeeds, so
    // the refusals above are about the pair and nothing else — after which a replay
    // of that same delete is the 404 too.
    let (status, body) = delete_association(&router, &rule_id, &association_id).await;
    assert_empty_success("the association this Region holds", status, &body);
    assert_gone(
        &router,
        &rule_id,
        &association_id,
        "the association this Region holds",
    )
    .await;
}

// ---------------------------------------------------------------------------
// Rule 4: a modelled constraint failure is a 400 that removes nothing.
// ---------------------------------------------------------------------------

/// A label outside the length or pattern its member carries is refused with HTTP 400
/// — never a 501, never a hollow success — and the association it named is still
/// there afterwards.
#[tokio::test]
async fn a_modelled_constraint_failure_is_refused_with_400() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].clone();
    let association_id = associate(&router, &rule_id, MODES[0], None).await;
    let before = read_association(&router, &rule_id, &association_id, "the baseline").await;

    // `RuleId` is `len 1..100`, `^[a-z0-9]+(-[a-z0-9]+)*$` and `AssociationId` is
    // `len 1..64`, `^[a-zA-Z0-9_-]{1,64}$`
    // (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`), so each row
    // breaks exactly one of those.
    let over_long_rule_id = "a".repeat(101);
    let over_long_association_id = "a".repeat(65);
    for (case, path_rule_id, path_association_id) in [
        (
            "a ruleId with characters outside its pattern",
            "Not_A_Rule_Id",
            association_id.as_str(),
        ),
        (
            "a ruleId whose hyphens bound an empty segment",
            "rule--id",
            association_id.as_str(),
        ),
        (
            "a ruleId past its length ceiling",
            over_long_rule_id.as_str(),
            association_id.as_str(),
        ),
        (
            "an associationId with a character outside its pattern",
            rule_id.as_str(),
            "not.an.association.id",
        ),
        (
            "an associationId past its length ceiling",
            rule_id.as_str(),
            over_long_association_id.as_str(),
        ),
    ] {
        let (status, body) = delete_association(&router, path_rule_id, path_association_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
    }

    let after = read_association(&router, &rule_id, &association_id, "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused delete must change nothing, got {after:?}"
    );

    // The well-formed request against the same association still succeeds, so the
    // refusals above are about the request and nothing else.
    let (status, body) = delete_association(&router, &rule_id, &association_id).await;
    assert_empty_success("the well-formed request", status, &body);
    assert_gone(&router, &rule_id, &association_id, "the well-formed request").await;
}

// ---------------------------------------------------------------------------
// Rule 5: the deleted association's idempotency token stops replaying to it.
// ---------------------------------------------------------------------------

/// A `ClientToken` replays to the association its create minted, so once that
/// association is deleted the token must resolve to nothing: the replayed create
/// mints a NEW association instead of answering with an id the store no longer holds
/// — asserted in both modes and over token shapes differing in length.
#[tokio::test]
async fn a_deleted_associations_client_token_stops_replaying() {
    let catalogue = published_rule_ids(&new_router()).await;

    for (index, mode) in MODES.iter().enumerate() {
        // `ClientToken` is `len 0..64`
        // (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`), so a
        // short token and a maximal one are both well formed.
        for token in ["t", &"k".repeat(64)] {
            let router = new_router();
            let rule_id = catalogue[index % catalogue.len()].clone();
            let case = format!("{rule_id} in {mode} under a {}-character token", token.len());

            let association_id =
                associate(&router, &rule_id, mode, Some(json!({"clientToken": token}))).await;
            // The token replays to that same association while it is there.
            let replayed =
                associate(&router, &rule_id, mode, Some(json!({"clientToken": token}))).await;
            assert_eq!(
                replayed, association_id,
                "case {case:?}: the token must replay to the association it minted, got {replayed}"
            );

            let (status, body) = delete_association(&router, &rule_id, &association_id).await;
            assert_empty_success(&case, status, &body);
            assert_gone(&router, &rule_id, &association_id, &case).await;

            let remade =
                associate(&router, &rule_id, mode, Some(json!({"clientToken": token}))).await;
            assert_ne!(
                remade, association_id,
                "case {case:?}: the token must not replay to a deleted association, got {remade}"
            );
            let detail = read_association(&router, &rule_id, &remade, &case).await;
            assert_eq!(
                detail["ruleAssociation"]["mode"],
                json!(mode),
                "case {case:?}: the re-create associates the pair it named, got {detail:?}"
            );

            // Another published rule was never associated here, so it is still free
            // and the delete above did not touch it.
            let free_rule = catalogue[(index + 1) % catalogue.len()].clone();
            let other = associate(&router, &free_rule, other_mode(mode), None).await;
            assert_ne!(
                other, remade,
                "case {case:?}: another rule's association is its own, got {other}"
            );
        }
    }
}
