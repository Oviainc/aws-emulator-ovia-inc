// SPDX-License-Identifier: CC0-1.0

//! The tagging operations of a user pool over the wire — `TagResource`,
//! `UntagResource` and `ListTagsForResource` — driven through the router exactly as
//! an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a user pool holds ONE tag set, a map of keys to values, and
//! the resource a tagging call addresses is the pool its `ResourceArn` names — the
//! `UserPoolId` inside it, in the Region it names, so a secondary replica carries
//! its own set. `TagResource` MERGES the keys it carries into that set (the call's
//! value wins for a key already there, every other key is left as it was),
//! `UntagResource` removes exactly the keys it names, and every reader renders the
//! STORED set, empty for a pool nothing has tagged. An ARN that is not a user-pool
//! ARN of this service names no resource (HTTP 400 `InvalidParameterException`), and
//! one naming a pool this service does not hold in that Region names a missing
//! resource (HTTP 400 `ResourceNotFoundException`); neither writes anything.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state::MAX_TAGS_PER_USER_POOL;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service, and its
/// Region is the one a created pool is PRIMARY in. The signature is never checked
/// here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The three operations that address a user pool by its ARN alone.
const TAGGING_OPERATIONS: [&str; 3] = ["TagResource", "UntagResource", "ListTagsForResource"];

/// One pool's tag set, ordered by key so two reads compare regardless of the order
/// the protocol served them in.
type TagSet = BTreeMap<String, String>;

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

/// The `message` member of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool named `name` and return its minted `UserPoolId` and ARN.
///
/// The pool is encrypted under a customer-managed key, which is what a pool has
/// to hold to be replicated at all: "you must use a multi-region key to enable
/// multi-region replication for a user pool"
/// (`smithy_query(query="doc",shape="KeyConfigurationType",member="KeyType")`).
async fn pool(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": name,
            "KeyConfiguration": {
                "KeyType": "CUSTOMER_MANAGED_KEY",
                "KmsKeyArn": "arn:aws:kms:us-east-1:123456789012:key/multi-region-example",
            },
        }),
    )
    .await;
    let id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id, got {created}"))
        .to_string();
    let arn = created["UserPool"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool ARN, got {created}"))
        .to_string();
    (id, arn)
}

/// `pool_arn` with its Region field replaced by `region`: the ARN a replica of that
/// pool carries, since a replicated pool keeps ONE id across the Regions it is
/// replicated to.
fn arn_in_region(pool_arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = pool_arn.split(':').collect();
    assert!(
        fields.len() > 3,
        "a pool ARN has a Region field, got {pool_arn:?}"
    );
    fields[3] = region;
    fields.join(":")
}

/// `pool_arn` with the pool it names replaced by `user_pool_id`, so a case can name
/// a pool that was never created in the ARN form a real one carries.
fn arn_of_pool_id(pool_arn: &str, user_pool_id: &str) -> String {
    let (prefix, _) = pool_arn
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("a pool ARN names its pool after a slash, got {pool_arn:?}"));
    format!("{prefix}/{user_pool_id}")
}

/// The tag set `ListTagsForResource` reports for `resource_arn`, as a map ordered by
/// key so two reads compare regardless of the order the protocol served them in.
async fn listed_tags(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> TagSet {
    let payload = ok(
        router,
        "ListTagsForResource",
        &json!({ "ResourceArn": resource_arn }),
    )
    .await;
    let tags = payload
        .get("Tags")
        .unwrap_or_else(|| panic!("case {case:?}: the read must carry Tags, got {payload}"));
    as_tag_map(case, tags)
}

/// The `UserPoolTags` `DescribeUserPool` reports for `user_pool_id`: the SAME stored
/// set, read back through the pool's own describe.
async fn described_tags(router: &MultiServiceRouter, case: &str, user_pool_id: &str) -> TagSet {
    let payload = ok(
        router,
        "DescribeUserPool",
        &json!({ "UserPoolId": user_pool_id }),
    )
    .await;
    let tags = payload["UserPool"].get("UserPoolTags").unwrap_or_else(|| {
        panic!("case {case:?}: the describe must carry UserPoolTags, got {payload}")
    });
    as_tag_map(case, tags)
}

/// A JSON tag map as an ordered Rust map.
fn as_tag_map(case: &str, tags: &Value) -> TagSet {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: a tag set is a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a tag value is a string, got {value}"));
            (key.clone(), value.to_string())
        })
        .collect()
}

