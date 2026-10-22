// SPDX-License-Identifier: CC0-1.0

//! What Aurora DSQL ANSWERS when a request cannot be served as sent, and what a
//! `clientToken` means when it comes back.
//!
//! Each test states a RULE and asserts it over a table of inputs — several
//! operations, several regions, several constraint classes, several ways for two
//! requests carrying one token to differ — because the rule is the contract and
//! any single recorded request is one row of it.
//!
//! Every case drives the real request path through [`crate::harness`], so the
//! boundary constraint checks, this service's refusal wording and the handlers
//! are all exercised the way a client exercises them.

use serde_json::{json, Value};

use crate::harness::{
    absent_identifier, expected_arn, is_cluster_id, wait_for_the_clock, Dsql,
};

/// The sentence AWS answers a missing resource with. The ARN is the request's
/// own: it is how the caller learns WHICH resource was missing.
fn arn_not_found_message(arn: &str) -> String {
    format!("The resource with ARN {arn} doesn't exist. Verify the ARN and try again.")
}

/// `TagMap`'s modelled maximum entry count. Source: `smithy_query(query="shape",
/// shape="TagMap")` — `@length(min: 0, max: 200)`.
const TAG_MAP_MAX_ENTRIES: usize = 200;

/// The message a token reused for a different request is refused with.
const CLIENT_TOKEN_MISMATCH: &str =
    "Requests using a client token should supply identical request parameters";

/// A `targetDefinition` naming a Kinesis stream and a role in `account`.
fn kinesis_target(account: &str, role: &str) -> Value {
    json!({
        "kinesis": {
            "streamArn": format!("arn:aws:kinesis:us-east-1:{account}:stream/dsql-cdc"),
            "roleArn": format!("arn:aws:iam::{account}:role/{role}"),
        }
    })
}

/// A `CreateStream` body whose target is in this account — the only part of it
/// under test elsewhere is the role.
fn stream_request(account: &str, role: &str) -> Value {
    json!({
        "targetDefinition": kinesis_target(account, role),
        "ordering": "UNORDERED",
        "format": "JSON",
    })
}

/// A tag map with `count` distinct, individually valid entries. Source for the
/// key/value shape: `smithy_query(query="shape", shape="TagMap")` — keys
/// `^[a-zA-Z0-9_.:/=+\-@ ]*$`, length 1..128.
fn tags(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for n in 0..count {
        map.insert(format!("key{n:04}"), Value::String(format!("value{n:04}")));
    }
    Value::Object(map)
}

/// RULE: an operation addressing a resource this service does not hold answers
/// 404 `ResourceNotFoundException` whose message NAMES THE ARN the call
/// addressed — for every cluster-addressed operation, in every region, and for a
/// stream of a cluster that does exist.
#[tokio::test]
async fn a_missing_resource_is_reported_by_the_arn_the_call_addressed() {
    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let dsql = Dsql::in_region(region);
        // A cluster that does exist, so a 404 is about the resource addressed and
        // not about an empty store.
        let live = dsql.create_settled().await;
        let absent = absent_identifier();
        let account = engine::EMULATOR_ACCOUNT_ID;

        let answers = vec![
            ("GetCluster", dsql.get(&absent).await),
            ("UpdateCluster", dsql.update(&absent, json!({})).await),
            ("DeleteCluster", dsql.delete(&absent).await),
            ("GetClusterPolicy", dsql.get_policy(&absent).await),
            ("DeleteClusterPolicy", dsql.delete_policy(&absent, "").await),
            (
                "PutClusterPolicy",
                dsql.put_policy(&absent, json!({"policy": "{}"})).await,
            ),
            (
                "CreateStream",
                dsql.create_stream(&absent, stream_request(account, "dsql-cdc"))
                    .await,
            ),
            (
                "GetStream",
                dsql.get_stream(&absent, &absent_identifier()).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status, 404,
                "case {operation} in {region}: an absent cluster must answer 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {operation} in {region}: the 404 must be a ResourceNotFoundException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.message(),
                arn_not_found_message(&expected_arn(region, &absent)),
                "case {operation} in {region}: the message must name the addressed ARN, got {:?}",
                answer.message()
            );
        }

        // A stream of a cluster that DOES exist is addressed by the stream's own
        // ARN, so that is the ARN the refusal names.
        let absent_stream = "yyyyyyyyyyyyyyyyyyyyyyyyyy";
        let answer = dsql.get_stream(&live, absent_stream).await;
        assert_eq!(
            answer.status, 404,
            "case GetStream of an absent stream in {region}: must answer 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.message(),
            arn_not_found_message(&format!(
                "arn:aws:dsql:{region}:{account}:cluster/{live}/stream/{absent_stream}"
            )),
            "case GetStream of an absent stream in {region}: the message must name the stream ARN, \
             got {:?}",
            answer.message()
        );
    }
}

