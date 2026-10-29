// SPDX-License-Identifier: CC0-1.0

//! The form an `ExpectedBucketOwner` may take on this service's list resources,
//! end to end over the wire.
//!
//! The rule under test: a member documented as an Amazon Web Services account ID
//! carries an account id and nothing else — exactly 12 ASCII digits — so a value
//! that cannot be one is refused with HTTP 400 and persists nothing, even where the
//! modelled constraint on the member is a LENGTH only (`AccountId`, len 12..12,
//! no pattern — `smithy_query(query="shape",shape="CreateIPSetRequest")`), which
//! the request boundary cannot refuse. The member is "the Amazon Web Services
//! account ID that owns the Amazon S3 bucket specified in the location parameter"
//! (`smithy_query(query="doc",shape="CreateIPSetRequest",member="ExpectedBucketOwner")`),
//! the same sentence on `CreateThreatIntelSet`, `UpdateIPSet`,
//! `UpdateThreatIntelSet` and the entity-set family.
//!
//! Every assertion runs over a TABLE of values spanning what the rule generalises
//! over — which character class the value leaves the digits for, where in the value
//! that character sits, the digit values themselves at both extremes, and whether
//! the value arrives on a create or on an update — rather than over one recorded
//! value. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in
//! the `detector_lifecycle` suite), so routing, restJson1 (de)serialization, the
//! modelled-constraint walk and the handlers are exercised together.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call, enabled_detector, error_code, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::LIST_EXPECTED_BUCKET_OWNER_MEMBER;

/// The error class every client rejection this service's handlers answer carries in
/// the body's `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The two list resources whose `ExpectedBucketOwner` targets `AccountId`, so the
/// handler is the enforcement point: each row is the URI collection, the member the
/// create's response carries the minted id in, and the member the list operation
/// reports ids under (`smithy_query(operation="CreateIPSet,ListIPSets")` and the
/// threat-list pair).
const LIST_RESOURCES: [(&str, &str, &str); 2] = [
    ("ipset", "ipSetId", "ipSetIds"),
    ("threatintelset", "threatIntelSetId", "threatIntelSetIds"),
];

/// Values that ARE account ids: exactly 12 ASCII digits. The rows span the digit
/// range at both extremes and in between, because the rule is about the form and
/// not about one recorded account.
const OWNERS_IN_FORM: [(&str, &str); 5] = [
    ("the all-zero account id", "000000000000"),
    ("the all-nine account id", "999999999999"),
    ("ascending digits", "123456789012"),
    ("descending digits", "210987654321"),
    ("a repeated-digit account id", "111122223333"),
];

/// Values of the modelled length that cannot be an account id at all. Each row
/// differs in WHICH non-digit class it uses and WHERE in the value it sits, so no
/// row passes by accident, and every row is exactly 12 characters long — the
/// modelled `len 12..12` — so what refuses it is the FORM check and not a length.
const OWNERS_OUT_OF_FORM: [(&str, &str); 10] = [
    ("letters only", "abcdefghijkl"),
    ("a word followed by digits", "owner1234567"),
    ("punctuation that looks like an ARN prefix", "arn:aws:s3::"),
    ("a single trailing letter", "12345678901a"),
    ("a single leading letter", "a12345678901"),
    ("a letter in the middle", "12345x789012"),
    ("an embedded space", "12345 789012"),
    ("a dash, as a formatted id might carry", "1234-5678-90"),
    ("a plus sign", "+12345678901"),
    ("digits with a trailing newline", "12345678901\n"),
];

/// A create body carrying `owner` as `expectedBucketOwner` when one is given. The
/// other members are the ones the model requires, and are the same on every list
/// resource here.
fn create_body(name: &str, owner: Option<&str>) -> Value {
    let mut body = json!({
        "name": name,
        "format": "TXT",
        "location": "s3://someone-elses-bucket/lists/ips.txt",
        "activate": false
    });
    if let Some(owner) = owner {
        body["expectedBucketOwner"] = json!(owner);
    }
    body
}

async fn create_list(
    router: &Router,
    detector_id: &str,
    collection: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{collection}"),
        Some(body),
    )
    .await
}

