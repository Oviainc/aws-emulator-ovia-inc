// SPDX-License-Identifier: CC0-1.0

//! `UpdateTerms` over the wire, driven through the router exactly as an SDK
//! drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.UpdateTerms`).
//!
//! The rule under test: `UpdateTerms` modifies the STORED set of terms documents
//! the submitted `TermsId` names in the submitted pool — an updatable member the
//! request carries replaces what was stored, a member it leaves out keeps its
//! stored value, and the resource's identity (its `TermsId`, its pool, its app
//! client and its creation instant) survives every update while its
//! last-modified instant advances. The response is the stored record, so a later
//! `DescribeTerms` renders the same thing. A document's name stays unique to its
//! app client, so a rename onto a name that app client already holds other
//! documents under is refused and stores nothing, while the same name on another
//! app client is free; an unknown pool or `TermsId` is a 404 and a request
//! outside a modelled constraint is a 400. Each table row names the case it
//! asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Every language `LinksType`'s key pattern admits, in the order the pattern
/// lists them:
/// `^cognito:(default|dutch|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional))$`
/// from `smithy_query(query="shape",shape="LinksType")` — thirteen values, which
/// is also the map's modelled length maximum.
const LANGUAGES: [&str; 13] = [
    "cognito:default",
    "cognito:dutch",
    "cognito:english",
    "cognito:french",
    "cognito:spanish",
    "cognito:german",
    "cognito:bahasa-indonesia",
    "cognito:italian",
    "cognito:japanese",
    "cognito:korean",
    "cognito:portuguese-brazil",
    "cognito:chinese-simplified",
    "cognito:chinese-traditional",
];

/// The two document types `TermsNameType`'s pattern admits
/// (`^(terms-of-use|privacy-policy)$`, from
/// `smithy_query(query="operation",operation="UpdateTerms")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

/// The members of `TermsType` that no update may disturb or that the case's
/// expectation pins exactly; `LastModifiedDate` is checked separately because it
/// advances.
const PINNED_MEMBERS: [&str; 8] = [
    "TermsId",
    "UserPoolId",
    "ClientId",
    "TermsName",
    "TermsSource",
    "Enforcement",
    "Links",
    "CreationDate",
];

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client in `user_pool_id` and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// A `Links` map as the wire carries it.
fn links_json(links: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (language, url) in links {
        map.insert((*language).to_string(), json!(url));
    }
    Value::Object(map)
}

/// Create one set of terms documents and return the stored `Terms` record.
async fn create_terms(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    terms_name: &str,
    links: &[(&str, &str)],
) -> Value {
    let created = ok(
        router,
        "CreateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": terms_name,
            // The only values `TermsSourceType` and `TermsEnforcementType` admit
            // (`smithy_query(query="shape",shape="TermsSourceType")`).
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": links_json(links),
        }),
    )
    .await;
    created["Terms"].clone()
}

/// Read one set of terms documents back through the implemented reader.
async fn describe_terms(router: &MultiServiceRouter, user_pool_id: &str, terms_id: &str) -> Value {
    let read = ok(
        router,
        "DescribeTerms",
        &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
    )
    .await;
    read["Terms"].clone()
}

/// The `TermsId` of a stored `Terms` record.
fn terms_id_of(terms: &Value) -> String {
    terms["TermsId"]
        .as_str()
        .expect("a minted TermsId")
        .to_string()
}

/// One row of the update table: the documents to create, and the updatable
/// members the `UpdateTerms` request then carries. The rows differ along what
/// the rule generalises over — which members are present, which document type,
/// and the count, ordering, character set and length of the links.
struct UpdateCase {
    case: &'static str,
    created_name: &'static str,
    created_links: Vec<(&'static str, &'static str)>,
    /// The wire members of the update request beyond its `UserPoolId` and
    /// `TermsId`, which is also exactly what the stored record must then carry.
    patch: Vec<(&'static str, Value)>,
}

impl UpdateCase {
    /// This case's `UpdateTerms` request for the stored documents `terms_id` of
    /// `user_pool_id`.
    fn request(&self, user_pool_id: &str, terms_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("TermsId".to_string(), json!(terms_id));
        for (member, value) in &self.patch {
            body.insert((*member).to_string(), value.clone());
        }
        Value::Object(body)
    }