/// One request refused by a modelled constraint, and the clause DSQL states it
/// with.
struct RefusalCase {
    name: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The value the request carried that failed. It must NOT appear in the
    /// answer: DSQL does not echo caller input back in these messages.
    offending: &'static str,
    clause: &'static str,
}

/// RULE: a value outside a modelled constraint is refused 400
/// `ValidationException` whose message is the AWS envelope built from the member
/// and the constraint it failed — stating the requirement only. It never echoes
/// the rejected value, and it quotes a `@pattern` as modelled minus the `^`/`$`
/// anchors.
#[tokio::test]
async fn a_constraint_violation_names_the_member_and_withholds_the_value() {
    let cases = vec![
        RefusalCase {
            name: "GetCluster identifier with punctuation",
            method: "GET",
            path: "/cluster/not-a-valid-identifier!!".to_string(),
            body: None,
            offending: "not-a-valid-identifier!!",
            clause: "Value at 'identifier' failed to satisfy constraint: \
                     Member must satisfy regular expression pattern: [a-z0-9]{26}",
        },
        RefusalCase {
            name: "DeleteCluster identifier in uppercase",
            method: "DELETE",
            path: "/cluster/TOO-SHORT-AND-UPPERCASE".to_string(),
            body: None,
            offending: "TOO-SHORT-AND-UPPERCASE",
            clause: "Value at 'identifier' failed to satisfy constraint: \
                     Member must satisfy regular expression pattern: [a-z0-9]{26}",
        },
        // Not an operation the issue cited: the same formatter serves every
        // operation, so the same clause comes out of the policy routes.
        RefusalCase {
            name: "PutClusterPolicy identifier too short",
            method: "POST",
            path: "/cluster/abc/policy".to_string(),
            body: Some(json!({"policy": "{}"})),
            offending: "abc",
            clause: "Value at 'identifier' failed to satisfy constraint: \
                     Member must satisfy regular expression pattern: [a-z0-9]{26}",
        },
        RefusalCase {
            name: "ListClusters maxResults below its range",
            method: "GET",
            path: "/cluster?max-results=0".to_string(),
            body: None,
            offending: "0",
            clause: "Value at 'maxResults' failed to satisfy constraint: \
                     Member must have value greater than or equal to 1",
        },
        RefusalCase {
            name: "ListClusters maxResults far below its range",
            method: "GET",
            path: "/cluster?max-results=-5".to_string(),
            body: None,
            offending: "-5",
            clause: "Value at 'maxResults' failed to satisfy constraint: \
                     Member must have value greater than or equal to 1",
        },
        RefusalCase {
            name: "ListClusters maxResults above its range",
            method: "GET",
            path: "/cluster?max-results=101".to_string(),
            body: None,
            offending: "101",
            clause: "Value at 'maxResults' failed to satisfy constraint: \
                     Member must have value less than or equal to 100",
        },
        // A length constraint, on a body member: same construction, different
        // requirement clause.
        RefusalCase {
            name: "PutClusterPolicy empty policy document",
            method: "POST",
            path: format!("/cluster/{}/policy", absent_identifier()),
            body: Some(json!({"policy": ""})),
            offending: "''",
            clause: "Value at 'policy' failed to satisfy constraint: \
                     Member must have length greater than or equal to 1",
        },
        // A different pattern, so the anchors are stripped from the pattern as
        // modelled rather than from one memorised regex.
        RefusalCase {
            name: "CreateCluster clientToken outside its pattern",
            method: "POST",
            path: "/cluster".to_string(),
            body: Some(json!({"deletionProtectionEnabled": false, "clientToken": "has a space"})),
            offending: "has a space",
            clause: "Value at 'clientToken' failed to satisfy constraint: \
                     Member must satisfy regular expression pattern: [!-~]+",
        },
    ];

    let dsql = Dsql::new();
    for case in cases {
        let answer = dsql.call(case.method, &case.path, case.body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: a constraint violation must answer 400, got {} {:?}",
            case.name, answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case {:?}: the refusal must be a ValidationException, got {:?}",
            case.name,
            answer.error
        );
        assert_eq!(
            answer.message(),
            format!("1 validation error detected: {}", case.clause),
            "case {:?}: expected the AWS envelope for the member and its constraint, got {:?}",
            case.name,
            answer.message()
        );
        assert!(
            !answer.message().contains(case.offending),
            "case {:?}: the message must not echo the rejected value {:?}, got {:?}",
            case.name,
            case.offending,
            answer.message()
        );
        assert!(
            !answer.message().contains('^') && !answer.message().contains('$'),
            "case {:?}: the message must quote the pattern unanchored, got {:?}",
            case.name,
            answer.message()
        );
    }

    // The bounds themselves are inside the range, so they are served.
    for accepted in [1, 100] {
        let answer = dsql.list(&format!("?max-results={accepted}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={accepted}: a published bound must be accepted, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: a tag map carrying more entries than `TagMap`'s modelled maximum is
/// refused as a bad request BODY — 400 `BadRequestException`, message
/// "Invalid request body" — on every operation that takes one, and none of the
/// oversized batch is applied. The maximum itself is accepted.
#[tokio::test]
async fn a_tag_map_over_its_modelled_maximum_is_a_bad_request_body() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let cluster_arn = expected_arn(dsql.region(), &cluster);
    let account = engine::EMULATOR_ACCOUNT_ID;

    let over = TAG_MAP_MAX_ENTRIES + 1;
    let mut stream_over = stream_request(account, "dsql-cdc");
    stream_over["tags"] = tags(over);

    let answers = vec![
        (
            "CreateCluster",
            dsql.create(json!({"deletionProtectionEnabled": false, "tags": tags(over)}))
                .await,
        ),
        (
            "TagResource",
            dsql.tag_resource(&cluster_arn, json!({"tags": tags(over)}))
                .await,
        ),
        (
            "CreateStream",
            dsql.create_stream(&cluster, stream_over).await,
        ),
    ];
    for (operation, answer) in answers {
        assert_eq!(
            answer.status, 400,
            "case {operation} with {over} tags: must answer 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("BadRequestException"),
            "case {operation} with {over} tags: the refusal must be a BadRequestException, got {:?}",
            answer.error
        );
        assert_eq!(
            answer.message(),
            "Invalid request body",
            "case {operation} with {over} tags: got {:?}",
            answer.message()
        );
    }

    // Nothing of the refused CreateCluster batch was applied: the only cluster is
    // the one created above, and it is untagged.
    let listed = dsql.list("?max-results=100").await;
    assert_eq!(
        listed.get("clusters").as_array().map(Vec::len),
        Some(1),
        "an oversized tag batch must create no cluster, got {:?}",
        listed.get("clusters")
    );
    assert_eq!(
        dsql.get(&cluster).await.get("tags"),
        Value::Null,
        "an oversized tag batch must leave the addressed cluster untagged"
    );

    // The modelled maximum is a legal request.
    let at_maximum = dsql
        .create(json!({"deletionProtectionEnabled": false, "tags": tags(TAG_MAP_MAX_ENTRIES)}))
        .await;
    assert_eq!(
        at_maximum.status, 200,
        "case CreateCluster with exactly {TAG_MAP_MAX_ENTRIES} tags: must be accepted, got {} {:?}",
        at_maximum.status, at_maximum.body
    );
    assert_eq!(
        dsql.get(&at_maximum.str("identifier"))
            .await
            .get("tags")
            .as_object()
            .map(serde_json::Map::len),
        Some(TAG_MAP_MAX_ENTRIES),
        "every tag of an accepted batch must be persisted"
    );
}

/// RULE: a `CreateCluster` `clientToken` names the request it first served. The
/// same token with the SAME parameters returns that first cluster and creates
/// nothing; the same token with ANY different parameter is 409
/// `ConflictException` and creates nothing either.
#[tokio::test]
async fn a_create_cluster_client_token_names_the_request_it_first_served() {
    let base = json!({
        "deletionProtectionEnabled": false,
        "tags": {"env": "test"},
        "kmsEncryptionKey": "alias/dsql-one",
        "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
    });

    // Each row differs from `base` in ONE member, so the comparison is over the
    // whole request rather than over whichever member a recording happened to
    // change.
    let differing = vec![
        (
            "a different tag value",
            json!({"tags": {"env": "prod"}}),
        ),
        (
            "deletion protection flipped",
            json!({"deletionProtectionEnabled": true}),
        ),
        (
            "a different encryption key",
            json!({"kmsEncryptionKey": "alias/dsql-two"}),
        ),
        (
            "a different policy document",
            json!({"policy": "{\"Version\":\"2012-10-17\"}"}),
        ),
        (
            "multi-Region properties added",
            json!({"multiRegionProperties": {"witnessRegion": "us-west-2"}}),
        ),
        ("a tag removed", json!({"tags": {}})),
    ];

    for (name, change) in differing {
        let dsql = Dsql::new();
        let token = format!("token-for-{}", name.replace(' ', "-"));

        let mut first_request = base.clone();
        first_request["clientToken"] = Value::String(token.clone());
        let first = dsql.create(first_request.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first CreateCluster must succeed, got {} {:?}",
            first.status, first.body
        );

        // The identical request is the same request: the first cluster comes back.
        let replay = dsql.create(first_request).await;
        assert_eq!(
            replay.status, 200,
            "case {name:?}: an identical replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.str("identifier"),
            first.str("identifier"),
            "case {name:?}: an identical replay must answer with the first cluster, got {:?}",
            replay.str("identifier")
        );

        // A different request wearing the same token is a conflict.
        let mut second_request = base.clone();
        second_request["clientToken"] = Value::String(token.clone());
        for (member, value) in change.as_object().expect("each change is an object") {
            second_request[member] = value.clone();
        }
        let conflict = dsql.create(second_request).await;
        assert_eq!(
            conflict.status, 409,
            "case {name:?}: reusing the token for a different request must answer 409, got {} {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error.as_deref(),
            Some("ConflictException"),
            "case {name:?}: the refusal must be a ConflictException, got {:?}",
            conflict.error
        );
        assert_eq!(
            conflict.message(),
            CLIENT_TOKEN_MISMATCH,
            "case {name:?}: got {:?}",
            conflict.message()
        );

        let listed = dsql.list("?max-results=100").await;
        assert_eq!(
            listed.get("clusters").as_array().map(Vec::len),
            Some(1),
            "case {name:?}: a reused token must never mint a second cluster, got {:?}",
            listed.get("clusters")
        );
    }
}

/// RULE: an `UpdateCluster` `clientToken` names the request it first served too —
/// so an immediate retry answers 200 with the same identifier and ARN instead of
/// the status conflict the first call's own `UPDATING` status would otherwise
/// cause, and the token reused for a different change is 409 `ConflictException`
/// with the first request's setting left in place.
#[tokio::test]
async fn an_update_cluster_client_token_replays_instead_of_conflicting() {
    // Row 1 retries while the first update is still in flight; row 2 retries once
    // the clock has settled the cluster back to ACTIVE.
    for (name, settle_before_replay) in [("while UPDATING", false), ("once ACTIVE again", true)] {
        let dsql = Dsql::new();
        let identifier = dsql.create_settled().await;
        let token = "update-token";

        let first = dsql
            .update(
                &identifier,
                json!({"deletionProtectionEnabled": true, "clientToken": token}),
            )
            .await;
        assert_eq!(
            first.status, 200,
            "case {name}: the first UpdateCluster must succeed, got {} {:?}",
            first.status, first.body
        );

        if settle_before_replay {
            wait_for_the_clock().await;
        }
        let replay = dsql
            .update(
                &identifier,
                json!({"deletionProtectionEnabled": true, "clientToken": token}),
            )
            .await;
        assert_eq!(
            replay.status, 200,
            "case {name}: an identical replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            (replay.str("identifier"), replay.str("arn")),
            (first.str("identifier"), first.str("arn")),
            "case {name}: a replay must answer with the same cluster, got {:?}",
            replay.body
        );

        let conflict = dsql
            .update(
                &identifier,
                json!({"deletionProtectionEnabled": false, "clientToken": token}),
            )
            .await;
        assert_eq!(
            conflict.status, 409,
            "case {name}: the token reused for a different change must answer 409, got {} {:?}",
            conflict.status, conflict.body
        );
        assert_eq!(
            conflict.error.as_deref(),
            Some("ConflictException"),
            "case {name}: the refusal must be a ConflictException, got {:?}",
            conflict.error
        );
        assert_eq!(
            conflict.message(),
            CLIENT_TOKEN_MISMATCH,
            "case {name}: got {:?}",
            conflict.message()
        );

        wait_for_the_clock().await;
        assert_eq!(
            dsql.get(&identifier).await.get("deletionProtectionEnabled"),
            json!(true),
            "case {name}: a refused token reuse must leave the first request's setting in place"
        );
    }
}

/// RULE: `DeleteCluster` against a deletion-protected cluster is refused as an
/// input-constraint violation — 400 `ValidationException`, reason
/// `deletionProtectionEnabled` — the cluster is untouched, and the identical call
/// succeeds once protection is disabled. Asserted however the protection got
/// there.
#[tokio::test]
async fn deletion_protection_refuses_the_delete_as_invalid_input() {
    for how in ["set at create", "set by update"] {
        let dsql = Dsql::new();
        let identifier = match how {
            "set at create" => {
                let created = dsql.create(json!({"deletionProtectionEnabled": true})).await;
                wait_for_the_clock().await;
                created.str("identifier")
            }
            _ => {
                let identifier = dsql.create_settled().await;
                dsql.update(&identifier, json!({"deletionProtectionEnabled": true}))
                    .await;
                wait_for_the_clock().await;
                identifier
            }
        };

        let refused = dsql.delete(&identifier).await;
        assert_eq!(
            refused.status, 400,
            "case {how}: a protected cluster must refuse the delete with 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {how}: the refusal must be a ValidationException, got {:?}",
            refused.error
        );
        assert_eq!(
            refused.str("reason"),
            "deletionProtectionEnabled",
            "case {how}: the reason must name the deletion-protection setting, got {:?}",
            refused.str("reason")
        );
        assert_eq!(
            refused.message(),
            "Can't delete cluster: Deletion protection is enabled. \
             To proceed, disable deletion protection and try again.",
            "case {how}: got {:?}",
            refused.message()
        );
        assert_eq!(
            dsql.get(&identifier).await.status,
            200,
            "case {how}: a refused delete must leave the cluster in place"
        );

        dsql.update(&identifier, json!({"deletionProtectionEnabled": false}))
            .await;
        wait_for_the_clock().await;
        assert_eq!(
            dsql.delete(&identifier).await.status,
            200,
            "case {how}: the identical delete must succeed once protection is off"
        );
    }
}

/// RULE: a cluster's resource-based policy round-trips through
/// `PutClusterPolicy` / `GetClusterPolicy` / `DeleteClusterPolicy`: a cluster with
/// no policy attached is 404, an attached policy is reported with the version
/// token the put answered with, a stale `expectedPolicyVersion` is refused, and a
/// deleted policy is 404 again.
#[tokio::test]
async fn the_cluster_policy_round_trips_and_reports_its_absence() {
    let dsql = Dsql::new();
    let identifier = dsql.create_settled().await;

    let absent = dsql.get_policy(&identifier).await;
    assert_eq!(
        absent.status, 404,
        "a cluster with no policy must answer 404, got {} {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error.as_deref(),
        Some("ResourceNotFoundException"),
        "the 404 must be a ResourceNotFoundException, got {:?}",
        absent.error
    );
    assert_eq!(
        absent.message(),
        format!(
            "Aurora DSQL can't find the policy for the resource {identifier}. \
             Verify that the policy exists."
        ),
        "got {:?}",
        absent.message()
    );

    // Several documents in turn: each attaches, reports back verbatim, and gets a
    // version token the previous one did not have.
    let documents = [
        json!({"Version": "2012-10-17", "Statement": []}),
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
                "Action": "dsql:DbConnect",
                "Resource": "*",
            }],
        }),
        json!({
            "Version": "2012-10-17",
            "Statement": {
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:DbConnectAdmin",
                "Resource": "*",
            },
        }),
    ];
    let mut versions: Vec<String> = Vec::new();
    for (round, document) in documents.iter().enumerate() {
        let serialized = serde_json::to_string(document).expect("a policy serializes");
        let put = dsql
            .put_policy(&identifier, json!({"policy": serialized}))
            .await;
        assert_eq!(
            put.status, 200,
            "case document #{round}: PutClusterPolicy must succeed, got {} {:?}",
            put.status, put.body
        );
        let version = put.str("policyVersion");
        assert!(
            !version.is_empty(),
            "case document #{round}: PutClusterPolicy must answer a policyVersion, got {:?}",
            put.body
        );
        assert!(
            !versions.contains(&version),
            "case document #{round}: each attached policy must get its own version, got {version:?} \
             after {versions:?}"
        );

        let got = dsql.get_policy(&identifier).await;
        assert_eq!(
            got.status, 200,
            "case document #{round}: GetClusterPolicy must report the attached policy, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.str("policy"),
            serialized,
            "case document #{round}: GetClusterPolicy must report the document verbatim"
        );
        assert_eq!(
            got.str("policyVersion"),
            version,
            "case document #{round}: GetClusterPolicy must report the version the put answered"
        );
        versions.push(version);
    }

    // A stale `expectedPolicyVersion` names a version that is no longer attached,
    // so neither a put nor a delete may act on it.
    let stale = versions[0].clone();
    let current = versions.last().cloned().expect("a version was issued");
    let refused_put = dsql
        .put_policy(
            &identifier,
            json!({"policy": "{}", "expectedPolicyVersion": stale}),
        )
        .await;
    assert_eq!(
        refused_put.status, 409,
        "a put against a stale version must answer 409, got {} {:?}",
        refused_put.status, refused_put.body
    );
    let refused_delete = dsql
        .delete_policy(&identifier, &format!("?expected-policy-version={stale}"))
        .await;
    assert_eq!(
        refused_delete.status, 409,
        "a delete against a stale version must answer 409, got {} {:?}",
        refused_delete.status, refused_delete.body
    );
    assert_eq!(
        dsql.get_policy(&identifier).await.str("policyVersion"),
        current,
        "a refused put or delete must leave the attached policy alone"
    );

    let deleted = dsql
        .delete_policy(&identifier, &format!("?expected-policy-version={current}"))
        .await;
    assert_eq!(
        deleted.status, 200,
        "a delete naming the current version must succeed, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.str("policyVersion"),
        current,
        "DeleteClusterPolicy must answer with the version it removed"
    );

    let gone = dsql.get_policy(&identifier).await;
    assert_eq!(
        gone.status, 404,
        "once the policy is deleted GetClusterPolicy must answer 404 again, got {} {:?}",
        gone.status, gone.body
    );

    // A second delete has no policy of that version to remove.
    let again = dsql
        .delete_policy(&identifier, &format!("?expected-policy-version={current}"))
        .await;
    assert_eq!(
        again.status, 404,
        "a second delete must answer 404, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.message(),
        format!(
            "Aurora DSQL can't find policy version {current} for resource {identifier}. \
             Verify that the policy exists and that the version is correct."
        ),
        "got {:?}",
        again.message()
    );
}

