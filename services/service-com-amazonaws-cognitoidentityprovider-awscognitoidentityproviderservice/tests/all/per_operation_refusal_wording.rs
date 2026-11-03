// SPDX-License-Identifier: CC0-1.0

//! How this service words the refusals whose sentence belongs to the OPERATION
//! rather than to the service, driven through the router exactly as an SDK
//! drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rules under test:
//!
//! * An app-client lookup made inside a named pool fails in two different ways
//!   and is worded by WHICH way it failed: a `ClientId` that names an app client
//!   of ANOTHER pool is reported with both halves of what was asked for — the id
//!   and the pool it is not in — while a `ClientId` that names no app client at
//!   all carries the sentence the asking operation answers for that case. A
//!   `ClientId` is minted per app client and lives in exactly one pool, so those
//!   two are all the ways the lookup can fail.
//! * A pool lookup carries the sentence its OWN operation answers: creating an
//!   app client and updating a pool report the unresolved pool by error class
//!   and status alone, with an empty message; setting log delivery reports it
//!   with its own sentence; the operations that word it `User pool <id> does not
//!   exist.` keep that.
//! * An operation addressed by the pair `UserPoolId` + `Domain` tells a pair
//!   that resolves NOTHING from one that resolves to a stored pool and a domain
//!   name ANOTHER stored pool holds: the first is the resource each operation
//!   cannot find, the second is a mismatch of the two parameters, refused as a
//!   bad parameter by both. Taking a name some pool already holds is refused the
//!   same way, with a sentence that quotes neither the name nor a pool.
//! * `DeleteUserPool` refuses a pool whose deletion protection is on with one
//!   fixed sentence that names no pool, and the refusal goes away once the
//!   protection does.
//! * A pool holds user import jobs up to the quota its own `LimitExceededException`
//!   states, which is far above what ONE `ListUserImportJobs` page carries.
//!
//! The rows of each table differ along what its rule generalises over — the way
//! the lookup failed, the form and length of the id asked for, the operation
//! asking, the form of the domain name, how the protection came to be on — and
//! every assertion names its case and reports the actual value.

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

/// The sentence `DescribeUserPoolClient` and `UpdateUserPoolClient` answer for a
/// `ClientId` that names no app client at all: fixed text in full.
const APP_CLIENT_NOT_FOUND: &str = "User pool client does not exist.";

/// The sentence both pair-addressed domain operations answer when the pair names
/// a stored pool and a domain name another stored pool holds.
const DOMAIN_OF_OTHER_POOL: &str = "The domain is not configured for this user pool.";

/// The sentence `CreateUserPoolDomain` answers for a name some pool already holds.
const DOMAIN_EXISTS: &str = "Domain already exists.";

/// The sentence `DeleteUserPoolDomain` answers for a pair that resolves nothing.
const NO_SUCH_DOMAIN: &str = "No such domain or user pool exists.";

/// The sentence `UpdateUserPoolDomain` answers for a pair that resolves nothing.
const DOMAIN_NOT_FOUND: &str = "User pool domain was not found.";

/// The sentence `DeleteUserPool` answers for a pool that is protected from
/// deletion: fixed text in full, naming no pool.
const DELETION_PROTECTED: &str = "The user pool cannot be deleted because deletion protection is \
                                  activated. Deletion protection must be inactivated first.";

/// An IAM role a user import job logs its results to.
const IMPORT_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// A CloudWatch Logs log group a user pool exports its logs to.
const LOG_GROUP: &str = "arn:aws:logs:us-east-1:000000000000:log-group:/aws/cognito/pool";

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
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
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

/// The `message` of an error body, under either casing the protocol admits. An
/// error that carries an EMPTY message answers the empty string, which is a
/// different answer from carrying none at all.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool named `pool_name`, carrying `members` beside the name, and
/// return its minted id.
async fn pool(router: &MultiServiceRouter, case: &str, pool_name: &str, members: &Value) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(pool_name));
    for (name, value) in members.as_object().expect("the members are an object") {
        body.insert(name.clone(), value.clone());
    }
    let created = ok(router, case, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id, got {created}"))
        .to_string()
}