    /// What `TermsType` must carry after this update: the created record with the
    /// submitted members replaced and every other member untouched.
    fn expected(&self, created: &Value) -> Value {
        let mut expected = created.clone();
        for (member, value) in &self.patch {
            expected[*member] = value.clone();
        }
        expected
    }
}

/// The table. The patch spans every subset the input models as optional —
/// `TermsName`, `TermsSource`, `Enforcement`, `Links`, all of them, and none of
/// them — and `Links` spans its modelled length 1..13 and its value's length
/// 1..1024, pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="operation",operation="UpdateTerms")` and
/// `smithy_query(query="shape",shape="LinksType")`.
fn update_cases() -> Vec<UpdateCase> {
    let url_at_maximum = format!(
        "https://terms.example.com/{}",
        "p".repeat(1024 - "https://terms.example.com/".len())
    );
    vec![
        UpdateCase {
            case: "TermsName alone, renamed to the other document type",
            created_name: TERMS_NAMES[0],
            created_links: vec![("cognito:default", "https://terms.example.com")],
            patch: vec![("TermsName", json!(TERMS_NAMES[1]))],
        },
        UpdateCase {
            case: "TermsName alone, re-applying the name the documents already carry",
            created_name: TERMS_NAMES[1],
            created_links: vec![("cognito:default", "https://privacy.example.com")],
            patch: vec![("TermsName", json!(TERMS_NAMES[1]))],
        },
        UpdateCase {
            case: "Links alone, one entry replaced by three submitted out of the pattern's order",
            created_name: TERMS_NAMES[0],
            created_links: vec![("cognito:default", "https://terms.example.com")],
            patch: vec![(
                "Links",
                links_json(&[
                    ("cognito:spanish", "https://terms.example.com/es"),
                    ("cognito:default", "https://terms.example.com"),
                    ("cognito:japanese", "https://terms.example.com/ja"),
                ]),
            )],
        },
        UpdateCase {
            case: "Links alone, all thirteen languages replaced by the map minimum 1",
            created_name: TERMS_NAMES[1],
            created_links: LANGUAGES
                .iter()
                .map(|language| (*language, "https://privacy.example.com"))
                .collect(),
            patch: vec![(
                "Links",
                links_json(&[("cognito:english", "https://privacy.example.com/en")]),
            )],
        },
        UpdateCase {
            case: "Links alone, replaced by the map maximum 13",
            created_name: TERMS_NAMES[0],
            created_links: vec![("cognito:default", "https://terms.example.com")],
            patch: vec![(
                "Links",
                Value::Object(
                    LANGUAGES
                        .iter()
                        .map(|language| {
                            let suffix = language.trim_start_matches("cognito:");
                            (
                                (*language).to_string(),
                                json!(format!("https://terms.example.com/{suffix}")),
                            )
                        })
                        .collect(),
                ),
            )],
        },
        UpdateCase {
            case: "Links alone, replaced by URLs carrying non-Latin letters, marks and symbols",
            created_name: TERMS_NAMES[1],
            created_links: vec![("cognito:default", "https://privacy.example.com")],
            patch: vec![(
                "Links",
                links_json(&[
                    ("cognito:chinese-traditional", "https://例え.example.com/隱私權政策"),
                    ("cognito:korean", "https://example.com/개인정보-정책?세금=€1±2"),
                ]),
            )],
        },
        UpdateCase {
            case: "Links alone, replaced by a URL at the value length maximum 1024",
            created_name: TERMS_NAMES[0],
            created_links: vec![("cognito:default", "https://terms.example.com")],
            patch: vec![("Links", json!({ "cognito:english": url_at_maximum }))],
        },
        UpdateCase {
            case: "TermsSource and Enforcement alone, the only values their enums admit",
            created_name: TERMS_NAMES[1],
            created_links: vec![("cognito:italian", "https://privacy.example.com/it")],
            patch: vec![("TermsSource", json!("LINK")), ("Enforcement", json!("NONE"))],
        },
        UpdateCase {
            case: "every updatable member at once",
            created_name: TERMS_NAMES[0],
            created_links: vec![("cognito:default", "https://terms.example.com")],
            patch: vec![
                ("TermsName", json!(TERMS_NAMES[1])),
                ("TermsSource", json!("LINK")),
                ("Enforcement", json!("NONE")),
                (
                    "Links",
                    links_json(&[
                        ("cognito:german", "https://privacy.example.com/de"),
                        ("cognito:dutch", "https://privacy.example.com/nl"),
                    ]),
                ),
            ],
        },
        UpdateCase {
            case: "no updatable member at all, so every stored member keeps its value",
            created_name: TERMS_NAMES[1],
            created_links: vec![
                ("cognito:default", "https://privacy.example.com"),
                ("cognito:french", "https://privacy.example.com/fr"),
            ],
            patch: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_submitted_members_and_keeps_every_other_stored_value() {
    let router = router();
    let user_pool_id = pool(&router, "update-terms-pool").await;

    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.case;
        // A terms document's name is unique to its app client, so each row gets
        // its own app client of the same pool.
        let client_id = client(&router, &user_pool_id, &format!("update-client-{index}")).await;
        let created = create_terms(
            &router,
            &user_pool_id,
            &client_id,
            case.created_name,
            &case.created_links,
        )
        .await;
        let terms_id = terms_id_of(&created);

        let response = ok(
            &router,
            "UpdateTerms",
            &case.request(&user_pool_id, &terms_id),
        )
        .await;
        let updated = &response["Terms"];
        let expected = case.expected(&created);

        for member in PINNED_MEMBERS {
            assert_eq!(
                updated[member], expected[member],
                "case {label:?}: {member} must be {}, got {updated}",
                expected[member]
            );
        }
        let creation = updated["CreationDate"].as_f64();
        let modified = updated["LastModifiedDate"].as_f64();
        assert!(
            modified.is_some() && modified >= creation,
            "case {label:?}: the last-modified instant must be at or after the creation instant, got {updated}"
        );

        // The round trip: the reader renders the UPDATED stored documents, member
        // for member.
        let read = describe_terms(&router, &user_pool_id, &terms_id).await;
        assert_eq!(
            read, *updated,
            "case {label:?}: DescribeTerms must render the updated terms documents, got {read}"
        );
    }
}

#[tokio::test]
async fn a_rename_onto_a_name_the_app_client_already_holds_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "rename-conflict-pool").await;

    // "Each request accepts one type of terms document ... Supply each type in
    // separate requests." (`UpdateTerms` in this service's model): one app client
    // can hold both document types, and neither may be renamed onto the other's
    // name. Each row renames one type onto the other.
    for (index, (from_name, onto_name)) in [
        (TERMS_NAMES[0], TERMS_NAMES[1]),
        (TERMS_NAMES[1], TERMS_NAMES[0]),
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("renaming {from_name} onto {onto_name}");
        let client_id = client(&router, &user_pool_id, &format!("conflict-{index}")).await;
        let subject = create_terms(
            &router,
            &user_pool_id,
            &client_id,
            from_name,
            &[("cognito:default", "https://subject.example.com")],
        )
        .await;
        let blocker = create_terms(
            &router,
            &user_pool_id,
            &client_id,
            onto_name,
            &[("cognito:default", "https://blocker.example.com")],
        )
        .await;

        let (status, payload) = call(
            &router,
            "UpdateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id_of(&subject),
                "TermsName": onto_name,
                // Links the refusal must NOT have stored either.
                "Links": { "cognito:default": "https://renamed.example.com" },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "TermsExistsException"),
            "case {label:?}: a rename onto a name the app client already holds is a 400 TermsExistsException, got {payload}"
        );
        // The refusal names the name that collided and the app client it
        // collided within: both come from the request and the stored documents,
        // so the sentence is built per request rather than fixed.
        assert_eq!(
            error_message(&payload),
            format!("Terms with name {onto_name} already exists for Client {client_id}."),
            "case {label:?}: the refusal must name the colliding name and the app client, got {payload}"
        );

        // Neither set of documents was touched: the refusal stored nothing.
        for (role, before) in [("renamed", &subject), ("blocking", &blocker)] {
            let read = describe_terms(&router, &user_pool_id, &terms_id_of(before)).await;
            assert_eq!(
                read, *before,
                "case {label:?}: the {role} terms documents must be exactly as they were stored, got {read}"
            );
        }

        // The same name on ANOTHER app client of the same pool is a different
        // resource, so renaming onto it is free: the name is unique to the app
        // client, not to the pool.
        let other_client = client(&router, &user_pool_id, &format!("conflict-other-{index}")).await;
        let other = create_terms(
            &router,
            &user_pool_id,
            &other_client,
            from_name,
            &[("cognito:default", "https://other.example.com")],
        )
        .await;
        let renamed = ok(
            &router,
            "UpdateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id_of(&other),
                "TermsName": onto_name,
            }),
        )
        .await;
        assert_eq!(
            renamed["Terms"]["TermsName"],
            json!(onto_name),
            "case {label:?}: a name only another app client holds is free to take, got {renamed}"
        );
    }
}

