// SPDX-License-Identifier: CC0-1.0

//! The character set a GuardDuty IPSet `Name` may use, end to end over the wire.
//!
//! The rule under test: where the model DOCUMENTS the characters a member allows,
//! a value carrying any other character is refused with HTTP 400 and persists
//! nothing, even though the modelled constraint on the member is a length only.
//! For the IPSet name that alphabet is "alphanumeric, whitespace, dash (-), and
//! underscores (_)"
//! (`smithy_query(query="doc",shape="CreateIPSetRequest",member="Name")`; the
//! member is `Name`, len 1..300, no pattern), and `UpdateIPSet` carries the same
//! member, so both operations are held to it.
//!
//! Every assertion runs over a TABLE of names spanning the classes the rule
//! generalises over — which allowed class the name uses, which disallowed
//! character it carries, where in the name it sits, and the size of the name at
//! both ends of the modelled length — rather than over one recorded value. Every
//! request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite).

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::IP_SET_NAME_MEMBER;

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// Names built only from the documented alphabet (alphanumeric, whitespace, dash,
/// underscore — the member's own documentation, quoted in this file's header), one
/// row per class it names plus both ends of the modelled `len 1..300`.
fn names_in_the_alphabet() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the model accepts", "a".to_string()),
        ("digits only", "0123456789".to_string()),
        ("mixed-case letters", "TrustedList".to_string()),
        ("the whitespace the alphabet lists", "trusted ip list".to_string()),
        ("dash and underscore", "trusted-ip_list".to_string()),
        (
            "every allowed class at once",
            "Trusted IP list 02 - prod_v1".to_string(),
        ),
        (
            "the longest name the model accepts, all of it in the alphabet",
            "n".repeat(300),
        ),
    ]
}

/// Names carrying a character the documentation does not list. Each row differs in
/// WHICH character is out of the alphabet and WHERE it sits, including a name that
/// is legal for a FILTER name (the period) to show the alphabet is this member's,
/// and one at the modelled maximum length so the refusal is not a length check.
fn names_outside_the_alphabet() -> Vec<(&'static str, String)> {
    vec![
        ("a slash", "bad/name".to_string()),
        ("an exclamation mark", "bad name!".to_string()),
        (
            "a period, which a filter name allows and this member does not",
            "list.v1".to_string(),
        ),
        ("a comma", "list,v2".to_string()),
        ("a colon", "colon:name".to_string()),
        ("a plus sign", "plus+name".to_string()),
        ("a percent escape", "name%20quoted".to_string()),
        ("an at sign in the leading position", "@list".to_string()),
        (
            "a tab, which is not whitespace a displayed name carries",
            "with\ttab".to_string(),
        ),
        ("a newline", "with\nnewline".to_string()),
        ("an ASCII control character", "null\u{0}byte".to_string()),
        ("arbitrary non-ASCII text", "リスト – トラスト".to_string()),
        ("an emoji", "rocket 🚀 list".to_string()),
        (
            "the modelled maximum length with one character outside the alphabet",
            format!("{}/", "n".repeat(299)),
        ),
    ]
}

/// A create body carrying `name`; the other members are the ones the model requires.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "format": "TXT",
        "location": "s3://my-bucket/lists/trusted.txt",
        "activate": false
    })
}

async fn create_ip_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/ipset"),
        Some(body),
    )
    .await
}

async fn update_ip_set(
    router: &Router,
    detector_id: &str,
    ip_set_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        Some(body),
    )
    .await
}

/// The `Name` `GetIPSet` reports for `ip_set_id`.
async fn stored_name(router: &Router, detector_id: &str, ip_set_id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/ipset/{ip_set_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-ip-set expected 200, got {status} {body:?}"
    );
    body["name"].as_str().unwrap_or_default().to_string()
}

/// The ids `ListIPSets` reports under `detector_id`.
async fn listed_ids(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/ipset"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-ip-sets expected 200, got {status} {body:?}"
    );
    body["ipSetIds"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// Assert a refusal: HTTP 400 under the service's client-rejection class, naming
/// the MEMBER at fault and never echoing the caller's value.
fn assert_refused(case: &str, name: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: a name outside the documented alphabet must be refused with 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(IP_SET_NAME_MEMBER),
        "case {case:?}: the refusal must name {IP_SET_NAME_MEMBER}, got {message:?}"
    );
    assert!(
        !message.contains(name),
        "case {case:?}: the refusal must not echo the rejected name, got {message:?}"
    );
}

