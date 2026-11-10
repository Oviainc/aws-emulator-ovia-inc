// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what is
//! asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this service
//! (`smithy_query(query="service_info")`), so EVERY operation evaluates the identity-based
//! IAM policies in force on the principal its credentials name — against
//! `aws-marketplace:<OperationName>` on the AWS Marketplace catalog resource the request
//! addresses — BEFORE it looks up anything. A request no statement allows (IAM's default
//! deny), or one an explicit `Deny` covers, is answered HTTP 403 `AccessDeniedException`
//! and reports nothing of the catalog, while a request a matching `Allow` covers reaches the
//! operation's own answer.
//!
//! The user, its policies and its access key live in the IAM store of the SAME shared engine
//! — the store that service's own handlers write — so the documents under test are the ones
//! `iam:PutUserPolicy` stores and the principal a refusal blames is the one the access key
//! belongs to.
//!
//! The tables span what the rule generalises over: the operation (all nine of them — five
//! record reads, two listings and two searches), the document in force (none at all, another
//! action of this service, another service's actions, an explicit deny beside an allow, an
//! allow of exactly the action, of the action in another case, of the whole service and of
//! everything), the resource an allow is scoped to (every resource, the catalog, one
//! listing's ARN, every listing's ARN, another catalog's) and the principal (an IAM user and
//! the account's own published credential).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use smithy_server_runtime::MultiServiceRouter;

/// The model's `POST /2026-02-05/<op>` route prefix (`OPERATIONS.md`).
const PREFIX: &str = "http://discovery-marketplace.us-east-1.amazonaws.com/2026-02-05";

/// This service's IAM action prefix, which is its `arnNamespace` and its signing name
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "aws-marketplace";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the IAM user every row signs with.
const USER_KEY_ID: &str = "AKIAEXAMPLEUSERKEY01";

/// The IAM user that key belongs to, and the name of its inline policy.
const USER_NAME: &str = "catalog-reader";
const POLICY_NAME: &str = "inline";

/// A router serving this service beside the IAM service, over ONE shared engine, so the
/// policies the cases seed are the ones that service holds.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// What one call answered: its HTTP status, the wire error code and the parsed body.
struct Answer {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Answer {
    /// The refusal's `message`, or the empty string when the answer carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether this answer is the modelled authorization refusal.
    fn refused(&self) -> bool {
        self.error_code.as_deref() == Some("AccessDeniedException")
    }
}

/// Call one operation of this service signed with `access_key_id` — what names the principal
/// the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    input: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{PREFIX}/{operation}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260205/us-east-1/{ACTION_PREFIX}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(input.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_code,
        body,
    }
}