/// Create one app client of `user_pool_id` and return its minted `ClientId`.
async fn client(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = ok(
        router,
        case,
        "CreateUserPoolClient",
        &json!({ "UserPoolId": user_pool_id, "ClientName": client_name }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted client id, got {created}"))
        .to_string()
}

/// The request `operation` carries for the pair `user_pool_id` + `client_id`:
/// the pair, plus whatever members the operation's model requires beside it so
/// the app-client lookup is what the request fails on.
fn client_request(operation: &str, user_pool_id: &str, client_id: &str) -> Value {
    let mut body = Map::new();
    body.insert("UserPoolId".to_string(), json!(user_pool_id));
    body.insert("ClientId".to_string(), json!(client_id));
    let beside = match operation {
        "CreateTerms" => json!({
            "TermsName": "terms-of-use",
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": { "cognito:default": "https://terms.example.com" },
        }),
        "DescribeTermsByClient" => json!({ "TermsName": "terms-of-use" }),
        // A `ClientSecretIdType`-shaped id (length 1..128) nothing minted: the
        // app-client lookup precedes the secret lookup, so it never resolves.
        "DeleteUserPoolClientSecret" => json!({ "ClientSecretId": "neverminted--1700000000000" }),
        "SetUICustomization" => json!({ "CSS": ".label { color: #111111; }" }),
        "CreateManagedLoginBranding" => json!({ "UseCognitoProvidedValues": true }),
        "AdminInitiateAuth" => json!({
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "someone", "PASSWORD": "Sup3r-secret-password!" },
        }),
        _ => json!({}),
    };
    for (name, value) in beside.as_object().expect("the members are an object") {
        body.insert(name.clone(), value.clone());
    }
    Value::Object(body)
}

/// How the `ClientId` a request asks for comes to name no app client of the pool
/// it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// A live app client of ANOTHER pool: the id resolves, the pairing does not.
    OfAnotherPool,
    /// A `ClientIdType`-shaped id (pattern `^[\w+]+$`) no create ever minted.
    NeverMinted(&'static str),
    /// An id no create minted, at the modelled length maximum, 128.
    OfMaximumLength,
    /// An id no create minted, at the modelled length minimum, 1.
    OfMinimumLength,
    /// The pool's own live app client id with one further `^[\w+]+$` character.
    NearMiss,
}

#[tokio::test]
async fn an_app_client_lookup_in_a_named_pool_is_worded_by_the_way_it_failed() {
    // Every operation that resolves an app client by the PAIR `UserPoolId` +
    // `ClientId`, with the sentence it answers for an id that names NO app
    // client: the two that address the app client itself quote nothing, and the
    // ones that resolve a client on the way to another resource — its terms
    // documents, its branding, its UI customization, its secrets — and the ones
    // that act for it quote the id asked for.
    let operations: &[(&str, bool)] = &[
        ("DescribeUserPoolClient", false),
        ("UpdateUserPoolClient", false),
        ("DeleteUserPoolClient", true),
        ("AddUserPoolClientSecret", true),
        ("ListUserPoolClientSecrets", true),
        ("DeleteUserPoolClientSecret", true),
        ("CreateTerms", true),
        ("DescribeTermsByClient", true),
        ("GetUICustomization", true),
        ("SetUICustomization", true),
        ("DescribeRiskConfiguration", true),
        ("SetRiskConfiguration", true),
        ("CreateManagedLoginBranding", true),
        ("AdminInitiateAuth", true),
    ];

    // The ways the lookup fails: the one id that resolves to another pool's app
    // client, and four shapes of id that resolve to none — spanning the pattern
    // `^[\w+]+$` and both published length bounds of `ClientIdType`
    // (`smithy_query(query="shape",shape="ClientIdType")`).
    let ways: &[(&str, Asked)] = &[
        ("a live app client of another pool", Asked::OfAnotherPool),
        (
            "a lower-case id no create minted",
            Asked::NeverMinted("1h57kf5cpq17m0eml12eukdi1o"),
        ),
        (
            "an id at the modelled maximum length, 128",
            Asked::OfMaximumLength,
        ),
        (
            "an id at the modelled minimum length, 1",
            Asked::OfMinimumLength,
        ),
        (
            "the pool's own client id with one more character",
            Asked::NearMiss,
        ),
    ];

    let router = router();
    let user_pool_id = pool(&router, "lookup", "lookup-pool", &json!({})).await;
    let other_pool = pool(&router, "lookup", "lookup-other-pool", &json!({})).await;
    let live = client(&router, "lookup", &user_pool_id, "live-client").await;
    let elsewhere = client(&router, "lookup", &other_pool, "elsewhere-client").await;
    let longest = "z".repeat(128);

    for (way, asked_kind) in ways {
        let asked = match asked_kind {
            Asked::OfAnotherPool => elsewhere.clone(),
            Asked::NeverMinted(id) => (*id).to_string(),
            Asked::OfMaximumLength => longest.clone(),
            Asked::OfMinimumLength => "a".to_string(),
            Asked::NearMiss => format!("{live}+"),
        };
        for (operation, quotes_the_id) in operations {
            let case = format!("{operation} on {way}");
            // Which sentence this lookup owes: the id and the pool it is not in
            // when the id names another pool's app client, and otherwise the
            // sentence this operation answers for an id that names none.
            let expected = match asked_kind {
                Asked::OfAnotherPool => {
                    format!("User pool client {asked} does not exist in user pool {user_pool_id}.")
                }
                _ if *quotes_the_id => format!("User pool client {asked} does not exist."),
                _ => APP_CLIENT_NOT_FOUND.to_string(),
            };

            let body = client_request(operation, &user_pool_id, &asked);
            let (status, payload) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected a 400 ResourceNotFoundException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The sentence for an id that names no app client at all quotes no
            // pool, whichever operation answers it, so no caller can read one
            // pool's existence out of another's refusal. A one-character id is a
            // substring of ordinary prose, so the id check is made on the ids
            // long enough to mean something.
            if *asked_kind != Asked::OfAnotherPool {
                assert!(
                    !message.contains(user_pool_id.as_str()),
                    "case {case:?}: the sentence must quote no pool, got {message:?}"
                );
                if !*quotes_the_id && asked.len() >= 4 {
                    assert!(
                        !message.contains(asked.as_str()),
                        "case {case:?}: the sentence must quote no client id, got {message:?}"
                    );
                }
            }
        }
    }

    // Every one of those was a refusal: both pools still hold exactly the app
    // client their create stored, and each reads back through its own pool.
    for (case, holder, held) in [
        ("the named pool", &user_pool_id, &live),
        ("the other pool", &other_pool, &elsewhere),
    ] {
        let described = ok(
            &router,
            case,
            "DescribeUserPoolClient",
            &json!({ "UserPoolId": holder, "ClientId": held }),
        )
        .await;
        assert_eq!(
            described["UserPoolClient"]["ClientId"],
            json!(held),
            "case {case:?}: the stored app client must still read back, got {described}"
        );
    }
}

/// How an operation reports a `UserPoolId` no stored pool answers to: the
/// sentence is the ASKING operation's, not one template for the service.
#[derive(Debug, Clone, Copy)]
enum PoolRefusal {
    /// The error class and status alone, with an empty message.
    Empty,
    /// This operation's own sentence, which joins `UserPool with ID: ` to
    /// `does not exist.` with no separator.
    WithIdUnspaced,
    /// `User pool <UserPoolId> does not exist.`
    WithId,
}

#[tokio::test]
async fn a_pool_lookup_carries_the_sentence_its_own_operation_answers() {
    // (the operation, what it answers, the members it carries beside the pool id)
    let operations: &[(&str, PoolRefusal, Value)] = &[
        (
            "CreateUserPoolClient",
            PoolRefusal::Empty,
            json!({ "ClientName": "client-of-an-absent-pool" }),
        ),
        ("UpdateUserPool", PoolRefusal::Empty, json!({})),
        (
            "SetLogDeliveryConfiguration",
            PoolRefusal::WithIdUnspaced,
            json!({
                "LogConfigurations": [{
                    "LogLevel": "ERROR",
                    "EventSource": "userNotification",
                    "CloudWatchLogsConfiguration": { "LogGroupArn": LOG_GROUP },
                }],
            }),
        ),
        // Every operation that answers the same sentence as the one above:
        // configuring a pool-level resource in another namespace — a hosted
        // login domain, a cross-Region replica — words the pool lookup the same
        // way, joined with no separator before `does`.
        (
            "CreateUserPoolDomain",
            PoolRefusal::WithIdUnspaced,
            json!({ "Domain": "domain-of-an-absent-pool" }),
        ),
        (
            "CreateUserPoolReplica",
            PoolRefusal::WithIdUnspaced,
            json!({ "RegionName": "us-west-2" }),
        ),
        // The controls: the operations whose own sentence names the pool keep it.
        ("DescribeUserPool", PoolRefusal::WithId, json!({})),
        ("ListUserPoolClients", PoolRefusal::WithId, json!({})),
        ("GetLogDeliveryConfiguration", PoolRefusal::WithId, json!({})),
        ("DeleteUserPool", PoolRefusal::WithId, json!({})),
        ("ListUserPoolReplicas", PoolRefusal::WithId, json!({})),
    ];

    // Ids satisfying `UserPoolIdType`'s pattern `^[\w-]+_[0-9a-zA-Z]+$` that no
    // create minted, so the lookup is what fails rather than a constraint: two
    // Regions, a numeric suffix and an upper-case one.
    let absent_pools = [
        "us-east-1_absentpool",
        "eu-west-2_0000000000",
        "us-west-2_ZZZZZZZZZ",
    ];

    let router = router();
    // One pool that DOES exist, so a refusal cannot be the store being empty.
    let live_pool = pool(&router, "pool lookup", "pool-lookup-live", &json!({})).await;

    for absent in absent_pools {
        for (operation, refusal, beside) in operations {
            let case = format!("{operation} against {absent}");
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(absent));
            for (name, value) in beside.as_object().expect("the members are an object") {
                body.insert(name.clone(), value.clone());
            }
            let (status, payload) = call(&router, operation, &Value::Object(body)).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected a 400 ResourceNotFoundException, got {status} \
                 with {payload}"
            );
            let expected = match refusal {
                PoolRefusal::Empty => String::new(),
                PoolRefusal::WithIdUnspaced => format!("UserPool with ID: {absent}does not exist."),
                PoolRefusal::WithId => format!("User pool {absent} does not exist."),
            };
            let message = error_message(&payload);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The stored pool is untouched by every refusal above, and the same
    // operations answer it rather than refusing.
    let described = ok(
        &router,
        "pool lookup",
        "DescribeUserPool",
        &json!({ "UserPoolId": live_pool }),
    )
    .await;
    assert_eq!(
        described["UserPool"]["Id"],
        json!(live_pool),
        "the stored pool must still read back, got {described}"
    );
}

#[tokio::test]
async fn a_domain_of_another_pool_is_a_parameter_mismatch_not_a_missing_resource() {
    // The names span what `DomainType` admits (length 1..63, pattern
    // `^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$`, from
    // `smithy_query(query="shape",shape="DomainType")`): a plain prefix, both
    // published length bounds and a hyphenated one. Each row runs on its own
    // pair of pools, since a pool holds at most one prefix domain at a time.
    let longest = format!("d{}", "x".repeat(62));
    let names = [
        ("a plain prefix", "pair-login".to_string()),
        ("the one-character length minimum", "a".to_string()),
        ("the 63-character length maximum", longest),
        ("a hyphenated prefix", "login-2-example".to_string()),
    ];

    for (index, (case, domain)) in names.iter().enumerate() {
        let router = router();
        let holder = pool(&router, case, &format!("pair-holder-{index}"), &json!({})).await;
        let claimant = pool(&router, case, &format!("pair-claimant-{index}"), &json!({})).await;
        ok(
            &router,
            case,
            "CreateUserPoolDomain",
            &json!({ "UserPoolId": holder, "Domain": domain }),
        )
        .await;

        // (what the claimant asks, the refusal it owes)
        let rows: &[(&str, &str, Value, &str, &str)] = &[
            (
                "taking a name a pool already holds",
                "CreateUserPoolDomain",
                json!({ "UserPoolId": claimant, "Domain": domain }),
                "InvalidParameterException",
                DOMAIN_EXISTS,
            ),
            (
                "removing a name another pool holds",
                "DeleteUserPoolDomain",
                json!({ "UserPoolId": claimant, "Domain": domain }),
                "InvalidParameterException",
                DOMAIN_OF_OTHER_POOL,
            ),
            (
                "reconfiguring a name another pool holds",
                "UpdateUserPoolDomain",
                json!({ "UserPoolId": claimant, "Domain": domain, "ManagedLoginVersion": 1 }),
                "InvalidParameterException",
                DOMAIN_OF_OTHER_POOL,
            ),
            // The controls: a pair that resolves NOTHING is each operation's own
            // not-found refusal, which the mismatch above must not be flattened
            // into.
            (
                "removing a name nothing created",
                "DeleteUserPoolDomain",
                json!({ "UserPoolId": claimant, "Domain": "pair-never-created" }),
                "InvalidParameterException",
                NO_SUCH_DOMAIN,
            ),
            (
                "reconfiguring a name nothing created",
                "UpdateUserPoolDomain",
                json!({ "UserPoolId": claimant, "Domain": "pair-never-created", "ManagedLoginVersion": 1 }),
                "ResourceNotFoundException",
                DOMAIN_NOT_FOUND,
            ),
        ];

        for (label, operation, body, expected_code, expected_message) in rows {
            let row = format!("{case}: {label}");
            let (status, payload) = call(&router, operation, body).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, *expected_code),
                "case {row:?}: expected a 400 {expected_code}, got {status} with {payload}"
            );
            let message = error_message(&payload);
            assert_eq!(
                message, *expected_message,
                "case {row:?}: expected {expected_message:?}, got {message:?}"
            );
            // Each sentence is fixed text in full: it names neither pool, and
            // for the names long enough not to occur inside ordinary prose, not
            // the domain either.
            assert!(
                !message.contains(holder.as_str()) && !message.contains(claimant.as_str()),
                "case {row:?}: the sentence must name no pool, got {message:?}"
            );
            if domain.len() >= 4 {
                assert!(
                    !message.contains(domain.as_str()),
                    "case {row:?}: the sentence must name no domain, got {message:?}"
                );
            }
        }

        // Every refusal left the domain with the pool that holds it, and the
        // holder can still reconfigure its own domain.
        let described = ok(
            &router,
            case,
            "DescribeUserPoolDomain",
            &json!({ "Domain": domain }),
        )
        .await;
        assert_eq!(
            described["DomainDescription"]["UserPoolId"],
            json!(holder),
            "case {case:?}: the stored domain must still belong to its pool, got {described}"
        );
        let updated = ok(
            &router,
            case,
            "UpdateUserPoolDomain",
            &json!({ "UserPoolId": holder, "Domain": domain, "ManagedLoginVersion": 2 }),
        )
        .await;
        assert_eq!(
            updated["ManagedLoginVersion"],
            json!(2),
            "case {case:?}: the holder's own pair resolves and updates, got {updated}"
        );
    }
}

