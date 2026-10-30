// SPDX-License-Identifier: CC0-1.0

//! User-pool groups over the wire: `CreateGroup` stores a group in the pool it
//! names and every later `GetGroup` renders the STORED group, driven through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a created group is keyed by (`UserPoolId`, `GroupName`),
//! it renders every member the request set and none it did not, a second group
//! under a name the pool already holds is refused, and a group of an absent pool
//! or an absent name is a 400. Each table row names the case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

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

/// One row of the `CreateGroup` table: a group name and the optional members the
/// request sets. The rows differ along what the rule generalises over — which
/// optional members are present, the form and length of the name, and the
/// precedence value.
struct GroupCase {
    case: &'static str,
    group_name: String,
    description: Option<String>,
    role_arn: Option<&'static str>,
    precedence: Option<i32>,
}

impl GroupCase {
    /// This case's `CreateGroup` request for `user_pool_id`: an optional member
    /// the case leaves unset is left OUT of the request, not sent as null.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("GroupName".to_string(), json!(self.group_name));
        if let Some(description) = &self.description {
            body.insert("Description".to_string(), json!(description));
        }
        if let Some(role_arn) = self.role_arn {
            body.insert("RoleArn".to_string(), json!(role_arn));
        }
        if let Some(precedence) = self.precedence {
            body.insert("Precedence".to_string(), json!(precedence));
        }
        Value::Object(body)
    }
}

/// The table. `Description` spans its modelled length 0..2048, `Precedence` its
/// modelled range 0.. (lowest value and a high one), `GroupName` the modelled
/// pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$` (ASCII, punctuation, non-ASCII
/// letters and numbers) and its length 1..128 — every constraint from
/// `smithy_query(query="operation",operation="CreateGroup")`.
fn group_cases() -> Vec<GroupCase> {
    vec![
        GroupCase {
            case: "name only: every optional member left unset",
            group_name: "admins".to_string(),
            description: None,
            role_arn: None,
            precedence: None,
        },
        GroupCase {
            case: "every optional member set, precedence at the range minimum",
            group_name: "ops.team-1_a".to_string(),
            description: Some("the operators".to_string()),
            role_arn: Some("arn:aws:iam::000000000000:role/group-role"),
            precedence: Some(0),
        },
        GroupCase {
            case: "empty description (the length minimum) and a high precedence",
            group_name: "readers".to_string(),
            description: Some(String::new()),
            role_arn: None,
            precedence: Some(2_147_483_647),
        },
        GroupCase {
            case: "description at the length maximum, non-ASCII letters and a number in the name",
            group_name: "グループ①".to_string(),
            description: Some("d".repeat(2048)),
            role_arn: None,
            precedence: Some(7),
        },
        GroupCase {
            case: "group name at the length maximum",
            group_name: "g".repeat(128),
            description: Some("longest name".to_string()),
            role_arn: Some("arn:aws:iam::000000000000:role/long-name-role"),
            precedence: Some(1),
        },
    ]
}

#[tokio::test]
async fn a_created_group_round_trips_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "group-pool").await;

    for case in group_cases() {
        let label = case.case;
        let created = ok(&router, "CreateGroup", &case.request(&user_pool_id)).await;
        let group = &created["Group"];

        assert_eq!(
            group["GroupName"],
            json!(case.group_name),
            "case {label:?}: the stored group name must be the created one, got {group}"
        );
        assert_eq!(
            group["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the group must name the pool it was created in, got {group}"
        );
        match case.description.as_deref().filter(|value| !value.is_empty()) {
            Some(description) => assert_eq!(
                group["Description"],
                json!(description),
                "case {label:?}: the stored description must be the created one, got {group}"
            ),
            // An empty `Description` is accepted (its modelled length starts at
            // 0) and materialised as no description at all, exactly like one the
            // request left out.
            None => assert!(
                group.get("Description").is_none(),
                "case {label:?}: a group created without a description renders none, got {group}"
            ),
        }
        match case.role_arn {
            Some(role_arn) => assert_eq!(
                group["RoleArn"],
                json!(role_arn),
                "case {label:?}: the stored role ARN must be the created one, got {group}"
            ),
            None => assert!(
                group.get("RoleArn").is_none(),
                "case {label:?}: a group created without a role ARN renders none, got {group}"
            ),
        }
        match case.precedence {
            Some(precedence) => assert_eq!(
                group["Precedence"],
                json!(precedence),
                "case {label:?}: the stored precedence must be the created one, got {group}"
            ),
            None => assert!(
                group.get("Precedence").is_none(),
                "case {label:?}: a group created without a precedence renders none, got {group}"
            ),
        }
        assert!(
            group["CreationDate"].is_number() && group["LastModifiedDate"].is_number(),
            "case {label:?}: a stored group carries both timestamps, got {group}"
        );
        assert_eq!(
            group["CreationDate"], group["LastModifiedDate"],
            "case {label:?}: a group nothing has updated was last modified when it was created, got {group}"
        );

        // The round trip: the reader renders the STORED group, member for member.
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            read["Group"], created["Group"],
            "case {label:?}: GetGroup must render the stored group, got {read}"
        );

        // And reading it again changes nothing a later caller sees.
        let again = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            again["Group"], read["Group"],
            "case {label:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn a_group_name_the_pool_already_holds_is_refused_and_the_stored_group_survives() {
    let router = router();
    let user_pool_id = pool(&router, "duplicate-pool").await;

    for case in group_cases() {
        let label = case.case;
        let created = ok(&router, "CreateGroup", &case.request(&user_pool_id)).await;

        // A second group under the same name, with DIFFERENT optional members:
        // refused, and the stored group is left exactly as it was.
        let (status, payload) = call(
            &router,
            "CreateGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "GroupName": case.group_name,
                "Description": "a second group under a taken name",
                "Precedence": 42,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "GroupExistsException"),
            "case {label:?}: a group name already taken is a 400 GroupExistsException, got {payload}"
        );

        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            read["Group"], created["Group"],
            "case {label:?}: a refused create must not have touched the stored group, got {read}"
        );
    }
}

