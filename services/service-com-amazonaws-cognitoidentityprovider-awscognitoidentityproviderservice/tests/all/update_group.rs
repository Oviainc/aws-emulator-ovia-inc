// SPDX-License-Identifier: CC0-1.0

//! `UpdateGroup` over the wire: it rewrites the properties a request carries on a
//! STORED group, keeps the ones it does not, and every later read renders that
//! stored group, driven through the router exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `UpdateGroup` updates ANY OF the named group's precedence,
//! IAM role and description — a member the request carries replaces what the group
//! held, a member it leaves out keeps its stored value — leaves the group's name,
//! pool and creation instant alone, advances its last-modified instant, returns the
//! stored group, and refuses an absent pool or group with a 404
//! `ResourceNotFoundException`. Each table row names the case it asserts.

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

/// The updatable members of a group: what a `CreateGroup` or `UpdateGroup` request
/// sets, and what the stored group must render afterwards. `None` means the member
/// is left OUT of the request, not sent as null.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Members {
    description: Option<String>,
    role_arn: Option<String>,
    precedence: Option<i32>,
}

impl Members {
    /// Every member unset — what a request that names only the group sends.
    fn empty() -> Self {
        Self {
            description: None,
            role_arn: None,
            precedence: None,
        }
    }

    fn description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    fn role_arn(mut self, role_arn: &str) -> Self {
        self.role_arn = Some(role_arn.to_string());
        self
    }

    fn precedence(mut self, precedence: i32) -> Self {
        self.precedence = Some(precedence);
        self
    }

    /// What the stored group holds after an `UpdateGroup` carrying `update` is
    /// applied to a group holding `self`: each member the update sets, and the
    /// stored value for each member it does not.
    fn merged(&self, update: &Members) -> Members {
        Members {
            description: update
                .description
                .clone()
                .or_else(|| self.description.clone()),
            role_arn: update.role_arn.clone().or_else(|| self.role_arn.clone()),
            precedence: update.precedence.or(self.precedence),
        }
    }

    /// A request body for `operation`'s shared input shape: the pool, the group name
    /// and only the members this case sets.
    fn request(&self, user_pool_id: &str, group_name: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("GroupName".to_string(), json!(group_name));
        if let Some(description) = &self.description {
            body.insert("Description".to_string(), json!(description));
        }
        if let Some(role_arn) = &self.role_arn {
            body.insert("RoleArn".to_string(), json!(role_arn));
        }
        if let Some(precedence) = self.precedence {
            body.insert("Precedence".to_string(), json!(precedence));
        }
        Value::Object(body)
    }

    /// Assert a rendered `GroupType` carries exactly these members: a value this
    /// case expects, and NO member at all for one it expects to be absent.
    fn assert_rendered(&self, label: &str, where_: &str, group: &Value) {
        match &self.description {
            Some(description) => assert_eq!(
                group["Description"],
                json!(description),
                "case {label:?}: {where_} must render the expected description, got {group}"
            ),
            None => assert!(
                group.get("Description").is_none(),
                "case {label:?}: {where_} renders no description when nothing ever set one, got {group}"
            ),
        }
        match &self.role_arn {
            Some(role_arn) => assert_eq!(
                group["RoleArn"],
                json!(role_arn),
                "case {label:?}: {where_} must render the expected role ARN, got {group}"
            ),
            None => assert!(
                group.get("RoleArn").is_none(),
                "case {label:?}: {where_} renders no role ARN when nothing ever set one, got {group}"
            ),
        }
        match self.precedence {
            Some(precedence) => assert_eq!(
                group["Precedence"],
                json!(precedence),
                "case {label:?}: {where_} must render the expected precedence, got {group}"
            ),
            None => assert!(
                group.get("Precedence").is_none(),
                "case {label:?}: {where_} renders no precedence when nothing ever set one, got {group}"
            ),
        }
    }
}

/// One row of the table: a group created with `created` members, then updated with
/// `updated` members. The rows differ along what the rule generalises over — which
/// members the create and the update set, which the update omits, the precedence
/// value (both published bounds of its range 0..2^31-1), and the length and form of
/// the description.
struct UpdateCase {
    case: &'static str,
    group_name: String,
    created: Members,
    updated: Members,
}

