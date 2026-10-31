// SPDX-License-Identifier: CC0-1.0

//! `DescribeTermsByClient` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a set of terms documents is addressed by the user pool,
//! the app client that holds it and the document type's name, a triple that
//! resolves at most one stored set; the describe renders that STORED set member
//! for member — the same record a read by `TermsId` renders, including whatever a
//! later update wrote — it changes nothing a later caller sees, and an absent
//! pool, an app client the pool does not hold, or a name that app client holds no
//! documents under is a 400 `ResourceNotFoundException` carrying no terms
//! documents — an unresolved pool or app client worded from the request's own
//! values, and an unresolved set of documents worded as the one fixed sentence
//! that class of refusal carries.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value it saw.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

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
/// `smithy_query(query="operation",operation="DescribeTermsByClient")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

/// The one sentence a lookup that resolves no terms documents answers, however
/// it looked them up — asserted over a whole class of such lookups in
/// `terms_not_found_message`.
const TERMS_NOT_FOUND_MESSAGE: &str = "Terms does not exist.";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
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

/// One row of the table: the document type the set is created under and the
/// language-to-URL map it carries. The rows differ along what the rule
/// generalises over — which document type, and the count, ordering, character
/// set and length of the links the describe has to render back.
struct TermsCase {
    case: &'static str,
    terms_name: &'static str,
    links: Vec<(String, String)>,
}

impl TermsCase {
    /// This case's `CreateTerms` request for `user_pool_id` and `client_id`.
    fn request(&self, user_pool_id: &str, client_id: &str) -> Value {
        json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": self.terms_name,
            // The only values `TermsSourceType` and `TermsEnforcementType`
            // admit (`smithy_query(query="shape",shape="TermsSourceType")`).
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": self.links_json(),
        })
    }

    /// The `Links` map as the wire carries it.
    fn links_json(&self) -> Value {
        let mut map = Map::new();
        for (language, url) in &self.links {
            map.insert(language.clone(), json!(url));
        }
        Value::Object(map)
    }
}

/// The table. `TermsName` spans both values its pattern admits, and `Links`
/// spans its modelled length 1..13 (one entry, several submitted out of the
/// pattern's order, and all thirteen languages) and its value's length 1..1024
/// and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$` (an ASCII URL, URLs carrying
/// non-Latin letters, marks and symbols, and one at the length maximum) — the
/// constraints `smithy_query(query="shape",shape="LinksType")` reports.
fn terms_cases() -> Vec<TermsCase> {
    vec![
        TermsCase {
            case: "one link, the map minimum 1, under the default language",
            terms_name: TERMS_NAMES[0],
            links: vec![(
                LANGUAGES[0].to_string(),
                "https://terms.example.com".to_string(),
            )],
        },
        TermsCase {
            case: "the other document type with three links, submitted out of the pattern's order",
            terms_name: TERMS_NAMES[1],
            links: vec![
                (
                    "cognito:italian".to_string(),
                    "https://privacy.example.com/it".to_string(),
                ),
                (
                    "cognito:default".to_string(),
                    "https://privacy.example.com".to_string(),
                ),
                (
                    "cognito:dutch".to_string(),
                    "https://privacy.example.com/nl".to_string(),
                ),
            ],
        },
        TermsCase {
            case: "all thirteen languages, the map maximum",
            terms_name: TERMS_NAMES[0],
            links: LANGUAGES
                .iter()
                .map(|language| {
                    let suffix = language.trim_start_matches("cognito:");
                    (
                        (*language).to_string(),
                        format!("https://terms.example.com/{suffix}"),
                    )
                })
                .collect(),
        },
        TermsCase {
            case: "URLs carrying non-Latin letters, marks and symbols",
            terms_name: TERMS_NAMES[1],
            links: vec![
                (
                    "cognito:chinese-simplified".to_string(),
                    "https://例え.example.com/隐私政策".to_string(),
                ),
                (
                    "cognito:portuguese-brazil".to_string(),
                    "https://example.com/política-de-privacidade?preço=R$1±2".to_string(),
                ),
            ],
        },
        TermsCase {
            case: "a URL at the value length maximum 1024",
            terms_name: TERMS_NAMES[0],
            links: vec![(
                "cognito:english".to_string(),
                format!(
                    "https://terms.example.com/{}",
                    "p".repeat(1024 - "https://terms.example.com/".len())
                ),
            )],
        },
    ]
}