/// The same tag map as a request body member.
fn tags_body(tags: &TagSet) -> Value {
    let mut body = Map::new();
    for (key, value) in tags {
        body.insert(key.clone(), json!(value));
    }
    Value::Object(body)
}

/// A tag map from its entries.
fn tag_map(entries: &[(&str, &str)]) -> TagSet {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn an_assigned_tag_set_is_what_every_reader_renders() {
    let router = router();

    // The rows differ along what the rule generalises over: how many entries the
    // set carries, and the length and characters of each key and value. The bounds
    // are the modelled ones — `TagKeysType` is 1..128 and `TagValueType` 0..256
    // (`smithy_query(query="shape",shape="UserPoolTagsType")`) — so a row sits at
    // each published bound.
    let rows: Vec<(&str, TagSet)> = vec![
        ("one tag", tag_map(&[("Environment", "Test")])),
        (
            "several tags at once",
            tag_map(&[
                ("Environment", "Production"),
                ("Owner", "platform"),
                ("cost-center", "4815"),
            ]),
        ),
        ("an empty value", tag_map(&[("Unset", "")])),
        (
            "a key and a value at their longest",
            BTreeMap::from([("k".repeat(128), "v".repeat(256))]),
        ),
        ("a key and a value of one character", tag_map(&[("k", "v")])),
        (
            "keys and values carrying the punctuation a tag admits",
            tag_map(&[("team:owner", "a.b-c_d/e+f=g@h"), ("spaced key", "two words")]),
        ),
    ];

    for (case, tags) in rows {
        // A pool per row, so each row's set is the only thing the pool holds.
        let (user_pool_id, pool_arn) = pool(&router, "tagged-pool").await;

        ok(
            &router,
            "TagResource",
            &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&tags) }),
        )
        .await;

        let listed = listed_tags(&router, case, &pool_arn).await;
        assert_eq!(
            listed, tags,
            "case {case:?}: the listing must render the stored set, got {listed:?}"
        );

        // The same stored set, read back through the pool's own describe: one tag
        // set per pool, not one per reader.
        let described = described_tags(&router, case, &user_pool_id).await;
        assert_eq!(
            described, tags,
            "case {case:?}: DescribeUserPool must report the stored set, got {described:?}"
        );

        // Listing is a read: a second one answers the same set.
        let again = listed_tags(&router, case, &pool_arn).await;
        assert_eq!(
            again, listed,
            "case {case:?}: reading twice must answer the same set, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_pool_nothing_has_tagged_reports_an_empty_set() {
    let router = router();
    let case = "a freshly created pool";
    let (user_pool_id, pool_arn) = pool(&router, "untagged-pool").await;

    let listed = listed_tags(&router, case, &pool_arn).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a pool nothing has tagged is assigned no tags, got {listed:?}"
    );
    let described = described_tags(&router, case, &user_pool_id).await;
    assert!(
        described.is_empty(),
        "case {case:?}: the describe must agree it holds none, got {described:?}"
    );
}