/// The table. The bounds come from
/// `smithy_query(query="operation",operation="UpdateGroup")`: `Description`'s length
/// 0..2048, `Precedence`'s range 0.. with a documented maximum of `2^31-1`, and
/// `RoleArn`'s length 20..2048.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "every member rewritten to a new value",
            group_name: "admins".to_string(),
            created: Members::empty()
                .description("the first description")
                .role_arn("arn:aws:iam::000000000000:role/first")
                .precedence(5),
            updated: Members::empty()
                .description("the second description")
                .role_arn("arn:aws:iam::000000000000:role/second")
                .precedence(6),
        },
        UpdateCase {
            case: "a group created bare gains every member",
            group_name: "readers".to_string(),
            created: Members::empty(),
            updated: Members::empty()
                .description("now described")
                .role_arn("arn:aws:iam::000000000000:role/readers")
                .precedence(0),
        },
        UpdateCase {
            case: "an update that sets only precedence keeps the other two members",
            group_name: "ops.team-1_a".to_string(),
            created: Members::empty()
                .description("kept")
                .role_arn("arn:aws:iam::000000000000:role/kept")
                .precedence(3),
            updated: Members::empty().precedence(2_147_483_647),
        },
        UpdateCase {
            case: "an update that sets only the description keeps role and precedence",
            group_name: "グループ①".to_string(),
            created: Members::empty()
                .description("short")
                .role_arn("arn:aws:iam::000000000000:role/unicode")
                .precedence(1),
            updated: Members::empty().description("d".repeat(2048).as_str()),
        },
        UpdateCase {
            case: "an update that names only the group keeps all three members",
            group_name: "g".repeat(128),
            created: Members::empty()
                .description("untouched")
                .role_arn("arn:aws:iam::000000000000:role/untouched")
                .precedence(9),
            updated: Members::empty(),
        },
        UpdateCase {
            case: "an update of a bare group that sets nothing leaves it bare",
            group_name: "still-bare".to_string(),
            created: Members::empty(),
            updated: Members::empty(),
        },
        UpdateCase {
            case: "an empty description (the length minimum) and precedence at its minimum",
            group_name: "edge-values".to_string(),
            created: Members::empty()
                .description("not empty yet")
                .precedence(4_096),
            updated: Members::empty().description("").precedence(0),
        },
        UpdateCase {
            case: "only the role ARN rewritten, at the length maximum of a plain IAM role ARN",
            group_name: "role-only".to_string(),
            created: Members::empty()
                .description("role swap")
                .role_arn("arn:aws:iam::000000000000:role/old")
                .precedence(2),
            updated: Members::empty().role_arn(
                format!("arn:aws:iam::000000000000:role/{}", "r".repeat(64)).as_str(),
            ),
        },
    ]
}

#[tokio::test]
async fn an_update_rewrites_the_members_it_carries_and_round_trips_through_stored_state() {
    let router = router();
    let user_pool_id = pool(&router, "update-group-pool").await;

    for case in update_cases() {
        let label = case.case;
        let created = ok(
            &router,
            "CreateGroup",
            &case.created.request(&user_pool_id, &case.group_name),
        )
        .await;
        let creation_date = created["Group"]["CreationDate"].clone();
        let expected = case.created.merged(&case.updated);

        let updated = ok(
            &router,
            "UpdateGroup",
            &case.updated.request(&user_pool_id, &case.group_name),
        )
        .await;
        let group = &updated["Group"];

        expected.assert_rendered(label, "UpdateGroup", group);
        assert_eq!(
            group["GroupName"],
            json!(case.group_name),
            "case {label:?}: an update must not change the group's name, got {group}"
        );
        assert_eq!(
            group["UserPoolId"],
            json!(user_pool_id),
            "case {label:?}: an update must not change the group's pool, got {group}"
        );
        assert_eq!(
            group["CreationDate"], creation_date,
            "case {label:?}: an update must not change the creation instant, got {group}"
        );
        assert!(
            group["LastModifiedDate"].is_number()
                && group["LastModifiedDate"].as_f64() >= creation_date.as_f64(),
            "case {label:?}: the last-modified instant must be at or after creation, got {group}"
        );

        // The round trip: the reader renders the STORED group, member for member.
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            read["Group"], updated["Group"],
            "case {label:?}: GetGroup must render what UpdateGroup stored, got {read}"
        );
        case.created
            .merged(&case.updated)
            .assert_rendered(label, "GetGroup", &read["Group"]);

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
async fn a_chain_of_updates_accumulates_into_the_stored_group() {
    let router = router();
    let user_pool_id = pool(&router, "update-group-sequence-pool").await;
    ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;

    // A chain of updates over the same group, each setting a different subset: after
    // each one the stored group holds that update's members over everything the
    // earlier ones set, and an update is idempotent when it repeats itself.
    let chain = [
        Members::empty().precedence(1),
        Members::empty()
            .description("described")
            .role_arn("arn:aws:iam::000000000000:role/a"),
        Members::empty().precedence(0),
        Members::empty().precedence(0),
        Members::empty()
            .description("final")
            .role_arn("arn:aws:iam::000000000000:role/b")
            .precedence(42),
        Members::empty(),
    ];
    let mut expected = Members::empty();
    for (index, members) in chain.iter().enumerate() {
        let label = format!("update {index}");
        expected = expected.merged(members);
        let updated = ok(
            &router,
            "UpdateGroup",
            &members.request(&user_pool_id, "admins"),
        )
        .await;
        expected.assert_rendered(&label, "UpdateGroup", &updated["Group"]);

        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
        )
        .await;
        assert_eq!(
            read["Group"], updated["Group"],
            "case {label:?}: GetGroup must render what the latest update stored, got {read}"
        );
    }
}

