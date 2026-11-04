// SPDX-License-Identifier: CC0-1.0

//! The delegated-administrator gate on the custom-detection-rule ORGANIZATION
//! configuration, and the idempotence of the association delete, end to end over the
//! wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. An organization-configuration operation is "available only to the delegated
//!    administrator account"
//!    (`smithy_query(query="doc",shape="CreateCustomDetectionRuleOrgConfiguration")`),
//!    so a caller that does not hold that standing is refused: HTTP 400 carrying the
//!    `InvalidInputException` class in the body's `__type` and `type`, the
//!    `AccessDeniedException` shape in `x-amzn-errortype`, and the fixed
//!    authorization message. It holds for all five of them — including the LISTING,
//!    which must not hand the organization's configuration data to such a caller —
//!    and whatever the request names: the refusal comes BEFORE any rule or
//!    configuration is resolved, so a request naming a configured pair and one naming
//!    nothing at all are answered alike.
//! 2. The standing is the Region's designation of the account the request is signed
//!    for: it is absent when nothing is designated, when another account is
//!    designated, when the designation was made through another Region's endpoint and
//!    once the clock has made its removal due — and present while a removal is
//!    accepted but not yet due.
//! 3. A refused call changes nothing: what the delegated administrator configured
//!    before it is byte-for-byte what the delegated administrator reads after it.
//! 4. `DeleteCustomDetectionRuleAssociation` is `@idempotent`
//!    (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`): a delete
//!    naming an association the endpoint resolves nothing for answers HTTP 200 with
//!    the empty response and removes nothing, whatever the reason the pair resolves to
//!    nothing — while the read and the update of that same absent pair stay the
//!    modelled 404, carrying the association id the request named.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    as_delegated_admin, call_in, error_code, error_shape, new_router, published_rule_ids, Router,
};
use crate::organization_admin_accounts::{disable_admin, enable_admin, past_the_removal};

/// The create and the listing are bound at `/custom-detection-rule/org-configuration`;
/// the read, the update and the delete hang `{RuleId}` off it
/// (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration,ListCustomDetectionRuleOrgConfigurations,GetCustomDetectionRuleOrgConfiguration,UpdateCustomDetectionRuleOrgConfiguration,DeleteCustomDetectionRuleOrgConfiguration")`).
const ORG_CONFIGURATION_PATH: &str = "/custom-detection-rule/org-configuration";

/// `CreateCustomDetectionRuleAssociation` is `POST /custom-detection-rule/association`,
/// and the read, the update and the delete of one association hang off
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,GetCustomDetectionRuleAssociation")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";
/// See [`ASSOCIATIONS_PATH`].
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// The Region the harness signs for by default, and a second one, so the standing is
/// shown to be the Region's own.
const HOME_REGION: &str = "us-east-1";
/// See [`HOME_REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// The error CLASS the body of an authorization refusal carries in both `__type` and
/// its own `type` member.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE such a refusal is enveloped in, as `x-amzn-errortype`
/// carries it (`smithy_query(operation="CreateCustomDetectionRuleOrgConfiguration")`
/// declares it).
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The modelled 404 shape the read and the update of an absent association answer.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The message a caller without the standing is refused with — written out here
/// rather than read from the service, so this suite pins the words themselves.
const NOT_AUTHORIZED: &str =
    "The request is rejected because the caller is not authorized to call this API.";

/// A well-formed account id (12 decimal digits) that is not the account this emulator
/// serves, so designating it leaves the caller without the standing.
const ANOTHER_ACCOUNT_ID: &str = "111122223333";

/// Well-formed account ids an organization configuration's scope may name.
const ACCOUNTS: [&str; 2] = ["222233334444", "333344445555"];

/// A well-formed `AssociationId` (`^[a-zA-Z0-9_-]{1,64}$`) no create ever minted.
const ABSENT_ASSOCIATION_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

// ---------------------------------------------------------------------------
// The five operations, as one table.
// ---------------------------------------------------------------------------

/// One organization-configuration operation, as a request this suite can send.
struct Operation {
    name: &'static str,
    method: &'static str,
    /// The path, given the rule the case is about.
    path: fn(&str) -> String,
    /// The body, or `None` for the operations that carry none.
    body: fn(&str) -> Option<Value>,
    /// What the DELEGATED ADMINISTRATOR is answered on a store where the rule has no
    /// organization configuration: the create records one (200), the listing reports
    /// an empty page (200), and the three that address a configuration answer the
    /// modelled 404 (`smithy_query(operation="GetCustomDetectionRuleOrgConfiguration")`).
    served_status: u16,
}