#[tokio::test]
async fn tagging_merges_into_the_set_the_pool_already_holds() {
    let router = router();
    let (user_pool_id, pool_arn) = pool(&router, "merging-pool").await;

    // Each row is one `TagResource` call against the SAME pool, with the set the
    // pool must hold afterwards. The rows differ along what a call does to the
    // stored set: add a key, add several, re-assign a key that is already there,
    // and carry no key at all.
    let rows: Vec<(&str, TagSet, TagSet)> = vec![
        (
            "the first assignment",
            tag_map(&[("Environment", "Test")]),
            tag_map(&[("Environment", "Test")]),
        ),
        (
            "two more keys beside it",
            tag_map(&[("Owner", "platform"), ("cost-center", "4815")]),
            tag_map(&[
                ("Environment", "Test"),
                ("Owner", "platform"),
                ("cost-center", "4815"),
            ]),
        ),
        (
            "a new value for a key already there",
            tag_map(&[("Environment", "Production")]),
            tag_map(&[
                ("Environment", "Production"),
                ("Owner", "platform"),
                ("cost-center", "4815"),
            ]),
        ),
        (
            "a value emptied in place",
            tag_map(&[("Owner", "")]),
            tag_map(&[
                ("Environment", "Production"),
                ("Owner", ""),
                ("cost-center", "4815"),
            ]),
        ),
        (
            "an assignment carrying no key",
            BTreeMap::new(),
            tag_map(&[
                ("Environment", "Production"),
                ("Owner", ""),
                ("cost-center", "4815"),
            ]),
        ),
    ];

    for (case, assigned, expected) in rows {
        ok(
            &router,
            "TagResource",
            &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&assigned) }),
        )
        .await;

        let listed = listed_tags(&router, case, &pool_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the assignment must merge into the stored set, got {listed:?}"
        );
        let described = described_tags(&router, case, &user_pool_id).await;
        assert_eq!(
            described, expected,
            "case {case:?}: the describe must report the merged set, got {described:?}"
        );
    }
}

#[tokio::test]
async fn untagging_removes_exactly_the_keys_it_names() {
    let router = router();

    // The set every row starts from.
    let stored = tag_map(&[
        ("Environment", "Production"),
        ("Owner", "platform"),
        ("cost-center", "4815"),
        ("Unset", ""),
    ]);

    // The rows differ along which keys a removal names: one, several, a key the
    // pool does not hold, a repeat of a removal already made, one whose value is
    // empty, and every key at once.
    let rows: Vec<(&str, Vec<&str>, TagSet)> = vec![
        (
            "one key",
            vec!["Owner"],
            tag_map(&[
                ("Environment", "Production"),
                ("cost-center", "4815"),
                ("Unset", ""),
            ]),
        ),
        (
            "two keys at once",
            vec!["Environment", "cost-center"],
            tag_map(&[("Owner", "platform"), ("Unset", "")]),
        ),
        (
            "a key the pool does not hold",
            vec!["never-assigned"],
            stored.clone(),
        ),
        (
            "a key the pool holds beside one it does not",
            vec!["Owner", "never-assigned"],
            tag_map(&[
                ("Environment", "Production"),
                ("cost-center", "4815"),
                ("Unset", ""),
            ]),
        ),
        (
            "a key whose value is empty",
            vec!["Unset"],
            tag_map(&[
                ("Environment", "Production"),
                ("Owner", "platform"),
                ("cost-center", "4815"),
            ]),
        ),
        (
            "every key at once",
            vec!["Environment", "Owner", "cost-center", "Unset"],
            BTreeMap::new(),
        ),
        ("no key at all", Vec::new(), stored.clone()),
    ];

    for (case, tag_keys, expected) in rows {
        // A pool per row, carrying the same starting set.
        let (user_pool_id, pool_arn) = pool(&router, "untagged-keys-pool").await;
        ok(
            &router,
            "TagResource",
            &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&stored) }),
        )
        .await;

        ok(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": pool_arn, "TagKeys": tag_keys }),
        )
        .await;

        let listed = listed_tags(&router, case, &pool_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the removal must leave every other key as it was, got {listed:?}"
        );
        let described = described_tags(&router, case, &user_pool_id).await;
        assert_eq!(
            described, expected,
            "case {case:?}: the describe must report what is left, got {described:?}"
        );

        // Removing the same keys again removes nothing more: a key that is gone is
        // nothing to remove.
        ok(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": pool_arn, "TagKeys": tag_keys }),
        )
        .await;
        let repeated = listed_tags(&router, case, &pool_arn).await;
        assert_eq!(
            repeated, expected,
            "case {case:?}: a repeated removal must change nothing, got {repeated:?}"
        );
    }
}