#[tokio::test]
async fn describe_by_client_renders_the_stored_terms_documents_of_that_client_and_name() {
    let router = router();
    let user_pool_id = pool(&router, "describe-by-client-pool").await;

    for (index, case) in terms_cases().into_iter().enumerate() {
        let label = case.case;
        // A terms document's name is unique to its app client, so each row gets
        // its own app client of the same pool.
        let client_id = client(&router, &user_pool_id, &format!("by-client-{index}")).await;
        let created = ok(
            &router,
            "CreateTerms",
            &case.request(&user_pool_id, &client_id),
        )
        .await;
        let terms_id = created["Terms"]["TermsId"]
            .as_str()
            .expect("a minted TermsId")
            .to_string();

        let read = ok(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": case.terms_name,
            }),
        )
        .await;
        assert_eq!(
            read["Terms"], created["Terms"],
            "case {label:?}: the describe must render the stored terms documents member for member, got {read}"
        );
        assert_eq!(
            read["Terms"]["Links"],
            case.links_json(),
            "case {label:?}: the stored links must come back language for language, got {read}"
        );
        assert_eq!(
            read["Terms"]["TermsId"],
            json!(terms_id),
            "case {label:?}: the describe must resolve the set the create minted, got {read}"
        );

        // The triple and the `TermsId` address the SAME stored record, so the two
        // readers answer the same thing.
        let by_id = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            read["Terms"], by_id["Terms"],
            "case {label:?}: a read by app client and name must answer what a read by TermsId answers, got {read} and {by_id}"
        );

        // And reading changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": case.terms_name,
            }),
        )
        .await;
        assert_eq!(
            again["Terms"], read["Terms"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn describe_by_client_reflects_what_a_later_update_wrote() {
    let router = router();
    let user_pool_id = pool(&router, "describe-by-client-update-pool").await;

    for (index, case) in terms_cases().into_iter().enumerate() {
        let label = case.case;
        let client_id = client(&router, &user_pool_id, &format!("updated-{index}")).await;
        let created = ok(
            &router,
            "CreateTerms",
            &case.request(&user_pool_id, &client_id),
        )
        .await;
        let terms_id = created["Terms"]["TermsId"]
            .as_str()
            .expect("a minted TermsId")
            .to_string();

        // Replace the stored link map with one the row did not create.
        let replacement = json!({ "cognito:french": format!("https://updated.example.com/{index}") });
        let updated = ok(
            &router,
            "UpdateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "TermsId": terms_id,
                "Links": replacement,
            }),
        )
        .await;

        let read = ok(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": case.terms_name,
            }),
        )
        .await;
        assert_eq!(
            read["Terms"], updated["Terms"],
            "case {label:?}: the describe must render the state the update wrote, got {read}"
        );
        assert_eq!(
            read["Terms"]["Links"], replacement,
            "case {label:?}: the replaced link map is what a later read sees, got {read}"
        );
    }
}