/// Every operation the delegated-administrator standing gates, each driven with a
/// well-formed request.
const OPERATIONS: [Operation; 5] = [
    Operation {
        name: "CreateCustomDetectionRuleOrgConfiguration",
        method: "POST",
        path: |_| ORG_CONFIGURATION_PATH.to_string(),
        body: |rule_id| {
            Some(json!({"ruleId": rule_id, "mode": MODES[0], "includeAccountIds": [ACCOUNTS[0]]}))
        },
        served_status: 200,
    },
    Operation {
        name: "GetCustomDetectionRuleOrgConfiguration",
        method: "GET",
        path: |rule_id| format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={}", MODES[0]),
        body: |_| None,
        served_status: 404,
    },
    Operation {
        name: "UpdateCustomDetectionRuleOrgConfiguration",
        method: "PUT",
        path: |rule_id| format!("{ORG_CONFIGURATION_PATH}/{rule_id}"),
        body: |_| Some(json!({"mode": MODES[1], "excludeAccountIds": [ACCOUNTS[1]]})),
        served_status: 404,
    },
    Operation {
        name: "DeleteCustomDetectionRuleOrgConfiguration",
        method: "DELETE",
        path: |rule_id| format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={}", MODES[0]),
        body: |_| None,
        served_status: 404,
    },
    Operation {
        name: "ListCustomDetectionRuleOrgConfigurations",
        method: "GET",
        path: |_| ORG_CONFIGURATION_PATH.to_string(),
        body: |_| None,
        served_status: 200,
    },
];

/// Send `operation` through `region`'s endpoint for `rule_id`.
async fn send(
    router: &Router,
    region: &str,
    operation: &Operation,
    rule_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        operation.method,
        &(operation.path)(rule_id),
        (operation.body)(rule_id),
    )
    .await
}

/// Assert one authorization refusal: the status, the class in the body's `__type` and
/// in its own `type` member, the modelled shape in `x-amzn-errortype`, the fixed
/// message, and that nothing of the organization's configuration was disclosed
/// beside it.
fn assert_not_authorized(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected {INVALID_INPUT} in __type, got {body:?}"
    );
    assert_eq!(
        body["type"],
        json!(INVALID_INPUT),
        "case {case:?}: expected {INVALID_INPUT} in the type member, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ACCESS_DENIED,
        "case {case:?}: expected {ACCESS_DENIED} in x-amzn-errortype, got {body:?}"
    );
    assert_eq!(
        body["message"],
        json!(NOT_AUTHORIZED),
        "case {case:?}: expected the fixed authorization message, got {body:?}"
    );
    assert_eq!(
        body["configurations"],
        Value::Null,
        "case {case:?}: a refused caller must be told no configuration, got {body:?}"
    );
    assert_eq!(
        body["configuration"],
        Value::Null,
        "case {case:?}: a refused caller must be told no configuration, got {body:?}"
    );
}

/// The standing a case leaves the Region's organization in before it calls.
#[derive(Debug, Clone, Copy)]
enum Standing {
    /// Nothing designated at all.
    Undesignated,
    /// Another account designated, so the caller is not the one.
    AnotherAccount,
    /// The served account designated through ANOTHER Region's endpoint only.
    AnotherRegion,
    /// The served account designated, then removed, with the clock past the removal.
    RemovalSettled,
}

/// Every standing that does NOT authorize the caller, as the rows of the table below.
const WITHOUT_STANDING: [(&str, Standing); 4] = [
    ("no delegated administrator at all", Standing::Undesignated),
    (
        "another account is the delegated administrator",
        Standing::AnotherAccount,
    ),
    (
        "the designation was made through another Region's endpoint",
        Standing::AnotherRegion,
    ),
    (
        "the designation's removal has fallen due",
        Standing::RemovalSettled,
    ),
];