#[tokio::test]
async fn each_pool_and_each_replica_holds_its_own_tag_set() {
    let router = router();

    // Two pools, so a call against one is not a call against the other.
    let (_, first_arn) = pool(&router, "first-tagged-pool").await;
    let (second_id, second_arn) = pool(&router, "second-tagged-pool").await;

    // A secondary replica of the second pool, created with its own tags: "You can
    // maintain tags independently on replica user pools"
    // (`CreateUserPoolReplica.UserPoolTags`).
    let replica_region = "us-west-2";
    let replica_tags = tag_map(&[("Environment", "Replica"), ("Owner", "replication")]);
    ok(
        &router,
        "CreateUserPoolReplica",
        &json!({
            "UserPoolId": second_id,
            "RegionName": replica_region,
            "UserPoolTags": tags_body(&replica_tags),
        }),
    )
    .await;
    let replica_arn = arn_in_region(&second_arn, replica_region);

    // The tags the create submitted are the replica's own, read back through the
    // ARN that names it.
    let case = "the replica the create tagged";
    let listed = listed_tags(&router, case, &replica_arn).await;
    assert_eq!(
        listed, replica_tags,
        "case {case:?}: the replica must report the set its create stored, got {listed:?}"
    );

    // Tagging the PRIMARY replica of the second pool leaves the secondary's set and
    // the other pool's alone.
    let primary_tags = tag_map(&[("Environment", "Primary")]);
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": second_arn, "Tags": tags_body(&primary_tags) }),
    )
    .await;

    let case = "the secondary replica after the primary is tagged";
    let listed = listed_tags(&router, case, &replica_arn).await;
    assert_eq!(
        listed, replica_tags,
        "case {case:?}: a replica keeps its own set, got {listed:?}"
    );
    let case = "the other pool after the second is tagged";
    let listed = listed_tags(&router, case, &first_arn).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: tagging one pool must not tag another, got {listed:?}"
    );

    // And a removal on the secondary replica leaves the primary's set alone.
    ok(
        &router,
        "UntagResource",
        &json!({ "ResourceArn": replica_arn, "TagKeys": ["Owner"] }),
    )
    .await;
    let case = "the secondary replica after its own removal";
    let listed = listed_tags(&router, case, &replica_arn).await;
    assert_eq!(
        listed,
        tag_map(&[("Environment", "Replica")]),
        "case {case:?}: the removal must apply to the replica it names, got {listed:?}"
    );
    let case = "the primary replica after the secondary's removal";
    let listed = listed_tags(&router, case, &second_arn).await;
    assert_eq!(
        listed, primary_tags,
        "case {case:?}: the primary keeps its own set, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_user_pool_of_this_service_is_refused() {
    let router = router();
    let (_, pool_arn) = pool(&router, "untouched-pool").await;
    let stored = tag_map(&[("Environment", "Production")]);
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&stored) }),
    )
    .await;

    // Each row is an ARN that satisfies `ArnType`'s modelled length and pattern —
    // so it reaches the handler — and names no user pool of this service: another
    // service's namespace, another resource type of this one, no resource id, and
    // fields no ARN of a user pool carries.
    let rows: Vec<(&str, String)> = vec![
        (
            "another AWS service's namespace",
            "arn:aws:s3:us-east-1:123456789012:bucket/user-pool-logs".to_string(),
        ),
        (
            "the sibling Amazon Cognito service",
            "arn:aws:cognito-identity:us-east-1:123456789012:identitypool/us-east-1:a-b-c"
                .to_string(),
        ),
        (
            "another resource type of this service",
            "arn:aws:cognito-idp:us-east-1:123456789012:group/engineering".to_string(),
        ),
        (
            "the resource type with no pool id after it",
            "arn:aws:cognito-idp:us-east-1:123456789012:userpool".to_string(),
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:cognito-idp:us-east-1:1234:userpool/us-east-1_abcdefghi".to_string(),
        ),
        (
            "a resource id no UserPoolId could be",
            "arn:aws:cognito-idp:us-east-1:123456789012:userpool/notapoolid".to_string(),
        ),
    ];

    for (case, resource_arn) in rows {
        for operation in TAGGING_OPERATIONS {
            let case = format!("{operation} against {case}");
            let request = request_for(operation, &resource_arn);
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "InvalidParameterException"),
                "case {case:?}: expected a 400 InvalidParameterException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            let expected = format!(
                "ResourceArn '{resource_arn}' is not the ARN of an Amazon Cognito user pool."
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // Every refusal above wrote nothing: the pool still holds the set it held.
    let case = "the pool every refused call named nothing of";
    let listed = listed_tags(&router, case, &pool_arn).await;
    assert_eq!(
        listed, stored,
        "case {case:?}: a refused call must leave the stored set unchanged, got {listed:?}"
    );
}

