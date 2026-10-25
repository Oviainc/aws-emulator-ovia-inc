// SPDX-License-Identifier: CC0-1.0

//! `AddCustomAttributes` over the wire, driven through the router exactly as an
//! SDK drives it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.<Op>`), and read back through
//! `DescribeUserPool`.
//!
//! The rule under test: an add APPENDS each requested attribute to the named
//! pool's schema, under the prefix Amazon Cognito gives it — "When you add an
//! attribute with a `Name` value of `MyAttribute`, Amazon Cognito creates the
//! custom attribute `custom:MyAttribute`. When `DeveloperOnlyAttribute` is
//! `true`, Amazon Cognito creates your attribute as `dev:MyAttribute`"
//! (`SchemaAttributeType.Name`, this service's model) — and never changes or
//! removes an entry the schema already holds, so a request that names one is
//! refused WHOLE and the schema only ever grows, in the order attributes were
//! added.
//!
//! Each table row names the case it asserts and every assertion reports the
//! actual value.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service; the
/// signature itself is never checked here.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The 20 standard attributes every pool's schema starts with, in the order
/// `CreateUserPool` stores them: the schema an add appends BEHIND.
const STANDARD_ATTRIBUTE_COUNT: usize = 20;

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

/// Create a pool named `name` and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateUserPool must mint an id, got {created}"))
        .to_string()
}

/// The pool `user_pool_id` names, as `DescribeUserPool` renders it.
async fn described_pool(router: &MultiServiceRouter, user_pool_id: &str) -> Value {
    ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await["UserPool"]
        .clone()
}

/// The pool's `SchemaAttributes`, in stored order.
async fn schema(router: &MultiServiceRouter, user_pool_id: &str) -> Vec<Value> {
    let described = described_pool(router, user_pool_id).await;
    described["SchemaAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("a described pool must carry SchemaAttributes, got {described}"))
        .clone()
}

/// The `Name` of every entry of `entries`, in order.
fn names(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["Name"]
                .as_str()
                .unwrap_or("<no Name>")
                .to_string()
        })
        .collect()
}

/// Assert `entry` renders every member `expected` names. `expected` lists only
/// the members the case asserts; anything else the entry carries is free.
fn assert_entry(case: &str, entry: &Value, expected: &Value) {
    let members: &Map<String, Value> = expected
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: an expectation is an object, got {expected}"));
    for (member, value) in members {
        assert_eq!(
            entry.get(member),
            Some(value),
            "case {case:?}: {member} of the stored attribute must be {value}, got {entry}"
        );
    }
}

/// One `AddCustomAttributes` row: the `CustomAttributes` the request carries and
/// the schema entries the pool must hold afterwards, in order. Rows differ along
/// what the rule generalises over — how many attributes one request carries,
/// their data type, mutability, developer-only flag, constraints and the FORM of
/// each name.
struct AddCase {
    case: &'static str,
    requested: Vec<Value>,
    expected: Vec<Value>,
}