#[tokio::test]
async fn one_name_on_two_app_clients_resolves_to_each_client_own_documents() {
    let router = router();
    let user_pool_id = pool(&router, "describe-by-client-scope-pool").await;
    let first = client(&router, &user_pool_id, "scope-first").await;
    let second = client(&router, &user_pool_id, "scope-second").await;

    // Both document types on both app clients: four stored sets under two names,
    // so the lookup has to use the app client AND the name.
    for terms_name in TERMS_NAMES {
        for client_id in [&first, &second] {
            ok(
                &router,
                "CreateTerms",
                &json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "TermsName": terms_name,
                    "TermsSource": "LINK",
                    "Enforcement": "NONE",
                    "Links": { "cognito:default": format!("https://example.com/{client_id}/{terms_name}") },
                }),
            )
            .await;
        }
    }

    for terms_name in TERMS_NAMES {
        for client_id in [&first, &second] {
            let label = format!("{terms_name} of {client_id}");
            let read = ok(
                &router,
                "DescribeTermsByClient",
                &json!({
                    "UserPoolId": user_pool_id,
                    "ClientId": client_id,
                    "TermsName": terms_name,
                }),
            )
            .await;
            assert_eq!(
                read["Terms"]["ClientId"],
                json!(client_id),
                "case {label:?}: the describe must answer the named app client's own documents, got {read}"
            );
            assert_eq!(
                read["Terms"]["TermsName"],
                json!(terms_name),
                "case {label:?}: the describe must answer the named document type, got {read}"
            );
            assert_eq!(
                read["Terms"]["Links"],
                json!({ "cognito:default": format!("https://example.com/{client_id}/{terms_name}") }),
                "case {label:?}: each stored set keeps its own links, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn an_absent_pool_client_or_name_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "describe-by-client-400-pool").await;
    let client_id = client(&router, &user_pool_id, "notfound-client").await;
    let bare_client = client(&router, &user_pool_id, "holds-nothing").await;
    let other_pool = pool(&router, "describe-by-client-other-pool").await;
    ok(
        &router,
        "CreateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": TERMS_NAMES[0],
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": { "cognito:default": "https://terms.example.com" },
        }),
    )
    .await;

    // Each row carries the message its refusal class is worded with: an
    // unresolvable pool names the pool, an app client of ANOTHER pool names both
    // the client and the pool it was looked for in, an id that names no app
    // client at all names just the id, while a lookup that resolves no terms
    // documents answers the one fixed sentence for that class, naming nothing
    // the request asked for.
    let other_pool_client = client(&router, &other_pool, "other-pool-client").await;
    for (label, body, expected_message) in [
        (
            "a user pool that does not exist",
            json!({
                "UserPoolId": "us-east-1_absent",
                "ClientId": client_id,
                "TermsName": TERMS_NAMES[0],
            }),
            "User pool us-east-1_absent does not exist.".to_string(),
        ),
        (
            "an app client id no pool holds",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "absentclientid",
                "TermsName": TERMS_NAMES[0],
            }),
            "User pool client absentclientid does not exist.".to_string(),
        ),
        (
            "an app client of ANOTHER pool, asked against the pool that stores the documents",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": other_pool_client,
                "TermsName": TERMS_NAMES[0],
            }),
            format!(
                "User pool client {other_pool_client} does not exist in user pool {user_pool_id}."
            ),
        ),
        (
            "this pool's own app client, asked against ANOTHER pool",
            json!({
                "UserPoolId": other_pool,
                "ClientId": client_id,
                "TermsName": TERMS_NAMES[0],
            }),
            format!("User pool client {client_id} does not exist in user pool {other_pool}."),
        ),
        (
            "the other document type, which this app client holds no documents under",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": TERMS_NAMES[1],
            }),
            TERMS_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "an app client of the same pool that holds no documents at all",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": bare_client,
                "TermsName": TERMS_NAMES[0],
            }),
            TERMS_NOT_FOUND_MESSAGE.to_string(),
        ),
    ] {
        let (status, payload) = call(&router, "DescribeTermsByClient", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {label:?}: the refusal must carry its class's wording, got {payload}"
        );
        // A refusal must never leak the terms documents of the pool it was asked
        // against.
        assert!(
            payload.get("Terms").is_none(),
            "case {label:?}: a refusal carries no terms documents, got {payload}"
        );
    }

    // The set that does exist is still readable: none of the refusals touched it.
    let read = ok(
        &router,
        "DescribeTermsByClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": TERMS_NAMES[0],
        }),
    )
    .await;
    assert_eq!(
        read["Terms"]["Links"],
        json!({ "cognito:default": "https://terms.example.com" }),
        "a refused read must leave the stored terms documents exactly as they were, got {read}"
    );
}