/// RULE: unless `bypassPolicyLockoutSafetyCheck` is set, `PutClusterPolicy`
/// refuses a document that denies EVERY principal the action that would replace
/// it (400 `ValidationException`) and attaches nothing; every other document is
/// attached.
#[tokio::test]
async fn a_policy_that_denies_every_principal_the_policy_action_is_refused() {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // Each row is a Statement, with whether it locks every principal out of
    // replacing the policy. `dsql:PutClusterPolicy` is the action that replaces
    // it; `*` and `dsql:*` cover it, and an explicit Deny beats any Allow.
    let cases = vec![
        (
            "deny every principal every dsql action",
            json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:*", "Resource": "*"}),
            true,
        ),
        (
            "deny every principal everything",
            json!({"Effect": "Deny", "Principal": "*", "Action": "*"}),
            true,
        ),
        (
            "deny the AWS principal wildcard the action itself",
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": "*"},
                "Action": ["dsql:DbConnect", "dsql:PutClusterPolicy"],
                "Resource": "*",
            }),
            true,
        ),
        (
            "deny every principal a wildcarded family that covers it",
            json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:Put*"}),
            true,
        ),
        (
            "deny every principal everything but one other action",
            json!({"Effect": "Deny", "Principal": "*", "NotAction": "dsql:DbConnect"}),
            true,
        ),
        (
            "allow every principal every dsql action",
            json!({"Effect": "Allow", "Principal": "*", "Action": "dsql:*", "Resource": "*"}),
            false,
        ),
        (
            "deny every principal a different action",
            json!({"Effect": "Deny", "Principal": "*", "Action": "dsql:DbConnect"}),
            false,
        ),
        (
            "deny one account the action",
            json!({
                "Effect": "Deny",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "dsql:*",
            }),
            false,
        ),
        (
            "deny every principal the action only under a condition",
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Condition": {"Bool": {"aws:SecureTransport": "false"}},
            }),
            false,
        ),
        (
            "deny every principal the action on another resource",
            json!({
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
                "Resource": format!("arn:aws:dsql:us-east-1:{account}:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa"),
            }),
            false,
        ),
    ];

    for (name, statement, locks_out) in cases {
        let dsql = Dsql::new();
        let identifier = dsql.create_settled().await;
        let document = serde_json::to_string(&json!({
            "Version": "2012-10-17",
            "Statement": [statement],
        }))
        .expect("a policy serializes");

        let answer = dsql
            .put_policy(&identifier, json!({"policy": document.clone()}))
            .await;
        if locks_out {
            assert_eq!(
                answer.status, 400,
                "case {name:?}: a lockout policy must be refused with 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {name:?}: the refusal must be a ValidationException, got {:?}",
                answer.error
            );
            assert_eq!(
                dsql.get_policy(&identifier).await.status,
                404,
                "case {name:?}: a refused put must attach nothing"
            );

            // The caller can accept the risk explicitly.
            let bypassed = dsql
                .put_policy(
                    &identifier,
                    json!({"policy": document, "bypassPolicyLockoutSafetyCheck": true}),
                )
                .await;
            assert_eq!(
                bypassed.status, 200,
                "case {name:?}: bypassPolicyLockoutSafetyCheck must apply it anyway, got {} {:?}",
                bypassed.status, bypassed.body
            );
        } else {
            assert_eq!(
                answer.status, 200,
                "case {name:?}: a policy that leaves the action reachable must be attached, \
                 got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                dsql.get_policy(&identifier).await.str("policy"),
                document,
                "case {name:?}: the attached document must read back verbatim"
            );
        }
    }

    // A document that is not a policy at all cannot be reasoned about.
    let dsql = Dsql::new();
    let identifier = dsql.create_settled().await;
    for not_a_policy in ["not json at all", "[]", "\"a string\"", "7"] {
        let answer = dsql
            .put_policy(&identifier, json!({"policy": not_a_policy}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {not_a_policy:?}: a document that is not a JSON policy must be refused with 400, \
             got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.str("reason"),
            "cannotParse",
            "case {not_a_policy:?}: the reason must say it could not be parsed, got {:?}",
            answer.str("reason")
        );
    }
}

