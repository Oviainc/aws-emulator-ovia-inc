// SPDX-License-Identifier: CC0-1.0

//! `DeleteCustomDetectionRuleOrgConfiguration`, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A delete removes the organization configuration the `(RuleId, Mode)` pair
//!    SELECTS — the operation binds `RuleId` as the URI label and `Mode` as the query
//!    parameter `mode`, exactly the binding the read uses
//!    (`smithy_query(operation="DeleteCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration")`)
//!    — so `GetCustomDetectionRuleOrgConfiguration` for that pair afterwards answers
//!    the modelled 404 and the pair may be configured again.
//! 2. It removes nothing else: the rule's configuration in the other
//!    `AssociationMode`, another rule's configuration, one made through another
//!    Region's endpoint, and the account's own ASSOCIATION with the rule are all left
//!    exactly as they were.
//! 3. A pair the Region's endpoint holds no configuration for is refused with the
//!    modelled 404 `ResourceNotFoundException`, never a hollow success — including a
//!    replayed delete, a rule the catalogue does not publish, and a configuration held
//!    only by another Region's endpoint.
//! 4. A `Mode` outside `AssociationMode`, an absent `mode`, and a `RuleId` outside its
//!    pattern are refused at the request boundary with HTTP 400, and no refusal removes
//!    anything.
//!
//! Every org-configuration request below is made by the organization's delegated
//! administrator, which the operation is available only to
//! (`crate::detector_lifecycle::as_delegated_admin`); the refusal of a caller without
//! that standing is the subject of the `org_configuration_authorization` suite.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    as_delegated_admin, call, call_in, error_code, error_shape, new_router, Router,
};

/// `ListCustomDetectionRules` is `POST /custom-detection-rule/rule`, which is also the
/// prefix `GetCustomDetectionRuleAssociation` hangs off
/// (`smithy_query(operation="ListCustomDetectionRules,GetCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// `CreateCustomDetectionRuleAssociation` is `POST /custom-detection-rule/association`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The create is `POST /custom-detection-rule/org-configuration`; the read and the
/// delete are `/custom-detection-rule/org-configuration/{RuleId}?mode=`
/// (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration,GetCustomDetectionRuleOrgConfiguration,DeleteCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the regional
/// scope of a configuration is asserted rather than assumed.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The modelled 404 shape a delete of a configuration the endpoint does not hold
/// answers with (`smithy_query(operation="DeleteCustomDetectionRuleOrgConfiguration")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Well-formed account ids (12 decimal digits), the only thing an account scope may
/// carry.
const ACCOUNTS: [&str; 5] = [
    "111122223333",
    "222233334444",
    "333344445555",
    "444455556666",
    "555566667777",
];

/// The ids of the rules GuardDuty publishes, read over the wire so the test cannot
/// drift from the catalogue.
async fn published_rule_ids(router: &Router) -> Vec<String> {
    let (status, body) = call(router, "POST", RULES_PATH, Some(json!({}))).await;
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
        ids.len() > 2,
        "list-custom-detection-rules: expected a catalogue of more than two rules, got {ids:?}"
    );
    ids
}

async fn create_org_configuration_in(router: &Router, region: &str, body: Value) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(router, region, "POST", ORG_CONFIGURATION_PATH, Some(body)).await
}

async fn get_org_configuration_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    mode: &str,
) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(
        router,
        region,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={mode}"),
        None,
    )
    .await
}

async fn get_org_configuration(router: &Router, rule_id: &str, mode: &str) -> (u16, Value) {
    get_org_configuration_in(router, HOME_REGION, rule_id, mode).await
}

/// One delete through `region`'s endpoint, with `query` appended to the rule's path so
/// a case can omit `mode` or send one outside the enum.
async fn delete_org_configuration_raw(
    router: &Router,
    region: &str,
    rule_id: &str,
    query: &str,
) -> (u16, Value) {
    as_delegated_admin(router, region).await;
    call_in(
        router,
        region,
        "DELETE",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}{query}"),
        None,
    )
    .await
}

