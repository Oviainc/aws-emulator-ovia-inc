// SPDX-License-Identifier: CC0-1.0

//! Which `resourceArn` values the tagging surface serves, and which name a resource
//! this endpoint does not hold.
//!
//! RULE under test: Aurora DSQL is a regional service, and an ARN's partition, region
//! and account segments are part of the resource's identity — `ClusterArn`'s pattern
//! pins them (`smithy_query(query="shape", shape="ClusterArn,StreamArn")`:
//! `^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`), and
//! every ARN the service hands back carries the region the resource was created in
//! and the account that owns it. So an ARN-addressed operation serves an ARN only
//! when that scope is BOTH the one the endpoint the request reached serves and the
//! one the named resource was minted with; any other well-formed Aurora DSQL ARN
//! names a resource of some other region, account or partition, which this endpoint
//! holds none of, and answers the tagging surface's own not-found (404
//! `NotFoundException`) — never a successful read of, or a write to, the
//! similarly-identified local resource.
//!
//! The rule covers every operation bound to `/tags/{resourceArn}`
//! (`ListTagsForResource`, `TagResource`, `UntagResource`, which resolve the ARN
//! through one shared resolution) and every tagged resource of this service (a
//! cluster, and a stream nested under a cluster's ARN).
//!
//! The table spans the class the rule generalises over: the ARN FIELD that differs
//! (region, account, partition, several at once, each left empty), the RESOURCE KIND
//! addressed, the OPERATION answering, and the DIRECTION of the mismatch (a foreign
//! ARN put to the endpoint that holds the resource, and the resource's own ARN —
//! or a re-spelling of it — put to another endpoint sharing the store). The reported
//! case, an ARN naming another Region and another account, is two rows.
//!
//! A refused write must also have had no effect, so every row re-reads the local
//! resource's tags through its own ARN afterwards; and the control rounds pin the
//! other side of the rule, so a 404 here is about the ARN's scope rather than about
//! the route or the store.
//!
//! The client is the shared `Dsql` harness in `harness.rs`, and `UntagResource`'s
//! query encoding is the one the `resource_tags` suite already spells.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::harness::{expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql};
use crate::resource_tags::untag;

/// The region this suite's endpoint is signed for, and therefore the region every
/// resource it creates is minted in.
const HOME: &str = "us-east-1";

/// Another region, signed for by a second view of the SAME store — the mirror
/// direction of the rule.
const AWAY: &str = "eu-west-1";

/// The tag every resource in this suite is created with, so a read has something to
/// report and a refused `UntagResource` has something it could have removed.
fn initial_tags() -> Value {
    json!({"env": "prod"})
}

/// The tag a cross-scope `TagResource` would write if it were served — a value no
/// local resource may end up carrying.
fn foreign_write() -> Value {
    json!({"owner": "someone-else"})
}

/// One ARN scope: the partition, region and account fields an ARN carries.
struct Scope {
    name: &'static str,
    partition: &'static str,
    region: &'static str,
    account: &'static str,
}

/// `arn:{partition}:dsql:{region}:{account}:{resource}` for one scope — the form the
/// service's own ARNs take, with only the scope fields varied.
fn scoped_arn(scope: &Scope, resource: &str) -> String {
    format!(
        "arn:{}:dsql:{}:{}:{}",
        scope.partition, scope.region, scope.account, resource
    )
}

/// Scopes that are NOT the one the [`HOME`] endpoint serves, each row differing from
/// it in a different field or combination of fields. Every row is otherwise a
/// well-formed Aurora DSQL ARN, so it reaches a handler rather than the boundary
/// validator, and every row keeps the resource path of a real local resource — the
/// only thing wrong with it is the scope.
fn foreign_scopes() -> Vec<Scope> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        Scope {
            name: "another region in the same partition and account",
            partition: "aws",
            region: AWAY,
            account,
        },
        Scope {
            name: "a region on the other side of the world",
            partition: "aws",
            region: "ap-northeast-1",
            account,
        },
        Scope {
            name: "a region name the endpoint's own region is a prefix of",
            partition: "aws",
            region: "us-east-11",
            account,
        },
        Scope {
            name: "another account id",
            partition: "aws",
            region: HOME,
            account: "999988887777",
        },
        Scope {
            name: "the account id AWS examples use",
            partition: "aws",
            region: HOME,
            account: "123456789012",
        },
        Scope {
            name: "another region AND another account",
            partition: "aws",
            region: AWAY,
            account: "999988887777",
        },
        Scope {
            name: "another partition, the endpoint's own region",
            partition: "aws-cn",
            region: HOME,
            account,
        },
        Scope {
            name: "another partition with a region of its own",
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            account,
        },
        Scope {
            name: "no region field at all",
            partition: "aws",
            region: "",
            account,
        },
        Scope {
            name: "no account field at all",
            partition: "aws",
            region: HOME,
            account: "",
        },
    ]
}