async fn update_list(
    router: &Router,
    detector_id: &str,
    collection: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        Some(body),
    )
    .await
}

async fn get_list(router: &Router, detector_id: &str, collection: &str, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}/{id}"),
        None,
    )
    .await
}

/// The ids the collection's list operation reports under `detector_id`.
async fn listed_ids(
    router: &Router,
    detector_id: &str,
    collection: &str,
    ids_member: &str,
    case: &str,
) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/{collection}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: listing {collection} expected 200, got {status} {body:?}"
    );
    body[ids_member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// The `expectedBucketOwner` the read reports, or `None` when the read reports none.
async fn stored_owner(
    router: &Router,
    detector_id: &str,
    collection: &str,
    id: &str,
    case: &str,
) -> Option<String> {
    let (status, body) = get_list(router, detector_id, collection, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the {collection} back expected 200, got {status} {body:?}"
    );
    body.get("expectedBucketOwner")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert one refusal: HTTP 400 under the service's client-rejection class, naming
/// the MEMBER at fault and never echoing the caller's value.
fn assert_refused(case: &str, owner: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: an owner that cannot be an account id must be refused with 400, \
         got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(LIST_EXPECTED_BUCKET_OWNER_MEMBER),
        "case {case:?}: the refusal must name {LIST_EXPECTED_BUCKET_OWNER_MEMBER}, \
         got {message:?}"
    );
    assert!(
        !message.contains(owner),
        "case {case:?}: the refusal must not echo the rejected value, got {message:?}"
    );
}

#[tokio::test]
async fn an_owner_in_account_id_form_round_trips_on_every_list_create() {
    for (collection, id_member, _) in LIST_RESOURCES {
        // One router per collection: a detector holds a bounded number of lists, and
        // the rows below stay inside the smaller of the two quotas.
        let router = new_router();
        let detector = enabled_detector(&router).await;

        for (case, owner) in OWNERS_IN_FORM {
            let case = format!("{collection}: {case}");
            let (status, body) = create_list(
                &router,
                &detector,
                collection,
                create_body("owned", Some(owner)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: an owner in account-id form must be accepted, got {status} {body:?}"
            );
            let id = body[id_member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a {id_member}, got {body:?}"))
                .to_string();
            let actual = stored_owner(&router, &detector, collection, &id, &case).await;
            assert_eq!(
                actual.as_deref(),
                Some(owner),
                "case {case:?}: expected the stored owner {owner:?}, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_whose_owner_is_not_an_account_id_is_refused_and_persists_nothing() {
    for (collection, id_member, ids_member) in LIST_RESOURCES {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        for (case, owner) in OWNERS_OUT_OF_FORM {
            let case = format!("{collection}: {case}");
            let (status, body) = create_list(
                &router,
                &detector,
                collection,
                create_body("rejected", Some(owner)),
            )
            .await;
            assert_refused(&case, owner, status, &body);
            assert!(
                body.get(id_member).is_none_or(Value::is_null),
                "case {case:?}: a refused create must mint no {id_member}, got {body:?}"
            );
            let listed = listed_ids(&router, &detector, collection, ids_member, &case).await;
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused create must persist nothing, the listing reported \
                 {listed:?}"
            );
        }

        // The detector is otherwise healthy: a value in account-id form still mints a
        // list, so the refusals above are about the value and nothing else.
        let case = format!("{collection}: an accepted owner after the refusals");
        let (status, body) = create_list(
            &router,
            &detector,
            collection,
            create_body("accepted", Some("123456789012")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let listed = listed_ids(&router, &detector, collection, ids_member, &case).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly the one accepted list, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_update_is_held_to_the_same_owner_form_as_the_create() {
    for (collection, id_member, _) in LIST_RESOURCES {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        let base_owner = "123456789012";
        let (status, body) = create_list(
            &router,
            &detector,
            collection,
            create_body("base", Some(base_owner)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"{collection}: the base list\": expected 200, got {status} {body:?}"
        );
        let id = body[id_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case \"{collection}: the base list\": expected a {id_member}, got {body:?}")
            })
            .to_string();

        // An update carrying a value that cannot be an account id is refused and
        // leaves the stored owner exactly where the create put it.
        for (case, owner) in OWNERS_OUT_OF_FORM {
            let case = format!("{collection}: update with {case}");
            let (status, body) = update_list(
                &router,
                &detector,
                collection,
                &id,
                json!({"expectedBucketOwner": owner}),
            )
            .await;
            assert_refused(&case, owner, status, &body);
            let actual = stored_owner(&router, &detector, collection, &id, &case).await;
            assert_eq!(
                actual.as_deref(),
                Some(base_owner),
                "case {case:?}: a refused update must leave the stored owner alone, \
                 got {actual:?}"
            );
        }

        // An update carrying one in form lands, and the read reports it verbatim.
        for (case, owner) in OWNERS_IN_FORM {
            let case = format!("{collection}: update with {case}");
            let (status, body) = update_list(
                &router,
                &detector,
                collection,
                &id,
                json!({"expectedBucketOwner": owner}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: an owner in account-id form must be accepted, got {status} {body:?}"
            );
            let actual = stored_owner(&router, &detector, collection, &id, &case).await;
            assert_eq!(
                actual.as_deref(),
                Some(owner),
                "case {case:?}: expected the updated owner {owner:?}, got {actual:?}"
            );
        }

        // An update that carries no owner at all is untouched by the rule, and leaves
        // the last accepted value in place.
        let case = format!("{collection}: an update with no expectedBucketOwner");
        let (status, body) = update_list(
            &router,
            &detector,
            collection,
            &id,
            json!({"location": "s3://someone-elses-bucket/lists/moved.txt"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let expected = OWNERS_IN_FORM.last().expect("a table row").1;
        let actual = stored_owner(&router, &detector, collection, &id, &case).await;
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected the last accepted owner {expected:?}, got {actual:?}"
        );
    }
}

/// A value of the modelled LENGTH in characters whose digits are not ASCII digits
/// is refused too: the rule is the account-id form, not "twelve of something".
#[tokio::test]
async fn an_owner_of_non_ascii_digits_is_refused() {
    // Twelve FULLWIDTH DIGIT code points: twelve characters that no account id is
    // written with.
    let owner = "１２３４５６７８９０１２";

    for (collection, id_member, ids_member) in LIST_RESOURCES {
        let router = new_router();
        let detector = enabled_detector(&router).await;
        let case = format!("{collection}: fullwidth digits");

        let (status, body) = create_list(
            &router,
            &detector,
            collection,
            create_body("fullwidth", Some(owner)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an owner written in non-ASCII digits must be refused with 400, \
             got {status} {body:?}"
        );
        assert!(
            body.get(id_member).is_none_or(Value::is_null),
            "case {case:?}: a refused create must mint no {id_member}, got {body:?}"
        );
        let listed = listed_ids(&router, &detector, collection, ids_member, &case).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused create must persist nothing, the listing reported {listed:?}"
        );
    }
}

/// The rule is the member's, not one operation's: the sibling entity sets carry the
/// same sentence on a member the model gives a digits-only pattern
/// (`smithy_query(query="shape",shape="CreateThreatEntitySetRequest")`), so the same
/// values are refused there as well — at the boundary rather than in the handler,
/// which is why the class is not asserted here, only the refusal.
#[tokio::test]
async fn the_entity_set_family_refuses_the_same_values() {
    for collection in ["threatentityset", "trustedentityset"] {
        let router = new_router();
        let detector = enabled_detector(&router).await;

        for (case, owner) in OWNERS_OUT_OF_FORM {
            let case = format!("{collection}: {case}");
            let (status, body) = create_list(
                &router,
                &detector,
                collection,
                create_body("entity-set", Some(owner)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: an owner that cannot be an account id must be refused with 400, \
                 got {status} {body:?}"
            );
        }

        // And one in form is accepted, so the refusals above are about the value.
        let case = format!("{collection}: an owner in account-id form");
        let (status, body) = create_list(
            &router,
            &detector,
            collection,
            create_body("entity-set", Some("123456789012")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
    }
}
