// SPDX-License-Identifier: CC0-1.0

//! Terms documents over the wire: `CreateTerms` stores one set of terms
//! documents for an app client of the pool it names and every later
//! `DescribeTerms` renders the STORED set, driven through the router exactly as
//! an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created set of terms documents is a resource of the
//! named app client, minted a `TermsId` that identifies it for later reads; it
//! renders `UserPoolId`, `ClientId`, `TermsName`, `TermsSource`, `Enforcement`
//! and the submitted language-to-URL map unchanged, its name is unique to that
//! app client (a duplicate is refused and leaves the stored set untouched, while
//! the same name on another app client is a different resource), and an absent
//! pool, app client or `TermsId` is a 404. Each table row names the case it
//! asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Every language `LinksType`'s key pattern admits, in the order the pattern
/// lists them:
/// `^cognito:(default|dutch|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional))$`
/// from
/// `smithy_query(query="shape",shape="LinksType")` — thirteen values, which is
/// also the map's modelled length maximum.
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
/// `smithy_query(query="operation",operation="CreateTerms")`).
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

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

/// Whether `candidate` is a canonical v4 UUID, the form `TermsIdType`'s pattern
/// demands (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`).
fn is_v4_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    lengths == vec![8, 4, 4, 4, 12]
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
        && groups[2].starts_with('4')
        && groups[3]
            .chars()
            .next()
            .is_some_and(|c| matches!(c.to_ascii_lowercase(), '8' | '9' | 'a' | 'b'))
}