/// The ARN the account holds the IAM user under, which a refusal names as the caller.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        ::engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLECATALOGREADER".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Attach `document` to the user as its inline policy, replacing whatever it carried — what
/// `iam:PutUserPolicy` does. `None` takes every policy off it, as `iam:DeleteUserPolicy`
/// does.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store.user_inline_policies.insert(
            iam::name_key(USER_NAME),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document: document.to_string(),
                },
            )]),
        );
    }
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The documents a row puts in force on the user, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("aws-marketplace-management:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of exactly this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["s3:GetObject", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// One operation of this service, as a client addresses it: the body the rule is judged on,
/// and the catalog resource its refusal names.
struct Operation {
    name: &'static str,
    route: &'static str,
    body: Value,
    /// The resource field of the ARN the action is judged against, after
    /// `catalog/<catalog>/`; empty for an operation that addresses the catalog itself.
    record: String,
}

/// The catalog this emulator serves, and a listing, product and offer id it holds — read
/// back through the service itself with the account's own credential, so no id and no
/// catalog name is written out here.
async fn catalog_records(router: &MultiServiceRouter) -> (String, String, String, String) {
    let page = call_as(
        router,
        ACCOUNT_KEY_ID,
        "searchListings",
        &json!({ "maxResults": 1 }),
    )
    .await;
    assert_eq!(
        page.status, 200,
        "case setup: the account's own credential must be served, got {:?}",
        page.body
    );
    let listing_id = page.body["listingSummaries"][0]["listingId"]
        .as_str()
        .expect("the catalog holds a listing")
        .to_string();
    let listing = call_as(
        router,
        ACCOUNT_KEY_ID,
        "getListing",
        &json!({ "listingId": listing_id }),
    )
    .await;
    assert_eq!(
        listing.status, 200,
        "case setup: the listing must read back, got {:?}",
        listing.body
    );
    let catalog = listing.body["catalog"]
        .as_str()
        .expect("a listing names its catalog")
        .to_string();
    let listing_id = listing.body["listingId"]
        .as_str()
        .expect("a listing names itself")
        .to_string();
    let entity = &listing.body["associatedEntities"][0];
    let product_id = entity["product"]["productId"]
        .as_str()
        .expect("a listing associates a product")
        .to_string();
    let offer_id = entity["offer"]["offerId"]
        .as_str()
        .expect("a listing associates an offer")
        .to_string();
    (catalog, listing_id, product_id, offer_id)
}

/// Every operation this service serves, each with a request of its own and the record its
/// refusal names. The rule covers all of them, so the table is the whole surface rather than
/// a sample of it.
fn operations(listing_id: &str, product_id: &str, offer_id: &str) -> Vec<Operation> {
    // An offer-set id of this catalog's own shape that no record carries, so the row gets
    // past authorization into the operation's own not-found answer rather than into a second
    // decision.
    let offer_set_id = "os-0000-no-such-set";
    vec![
        Operation {
            name: "GetListing",
            route: "getListing",
            body: json!({ "listingId": listing_id }),
            record: format!("listing/{listing_id}"),
        },
        Operation {
            name: "GetProduct",
            route: "getProduct",
            body: json!({ "productId": product_id }),
            record: format!("product/{product_id}"),
        },
        Operation {
            name: "GetOffer",
            route: "getOffer",
            body: json!({ "offerId": offer_id }),
            record: format!("offer/{offer_id}"),
        },
        Operation {
            name: "GetOfferTerms",
            route: "getOfferTerms",
            body: json!({ "offerId": offer_id }),
            record: format!("offer/{offer_id}"),
        },
        Operation {
            name: "GetOfferSet",
            route: "getOfferSet",
            body: json!({ "offerSetId": offer_set_id }),
            record: format!("offerSet/{offer_set_id}"),
        },
        Operation {
            name: "ListFulfillmentOptions",
            route: "listFulfillmentOptions",
            body: json!({ "productId": product_id }),
            record: format!("product/{product_id}"),
        },
        Operation {
            name: "ListPurchaseOptions",
            route: "listPurchaseOptions",
            body: json!({}),
            record: String::new(),
        },
        Operation {
            name: "SearchListings",
            route: "searchListings",
            body: json!({ "maxResults": 5 }),
            record: String::new(),
        },
        Operation {
            name: "SearchFacets",
            route: "searchFacets",
            body: json!({}),
            record: String::new(),
        },
    ]
}

/// The resource ARN an operation addressing `record` under `catalog` is judged against: an
/// AWS Marketplace catalog ARN, which carries no Region and no account.
fn resource_of(catalog: &str, record: &str) -> String {
    if record.is_empty() {
        format!("arn:aws:{ACTION_PREFIX}:::catalog/{catalog}")
    } else {
        format!("arn:aws:{ACTION_PREFIX}:::catalog/{catalog}/{record}")
    }
}

/// THE RULE: every operation is judged against its OWN `aws-marketplace:<OperationName>`
/// action before it reads anything; a refusal is HTTP 403 `AccessDeniedException` naming the
/// principal, the action, the catalog resource the request addressed and which of IAM's
/// rules refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    let (probe, _) = router_with_identity();
    let (catalog, listing_id, product_id, offer_id) = catalog_records(&probe).await;

    for operation in operations(&listing_id, &product_id, &offer_id) {
        let action = format!("{ACTION_PREFIX}:{}", operation.name);
        // Another action of this service, which a row grants instead of the one under test.
        let other = operations(&listing_id, &product_id, &offer_id)
            .into_iter()
            .map(|row| format!("{ACTION_PREFIX}:{}", row.name))
            .find(|candidate| *candidate != action)
            .expect("this service has more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{} with {row}", operation.name);
            // A fresh store, user and policy per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(&engine, document.as_ref());

            let answer = call_as(&router, USER_KEY_ID, operation.route, &operation.body).await;
            match refused {
                Some(refusal) => {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?}: expected HTTP 403, got {} ({:?})",
                        answer.status, answer.body
                    );
                    assert!(
                        answer.refused(),
                        "case {case:?}: expected an AccessDeniedException, got {:?} ({:?})",
                        answer.error_code,
                        answer.body
                    );
                    assert_eq!(
                        answer.message(),
                        format!(
                            "User: {} is not authorized to perform: {action} on resource: {} {}",
                            user_arn(),
                            resource_of(&catalog, &operation.record),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {:?}",
                        answer.body
                    );
                }
                None => {
                    // Authorization answers BEFORE anything is read, so an allowed call falls
                    // through to the operation's own verdict — here the record's own answer,
                    // or a not-found for the offer-set id nothing created — and never to
                    // another authorization decision.
                    assert!(
                        !answer.refused() && answer.status != 403,
                        "case {case:?}: an allowed request must not be refused, got {} ({:?})",
                        answer.status,
                        answer.body
                    );
                }
            }
        }
    }
}