/// The same ARN put to every operation bound to `/tags/{resourceArn}`, each with a
/// request that is otherwise complete, so the only thing any of them can fail on is
/// the ARN. The write comes first, so a row that wrongly served it would be visible
/// in the local resource's tags afterwards.
async fn every_tagging_operation(dsql: &Dsql, arn: &str) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "TagResource",
            dsql.tag_resource(arn, json!({"tags": foreign_write()})).await,
        ),
        ("ListTagsForResource", dsql.list_tags_for_resource(arn).await),
        ("UntagResource", untag(dsql, arn, &["env".to_string()]).await),
    ]
}

/// Assert one answer is the tagging surface's own "no such resource" refusal: HTTP
/// 404 with the wire code `NotFoundException` and that surface's fixed sentence,
/// which interpolates nothing the request carried. The identifier-addressed
/// operations answer the modelled `ResourceNotFoundException` instead; the two
/// surfaces are compared in `tests/all/fixed_refusal_sentences.rs`.
fn assert_not_found(case: &str, arn: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: an ARN this endpoint does not serve must answer 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("NotFoundException"),
        "case {case:?}: the wire code must be NotFoundException, got {:?}",
        answer.error
    );
    assert_eq!(
        answer.message(),
        "Resource was not found",
        "case {case:?}: the tagging surface's fixed sentence must state the refusal, got {:?}",
        answer.message()
    );
    assert!(
        !answer.message().contains(arn),
        "case {case:?}: the sentence must interpolate nothing from the request ({arn:?}), \
         got {:?}",
        answer.message()
    );
}

/// Assert the resource at `local_arn` still carries exactly `expected`, read through
/// the endpoint that does serve it.
async fn assert_tags_unchanged(case: &str, dsql: &Dsql, local_arn: &str, expected: &Value) {
    let listed = dsql.list_tags_for_resource(local_arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the resource's own endpoint must still serve its own ARN, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        &listed.get("tags"),
        expected,
        "case {case:?}: a refused cross-scope call must leave the local tags at {expected:?}, \
         got {:?}",
        listed.get("tags")
    );
}

/// A `targetDefinition` naming a Kinesis destination, as `CreateStream` requires.
/// Source: `smithy_query(query="shape", shape="KinesisTargetDefinition")` — a
/// `streamArn` and a `roleArn` in the caller's own account.
fn kinesis_target(region: &str) -> Value {
    let account = engine::EMULATOR_ACCOUNT_ID;
    json!({"kinesis": {
        "streamArn": format!("arn:aws:kinesis:{region}:{account}:stream/changes"),
        "roleArn": format!("arn:aws:iam::{account}:role/dsql-cdc"),
    }})
}

/// A cluster carrying [`initial_tags`] and a stream of it carrying the same, both
/// created through `dsql`: the identifiers and the ARNs the service minted for them.
struct Tagged {
    cluster: String,
    cluster_arn: String,
    stream: String,
    stream_arn: String,
}

async fn tagged_resources(dsql: &Dsql) -> Tagged {
    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "tags": initial_tags()}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster must answer 200, got {} {:?}",
        created.status, created.body
    );
    let cluster = created.str("identifier");
    let cluster_arn = created.str("arn");
    assert_eq!(
        cluster_arn,
        expected_arn(dsql.region(), &cluster),
        "CreateCluster must answer the cluster's own ARN, got {cluster_arn:?}"
    );

    // `CreateStream` refuses a cluster the clock still owes its `ACTIVE` status.
    wait_for_the_clock().await;
    let created_stream = dsql
        .create_stream(
            &cluster,
            json!({
                "format": "JSON",
                "ordering": "UNORDERED",
                "targetDefinition": kinesis_target(dsql.region()),
                "tags": initial_tags(),
            }),
        )
        .await;
    assert_eq!(
        created_stream.status, 200,
        "CreateStream must answer 200, got {} {:?}",
        created_stream.status, created_stream.body
    );
    let stream = created_stream.str("streamIdentifier");
    let stream_arn = created_stream.str("arn");
    assert_eq!(
        stream_arn,
        expected_stream_arn(dsql.region(), &cluster, &stream),
        "CreateStream must answer the stream's nested ARN, got {stream_arn:?}"
    );

    Tagged {
        cluster,
        cluster_arn,
        stream,
        stream_arn,
    }
}