#[tokio::test]
async fn a_name_inside_the_documented_alphabet_round_trips() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (case, name) in names_in_the_alphabet() {
        let (status, body) = create_ip_set(&router, &detector, create_body(&name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a name inside the documented alphabet must be accepted, got {status} {body:?}"
        );
        let id = body["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {body:?}"))
            .to_string();
        let actual = stored_name(&router, &detector, &id, case).await;
        assert_eq!(
            actual, name,
            "case {case:?}: expected the stored name {name:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_create_whose_name_leaves_the_alphabet_is_refused_and_persists_nothing() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    for (case, name) in names_outside_the_alphabet() {
        let (status, body) = create_ip_set(&router, &detector, create_body(&name)).await;
        assert_refused(case, &name, status, &body);
        assert!(
            body.get("ipSetId").is_none_or(Value::is_null),
            "case {case:?}: a refused create must mint no ipSetId, got {body:?}"
        );
        let listed = listed_ids(&router, &detector, case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused create must persist nothing, ListIPSets reported {listed:?}"
        );
    }

    // The detector is otherwise healthy: a name inside the alphabet still mints an
    // IPSet, so the refusals above are about the value and nothing else.
    let case = "an accepted name after the refusals";
    let (status, body) = create_ip_set(&router, &detector, create_body("after refusals")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let listed = listed_ids(&router, &detector, case).await;
    assert_eq!(
        listed.len(),
        1,
        "case {case:?}: expected exactly the one accepted IPSet, got {listed:?}"
    );
}

#[tokio::test]
async fn an_update_is_held_to_the_same_alphabet_as_the_create() {
    let router = new_router();
    let detector = enabled_detector(&router).await;

    let (status, body) = create_ip_set(&router, &detector, create_body("base list")).await;
    assert_eq!(
        status, 200,
        "case \"the base IPSet\": expected 200, got {status} {body:?}"
    );
    let id = body["ipSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the base IPSet\": expected an ipSetId, got {body:?}"))
        .to_string();

    // A rename out of the alphabet is refused and changes nothing a later read sees.
    for (case, name) in names_outside_the_alphabet() {
        let (status, body) = update_ip_set(&router, &detector, &id, json!({"name": name})).await;
        assert_refused(case, &name, status, &body);
        let actual = stored_name(&router, &detector, &id, case).await;
        assert_eq!(
            actual, "base list",
            "case {case:?}: a refused rename must leave the stored name alone, got {actual:?}"
        );
    }

    // A rename inside it lands, and the read reports it verbatim.
    for (case, name) in names_in_the_alphabet() {
        let (status, body) = update_ip_set(&router, &detector, &id, json!({"name": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a name inside the documented alphabet must be accepted, got {status} {body:?}"
        );
        let actual = stored_name(&router, &detector, &id, case).await;
        assert_eq!(
            actual, name,
            "case {case:?}: expected the renamed IPSet to report {name:?}, got {actual:?}"
        );
    }

    // An update that carries no name at all is untouched by the rule.
    let case = "an update with no name member";
    let (status, body) = update_ip_set(
        &router,
        &detector,
        &id,
        json!({"location": "s3://my-bucket/lists/moved.txt"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body:?}"
    );
    let expected = names_in_the_alphabet().pop().expect("a table row").1;
    let actual = stored_name(&router, &detector, &id, case).await;
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the last stored name {expected:?}, got {actual:?}"
    );
}

/// The alphabet is the one THAT MEMBER documents, not one alphabet for every name
/// on the service: the IPSet name lists whitespace, the entity-set name does not
/// ("lowercase letters, uppercase letters, numbers, dash (-), and underscore (_)"
/// — `smithy_query(query="doc",shape="CreateThreatEntitySetRequest",member="Name")`),
/// so the same name is accepted by one create and refused by the other.
#[tokio::test]
async fn whitespace_is_in_the_ip_set_alphabet_and_not_the_entity_set_one() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let name = "list with spaces";

    let (status, body) = create_ip_set(&router, &detector, create_body(name)).await;
    assert_eq!(
        status, 200,
        "case \"ipset\": whitespace is in the IPSet name alphabet, got {status} {body:?}"
    );

    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{detector}/threatentityset"),
        Some(json!({
            "name": name,
            "format": "TXT",
            "location": "s3://my-bucket/lists/threats.txt",
            "activate": false
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"threatentityset\": whitespace is not in the entity-set name alphabet, got {status} {body:?}"
    );
    let class = error_code(&body);
    assert_eq!(
        class, INVALID_INPUT,
        "case \"threatentityset\": expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
}