/// RULE: the decision is taken before the catalog is read, so a REFUSED read reports nothing
/// of the record it named — not even whether the catalog holds it.
#[tokio::test]
async fn a_refused_read_reports_nothing_of_the_catalog() {
    let (probe, _) = router_with_identity();
    let (_, listing_id, product_id, offer_id) = catalog_records(&probe).await;

    let (router, engine) = router_with_identity();
    given_user_key(&engine);
    given_user_policy(&engine, None);

    for operation in operations(&listing_id, &product_id, &offer_id) {
        let answer = call_as(&router, USER_KEY_ID, operation.route, &operation.body).await;
        assert!(
            answer.refused(),
            "case {}: expected the refusal, got {} ({:?})",
            operation.name,
            answer.status,
            answer.body
        );
        let object = answer.body.as_object().cloned().unwrap_or_default();
        let reported: Vec<&String> = object
            .keys()
            .filter(|member| *member != "message" && *member != "__type")
            .collect();
        assert!(
            reported.is_empty(),
            "case {}: a refused call reports only the error, got {reported:?}",
            operation.name
        );
        // And the refusal does not say whether the record exists: the GetOfferSet row names
        // an id nothing created, and its refusal reads exactly like the rows that name real
        // records.
        assert!(
            !answer.message().contains("not found"),
            "case {}: a refusal must disclose nothing about existence, got {:?}",
            operation.name,
            answer.message()
        );
    }
}

/// RULE: a request is judged against the ARN of the RECORD it addressed, so an allow scoped
/// to one record covers the operations that address it and no other.
#[tokio::test]
async fn an_action_is_judged_against_the_record_the_request_addressed() {
    let (probe, _) = router_with_identity();
    let (catalog, listing_id, product_id, offer_id) = catalog_records(&probe).await;
    let this_listing = resource_of(&catalog, &format!("listing/{listing_id}"));
    let any_listing = resource_of(&catalog, "listing/*");
    let this_catalog = resource_of(&catalog, "");
    let another_catalog = resource_of("AnotherCatalog", "listing/*");

    // (case, the Resource the allow is scoped to, which operations it covers)
    let rows: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "every resource",
            "*".to_string(),
            vec!["GetListing", "GetProduct", "SearchListings"],
        ),
        (
            "exactly this listing's ARN",
            this_listing,
            vec!["GetListing"],
        ),
        ("any listing's ARN", any_listing, vec!["GetListing"]),
        (
            "the catalog itself",
            this_catalog,
            vec!["SearchListings"],
        ),
        ("another catalog's listings", another_catalog, Vec::new()),
    ];

    for (case, resource, covered) in rows {
        for operation in operations(&listing_id, &product_id, &offer_id) {
            if !["GetListing", "GetProduct", "SearchListings"].contains(&operation.name) {
                continue;
            }
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(
                &engine,
                Some(&json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": format!("{ACTION_PREFIX}:*"),
                        "Resource": resource.clone(),
                    }],
                })),
            );

            let answer = call_as(&router, USER_KEY_ID, operation.route, &operation.body).await;
            let allowed = covered.contains(&operation.name);
            assert_eq!(
                answer.refused(),
                !allowed,
                "case {case:?}/{}: an allow scoped to {resource:?} must {}cover this operation, \
                 got {} ({:?})",
                operation.name,
                if allowed { "" } else { "not " },
                answer.status,
                answer.body
            );
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// user's policies say. This is what every other suite of this crate signs with.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let (probe, _) = router_with_identity();
    let (_, listing_id, product_id, offer_id) = catalog_records(&probe).await;

    let (router, engine) = router_with_identity();
    given_user_key(&engine);
    // The user carries nothing at all, which is what refuses it above.
    given_user_policy(&engine, None);

    for operation in operations(&listing_id, &product_id, &offer_id) {
        let answer = call_as(&router, ACCOUNT_KEY_ID, operation.route, &operation.body).await;
        assert!(
            !answer.refused() && answer.status != 403,
            "case {}: the account's own credential must not be refused, got {} ({:?})",
            operation.name,
            answer.status,
            answer.body
        );
    }
}

/// RULE: an allowed read is the operation's OWN answer — the listing the request named, read
/// back whole — so the gate decides who may read and changes nothing about what is read.
#[tokio::test]
async fn an_allowed_read_answers_from_the_catalog() {
    let (probe, _) = router_with_identity();
    let (_, listing_id, _, _) = catalog_records(&probe).await;

    let (router, engine) = router_with_identity();
    given_user_key(&engine);
    given_user_policy(
        &engine,
        Some(&allow(json!(format!("{ACTION_PREFIX}:GetListing")))),
    );

    let answer = call_as(
        &router,
        USER_KEY_ID,
        "getListing",
        &json!({ "listingId": listing_id }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case an allowed GetListing: expected HTTP 200, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["listingId"].as_str(),
        Some(listing_id.as_str()),
        "case an allowed GetListing: expected the listing it named, got {:?}",
        answer.body
    );
}