/// The rows. The data types are the whole `AttributeDataType` enum (`String |
/// Number | DateTime | Boolean`, from
/// `smithy_query(query="shape",shape="AttributeDataType")`), and the name forms
/// span what `CustomAttributeNameType` admits (length 1..20, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, from
/// `smithy_query(query="shape",shape="CustomAttributeNameType")`): both
/// published length bounds, a non-ASCII name and a punctuation-heavy one.
fn add_cases() -> Vec<AddCase> {
    // The modelled maximum of one request: `CustomAttributes` is "len 1..25"
    // (`smithy_query(query="operation",operation="AddCustomAttributes")`).
    let bulk_requested: Vec<Value> = (0..25)
        .map(|index| json!({ "Name": format!("bulk{index}") }))
        .collect();
    let bulk_expected: Vec<Value> = (0..25)
        .map(|index| {
            json!({
                "Name": format!("custom:bulk{index}"),
                "AttributeDataType": "String",
                "Mutable": true,
                "Required": false,
            })
        })
        .collect();

    vec![
        AddCase {
            case: "one attribute with every property defaulted",
            requested: vec![json!({ "Name": "department" })],
            expected: vec![json!({
                "Name": "custom:department",
                "AttributeDataType": "String",
                "DeveloperOnlyAttribute": false,
                "Mutable": true,
                "Required": false,
            })],
        },
        AddCase {
            case: "one attribute of each modelled data type, in request order",
            requested: vec![
                json!({ "Name": "note", "AttributeDataType": "String" }),
                json!({ "Name": "seats", "AttributeDataType": "Number" }),
                json!({ "Name": "joined", "AttributeDataType": "DateTime" }),
                json!({ "Name": "member", "AttributeDataType": "Boolean" }),
            ],
            expected: vec![
                json!({ "Name": "custom:note", "AttributeDataType": "String" }),
                json!({ "Name": "custom:seats", "AttributeDataType": "Number" }),
                json!({ "Name": "custom:joined", "AttributeDataType": "DateTime" }),
                json!({ "Name": "custom:member", "AttributeDataType": "Boolean" }),
            ],
        },
        AddCase {
            case: "an immutable attribute beside a mutable one",
            requested: vec![
                json!({ "Name": "frozen", "Mutable": false }),
                json!({ "Name": "editable", "Mutable": true }),
            ],
            expected: vec![
                json!({ "Name": "custom:frozen", "Mutable": false }),
                json!({ "Name": "custom:editable", "Mutable": true }),
            ],
        },
        AddCase {
            case: "a developer-only attribute carries the dev: prefix",
            requested: vec![
                json!({ "Name": "internal", "DeveloperOnlyAttribute": true }),
                json!({ "Name": "public", "DeveloperOnlyAttribute": false }),
            ],
            expected: vec![
                json!({
                    "Name": "dev:internal",
                    "DeveloperOnlyAttribute": true,
                    "Required": false,
                }),
                json!({
                    "Name": "custom:public",
                    "DeveloperOnlyAttribute": false,
                    "Required": false,
                }),
            ],
        },
        AddCase {
            case: "the constraints of a Number and of a String attribute are stored as sent",
            requested: vec![
                json!({
                    "Name": "rank",
                    "AttributeDataType": "Number",
                    "NumberAttributeConstraints": { "MinValue": "1", "MaxValue": "100" },
                }),
                json!({
                    "Name": "nickname2",
                    "AttributeDataType": "String",
                    "StringAttributeConstraints": { "MinLength": "2", "MaxLength": "32" },
                }),
            ],
            expected: vec![
                json!({
                    "Name": "custom:rank",
                    "AttributeDataType": "Number",
                    "NumberAttributeConstraints": { "MinValue": "1", "MaxValue": "100" },
                }),
                json!({
                    "Name": "custom:nickname2",
                    "AttributeDataType": "String",
                    "StringAttributeConstraints": { "MinLength": "2", "MaxLength": "32" },
                }),
            ],
        },
        AddCase {
            case: "a custom attribute asked to be required is stored not required",
            requested: vec![json!({ "Name": "wanted", "Required": true })],
            expected: vec![json!({ "Name": "custom:wanted", "Required": false })],
        },
        AddCase {
            case: "the name forms the model admits",
            requested: vec![
                json!({ "Name": "a".repeat(20) }),
                json!({ "Name": "z" }),
                json!({ "Name": "属性①" }),
                json!({ "Name": "team.cost-centre" }),
            ],
            expected: vec![
                json!({ "Name": format!("custom:{}", "a".repeat(20)) }),
                json!({ "Name": "custom:z" }),
                json!({ "Name": "custom:属性①" }),
                json!({ "Name": "custom:team.cost-centre" }),
            ],
        },
        AddCase {
            case: "a name that already carries the prefix is stored as sent",
            requested: vec![
                json!({ "Name": "custom:tier" }),
                json!({ "Name": "dev:flag", "DeveloperOnlyAttribute": true }),
            ],
            expected: vec![
                json!({ "Name": "custom:tier" }),
                json!({ "Name": "dev:flag" }),
            ],
        },
        AddCase {
            case: "a name that matches a standard attribute becomes a distinct custom one",
            requested: vec![json!({ "Name": "email" })],
            expected: vec![json!({
                "Name": "custom:email",
                "AttributeDataType": "String",
                "Mutable": true,
                "Required": false,
            })],
        },
        AddCase {
            case: "the 25-attribute request maximum",
            requested: bulk_requested,
            expected: bulk_expected,
        },
    ]
}