#[tokio::test]
async fn an_arn_naming_a_pool_this_service_does_not_hold_is_a_400() {
    let router = router();
    let (_, pool_arn) = pool(&router, "present-pool").await;

    // Each row is a well-formed user-pool ARN of this service naming a pool it does
    // not hold THERE: ids no create ever minted, and the ARN of a pool that exists
    // in a Region it has no replica in.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a pool id no create minted",
            arn_of_pool_id(&pool_arn, "us-east-1_absentpool"),
            "us-east-1_absentpool".to_string(),
        ),
        (
            "a pool id of another Region",
            arn_of_pool_id(&pool_arn, "eu-west-2_0000000000"),
            "eu-west-2_0000000000".to_string(),
        ),
        (
            "a stored pool in a Region it holds no replica in",
            arn_in_region(&pool_arn, "ap-south-1"),
            pool_arn
                .rsplit_once('/')
                .map(|(_, id)| id.to_string())
                .expect("a pool ARN names its pool after a slash"),
        ),
    ];

    for (case, resource_arn, user_pool_id) in rows {
        for operation in TAGGING_OPERATIONS {
            let case = format!("{operation} against {case}");
            let request = request_for(operation, &resource_arn);
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected a 400 ResourceNotFoundException, got {status} \
                 with {payload}"
            );
            let message = error_message(&payload);
            let expected = format!("User pool {user_pool_id} does not exist.");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The pool that does exist is untouched by every call above.
    let case = "the stored pool none of those calls addressed";
    let listed = listed_tags(&router, case, &pool_arn).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: a refused call must write nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_pool_holds_no_more_tags_than_the_documented_limit() {
    let router = router();
    let (_, pool_arn) = pool(&router, "capped-pool").await;

    // Exactly the limit is accepted: "A user pool can have as many as 50 tags"
    // (`TagResource` in this service's model).
    let at_the_limit: TagSet = (0..MAX_TAGS_PER_USER_POOL)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect();
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&at_the_limit) }),
    )
    .await;
    let case = "a pool tagged up to the limit";
    let listed = listed_tags(&router, case, &pool_arn).await;
    assert_eq!(
        listed, at_the_limit,
        "case {case:?}: the limit itself is accepted, got {} entries",
        listed.len()
    );

    // Re-assigning keys the pool already holds never takes it past the limit,
    // however many the call carries: the ceiling is on the RESULT.
    let case = "re-assigning every key the pool holds";
    let reassigned: TagSet = at_the_limit
        .keys()
        .map(|key| (key.clone(), "reassigned".to_string()))
        .collect();
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceArn": pool_arn, "Tags": tags_body(&reassigned) }),
    )
    .await;
    let listed = listed_tags(&router, case, &pool_arn).await;
    assert_eq!(
        listed, reassigned,
        "case {case:?}: every value must be the one just assigned, got {listed:?}"
    );

    // One key past the limit is refused, and the refusal writes nothing.
    let case = "one key past the limit";
    let (status, payload) = call(
        &router,
        "TagResource",
        &json!({
            "ResourceArn": pool_arn,
            "Tags": tags_body(&tag_map(&[("one-too-many", "value")])),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
    );
    let message = error_message(&payload);
    let expected = format!("A user pool can have as many as {MAX_TAGS_PER_USER_POOL} tags.");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    let listed = listed_tags(&router, case, &pool_arn).await;
    assert_eq!(
        listed, reassigned,
        "case {case:?}: a refused assignment must leave the stored set unchanged, got {listed:?}"
    );
}

/// The request `operation` takes against `resource_arn`: each of the three carries
/// the ARN, and the two that write carry the keys they write.
fn request_for(operation: &str, resource_arn: &str) -> Value {
    let mut request = Map::new();
    request.insert("ResourceArn".to_string(), json!(resource_arn));
    match operation {
        "TagResource" => {
            request.insert("Tags".to_string(), json!({ "Environment": "Test" }));
        }
        "UntagResource" => {
            request.insert("TagKeys".to_string(), json!(["Environment"]));
        }
        _ => {}
    }
    Value::Object(request)
}