#[tokio::test]
async fn both_enforcement_points_of_the_name_uniqueness_rule_word_the_refusal_the_same_way() {
    let router = router();
    let user_pool_id = pool(&router, "terms-exists-wording-pool").await;

    // A name unique to the app client is enforced on a CREATE of a second set of
    // documents under that name and on a RENAME onto it, and both refusals are
    // the same error class, so both say the same sentence about the same name and
    // app client. Each row is one document type, taken from the other.
    for (index, terms_name) in TERMS_NAMES.into_iter().enumerate() {
        let client_id = client(&router, &user_pool_id, &format!("wording-{index}")).await;
        let held = create_terms(
            &router,
            &user_pool_id,
            &client_id,
            terms_name,
            &[("cognito:default", "https://held.example.com")],
        )
        .await;
        let other_name = TERMS_NAMES[(index + 1) % TERMS_NAMES.len()];
        let renameable = create_terms(
            &router,
            &user_pool_id,
            &client_id,
            other_name,
            &[("cognito:default", "https://renameable.example.com")],
        )
        .await;
        let expected = format!("Terms with name {terms_name} already exists for Client {client_id}.");

        let (create_status, created) = call(
            &router,
            "CreateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://second.example.com" },
            }),
        )
        .await;
        assert_eq!(
            (create_status, error_code(&created), error_message(&created)),
            (400, "TermsExistsException", expected.as_str()),
            "case {terms_name:?}: a duplicate create must carry the one TermsExistsException sentence, got {created}"
        );

        let (update_status, updated) = call(
            &router,
            "UpdateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id_of(&renameable),
                "TermsName": terms_name,
            }),
        )
        .await;
        assert_eq!(
            (update_status, error_code(&updated), error_message(&updated)),
            (400, "TermsExistsException", expected.as_str()),
            "case {terms_name:?}: a colliding rename must carry the same sentence, got {updated}"
        );

        // Neither refusal disturbed what is stored.
        for (role, before) in [("held", &held), ("renameable", &renameable)] {
            let read = describe_terms(&router, &user_pool_id, &terms_id_of(before)).await;
            assert_eq!(
                read, *before,
                "case {terms_name:?}: the {role} terms documents must be exactly as they were stored, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_of_an_absent_pool_or_terms_id_is_a_404() {
    let router = router();
    let user_pool_id = pool(&router, "update-notfound-pool").await;
    let client_id = client(&router, &user_pool_id, "update-notfound-client").await;
    let other_pool = pool(&router, "update-other-pool").await;
    let stored = create_terms(
        &router,
        &user_pool_id,
        &client_id,
        "terms-of-use",
        &[("cognito:default", "https://terms.example.com")],
    )
    .await;
    let terms_id = terms_id_of(&stored);
    // A well-formed v4 UUID no `CreateTerms` minted here.
    let absent_terms_id = "00000000-0000-4000-8000-000000000000";

    for (label, body) in [
        (
            "an absent user pool",
            json!({
                "UserPoolId": "us-east-1_absent",
                "TermsId": terms_id,
                "TermsName": "privacy-policy",
            }),
        ),
        (
            "a TermsId the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": absent_terms_id,
                "TermsName": "privacy-policy",
            }),
        ),
        (
            "a TermsId stored in another pool",
            json!({
                "UserPoolId": other_pool,
                "TermsId": terms_id,
                "TermsName": "privacy-policy",
            }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateTerms", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {payload}"
        );
    }

    // None of the refused updates touched the stored documents.
    let read = describe_terms(&router, &user_pool_id, &terms_id).await;
    assert_eq!(
        read, stored,
        "a refused update must leave the stored terms documents exactly as they were, got {read}"
    );
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused_and_changes_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "update-constraint-pool").await;
    let client_id = client(&router, &user_pool_id, "update-constraint-client").await;
    let stored = create_terms(
        &router,
        &user_pool_id,
        &client_id,
        "terms-of-use",
        &[("cognito:default", "https://terms.example.com")],
    )
    .await;
    let terms_id = terms_id_of(&stored);

    let fourteen_links: Map<String, Value> = LANGUAGES
        .iter()
        .map(|language| ((*language).to_string(), json!("https://example.com")))
        .chain([(
            "cognito:dutch ".to_string(),
            json!("https://example.com/extra"),
        )])
        .collect();
    let over_long_url = format!("https://terms.example.com/{}", "p".repeat(1024));

    for (label, body) in [
        (
            "an empty Links map, below the modelled length minimum 1",
            json!({ "UserPoolId": user_pool_id, "TermsId": terms_id, "Links": {} }),
        ),
        (
            "fourteen Links entries, past the modelled length maximum 13",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id,
                "Links": Value::Object(fourteen_links),
            }),
        ),
        (
            "a language the key pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id,
                "Links": { "cognito:klingon": "https://terms.example.com" },
            }),
        ),
        (
            "an empty URL, below the value length minimum 1",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id,
                "Links": { "cognito:default": "" },
            }),
        ),
        (
            "a URL one character past the value length maximum 1024",
            json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id,
                "Links": { "cognito:default": over_long_url },
            }),
        ),
        (
            "a TermsName outside the two the pattern admits",
            json!({ "UserPoolId": user_pool_id, "TermsId": terms_id, "TermsName": "tos" }),
        ),
        (
            "a TermsSource outside the one value the enum admits",
            json!({ "UserPoolId": user_pool_id, "TermsId": terms_id, "TermsSource": "DOCUMENT" }),
        ),
        (
            "an Enforcement outside the one value the enum admits",
            json!({ "UserPoolId": user_pool_id, "TermsId": terms_id, "Enforcement": "REQUIRED" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "TermsId": terms_id }),
        ),
        (
            "a TermsId that is not the v4 UUID the pattern demands",
            json!({ "UserPoolId": user_pool_id, "TermsId": "not-a-uuid" }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateTerms", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );

        let read = describe_terms(&router, &user_pool_id, &terms_id).await;
        assert_eq!(
            read, stored,
            "case {label:?}: a refused update must change nothing a later read sees, got {read}"
        );
    }
}