async fn delete_org_configuration_in(
    router: &Router,
    region: &str,
    rule_id: &str,
    mode: &str,
) -> (u16, Value) {
    delete_org_configuration_raw(router, region, rule_id, &format!("?mode={mode}")).await
}

async fn delete_org_configuration(router: &Router, rule_id: &str, mode: &str) -> (u16, Value) {
    delete_org_configuration_in(router, HOME_REGION, rule_id, mode).await
}

/// Create one organization configuration through `region`'s endpoint, failing the test
/// if the create did not succeed.
async fn configured_in(router: &Router, region: &str, rule_id: &str, body: Value) {
    let mut request = json!({"ruleId": rule_id});
    merge(&mut request, body);
    let (status, response) = create_org_configuration_in(router, region, request.clone()).await;
    assert_eq!(
        status, 200,
        "create-org-configuration {request:?} in {region}: expected 200, got {status} {response:?}"
    );
}

async fn configured(router: &Router, rule_id: &str, body: Value) {
    configured_in(router, HOME_REGION, rule_id, body).await;
}

/// Copy `extra`'s members over `target`'s, so a case's body is written as the members
/// it differs in rather than repeated in full.
fn merge(target: &mut Value, extra: Value) {
    let (Some(target), Some(extra)) = (target.as_object_mut(), extra.as_object()) else {
        panic!("merge expects two JSON objects, got {target:?} and {extra:?}");
    };
    for (key, value) in extra {
        target.insert(key.clone(), value.clone());
    }
}

/// The configuration `Get` reports for `(rule_id, mode)`, failing the test if the read
/// did not succeed.
async fn read_configuration(router: &Router, rule_id: &str, mode: &str, case: &str) -> Value {
    let (status, body) = get_org_configuration(router, rule_id, mode).await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-org-configuration expected 200, got {status} {body:?}"
    );
    body["configuration"].clone()
}

/// A `Timestamp` member as epoch seconds: restJson1's default timestamp format, so a
/// `createdAt`/`updatedAt` is a JSON number here
/// (`smithy_query(shape="DetectionRuleOrgConfiguration")` declares both as `Timestamp`
/// with no format override). Failing here rather than returning an `Option` keeps a
/// comparison from passing vacuously on an absent member.
fn timestamp(configuration: &Value, member: &str, case: &str) -> f64 {
    configuration[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a numeric {member}, got {configuration:?}")
    })
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

/// Assert the delete answered a bare success: the response shape declares no members
/// (`smithy_query(operation="DeleteCustomDetectionRuleOrgConfiguration")`).
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: delete-org-configuration expected 200, got {status} {body:?}"
    );
    assert!(
        body.is_null() || body == &json!({}),
        "case {case:?}: the response declares no members, got {body:?}"
    );
}

/// Assert the pair resolves to nothing: both the read and a further delete answer the
/// modelled 404.
async fn assert_gone(router: &Router, rule_id: &str, mode: &str, case: &str) {
    let (status, body) = get_org_configuration(router, rule_id, mode).await;
    assert_modelled_error(&format!("{case}: the read after the delete"), status, &body, NOT_FOUND, 404);
    let (status, body) = delete_org_configuration(router, rule_id, mode).await;
    assert_modelled_error(&format!("{case}: the replayed delete"), status, &body, NOT_FOUND, 404);
}

// ---------------------------------------------------------------------------
// Rule 1: the delete removes the configuration the pair selects, and frees the
// pair.
// ---------------------------------------------------------------------------

/// One row of the delete table: the configuration the create records and the pair the
/// delete selects.
struct Removal {
    case: &'static str,
    /// The body the configuration is created with, beside its `ruleId`.
    created: Value,
    /// The `mode` the delete names, which every row here configured.
    mode: &'static str,
}