#[tokio::test]
async fn an_update_touches_only_the_group_it_names() {
    let router = router();
    let first_pool = pool(&router, "update-first-pool").await;
    let second_pool = pool(&router, "update-second-pool").await;

    // The same group name in two pools, plus a sibling group in the first pool.
    for (user_pool_id, group_name) in [
        (&first_pool, "admins"),
        (&first_pool, "readers"),
        (&second_pool, "admins"),
    ] {
        ok(
            &router,
            "CreateGroup",
            &json!({
                "UserPoolId": user_pool_id,
                "GroupName": group_name,
                "Description": "untouched",
                "Precedence": 7,
            }),
        )
        .await;
    }

    ok(
        &router,
        "UpdateGroup",
        &json!({
            "UserPoolId": first_pool,
            "GroupName": "admins",
            "Description": "updated",
            "Precedence": 1,
        }),
    )
    .await;

    for (label, user_pool_id, group_name, description, precedence) in [
        (
            "the updated group",
            &first_pool,
            "admins",
            "updated",
            1_i32,
        ),
        (
            "a sibling group in the same pool",
            &first_pool,
            "readers",
            "untouched",
            7,
        ),
        (
            "the same name in another pool",
            &second_pool,
            "admins",
            "untouched",
            7,
        ),
    ] {
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;
        assert_eq!(
            (
                read["Group"]["Description"].clone(),
                read["Group"]["Precedence"].clone()
            ),
            (json!(description), json!(precedence)),
            "case {label:?}: an update must reach only the group it names, got {read}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_or_group_is_a_404_and_stores_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "update-notfound-pool").await;
    let created = ok(
        &router,
        "CreateGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins", "Precedence": 3 }),
    )
    .await;

    for (label, body) in [
        (
            "UpdateGroup on an absent pool",
            json!({ "UserPoolId": "us-east-1_absent", "GroupName": "admins", "Precedence": 1 }),
        ),
        (
            "UpdateGroup for a name the pool does not hold",
            json!({ "UserPoolId": user_pool_id, "GroupName": "absent-group", "Precedence": 1 }),
        ),
        (
            "UpdateGroup for a name differing only in case",
            json!({ "UserPoolId": user_pool_id, "GroupName": "Admins", "Precedence": 1 }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateGroup", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {payload}"
        );
    }

    // None of the refused updates created or changed anything.
    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"], created["Group"],
        "a refused update must leave the stored group exactly as it was, got {read}"
    );
    let (status, payload) = call(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "absent-group" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (404, "ResourceNotFoundException"),
        "a refused update stores no group, got {payload}"
    );
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "update-constraint-pool").await;
    let created = ok(
        &router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": "admins",
            "Description": "survives every refusal",
            "Precedence": 3,
        }),
    )
    .await;

    // One value past each published bound of `UpdateGroup`'s input
    // (`smithy_query(query="operation",operation="UpdateGroup")`): the group name's
    // length 1..128 and pattern, the description's length 0..2048, the precedence's
    // range 0.., the role ARN's length 20..2048 and its pattern, and the user pool
    // id's pattern.
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
                "GroupName": "admins",
                "Description": "d".repeat(2049),
            }),
        ),
        (
            "a precedence of -1, below the range minimum 0",
            json!({ "UserPoolId": user_pool_id, "GroupName": "admins", "Precedence": -1 }),
        ),
        (
            "a role ARN of 19 characters, below the length minimum 20",
            json!({ "UserPoolId": user_pool_id, "GroupName": "admins", "RoleArn": "arn:aws:iam::0:r/x" }),
        ),
        (
            "a role ARN the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "GroupName": "admins", "RoleArn": "not an arn at all!!" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "GroupName": "admins" }),
        ),
    ] {
        let (status, payload) = call(&router, "UpdateGroup", &body).await;
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

    // Every refusal left the stored group alone.
    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"], created["Group"],
        "a refused update must not have touched the stored group, got {read}"
    );
}