#[tokio::test]
async fn an_add_appends_every_requested_attribute_to_the_pools_schema() {
    let router = router();

    for (index, row) in add_cases().into_iter().enumerate() {
        let case = row.case;
        let user_pool_id = pool(&router, &format!("append-pool-{index}")).await;
        let created = described_pool(&router, &user_pool_id).await;
        let before = schema(&router, &user_pool_id).await;
        assert_eq!(
            before.len(),
            STANDARD_ATTRIBUTE_COUNT,
            "case {case:?}: a new pool starts with the standard attributes, got {:?}",
            names(&before)
        );

        let answered = ok(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": row.requested }),
        )
        .await;
        assert!(
            answered.is_null() || answered.as_object().is_some_and(Map::is_empty),
            "case {case:?}: AddCustomAttributesResponse models no members, got {answered}"
        );

        let after = schema(&router, &user_pool_id).await;
        // Appended: every entry the schema already held is still there, in place,
        // unchanged, and the new entries follow in request order.
        assert_eq!(
            after.len(),
            before.len() + row.expected.len(),
            "case {case:?}: the schema must grow by the attributes added, got {:?}",
            names(&after)
        );
        assert_eq!(
            &after[..before.len()],
            &before[..],
            "case {case:?}: the entries the schema already held must be untouched, got {:?}",
            names(&after)
        );
        for (position, expected) in row.expected.iter().enumerate() {
            assert_entry(case, &after[before.len() + position], expected);
        }

        // The pool's identity survives; only its last-modified instant moved.
        for member in ["Id", "Arn", "CreationDate", "Name"] {
            assert_eq!(
                after_member(&described_pool(&router, &user_pool_id).await, member),
                after_member(&created, member),
                "case {case:?}: {member} must survive an add"
            );
        }
    }
}

/// `pool`'s `member`, as rendered.
fn after_member(pool: &Value, member: &str) -> Value {
    pool.get(member).cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn adds_accumulate_in_the_order_the_attributes_were_added() {
    let router = router();
    let user_pool_id = pool(&router, "accumulating-pool").await;
    let before = schema(&router, &user_pool_id).await;

    // Three separate calls, the second and third carrying more than one
    // attribute: the schema reads in addition order across calls, not just
    // within one.
    let batches = [
        vec!["first"],
        vec!["second", "third"],
        vec!["fourth", "fifth", "sixth"],
    ];
    let mut expected: Vec<String> = names(&before);
    for batch in &batches {
        let requested: Vec<Value> = batch
            .iter()
            .map(|name| json!({ "Name": name }))
            .collect();
        ok(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": requested }),
        )
        .await;
        expected.extend(batch.iter().map(|name| format!("custom:{name}")));

        let after = names(&schema(&router, &user_pool_id).await);
        assert_eq!(
            after, expected,
            "case {batch:?}: the schema must read in addition order, got {after:?}"
        );
    }
}

#[tokio::test]
async fn an_add_advances_the_pools_last_modified_instant() {
    let router = router();
    let user_pool_id = pool(&router, "last-modified-pool").await;
    let case = "an add advances last modified";

    let before = described_pool(&router, &user_pool_id).await["LastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a pool must carry a LastModifiedDate"));
    tokio::time::sleep(Duration::from_millis(5)).await;
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [{ "Name": "moved" }],
        }),
    )
    .await;
    let after_add = described_pool(&router, &user_pool_id).await;
    let after = after_add["LastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a pool must carry a LastModifiedDate"));
    assert!(
        after > before,
        "case {case:?}: an add must advance LastModifiedDate past {before}, got {after}"
    );

    // A read of the schema does not move it.
    let read_again = described_pool(&router, &user_pool_id).await["LastModifiedDate"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a pool must carry a LastModifiedDate"));
    assert_eq!(
        read_again, after,
        "case {case:?}: DescribeUserPool must not move LastModifiedDate, got {read_again}"
    );
}