/// RULE: `CreateStream` may only be asked to pass a role in the caller's own
/// account. A role in another account is an authorization failure — 403
/// `AccessDeniedException` — and no stream is created; a role in this account is
/// accepted and the stream reads back through `GetStream`.
#[tokio::test]
async fn a_stream_target_role_must_belong_to_this_account() {
    let own = engine::EMULATOR_ACCOUNT_ID;
    let cases = vec![
        ("this account", own, "dsql-cdc", true),
        ("this account, role with a path", own, "service-role/dsql-cdc", true),
        ("another account", "210987654321", "dsql-cdc", false),
        ("a third account", "999999999999", "service-role/dsql-cdc", false),
        ("yet another account", "111111111111", "dsql-cdc", false),
    ];

    for (name, account, role, allowed) in cases {
        let dsql = Dsql::new();
        let cluster = dsql.create_settled().await;
        let answer = dsql
            .create_stream(&cluster, stream_request(account, role))
            .await;

        if allowed {
            assert_eq!(
                answer.status, 200,
                "case {name:?}: a role in this account must be passable, got {} {:?}",
                answer.status, answer.body
            );
            let stream_identifier = answer.str("streamIdentifier");
            assert!(
                is_cluster_id(&stream_identifier),
                "case {name:?}: the minted StreamId must satisfy ^[a-z0-9]{{26}}$, \
                 got {stream_identifier:?}"
            );
            assert_eq!(
                answer.str("arn"),
                format!(
                    "arn:aws:dsql:{}:{own}:cluster/{cluster}/stream/{stream_identifier}",
                    dsql.region()
                ),
                "case {name:?}: the stream ARN must name the cluster and the stream"
            );

            // Round trip: the reader reports what the create persisted.
            let got = dsql.get_stream(&cluster, &stream_identifier).await;
            assert_eq!(
                got.status, 200,
                "case {name:?}: GetStream must report the created stream, got {} {:?}",
                got.status, got.body
            );
            assert_eq!(
                got.str("arn"),
                answer.str("arn"),
                "case {name:?}: GetStream must report the stored ARN"
            );
            assert_eq!(
                got.get("targetDefinition"),
                kinesis_target(account, role),
                "case {name:?}: GetStream must report the stored target definition"
            );
            assert_eq!(
                (got.str("ordering"), got.str("format")),
                ("UNORDERED".to_string(), "JSON".to_string()),
                "case {name:?}: GetStream must report the stored ordering and format"
            );
            // The clock, not a caller, carries the stream to ACTIVE.
            assert_eq!(
                got.str("status"),
                "CREATING",
                "case {name:?}: a fresh stream reports CREATING, got {:?}",
                got.str("status")
            );
            wait_for_the_clock().await;
            assert_eq!(
                dsql.get_stream(&cluster, &stream_identifier)
                    .await
                    .str("status"),
                "ACTIVE",
                "case {name:?}: the clock must settle the stream into ACTIVE"
            );
        } else {
            assert_eq!(
                answer.status, 403,
                "case {name:?}: passing a foreign-account role must answer 403, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("AccessDeniedException"),
                "case {name:?}: the refusal must be an AccessDeniedException, got {:?}",
                answer.error
            );
            assert_eq!(
                answer.message(),
                "Cross-account pass role is not allowed.",
                "case {name:?}: got {:?}",
                answer.message()
            );
            assert_eq!(
                dsql.get_stream(&cluster, &absent_identifier()).await.status,
                404,
                "case {name:?}: a refused CreateStream must create no stream"
            );
        }
    }
}