#[tokio::test]
async fn a_pool_protected_from_deletion_is_refused_with_one_fixed_sentence() {
    // The rows differ along how the protection came to be on and on which pool,
    // so the sentence cannot be coming from either.
    let rows: &[(&str, &str, bool)] = &[
        ("a pool created with the protection on", "protected-at-create", false),
        ("a pool an update switched it on for", "protected-by-update", true),
    ];

    let router = router();
    for (case, pool_name, by_update) in rows {
        let created_with = if *by_update {
            json!({})
        } else {
            json!({ "DeletionProtection": "ACTIVE" })
        };
        let user_pool_id = pool(&router, case, pool_name, &created_with).await;
        if *by_update {
            ok(
                &router,
                case,
                "UpdateUserPool",
                &json!({ "UserPoolId": user_pool_id, "DeletionProtection": "ACTIVE" }),
            )
            .await;
        }

        let (status, payload) = call(
            &router,
            "DeleteUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, DELETION_PROTECTED,
            "case {case:?}: expected {DELETION_PROTECTED:?}, got {message:?}"
        );
        assert!(
            !message.contains(user_pool_id.as_str()),
            "case {case:?}: the sentence must name no pool, got {message:?}"
        );

        // The refusal deleted nothing: the pool still reads back, and it is
        // deletable again once the protection is switched off.
        let described = ok(
            &router,
            case,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            described["UserPool"]["DeletionProtection"],
            json!("ACTIVE"),
            "case {case:?}: the refused delete leaves the protected pool alone, got {described}"
        );
        ok(
            &router,
            case,
            "UpdateUserPool",
            &json!({ "UserPoolId": user_pool_id, "DeletionProtection": "INACTIVE" }),
        )
        .await;
        ok(
            &router,
            case,
            "DeleteUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        let (status, payload) = call(
            &router,
            "DescribeUserPool",
            &json!({ "UserPoolId": user_pool_id }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: the deleted pool must be gone, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_pool_holds_user_import_jobs_up_to_the_quota_its_refusal_states() {
    /// The quota a pool's user import jobs are bounded by, which the refusal
    /// states in full.
    const LIMIT: usize = 1_000;

    let router = router();
    let case = "the import-job quota";
    let user_pool_id = pool(&router, case, "import-job-quota-pool", &json!({})).await;

    // The jobs a pool may hold are its quota, which is far above the 50 entries
    // ONE `ListUserImportJobs` page carries: the counts checked here are the
    // page length, one past it, and the quota itself.
    for index in 1..=LIMIT {
        let created = ok(
            &router,
            case,
            "CreateUserImportJob",
            &json!({
                "UserPoolId": user_pool_id,
                "JobName": format!("quota-job-{index}"),
                "CloudWatchLogsRoleArn": IMPORT_ROLE,
            }),
        )
        .await;
        if index == 50 || index == 51 || index == LIMIT {
            assert_eq!(
                created["UserImportJob"]["JobName"],
                json!(format!("quota-job-{index}")),
                "case {case:?}: job {index} must be stored, got {created}"
            );
        }
    }

    // One job past the quota is refused, and the sentence states the quota.
    let (status, payload) = call(
        &router,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": "one-job-too-many",
            "CloudWatchLogsRoleArn": IMPORT_ROLE,
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "LimitExceededException"),
        "case {case:?}: expected a 400 LimitExceededException, got {status} with {payload}"
    );
    let expected = format!("The limit for user pool import jobs is: {LIMIT}.");
    let message = error_message(&payload);
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(user_pool_id.as_str()),
        "case {case:?}: the sentence must name no pool, got {message:?}"
    );

    // The refusal stored nothing: the first page still reports the page's worth
    // of jobs the pool held before it.
    let listing = ok(
        &router,
        case,
        "ListUserImportJobs",
        &json!({ "UserPoolId": user_pool_id, "MaxResults": 50 }),
    )
    .await;
    assert_eq!(
        listing["UserImportJobs"].as_array().map(Vec::len),
        Some(50),
        "case {case:?}: one page reports its own length, got {listing}"
    );
}