#[tokio::test]
async fn a_group_is_keyed_by_its_pool_and_its_exact_name() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;

    // The same name in two pools is two groups, each naming its own pool.
    for (label, user_pool_id, description) in [
        ("the first pool", &first_pool, "the first pool's admins"),
        ("the second pool", &second_pool, "the second pool's admins"),
    ] {
        let created = ok(
            &router,
            "CreateGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "GroupName": "admins",
                "Description": description,
            }),
        )
        .await;
        assert_eq!(
            created["Group"]["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: the group must name the pool it was created in, got {created}"
        );
    }
    for (label, user_pool_id, description) in [
        ("the first pool", &first_pool, "the first pool's admins"),
        ("the second pool", &second_pool, "the second pool's admins"),
    ] {
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
        )
        .await;
        assert_eq!(
            read["Group"]["Description"],
            json!(description),
            "case {label:?}: each pool's group keeps its own members, got {read}"
        );
    }

    // `GroupNameType` carries no case-insensitivity trait, so names differing
    // only in case are different groups.
    let created = ok(
        &router,
        "CreateGroup",
        &json!({
            "UserPoolId": first_pool,
            "GroupName": "Admins",
            "Description": "capitalised",
        }),
    )
    .await;
    assert_eq!(
        created["Group"]["GroupName"], "Admins",
        "a name differing only in case is a different group, got {created}"
    );
    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": first_pool, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"]["Description"],
        json!("the first pool's admins"),
        "a group is matched by its exact name, got {read}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_group_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-pool").await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;

    for (label, operation, body) in [
        (
            "CreateGroup on an absent pool",
            "CreateGroup",
            json!({ "UserPoolId": "us-east-1_absent", "GroupName": "admins" }),
        ),
        (
            "GetGroup on an absent pool",
            "GetGroup",
            json!({ "UserPoolId": "us-east-1_absent", "GroupName": "admins" }),
        ),
        (
            "GetGroup for a name the pool does not hold",
            "GetGroup",
            json!({ "UserPoolId": user_pool_id, "GroupName": "absent-group" }),
        ),
    ] {
        let (status, payload) = call(&router, operation, &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: must be a 400 ResourceNotFoundException, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;

    // One value past each published bound of `CreateGroup`'s input
    // (`smithy_query(query="operation",operation="CreateGroup")`): the group
    // name's length 1..128 and pattern, the description's length 0..2048, and
    // the precedence's range 0...
    for (label, body) in [
        (
            "an empty group name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "GroupName": "" }),
        ),
        (
            "a group name of 129 characters, past the length maximum 128",
            json!({ "UserPoolId": user_pool_id, "GroupName": "g".repeat(129) }),
        ),
        (
            "a group name holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "GroupName": "two words" }),
        ),
        (
            "a description of 2049 characters, past the length maximum 2048",
            json!({
                "UserPoolId": user_pool_id,
                "GroupName": "over-long-description",
                "Description": "d".repeat(2049),
            }),
        ),
        (
            "a precedence of -1, below the range minimum 0",
            json!({
                "UserPoolId": user_pool_id,
                "GroupName": "negative-precedence",
                "Precedence": -1,
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "GroupName": "admins" }),
        ),
    ] {
        let (status, payload) = call(&router, "CreateGroup", &body).await;
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

    // None of the refused requests stored anything.
    for (label, group_name) in [
        ("over-long description", "over-long-description"),
        ("negative precedence", "negative-precedence"),
    ] {
        let (status, payload) = call(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {label:?}: a refused create stores no group, got {payload}"
        );
    }
}