/// Leave `region`'s organization in `standing`.
async fn establish(router: &Router, region: &str, standing: Standing) {
    match standing {
        Standing::Undesignated => {}
        Standing::AnotherAccount => {
            let (status, body) = enable_admin(router, region, ANOTHER_ACCOUNT_ID).await;
            assert_eq!(
                status, 200,
                "designating another account: expected 200, got {status} {body:?}"
            );
        }
        Standing::AnotherRegion => {
            as_delegated_admin(router, OTHER_REGION).await;
        }
        Standing::RemovalSettled => {
            as_delegated_admin(router, region).await;
            let account_id =
                service_com_amazonaws_guardduty_guarddutyapiservice::state::administrator_account_id();
            let (status, body) = disable_admin(router, region, &account_id).await;
            assert_eq!(
                status, 200,
                "removing the designation: expected 200, got {status} {body:?}"
            );
            tokio::time::sleep(past_the_removal()).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rules 1 + 2: every one of the five refuses a caller without the standing.
// ---------------------------------------------------------------------------

/// Every organization-configuration operation refuses a caller that is not the
/// organization's delegated administrator, over rows differing in WHY the standing is
/// absent and in whether the pair the request names is configured at all — so the
/// refusal is shown to come before the resource is resolved.
#[tokio::test]
async fn an_org_configuration_operation_refuses_a_caller_without_the_standing() {
    let catalogue = published_rule_ids(&new_router()).await;

    for (why, standing) in WITHOUT_STANDING {
        for configured in [false, true] {
            for operation in &OPERATIONS {
                // One store per case, so no case can see what another one wrote.
                let router = new_router();
                let rule_id = catalogue[0].as_str();
                let case = format!(
                    "{}: {why}, the pair {}",
                    operation.name,
                    if configured {
                        "already configured"
                    } else {
                        "configured by nobody"
                    }
                );

                if configured {
                    // Configured by the delegated administrator, which is the only
                    // caller that can — then the standing is taken away.
                    as_delegated_admin(&router, HOME_REGION).await;
                    let (status, body) = call_in(
                        &router,
                        HOME_REGION,
                        "POST",
                        ORG_CONFIGURATION_PATH,
                        Some(json!({"ruleId": rule_id, "mode": MODES[0]})),
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: create-org-configuration expected 200, got {status} {body:?}"
                    );
                    let account_id =
                        service_com_amazonaws_guardduty_guarddutyapiservice::state::administrator_account_id();
                    let (status, body) = disable_admin(&router, HOME_REGION, &account_id).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: removing the designation expected 200, got {status} {body:?}"
                    );
                    tokio::time::sleep(past_the_removal()).await;
                }

                establish(&router, HOME_REGION, standing).await;

                let (status, body) = send(&router, HOME_REGION, operation, rule_id).await;
                assert_not_authorized(&case, status, &body);

                // A rule the catalogue does not publish is answered the SAME way:
                // the standing is judged before the rule id is resolved.
                let (status, body) =
                    send(&router, HOME_REGION, operation, "no-such-published-rule").await;
                assert_not_authorized(&format!("{case}, for an unpublished rule"), status, &body);
            }
        }
    }
}

/// The complement: with the standing in place every one of the five is SERVED — each
/// over a store of its own, so each is answered about a rule nothing has configured
/// and the status is the one its own contract gives — and a removal that has been
/// accepted but is not yet due leaves the standing in place, so the gate turns on the
/// designation and on nothing else.
#[tokio::test]
async fn the_delegated_administrator_is_served_by_all_five() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].as_str();

    for operation in &OPERATIONS {
        let router = new_router();
        as_delegated_admin(&router, HOME_REGION).await;
        let (status, body) = send(&router, HOME_REGION, operation, rule_id).await;
        assert_eq!(
            status, operation.served_status,
            "case {:?}: expected the delegated administrator to be answered {}, got {status} {body:?}",
            operation.name, operation.served_status
        );
        assert_ne!(
            error_shape(&body),
            ACCESS_DENIED,
            "case {:?}: the delegated administrator must not be refused, got {status} {body:?}",
            operation.name
        );
    }

    // The create/read round trip, as the delegated administrator: what the create
    // records is what the read reports.
    as_delegated_admin(&router, HOME_REGION).await;
    let round_trip_rule = catalogue[1].as_str();
    let (status, body) = call_in(
        &router,
        HOME_REGION,
        "POST",
        ORG_CONFIGURATION_PATH,
        Some(json!({"ruleId": round_trip_rule, "mode": MODES[1], "includeAccountIds": [ACCOUNTS[0]]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-org-configuration as the delegated administrator: expected 200, got {status} {body:?}"
    );
    let (status, read) = call_in(
        &router,
        HOME_REGION,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{round_trip_rule}?mode={}", MODES[1]),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-org-configuration as the delegated administrator: expected 200, got {status} {read:?}"
    );
    assert_eq!(
        read["configuration"]["includeAccountIds"],
        json!([ACCOUNTS[0]]),
        "expected the scope the create recorded, got {read:?}"
    );

    // A removal that is accepted but not yet due leaves the designation in place, so
    // the same read is still served.
    let account_id =
        service_com_amazonaws_guardduty_guarddutyapiservice::state::administrator_account_id();
    let (status, body) = disable_admin(&router, HOME_REGION, &account_id).await;
    assert_eq!(
        status, 200,
        "removing the designation: expected 200, got {status} {body:?}"
    );
    let (status, pending) = call_in(
        &router,
        HOME_REGION,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{round_trip_rule}?mode={}", MODES[1]),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "a removal still in flight leaves the standing in place, got {status} {pending:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a refused call changes nothing.
// ---------------------------------------------------------------------------

/// A configuration the delegated administrator recorded survives every refused call
/// the five operations can make, and the listing still reports exactly it.
#[tokio::test]
async fn a_refused_call_leaves_the_configuration_alone() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].as_str();

    as_delegated_admin(&router, HOME_REGION).await;
    let (status, body) = call_in(
        &router,
        HOME_REGION,
        "POST",
        ORG_CONFIGURATION_PATH,
        Some(json!({"ruleId": rule_id, "mode": MODES[0], "excludeAccountIds": [ACCOUNTS[1]]})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-org-configuration: expected 200, got {status} {body:?}"
    );
    let before = stored_facts(&read_as_admin(&router, rule_id, MODES[0], "the baseline").await);

    // The standing goes away, and every one of the five is refused.
    let account_id =
        service_com_amazonaws_guardduty_guarddutyapiservice::state::administrator_account_id();
    let (status, body) = disable_admin(&router, HOME_REGION, &account_id).await;
    assert_eq!(
        status, 200,
        "removing the designation: expected 200, got {status} {body:?}"
    );
    tokio::time::sleep(past_the_removal()).await;
    for operation in &OPERATIONS {
        let (status, body) = send(&router, HOME_REGION, operation, rule_id).await;
        assert_not_authorized(operation.name, status, &body);
    }

    // The standing comes back, and the configuration is exactly what it was. Only
    // the STORED facts are compared: `Status` is derived from the roll-out instant
    // the create stored, so the clock — not a refused call — moves it.
    let after = stored_facts(&read_as_admin(&router, rule_id, MODES[0], "after the refusals").await);
    assert_eq!(
        after, before,
        "a refused call must change nothing, got {after:?}"
    );
    let (status, listed) = call_in(
        &router,
        HOME_REGION,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}?maxResults=100"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-org-configurations as the delegated administrator: expected 200, got {status} {listed:?}"
    );
    let pairs: Vec<Value> = listed["configurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        pairs.len(),
        1,
        "expected the one configuration the create recorded, got {listed:?}"
    );
    assert_eq!(
        pairs[0]["ruleId"],
        json!(rule_id),
        "expected the rule the create named, got {listed:?}"
    );
}

/// The members of a reported configuration that a STORE holds, with the derived
/// `Status` left out: the configuration's roll-out is a transition the clock owes, so
/// a comparison across a wait is about what was written and not about that.
fn stored_facts(configuration: &Value) -> Value {
    json!({
        "ruleId": configuration["ruleId"],
        "mode": configuration["mode"],
        "includeAccountIds": configuration["includeAccountIds"],
        "excludeAccountIds": configuration["excludeAccountIds"],
        "createdAt": configuration["createdAt"],
        "updatedAt": configuration["updatedAt"],
    })
}

/// The configuration for `(rule_id, mode)` as the delegated administrator reads it,
/// re-establishing the standing first.
async fn read_as_admin(router: &Router, rule_id: &str, mode: &str, case: &str) -> Value {
    as_delegated_admin(router, HOME_REGION).await;
    let (status, body) = call_in(
        router,
        HOME_REGION,
        "GET",
        &format!("{ORG_CONFIGURATION_PATH}/{rule_id}?mode={mode}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-org-configuration expected 200, got {status} {body:?}"
    );
    body["configuration"].clone()
}

// ---------------------------------------------------------------------------
// Rule 4: the association delete is idempotent.
// ---------------------------------------------------------------------------

/// A delete naming an association the endpoint resolves nothing for answers the empty
/// 200 and removes nothing, over rows differing in WHY it resolves to nothing — while
/// the read and the update of that same pair stay the modelled 404 and name the
/// association id the request carried.
#[tokio::test]
async fn deleting_an_absent_association_succeeds_and_removes_nothing() {
    let router = new_router();
    let catalogue = published_rule_ids(&router).await;
    let rule_id = catalogue[0].clone();
    let other_rule = catalogue[1].clone();

    // One association this Region's endpoint holds, and one another Region's does, so
    // every empty delete below can be shown to have left both alone.
    let held = associate(&router, HOME_REGION, &rule_id, MODES[0]).await;
    let elsewhere = associate(&router, OTHER_REGION, &other_rule, MODES[1]).await;
    let before = read_association(&router, HOME_REGION, &rule_id, &held).await;

    for (case, path_rule_id, path_association_id) in [
        (
            "an associationId no create ever minted",
            rule_id.as_str(),
            ABSENT_ASSOCIATION_ID,
        ),
        (
            "a minted associationId under another published rule",
            other_rule.as_str(),
            held.as_str(),
        ),
        (
            "an association held only by another Region's endpoint",
            other_rule.as_str(),
            elsewhere.as_str(),
        ),
        (
            "a well-formed ruleId the catalogue does not publish",
            "no-such-published-rule",
            held.as_str(),
        ),
        (
            "the shortest associationId the member allows, never minted",
            rule_id.as_str(),
            "a",
        ),
        (
            // `len 1..64` (`smithy_query(operation="DeleteCustomDetectionRuleAssociation")`):
            // 64 characters is inside the bound, so this reaches the handler.
            "the longest associationId the member allows, never minted",
            rule_id.as_str(),
            "a234567890123456789012345678901234567890123456789012345678901234",
        ),
    ] {
        let path = format!("{RULES_PATH}/{path_rule_id}/association/{path_association_id}");
        let (status, body) = call_in(&router, HOME_REGION, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an idempotent delete of an absent association expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body.as_object().map(serde_json::Map::len).unwrap_or(0),
            0,
            "case {case:?}: the response shape declares no members, got {body:?}"
        );

        // The read of the same pair is still the modelled 404, naming the id asked
        // for: the delete's silence is about the delete and not about the resource.
        let (status, read) = call_in(&router, HOME_REGION, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: get-association for an absent pair expected 404, got {status} {read:?}"
        );
        assert_eq!(
            error_code(&read),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND} in __type, got {read:?}"
        );
        assert_eq!(
            read["message"],
            json!(format!("Association not found: {path_association_id}")),
            "case {case:?}: the message names the association id the request carried, got {read:?}"
        );

        // The update of the same pair is the same 404, with the same message.
        let (status, updated) = call_in(
            &router,
            HOME_REGION,
            "PUT",
            &path,
            Some(json!({"mode": MODES[1]})),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: update-association for an absent pair expected 404, got {status} {updated:?}"
        );
        assert_eq!(
            updated["message"],
            json!(format!("Association not found: {path_association_id}")),
            "case {case:?}: the message names the association id the request carried, got {updated:?}"
        );
    }

    // Nothing above removed anything.
    let after = read_association(&router, HOME_REGION, &rule_id, &held).await;
    assert_eq!(
        after, before,
        "a delete of a pair that resolves to nothing must change nothing, got {after:?}"
    );
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "GET",
        &format!("{RULES_PATH}/{other_rule}/association/{elsewhere}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the other Region must still hold its association, got {status} {body:?}"
    );

    // And the delete of the association that IS there removes it, after which the
    // replayed delete is the empty 200 too — the post-condition is already met.
    let path = format!("{RULES_PATH}/{rule_id}/association/{held}");
    let (status, body) = call_in(&router, HOME_REGION, "DELETE", &path, None).await;
    assert_eq!(
        status, 200,
        "deleting the association this Region holds: expected 200, got {status} {body:?}"
    );
    let (status, read) = call_in(&router, HOME_REGION, "GET", &path, None).await;
    assert_eq!(
        status, 404,
        "the deleted association must be gone, got {status} {read:?}"
    );
    let (status, replayed) = call_in(&router, HOME_REGION, "DELETE", &path, None).await;
    assert_eq!(
        status, 200,
        "the replayed delete expected 200, got {status} {replayed:?}"
    );
}

/// Associate `region`'s account with `rule_id` in `mode`, returning the minted
/// `AssociationId`.
async fn associate(router: &Router, region: &str, rule_id: &str, mode: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        ASSOCIATIONS_PATH,
        Some(json!({"ruleId": rule_id, "mode": mode})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"]["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-association: expected an associationId, got {body:?}"))
        .to_string()
}

/// The whole read of one association, failing the test if it did not succeed.
async fn read_association(
    router: &Router,
    region: &str,
    rule_id: &str,
    association_id: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{RULES_PATH}/{rule_id}/association/{association_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-association {rule_id}/{association_id}: expected 200, got {status} {body:?}"
    );
    body
}