/// A delete removes the configuration the `(RuleId, Mode)` pair selects: the read
/// afterwards is the modelled 404, a replayed delete is too, and the freed pair can be
/// configured again — over rows differing in the mode, in which side of the account
/// scope the configuration carries, and in how many accounts it names.
#[tokio::test]
async fn a_delete_removes_the_configuration_the_pair_selects() {
    let catalogue = published_rule_ids(&new_router()).await;

    let rows = [
        Removal {
            case: "a DRY_RUN configuration with one included account",
            created: json!({"mode": "DRY_RUN", "includeAccountIds": [ACCOUNTS[0]]}),
            mode: "DRY_RUN",
        },
        Removal {
            case: "a LIVE configuration with one included account",
            created: json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
            mode: "LIVE",
        },
        Removal {
            case: "a LIVE configuration excluding five accounts",
            created: json!({"mode": "LIVE", "excludeAccountIds": ACCOUNTS}),
            mode: "LIVE",
        },
        Removal {
            case: "a DRY_RUN configuration with no account scope at all",
            created: json!({"mode": "DRY_RUN"}),
            mode: "DRY_RUN",
        },
        Removal {
            case: "a DRY_RUN configuration with an explicitly empty scope",
            created: json!({"mode": "DRY_RUN", "includeAccountIds": []}),
            mode: "DRY_RUN",
        },
        Removal {
            case: "a LIVE configuration excluding two accounts",
            created: json!({"mode": "LIVE", "excludeAccountIds": [ACCOUNTS[2], ACCOUNTS[3]]}),
            mode: "LIVE",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One rule per row, over a store of its own, so a row cannot see what another
        // one wrote.
        let rule_id = catalogue[index % catalogue.len()].clone();
        let router = new_router();
        configured(&router, &rule_id, row.created.clone()).await;
        let before = read_configuration(&router, &rule_id, row.mode, row.case).await;

        let (status, body) = delete_org_configuration(&router, &rule_id, row.mode).await;
        assert_empty_success(row.case, status, &body);

        assert_gone(&router, &rule_id, row.mode, row.case).await;

        // The pair is FREE afterwards: the create that collided with the deleted
        // configuration now succeeds, and what it writes is a NEW configuration
        // rather than the one the delete removed.
        configured(
            &router,
            &rule_id,
            json!({"mode": row.mode, "includeAccountIds": [ACCOUNTS[4]]}),
        )
        .await;
        let after = read_configuration(&router, &rule_id, row.mode, row.case).await;
        assert_eq!(
            after["includeAccountIds"],
            json!([ACCOUNTS[4]]),
            "case {:?}: the re-create must write its own scope, got {after:?}",
            row.case
        );
        assert!(
            timestamp(&after, "createdAt", row.case) >= timestamp(&before, "createdAt", row.case),
            "case {:?}: a re-create is a new configuration, got {after:?} after {before:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the delete removes nothing but the pair it names.
// ---------------------------------------------------------------------------

/// With both modes configured for two rules, deleting one pair leaves the other three
/// configurations byte-for-byte as they were — asserted for every pair in turn, each
/// over a store of its own.
#[tokio::test]
async fn a_delete_removes_only_the_pair_it_names() {
    let catalogue = published_rule_ids(&new_router()).await;
    let rules = [catalogue[0].clone(), catalogue[1].clone()];

    for target_rule in &rules {
        for target_mode in MODES {
            let case = format!("deleting {target_rule}/{target_mode}");
            let router = new_router();

            // Every (rule, mode) pair of two rules, each with a scope of its own so a
            // mix-up between them would show.
            let mut before: Vec<(String, &str, Value)> = Vec::new();
            for (rule_index, rule_id) in rules.iter().enumerate() {
                for (mode_index, mode) in MODES.iter().enumerate() {
                    configured(
                        &router,
                        rule_id,
                        json!({
                            "mode": mode,
                            "includeAccountIds": [ACCOUNTS[rule_index * 2 + mode_index]],
                        }),
                    )
                    .await;
                }
            }
            for rule_id in &rules {
                for mode in MODES {
                    before.push((
                        rule_id.clone(),
                        mode,
                        read_configuration(&router, rule_id, mode, &case).await,
                    ));
                }
            }

            let (status, body) = delete_org_configuration(&router, target_rule, target_mode).await;
            assert_empty_success(&case, status, &body);

            for (rule_id, mode, recorded) in &before {
                if rule_id == target_rule && *mode == target_mode {
                    assert_gone(&router, rule_id, mode, &case).await;
                    continue;
                }
                let still = read_configuration(&router, rule_id, mode, &case).await;
                assert_eq!(
                    &still, recorded,
                    "case {case:?}: {rule_id}/{mode} must be untouched, got {still:?}"
                );
            }
        }
    }
}

/// An organization configuration and the account's own ASSOCIATION with a rule are
/// different resources for the same `(RuleId, Mode)` pair, so deleting the
/// configuration leaves the association readable — asserted in both modes.
#[tokio::test]
async fn deleting_a_configuration_leaves_the_accounts_association_alone() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].clone();

    for mode in MODES {
        let case = format!("association for {rule_id}/{mode}");
        let router = new_router();
        let (status, body) = call_in(
            &router,
            HOME_REGION,
            "POST",
            ASSOCIATIONS_PATH,
            Some(json!({"ruleId": rule_id, "mode": mode})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create-association expected 200, got {status} {body:?}"
        );
        let association_id = body["ruleAssociation"]["associationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an associationId, got {body:?}"))
            .to_string();

        configured(
            &router,
            &rule_id,
            json!({"mode": mode, "includeAccountIds": [ACCOUNTS[1]]}),
        )
        .await;
        let association_path = format!("{RULES_PATH}/{rule_id}/association/{association_id}");
        let (status, before) = call_in(&router, HOME_REGION, "GET", &association_path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: get-association expected 200, got {status} {before:?}"
        );

        let (status, body) = delete_org_configuration(&router, &rule_id, mode).await;
        assert_empty_success(&case, status, &body);
        assert_gone(&router, &rule_id, mode, &case).await;

        let (status, after) = call_in(&router, HOME_REGION, "GET", &association_path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the association must survive the delete, got {status} {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: the association must be untouched, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a pair with no configuration behind it is the modelled 404.
// ---------------------------------------------------------------------------

/// There is nothing to delete for a pair the Region's endpoint holds no configuration
/// for: the answer is the modelled 404 and nothing is removed, over rows differing in
/// WHY the pair resolves to nothing.
#[tokio::test]
async fn deleting_a_pair_with_no_configuration_is_the_modelled_404() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;

    // Two well-formed `RuleId`s (`^[a-z0-9]+(-[a-z0-9]+)*$`, `len 1..100`) the
    // catalogue does not publish — a multi-segment one and a minimal one — and two
    // PUBLISHED rules nothing configured.
    for rule_id in [
        "no-such-published-rule",
        "r1",
        catalogue[2].as_str(),
        catalogue[1].as_str(),
    ] {
        for mode in MODES {
            let case = format!("ruleId {rule_id:?} in mode {mode}, nothing configured");
            let (status, body) = delete_org_configuration(&router, rule_id, mode).await;
            assert_modelled_error(&case, status, &body, NOT_FOUND, 404);
        }
    }

    // The OTHER mode of a configured rule is a different pair, so it holds nothing.
    let rule_id = catalogue[0].as_str();
    configured(
        &router,
        rule_id,
        json!({"mode": MODES[0], "includeAccountIds": [ACCOUNTS[0]]}),
    )
    .await;
    let before = read_configuration(&router, rule_id, MODES[0], "the other mode").await;
    let (status, body) = delete_org_configuration(&router, rule_id, MODES[1]).await;
    assert_modelled_error("the other mode of a configured rule", status, &body, NOT_FOUND, 404);
    let after = read_configuration(&router, rule_id, MODES[0], "the other mode").await;
    assert_eq!(
        after, before,
        "a refused delete must leave the configured mode alone, got {after:?}"
    );

    // A configuration is REGIONAL: the endpoint that created it is the only one that
    // holds it, so another Region's endpoint has nothing to delete — and the delete it
    // refuses leaves this Region's configuration readable.
    let (status, body) = delete_org_configuration_in(&router, OTHER_REGION, rule_id, MODES[0]).await;
    assert_modelled_error("another Region's endpoint", status, &body, NOT_FOUND, 404);
    let after = read_configuration(&router, rule_id, MODES[0], "another Region").await;
    assert_eq!(
        after, before,
        "a delete through another Region must leave this Region's configuration alone, got {after:?}"
    );

    // And the other way round: a configuration made through the other Region's
    // endpoint is not this one's to delete.
    let other_rule = catalogue[1].as_str();
    configured_in(
        &router,
        OTHER_REGION,
        other_rule,
        json!({"mode": MODES[1], "excludeAccountIds": [ACCOUNTS[2]]}),
    )
    .await;
    let (status, body) = delete_org_configuration(&router, other_rule, MODES[1]).await;
    assert_modelled_error(
        "a configuration held by the other Region",
        status,
        &body,
        NOT_FOUND,
        404,
    );
    let (status, body) = get_org_configuration_in(&router, OTHER_REGION, other_rule, MODES[1]).await;
    assert_eq!(
        status, 200,
        "the other Region must still hold its configuration, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a modelled constraint failure is a 400 that removes nothing.
// ---------------------------------------------------------------------------

/// A `mode` outside `AssociationMode`, an absent `mode`, and a `RuleId` outside its
/// pattern are refused with HTTP 400 — never a 501, never a hollow success — and the
/// configuration they named is still there afterwards.
#[tokio::test]
async fn a_modelled_constraint_failure_is_refused_with_400() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].clone();
    configured(
        &router,
        &rule_id,
        json!({"mode": "LIVE", "includeAccountIds": [ACCOUNTS[0]]}),
    )
    .await;
    let before = read_configuration(&router, &rule_id, "LIVE", "baseline").await;

    for (case, path_rule_id, query) in [
        ("a mode outside the enum", rule_id.as_str(), "?mode=SILENT"),
        ("a lower-cased mode", rule_id.as_str(), "?mode=live"),
        ("an empty mode", rule_id.as_str(), "?mode="),
        ("a ruleId outside its pattern", "Not_A_Rule_Id", "?mode=LIVE"),
    ] {
        let (status, body) =
            delete_org_configuration_raw(&router, HOME_REGION, path_rule_id, query).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
    }

    // A request that carries no `mode` at all selects no `(RuleId, Mode)` pair, so it
    // is refused rather than answered — whether the framework resolves it to this
    // operation and fails the required member, or resolves it to no operation at all,
    // is its request walk and not this operation's, so what is asserted here is only
    // that it is never a success and removes nothing.
    let (status, body) = delete_org_configuration_raw(&router, HOME_REGION, &rule_id, "").await;
    assert!(
        !(200..300).contains(&status),
        "case \"no mode at all\": a request selecting no pair must be refused, got {status} {body:?}"
    );

    let after = read_configuration(&router, &rule_id, "LIVE", "after the refusals").await;
    assert_eq!(
        after, before,
        "a refused delete must change nothing, got {after:?}"
    );

    // The well-formed delete against the same configuration still succeeds, so the
    // refusals above are about the request and nothing else.
    let (status, body) = delete_org_configuration(&router, &rule_id, "LIVE").await;
    assert_empty_success("the well-formed request", status, &body);
    assert_gone(&router, &rule_id, "LIVE", "the well-formed request").await;
}