#[tokio::test]
async fn an_attribute_the_schema_already_holds_is_refused_and_nothing_is_added() {
    let router = router();
    let user_pool_id = pool(&router, "append-only-pool").await;

    // The schema this pool starts the test with: four custom attributes and one
    // developer-only one.
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [
                { "Name": "department", "AttributeDataType": "String", "Mutable": true },
                { "Name": "seats", "AttributeDataType": "Number" },
                { "Name": "属性①" },
                { "Name": "internal", "DeveloperOnlyAttribute": true },
            ],
        }),
    )
    .await;
    let established = schema(&router, &user_pool_id).await;

    // Each row is a request that names a taken name, differing along HOW it is
    // taken and what it tries to change about it.
    let refused = [
        (
            "the same attribute again, unchanged",
            json!([{ "Name": "department" }]),
        ),
        (
            "the same attribute with another data type",
            json!([{ "Name": "seats", "AttributeDataType": "String" }]),
        ),
        (
            "the same attribute asked to become immutable",
            json!([{ "Name": "department", "Mutable": false }]),
        ),
        (
            "the stored prefixed name sent as it is stored",
            json!([{ "Name": "custom:department" }]),
        ),
        (
            "a taken developer-only name",
            json!([{ "Name": "internal", "DeveloperOnlyAttribute": true }]),
        ),
        (
            "a taken non-ASCII name",
            json!([{ "Name": "属性①" }]),
        ),
        (
            "a batch whose LAST entry is taken, so the fresh ones are not added either",
            json!([{ "Name": "fresh1" }, { "Name": "fresh2" }, { "Name": "seats" }]),
        ),
        (
            "a batch that names the same fresh attribute twice",
            json!([{ "Name": "twice" }, { "Name": "twice", "AttributeDataType": "Number" }]),
        ),
        (
            "a standard attribute re-added under its stored custom name",
            json!([{ "Name": "custom:email" }, { "Name": "custom:email" }]),
        ),
    ];

    for (case, custom_attributes) in refused {
        let (status, payload) = call(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": custom_attributes }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let after = schema(&router, &user_pool_id).await;
        assert_eq!(
            after, established,
            "case {case:?}: a refused add must change nothing, got {:?}",
            names(&after)
        );
    }

    // The schema still GROWS by a request that names only free names, so the
    // refusals above are about the names and not about a frozen schema.
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [{ "Name": "fresh1" }, { "Name": "fresh2" }],
        }),
    )
    .await;
    let grown = names(&schema(&router, &user_pool_id).await);
    assert_eq!(
        grown.len(),
        established.len() + 2,
        "an add of free names must still grow the schema, got {grown:?}"
    );
    assert_eq!(
        &grown[grown.len() - 2..],
        ["custom:fresh1".to_string(), "custom:fresh2".to_string()],
        "the free names must be appended last, got {grown:?}"
    );
}

#[tokio::test]
async fn a_request_the_handler_cannot_serve_is_refused_without_touching_the_schema() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    let established = schema(&router, &user_pool_id).await;

    // 26 attributes is one past the modelled `CustomAttributes` maximum of 25,
    // an empty list is one below its minimum of 1, an empty name is below
    // `CustomAttributeNameType`'s minimum length of 1, a 21-character name is one
    // past its maximum of 20, and an attribute with no `Name` at all names
    // nothing to add.
    let too_many: Vec<Value> = (0..26)
        .map(|index| json!({ "Name": format!("over{index}") }))
        .collect();
    let rows: Vec<(&str, Value)> = vec![
        ("26 attributes, one past the request maximum", json!(too_many)),
        ("no attributes at all", json!([])),
        ("an empty name", json!([{ "Name": "" }])),
        (
            "a 21-character name, one past the name maximum",
            json!([{ "Name": "b".repeat(21) }]),
        ),
        ("an attribute with no Name", json!([{ "AttributeDataType": "String" }])),
    ];

    for (case, custom_attributes) in rows {
        let (status, payload) = call(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": custom_attributes }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} with {payload}"
        );
        let after = schema(&router, &user_pool_id).await;
        assert_eq!(
            after, established,
            "case {case:?}: a refused add must change nothing, got {:?}",
            names(&after)
        );
    }
}

#[tokio::test]
async fn an_add_past_the_pools_custom_attribute_quota_is_refused_whole() {
    let router = router();
    let user_pool_id = pool(&router, "quota-pool").await;

    // Amazon Cognito's quota is 50 custom attributes per user pool, reached here
    // in two requests of the modelled 25-attribute maximum.
    for batch in 0..2 {
        let requested: Vec<Value> = (0..25)
            .map(|index| json!({ "Name": format!("q{batch}x{index}") }))
            .collect();
        ok(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": requested }),
        )
        .await;
    }
    let full = schema(&router, &user_pool_id).await;
    assert_eq!(
        full.len(),
        STANDARD_ATTRIBUTE_COUNT + 50,
        "a pool fills up to its custom-attribute quota, got {:?}",
        names(&full)
    );

    // Each row is a request that does not fit in what is left, differing along how
    // far past the quota it reaches.
    let over = [
        ("one attribute past a full pool", 1),
        ("two attributes past a full pool", 2),
        ("a full request past a full pool", 25),
    ];
    for (case, count) in over {
        let requested: Vec<Value> = (0..count)
            .map(|index| json!({ "Name": format!("over{count}x{index}") }))
            .collect();
        let (status, payload) = call(
            &router,
            "AddCustomAttributes",
            &json!({ "UserPoolId": user_pool_id, "CustomAttributes": requested }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        let after = schema(&router, &user_pool_id).await;
        assert_eq!(
            after, full,
            "case {case:?}: a refused add must change nothing, got {:?}",
            names(&after)
        );
    }

    // A pool that is not full still takes the same request, so the refusals above
    // are about the quota and not about the attributes asked for.
    let fresh_id = pool(&router, "quota-room-pool").await;
    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": fresh_id,
            "CustomAttributes": [{ "Name": "over1x0" }],
        }),
    )
    .await;
    assert!(
        names(&schema(&router, &fresh_id).await).contains(&"custom:over1x0".to_string()),
        "a pool with room still takes the attribute"
    );
}