/// RULE: a `CreateStream` `clientToken` names the request it first served, like
/// every other `@idempotencyToken` in this service: an identical repeat answers
/// with the first stream, and a different request wearing the token is 409.
#[tokio::test]
async fn a_create_stream_client_token_names_the_request_it_first_served() {
    let dsql = Dsql::new();
    let cluster = dsql.create_settled().await;
    let account = engine::EMULATOR_ACCOUNT_ID;

    let mut request = stream_request(account, "dsql-cdc");
    request["clientToken"] = Value::String("stream-token".to_string());

    let first = dsql.create_stream(&cluster, request.clone()).await;
    assert_eq!(
        first.status, 200,
        "the first CreateStream must succeed, got {} {:?}",
        first.status, first.body
    );
    let replay = dsql.create_stream(&cluster, request.clone()).await;
    assert_eq!(
        replay.str("streamIdentifier"),
        first.str("streamIdentifier"),
        "an identical replay must answer with the first stream, got {:?}",
        replay.body
    );

    let mut different = request.clone();
    different["tags"] = json!({"env": "test"});
    let conflict = dsql.create_stream(&cluster, different).await;
    assert_eq!(
        conflict.status, 409,
        "the token reused for a different request must answer 409, got {} {:?}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.message(),
        CLIENT_TOKEN_MISMATCH,
        "got {:?}",
        conflict.message()
    );
}