/// RULE: an ARN whose partition, region or account is not the one this endpoint
/// serves names a resource this endpoint does not hold — for every tagged resource
/// kind and every operation on the tagging path — so it answers the modelled 404 and
/// the similarly-identified local resource is neither read nor written.
#[tokio::test]
async fn an_arn_outside_the_endpoints_scope_is_not_found() {
    let dsql = Dsql::in_region(HOME);
    let held = tagged_resources(&dsql).await;

    // The resource path of each tagged resource, with its own (local) ARN beside it.
    let resources: &[(&str, String, &str)] = &[
        (
            "cluster",
            format!("cluster/{}", held.cluster),
            &held.cluster_arn,
        ),
        (
            "stream of a cluster",
            format!("cluster/{}/stream/{}", held.cluster, held.stream),
            &held.stream_arn,
        ),
    ];

    let scopes = foreign_scopes();
    assert!(
        scopes.len() >= 8,
        "the table must span the fields an ARN's scope is made of, got {} rows",
        scopes.len()
    );

    for (kind, resource_path, local_arn) in resources {
        for scope in &scopes {
            let arn = scoped_arn(scope, resource_path);
            assert_ne!(
                &arn, local_arn,
                "case {:?}: a foreign-scope row must differ from the resource's own ARN",
                scope.name
            );
            for (operation, answer) in every_tagging_operation(&dsql, &arn).await {
                let case = format!("{operation} on a {kind} with {}", scope.name);
                assert_not_found(&case, &arn, &answer);
            }
            assert_tags_unchanged(
                &format!("a {kind} after {}", scope.name),
                &dsql,
                local_arn,
                &initial_tags(),
            )
            .await;
        }
    }

    // The other side of the rule: each resource's OWN ARN, at the endpoint that
    // minted it, is served — read, written and read back through the resource's own
    // reader — so the 404s above are about the ARN's scope and not about the route.
    for (kind, _, local_arn) in resources {
        let listed = dsql.list_tags_for_resource(local_arn).await;
        assert_eq!(
            listed.get("tags"),
            initial_tags(),
            "case {kind:?}: the resource's own ARN must report its tags, got {:?}",
            listed.get("tags")
        );

        let tagged = dsql
            .tag_resource(local_arn, json!({"tags": {"owner": "team-a"}}))
            .await;
        assert_eq!(
            tagged.status, 200,
            "case {kind:?}: TagResource on the resource's own ARN must answer 200, got {} {:?}",
            tagged.status, tagged.body
        );
        let own = match *kind {
            "cluster" => dsql.get(&held.cluster).await,
            _ => dsql.get_stream(&held.cluster, &held.stream).await,
        };
        assert_eq!(
            own.get("tags"),
            json!({"env": "prod", "owner": "team-a"}),
            "case {kind:?}: the resource's own reader must report the written tag, got {:?}",
            own.get("tags")
        );

        let removed = untag(&dsql, local_arn, &["owner".to_string()]).await;
        assert_eq!(
            removed.status, 200,
            "case {kind:?}: UntagResource on the resource's own ARN must answer 200, got {} {:?}",
            removed.status, removed.body
        );
        assert_tags_unchanged(
            &format!("a {kind} back at its initial tags"),
            &dsql,
            local_arn,
            &initial_tags(),
        )
        .await;
    }
}

/// RULE (the mirror direction): an endpoint serving another region does not serve a
/// resource minted elsewhere — neither through that resource's own ARN, which names
/// another region, nor through an ARN re-spelled for this endpoint's region, which
/// names a resource of this region that does not exist. Both endpoints read ONE
/// store, exactly as the production router builds them, so this is about the ARN's
/// scope rather than about two stores.
#[tokio::test]
async fn an_endpoint_does_not_serve_a_resource_minted_in_another_region() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let home = Dsql::sharing(&engine, HOME);
    let away = Dsql::sharing(&engine, AWAY);

    let held = tagged_resources(&home).await;
    let cases: &[(&str, String, &str)] = &[
        (
            "the cluster's own ARN",
            held.cluster_arn.clone(),
            &held.cluster_arn,
        ),
        (
            "the stream's own nested ARN",
            held.stream_arn.clone(),
            &held.stream_arn,
        ),
        (
            "the cluster's id re-spelled for the reading endpoint's region",
            expected_arn(AWAY, &held.cluster),
            &held.cluster_arn,
        ),
        (
            "the stream's id re-spelled for the reading endpoint's region",
            expected_stream_arn(AWAY, &held.cluster, &held.stream),
            &held.stream_arn,
        ),
    ];

    for (name, arn, local_arn) in cases {
        for (operation, answer) in every_tagging_operation(&away, arn).await {
            let case = format!("{operation} at the {AWAY} endpoint with {name}");
            assert_not_found(&case, arn, &answer);
        }
        assert_tags_unchanged(
            &format!("the {HOME} resource after {name}"),
            &home,
            local_arn,
            &initial_tags(),
        )
        .await;
    }

    // And a cluster the other endpoint creates ITSELF is served there and nowhere
    // else: the same rule, read from the other region's side.
    let created = away
        .create(json!({"deletionProtectionEnabled": false, "tags": initial_tags()}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster at the {AWAY} endpoint must answer 200, got {} {:?}",
        created.status, created.body
    );
    let away_arn = created.str("arn");
    assert_eq!(
        away_arn,
        expected_arn(AWAY, &created.str("identifier")),
        "the cluster must carry its own region's ARN, got {away_arn:?}"
    );
    let listed = away.list_tags_for_resource(&away_arn).await;
    assert_eq!(
        listed.get("tags"),
        initial_tags(),
        "the creating endpoint must report its own cluster's tags, got {:?}",
        listed.get("tags")
    );
    for (operation, answer) in every_tagging_operation(&home, &away_arn).await {
        assert_not_found(
            &format!("{operation} at the {HOME} endpoint with another region's cluster"),
            &away_arn,
            &answer,
        );
    }
    assert_tags_unchanged(
        "the other region's cluster after the cross-scope calls",
        &away,
        &away_arn,
        &initial_tags(),
    )
    .await;
}