/// Every set of terms documents of `user_pool_id` as `ListTerms` reports it, as
/// `(TermsId, TermsName)` pairs in the order the listing answers, so a pool's
/// documents before and after a refused lookup can be compared whole.
async fn listed_terms(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<(String, String)> {
    let listed = ok(router, "ListTerms", &json!({ "UserPoolId": user_pool_id })).await;
    listed["Terms"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTerms must answer a Terms list, got {listed}"))
        .iter()
        .map(|entry| {
            (
                entry["TermsId"].as_str().unwrap_or_default().to_string(),
                entry["TermsName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_client_of_another_pool_is_refused_and_leaves_every_pool_listing_untouched() {
    let router = router();

    // Three pools: one holding documents of two of its app clients, one holding
    // a single document, and one holding none — so the refusal is asserted
    // against a named pool that does hold the requested name, one that holds the
    // other name, and one that holds nothing at all.
    let first = pool(&router, "cross-pool-first").await;
    let second = pool(&router, "cross-pool-second").await;
    let empty = pool(&router, "cross-pool-empty").await;

    let first_a = client(&router, &first, "first-a").await;
    let first_b = client(&router, &first, "first-b").await;
    let second_a = client(&router, &second, "second-a").await;

    for (user_pool_id, client_id, terms_name) in [
        (&first, &first_a, TERMS_NAMES[0]),
        (&first, &first_b, TERMS_NAMES[1]),
        (&second, &second_a, TERMS_NAMES[0]),
    ] {
        ok(
            &router,
            "CreateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": format!("https://example.com/{terms_name}") },
            }),
        )
        .await;
    }

    let pools = [
        ("the pool holding two documents", first.clone(), 2),
        ("the pool holding one document", second.clone(), 1),
        ("the pool holding none", empty.clone(), 0),
    ];
    let mut before = Vec::new();
    for (label, user_pool_id, expected_count) in &pools {
        let listing = listed_terms(&router, user_pool_id).await;
        assert_eq!(
            listing.len(),
            *expected_count,
            "case {label:?}: the listing must report exactly the documents that were created, got {listing:?}"
        );
        before.push((*label, user_pool_id.clone(), listing));
    }

    // A ClientId that is a real app client of ANOTHER pool, and one no pool ever
    // minted, are both refused: neither is an app client of the pool named by
    // the request, so that pool has no documents of it to describe. The two are
    // worded differently — the id that resolves to another pool's client is
    // reported with the pool it is not in, the one that resolves to nothing by
    // its id alone.
    let never_minted = "0000000000notanappclient".to_string();
    for (label, user_pool_id, client_id, terms_name) in [
        (
            "an app client of the second pool, asked against the first, for a name the first pool does hold",
            &first,
            &second_a,
            TERMS_NAMES[0],
        ),
        (
            "an app client of the second pool, asked against the first, for the other document type",
            &first,
            &second_a,
            TERMS_NAMES[1],
        ),
        (
            "an app client of the first pool, asked against the second",
            &second,
            &first_b,
            TERMS_NAMES[1],
        ),
        (
            "an app client of the first pool, asked against a pool that holds no documents",
            &empty,
            &first_a,
            TERMS_NAMES[0],
        ),
        (
            "a well formed client id no pool ever minted",
            &first,
            &never_minted,
            TERMS_NAMES[0],
        ),
    ] {
        let (status, payload) = call(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: a client that is not an app client of the named pool is a 400 ResourceNotFoundException, got {payload}"
        );
        let expected = if client_id == &never_minted {
            format!("User pool client {client_id} does not exist.")
        } else {
            format!("User pool client {client_id} does not exist in user pool {user_pool_id}.")
        };
        assert_eq!(
            error_message(&payload),
            expected,
            "case {label:?}: the refusal must carry {expected:?}, got {payload}"
        );
        assert!(
            payload.get("Terms").is_none(),
            "case {label:?}: a refusal must not answer the named pool's terms documents, got {payload}"
        );

        // The failed lookup is a read: every pool still reports exactly the
        // documents that were created, in the same order.
        for (pool_label, user_pool_id, expected) in &before {
            let listing = listed_terms(&router, user_pool_id).await;
            assert_eq!(
                &listing, expected,
                "case {label:?}: after the refusal, {pool_label} must still list exactly its created documents, got {listing:?}"
            );
        }
    }

    // And the matching triples still resolve, so the scoping rule refuses the
    // foreign client without costing the pool's own app clients their documents.
    for (user_pool_id, client_id, terms_name) in [
        (&first, &first_a, TERMS_NAMES[0]),
        (&first, &first_b, TERMS_NAMES[1]),
        (&second, &second_a, TERMS_NAMES[0]),
    ] {
        let label = format!("{terms_name} of {client_id} in {user_pool_id}");
        let read = ok(
            &router,
            "DescribeTermsByClient",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": terms_name,
            }),
        )
        .await;
        assert_eq!(
            (&read["Terms"]["ClientId"], &read["Terms"]["TermsName"]),
            (&json!(client_id), &json!(terms_name)),
            "case {label:?}: the matching triple must resolve to that app client's document of that name, got {read}"
        );
        assert_eq!(
            read["Terms"]["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the resolved document belongs to the pool named by the request, got {read}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "describe-by-client-constraint-pool").await;
    let client_id = client(&router, &user_pool_id, "constraint-client").await;

    for (label, body) in [
        (
            "a TermsName outside the two the pattern admits",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "tos",
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({
                "UserPoolId": "no-underscore",
                "ClientId": client_id,
                "TermsName": TERMS_NAMES[0],
            }),
        ),
        (
            "a client id the pattern excludes",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "has-dashes",
                "TermsName": TERMS_NAMES[0],
            }),
        ),
        (
            "an empty client id, below the length minimum 1",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "",
                "TermsName": TERMS_NAMES[0],
            }),
        ),
        (
            "a client id one character past the length maximum 128",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "c".repeat(129),
                "TermsName": TERMS_NAMES[0],
            }),
        ),
        (
            "no TermsName at all, though the member is required",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
            }),
        ),
    ] {
        let (status, payload) = call(&router, "DescribeTermsByClient", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // A client id at the length maximum 128 is INSIDE the constraint, so it is
    // answered by the lookup (a 400) rather than refused as malformed.
    let (status, payload) = call(
        &router,
        "DescribeTermsByClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": "c".repeat(128),
            "TermsName": TERMS_NAMES[0],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "a client id at the length maximum 128 is well formed, so it is a 400 and not a 400, got {payload}"
    );
}