#[tokio::test]
async fn an_add_to_an_absent_pool_is_a_404_and_touches_no_other_pool() {
    let router = router();
    // A live pool, so each refusal is about the id and not about an empty store.
    let live_id = pool(&router, "live-pool").await;
    let other_id = pool(&router, "other-pool").await;
    ok(
        &router,
        "AddCustomAttributes",
        &json!({ "UserPoolId": live_id, "CustomAttributes": [{ "Name": "only-here" }] }),
    )
    .await;
    let live = schema(&router, &live_id).await;
    let other = schema(&router, &other_id).await;

    // Ids no pool was created under, each satisfying the modelled
    // `UserPoolIdType` pattern `^[\w-]+_[0-9a-zA-Z]+$` in a different form.
    let absent_ids = [
        ("another region", "us-west-2_39e6f417e"),
        ("an upper-case suffix", "eu-central-1_ABCDEFGHI"),
        ("a single-character suffix", "us-east-1_0"),
        ("an underscore-only prefix", "pool_abc123"),
    ];
    for (case, user_pool_id) in absent_ids {
        let (status, payload) = call(
            &router,
            "AddCustomAttributes",
            &json!({
                "UserPoolId": user_pool_id,
                "CustomAttributes": [{ "Name": "nowhere" }],
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {status} with {payload}"
        );
    }

    // An add lands in the pool it names and in no other.
    assert_eq!(
        names(&schema(&router, &live_id).await),
        names(&live),
        "the named pool's schema is unchanged by the refused adds"
    );
    assert!(
        !names(&other).contains(&"custom:only-here".to_string()),
        "an add must not reach another pool, got {:?}",
        names(&other)
    );
    assert_eq!(
        other.len(),
        STANDARD_ATTRIBUTE_COUNT,
        "the other pool keeps just the standard attributes, got {:?}",
        names(&other)
    );
}

#[tokio::test]
async fn an_added_attribute_becomes_one_a_user_can_hold_a_value_for() {
    let router = router();
    let user_pool_id = pool(&router, "user-attribute-pool").await;

    // Before the add the pool's schema does not declare the attribute, so a user
    // cannot hold a value for it.
    let (status, payload) = call(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "too-early",
            "UserAttributes": [{ "Name": "custom:department", "Value": "ops" }],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"before the add\": an undeclared attribute is refused, got {status} with {payload}"
    );

    ok(
        &router,
        "AddCustomAttributes",
        &json!({
            "UserPoolId": user_pool_id,
            "CustomAttributes": [
                { "Name": "department" },
                { "Name": "seats", "AttributeDataType": "Number" },
                { "Name": "internal", "DeveloperOnlyAttribute": true },
            ],
        }),
    )
    .await;

    // After it, the same write is served and the value reads back through
    // `AdminGetUser`.
    let written = [
        ("a custom String attribute", "custom:department", "ops"),
        ("a custom Number attribute", "custom:seats", "12"),
        ("a developer-only attribute", "dev:internal", "true"),
    ];
    let attributes: Vec<Value> = written
        .iter()
        .map(|(_, name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    ok(
        &router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "after",
            "UserAttributes": attributes,
        }),
    )
    .await;
    let read = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "after" }),
    )
    .await;
    let stored = read["UserAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminGetUser must render UserAttributes, got {read}"))
        .clone();
    for (case, name, value) in written {
        let found = stored
            .iter()
            .find(|attribute| attribute["Name"].as_str() == Some(name));
        assert_eq!(
            found.map(|attribute| attribute["Value"].clone()),
            Some(json!(value)),
            "case {case:?}: {name} must read back as {value:?}, got {stored:?}"
        );
    }
}