/// One row of the `CreateTerms` table: the document type, the source and
/// enforcement values the request sends, and the language-to-URL map. The rows
/// differ along what the rule generalises over — which document type, and the
/// count, ordering, character set and length of the links.
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
/// and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$` (an ASCII URL, a URL carrying
/// non-Latin letters and marks, and one at the length maximum) — every
/// constraint from
/// `smithy_query(query="operation",operation="CreateTerms")` and
/// `smithy_query(query="shape",shape="LinksType")`.
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
                    "cognito:spanish".to_string(),
                    "https://privacy.example.com/es".to_string(),
                ),
                (
                    "cognito:default".to_string(),
                    "https://privacy.example.com".to_string(),
                ),
                (
                    "cognito:japanese".to_string(),
                    "https://privacy.example.com/ja".to_string(),
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
            case: "a URL carrying non-Latin letters, marks and symbols",
            terms_name: TERMS_NAMES[1],
            links: vec![
                (
                    "cognito:chinese-traditional".to_string(),
                    "https://例え.example.com/隱私權政策".to_string(),
                ),
                (
                    "cognito:korean".to_string(),
                    "https://example.com/개인정보-정책?세금=€1±2".to_string(),
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
async fn created_terms_documents_round_trip_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "terms-pool").await;

    let mut minted_ids: BTreeSet<String> = BTreeSet::new();
    for (index, case) in terms_cases().into_iter().enumerate() {
        let label = case.case;
        // A terms document's name is unique to its app client, so each row gets
        // its own app client of the same pool.
        let client_id = client(&router, &user_pool_id, &format!("terms-client-{index}")).await;
        let created = ok(
            &router,
            "CreateTerms",
            &case.request(&user_pool_id, &client_id),
        )
        .await;
        let terms = &created["Terms"];

        let terms_id = terms["TermsId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_v4_uuid(&terms_id),
            "case {label:?}: the minted TermsId must be a v4 UUID, got {terms_id:?}"
        );
        assert!(
            minted_ids.insert(terms_id.clone()),
            "case {label:?}: each created set of terms documents must get its own TermsId, got {terms_id:?} twice"
        );
        assert_eq!(
            terms["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the terms documents must name the pool they were created in, got {terms}"
        );
        assert_eq!(
            terms["ClientId"],
            json!(client_id),
            "case {label:?}: the terms documents must name the app client they were created for, got {terms}"
        );
        assert_eq!(
            terms["TermsName"],
            json!(case.terms_name),
            "case {label:?}: the stored name must be the created one, got {terms}"
        );
        assert_eq!(
            terms["TermsSource"],
            json!("LINK"),
            "case {label:?}: the stored source must be the created one, got {terms}"
        );
        assert_eq!(
            terms["Enforcement"],
            json!("NONE"),
            "case {label:?}: the stored enforcement must be the created one, got {terms}"
        );
        assert_eq!(
            terms["Links"],
            case.links_json(),
            "case {label:?}: the stored links must be the submitted ones, language for language, got {terms}"
        );
        assert!(
            terms["CreationDate"].is_number() && terms["LastModifiedDate"].is_number(),
            "case {label:?}: a created set of terms documents carries both timestamps, got {terms}"
        );
        assert_eq!(
            terms["CreationDate"], terms["LastModifiedDate"],
            "case {label:?}: a set of terms documents nothing has modified was last modified when it was created, got {terms}"
        );

        // The round trip: the reader renders the STORED documents, member for
        // member.
        let read = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            read["Terms"], created["Terms"],
            "case {label:?}: DescribeTerms must render the stored terms documents, got {read}"
        );

        // And reading them again changes nothing a later caller sees.
        let again = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            again["Terms"], read["Terms"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_name_the_app_client_already_holds_is_refused_and_the_stored_documents_survive() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-terms-pool").await;

    for (index, case) in terms_cases().into_iter().enumerate() {
        let label = case.case;
        let client_id = client(&router, &user_pool_id, &format!("duplicate-{index}")).await;
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

        // A second set of documents under the same name for the same app client,
        // with DIFFERENT links: refused, and the stored one is left exactly as it
        // was.
        let (status, payload) = call(
            &router,
            "CreateTerms",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": case.terms_name,
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://second.example.com" },
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "TermsExistsException"),
            "case {label:?}: a duplicate TermsName on one app client is a 400 TermsExistsException, got {payload}"
        );

        let read = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            read["Terms"], created["Terms"],
            "case {label:?}: a refused create must not have touched the stored terms documents, got {read}"
        );

        // The SAME name on another app client of the same pool is a different
        // resource: the name is unique to the app client, not to the pool.
        let other_client =
            client(&router, &user_pool_id, &format!("duplicate-other-{index}")).await;
        let other = ok(
            &router,
            "CreateTerms",
            &case.request(&user_pool_id, &other_client),
        )
        .await;
        assert_ne!(
            other["Terms"]["TermsId"], created["Terms"]["TermsId"],
            "case {label:?}: the same name on another app client is a different resource, got {other}"
        );
        assert_eq!(
            other["Terms"]["ClientId"],
            json!(other_client),
            "case {label:?}: the second set of documents must name its own app client, got {other}"
        );
    }
}

#[tokio::test]
async fn both_document_types_live_side_by_side_on_one_app_client() {
    let router = router();
    let user_pool_id = pool(&router, "both-types-pool").await;
    let client_id = client(&router, &user_pool_id, "both-types-client").await;

    // "Each request accepts one type of terms document ... Supply each type in
    // separate requests." (`CreateTerms` in this service's model): the two
    // document types are two resources of one app client.
    let mut by_name = Vec::new();
    for terms_name in TERMS_NAMES {
        let created = ok(
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
        by_name.push((terms_name, created));
    }

    for (terms_name, created) in &by_name {
        let terms_id = created["Terms"]["TermsId"]
            .as_str()
            .expect("a minted TermsId");
        let read = ok(
            &router,
            "DescribeTerms",
            &json!({ "UserPoolId": user_pool_id, "TermsId": terms_id }),
        )
        .await;
        assert_eq!(
            read["Terms"], created["Terms"],
            "case {terms_name:?}: each document type keeps its own stored state, got {read}"
        );
    }
    assert_ne!(
        by_name[0].1["Terms"]["TermsId"], by_name[1].1["Terms"]["TermsId"],
        "the two document types of one app client are two resources, got {:?} and {:?}",
        by_name[0].1, by_name[1].1
    );
}

#[tokio::test]
async fn an_absent_pool_client_or_terms_id_is_a_404() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-terms-pool").await;
    let client_id = client(&router, &user_pool_id, "notfound-terms-client").await;
    let other_pool = pool(&router, "other-terms-pool").await;
    let created = ok(
        &router,
        "CreateTerms",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "TermsName": "terms-of-use",
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": { "cognito:default": "https://terms.example.com" },
        }),
    )
    .await;
    let terms_id = created["Terms"]["TermsId"]
        .as_str()
        .expect("a minted TermsId")
        .to_string();
    let absent_terms_id = "00000000-0000-4000-8000-000000000000";

    for (label, operation, body) in [
        (
            "CreateTerms on an absent pool",
            "CreateTerms",
            json!({
                "UserPoolId": "us-east-1_absent",
                "ClientId": client_id,
                "TermsName": "terms-of-use",
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://terms.example.com" },
            }),
        ),
        (
            "CreateTerms for an app client the pool does not hold",
            "CreateTerms",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": "absentclientid",
                "TermsName": "terms-of-use",
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://terms.example.com" },
            }),
        ),
        (
            "CreateTerms for an app client of ANOTHER pool",
            "CreateTerms",
            json!({
                "UserPoolId": other_pool,
                "ClientId": client_id,
                "TermsName": "terms-of-use",
                "TermsSource": "LINK",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://terms.example.com" },
            }),
        ),
        (
            "DescribeTerms on an absent pool",
            "DescribeTerms",
            json!({ "UserPoolId": "us-east-1_absent", "TermsId": terms_id }),
        ),
        (
            "DescribeTerms for a TermsId the pool does not hold",
            "DescribeTerms",
            json!({ "UserPoolId": user_pool_id, "TermsId": absent_terms_id }),
        ),
        (
            "DescribeTerms for a TermsId stored in another pool",
            "DescribeTerms",
            json!({ "UserPoolId": other_pool, "TermsId": terms_id }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "terms-constraint-pool").await;
    let client_id = client(&router, &user_pool_id, "terms-constraint-client").await;

    /// A `CreateTerms` request for this pool and client, with `Links` and
    /// `TermsName` overridden per case; a `None` leaves `Links` out entirely.
    fn request(
        user_pool_id: &str,
        client_id: &str,
        terms_name: &str,
        links: Option<Value>,
    ) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ClientId".to_string(), json!(client_id));
        body.insert("TermsName".to_string(), json!(terms_name));
        body.insert("TermsSource".to_string(), json!("LINK"));
        body.insert("Enforcement".to_string(), json!("NONE"));
        if let Some(links) = links {
            body.insert("Links".to_string(), links);
        }
        Value::Object(body)
    }

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
            "no Links member at all, so no document the required Links could render",
            request(&user_pool_id, &client_id, "terms-of-use", None),
        ),
        (
            "an empty Links map, below the modelled length minimum 1",
            request(
                &user_pool_id,
                &client_id,
                "terms-of-use",
                Some(json!({})),
            ),
        ),
        (
            "fourteen Links entries, past the modelled length maximum 13",
            request(
                &user_pool_id,
                &client_id,
                "terms-of-use",
                Some(Value::Object(fourteen_links)),
            ),
        ),
        (
            "a language the key pattern excludes",
            request(
                &user_pool_id,
                &client_id,
                "terms-of-use",
                Some(json!({ "cognito:klingon": "https://terms.example.com" })),
            ),
        ),
        (
            "an empty URL, below the value length minimum 1",
            request(
                &user_pool_id,
                &client_id,
                "terms-of-use",
                Some(json!({ "cognito:default": "" })),
            ),
        ),
        (
            "a URL one character past the value length maximum 1024",
            request(
                &user_pool_id,
                &client_id,
                "terms-of-use",
                Some(json!({ "cognito:default": over_long_url })),
            ),
        ),
        (
            "a TermsName outside the two the pattern admits",
            request(
                &user_pool_id,
                &client_id,
                "tos",
                Some(json!({ "cognito:default": "https://terms.example.com" })),
            ),
        ),
        (
            "a TermsSource outside the one value the enum admits",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "terms-of-use",
                "TermsSource": "DOCUMENT",
                "Enforcement": "NONE",
                "Links": { "cognito:default": "https://terms.example.com" },
            }),
        ),
        (
            "an Enforcement outside the one value the enum admits",
            json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "TermsName": "terms-of-use",
                "TermsSource": "LINK",
                "Enforcement": "REQUIRED",
                "Links": { "cognito:default": "https://terms.example.com" },
            }),
        ),
        (
            "a user pool id the pattern excludes",
            request(
                "no-underscore",
                &client_id,
                "terms-of-use",
                Some(json!({ "cognito:default": "https://terms.example.com" })),
            ),
        ),
    ] {
        let (status, payload) = call(&router, "CreateTerms", &body).await;
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

    // None of the refused requests stored anything: the name is still free for
    // this app client, so a well-formed create under it succeeds.
    let created = ok(
        &router,
        "CreateTerms",
        &request(
            &user_pool_id,
            &client_id,
            "terms-of-use",
            Some(json!({ "cognito:default": "https://terms.example.com" })),
        ),
    )
    .await;
    assert_eq!(
        created["Terms"]["Links"],
        json!({ "cognito:default": "https://terms.example.com" }),
        "a refused create stores no terms documents, so this one is the first under its name, got {created}"
    );
}
