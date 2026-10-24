// SPDX-License-Identifier: CC0-1.0

//! WHICH fault a refused MemoryDB request answers with, and in WHICH ORDER the checks
//! that produce it run, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a request that names a RESOURCE the store does not hold answers that resource's
//!   not-found fault, whatever else the request carries — the resource is resolved
//!   before what is asked of it;
//! * a request whose VALUE breaks a rule of the service is refused on that value before
//!   any resource it names is resolved, so a request that is wrong twice answers for the
//!   value;
//! * a value the service cannot honour at all is the modelled
//!   `InvalidParameterValueException`, while members that cannot hold TOGETHER — a
//!   window stated twice, an engine that cannot do what the request asks of it — are the
//!   modelled `InvalidParameterCombinationException`;
//! * a resource ARN is read against the ENDPOINT it was sent to: a region or an account
//!   this endpoint does not serve is an invalid parameter value, while a GLOBAL
//!   (multi-Region) resource's ARN carries no region at all and still resolves.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, call_signed, error_code, ok, router};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints carries.
const REGION: &str = "us-west-2";

/// Another region the same emulator answers for, used for the ARNs and the calls whose
/// scope is NOT this endpoint's.
const OTHER_REGION: &str = "eu-west-1";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An account id that is not this emulator's.
const OTHER_ACCOUNT: &str = "999988887777";

/// A node type MemoryDB publishes, for the clusters these cases seed.
const NODE_TYPE: &str = "db.t4g.small";

/// A name NO resource of any kind in these cases carries.
const ABSENT: &str = "nothing-here";

/// The ACL every MemoryDB account already has, so a create that is meant to get PAST
/// the ACL lookup can name one that exists.
const OPEN_ACCESS_ACL: &str = "open-access";

/// The service's sentence for a node type it does not offer (recorded MemoryDB
/// behaviour: the rule is stated, the value is not quoted back).
const INVALID_NODE_TYPE: &str = "Invalid node type specified.";

/// The service's sentence for an engine that cannot back a replicated cluster.
const NO_REPLICATION: &str = "Specified engine does not support replication.";

/// The service's sentence for an `UpdateUser` naming no field to change.
const EMPTY_USER_UPDATE: &str = "Update user needs to have at least one field specified.";

/// The service's sentence for an ARN whose REGION this endpoint does not serve. It
/// names the account too, because the service does not say which of the two it read the
/// region against.
const FOREIGN_REGION: &str = "Please check the region or customer ID specified in the request.";

/// The service's sentence for an ARN whose ACCOUNT is not the caller's — a different
/// sentence from [`FOREIGN_REGION`], because the service words the two differently.
const FOREIGN_ACCOUNT: &str = "The resource ARN does not belong to the caller's account.";

/// One `arn:aws:memorydb:<region>:<account>:<type>/<name>`.
fn arn(region: &str, account: &str, resource_type: &str, name: &str) -> String {
    format!("arn:aws:memorydb:{region}:{account}:{resource_type}/{name}")
}

/// The `message` of a failure body.
fn error_message(body: &Value, case: &str) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// Call `operation` and assert the status, the `__type` and — when `message` is given —
/// the sentence it carries.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    code: &str,
    message: Option<&str>,
    case: &str,
) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, code),
        "case {case:?}: expected 400 {code}, got {status} with {answered}"
    );
    if let Some(message) = message {
        assert_eq!(
            error_message(&answered, case),
            message,
            "case {case:?}: expected the service's sentence for the rule, got {answered}"
        );
    }
}

/// A router holding one cluster, so the cases that are about a MISSING resource are
/// about the name in the request rather than about an empty store.
async fn seeded() -> (MultiServiceRouter, String) {
    let router = router();
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "seeded-cluster",
            "NodeType": NODE_TYPE,
            "ACLName": OPEN_ACCESS_ACL,
        }),
        "seed cluster",
    )
    .await;
    (router, "seeded-cluster".to_string())
}

/// A tagging call's ARN resolves against the ENDPOINT it was sent to: a region or an
/// account this endpoint does not serve is the modelled `InvalidParameterValueException`
/// — not a not-found fault, which would claim the endpoint looked — for every one of the
/// three tagging operations and every kind of ARN.
#[tokio::test]
async fn an_arn_naming_another_scope_is_an_invalid_parameter_value() {
    let (router, cluster) = seeded().await;

    // The rows differ along what the rule generalises over: which part of the scope is
    // foreign (region, account, both), the kind of resource the ARN names, and whether
    // the resource it names EXISTS in this store. Each carries the sentence the service
    // words THAT part of the scope with; a row that is foreign in both is told about the
    // region, the part the service reads first.
    let rows: Vec<(String, String, &str)> = vec![
        (
            "another region, a stored cluster".to_string(),
            arn(OTHER_REGION, ACCOUNT, "cluster", &cluster),
            FOREIGN_REGION,
        ),
        (
            "another region, no such cluster".to_string(),
            arn(OTHER_REGION, ACCOUNT, "cluster", ABSENT),
            FOREIGN_REGION,
        ),
        (
            "another account".to_string(),
            arn(REGION, OTHER_ACCOUNT, "cluster", &cluster),
            FOREIGN_ACCOUNT,
        ),
        (
            "another region and account".to_string(),
            arn(OTHER_REGION, OTHER_ACCOUNT, "acl", OPEN_ACCESS_ACL),
            FOREIGN_REGION,
        ),
        (
            "another account, a user ARN".to_string(),
            arn(REGION, OTHER_ACCOUNT, "user", "default"),
            FOREIGN_ACCOUNT,
        ),
        (
            "another account, a parameter group ARN".to_string(),
            arn(REGION, OTHER_ACCOUNT, "parametergroup", ABSENT),
            FOREIGN_ACCOUNT,
        ),
        (
            // A global resource has no region to compare, but it still belongs to an
            // account.
            "another account, a multi-Region cluster ARN".to_string(),
            format!("arn:aws:memorydb::{OTHER_ACCOUNT}:multiregioncluster/{ABSENT}"),
            FOREIGN_ACCOUNT,
        ),
    ];

    for (label, candidate, sentence) in rows {
        for (operation, payload) in [
            ("ListTags", json!({})),
            ("TagResource", json!({ "Tags": [{ "Key": "k", "Value": "v" }] })),
            ("UntagResource", json!({ "TagKeys": ["k"] })),
        ] {
            let case = format!("{label} / {operation}");
            let mut body = payload;
            body["ResourceArn"] = json!(candidate);
            refused(
                &router,
                operation,
                &body,
                "InvalidParameterValueException",
                Some(sentence),
                &case,
            )
            .await;
        }
    }

    // The same ARN is served by the endpoint whose scope it names, so what the rows
    // above assert is the SCOPE and not the ARN: this one is read from its own region,
    // and the same call signed elsewhere is refused.
    let own = arn(REGION, ACCOUNT, "cluster", &cluster);
    let case = "the stored ARN at its own endpoint";
    let body = ok(&router, "ListTags", &json!({ "ResourceArn": own }), case).await;
    assert!(
        body.get("TagList").is_some(),
        "case {case:?}: expected a TagList, got {body}"
    );
    let case = "the stored ARN at another region's endpoint";
    let (status, answered) = call_signed(
        &router,
        OTHER_REGION,
        "ListTags",
        &json!({ "ResourceArn": arn(REGION, ACCOUNT, "cluster", &cluster) }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "InvalidParameterValueException"),
        "case {case:?}: got {status} with {answered}"
    );
}

/// A GLOBAL (multi-Region) resource's ARN carries no region, so one that omits the
/// region segment is well formed and resolves to the resource it names — answering that
/// kind's not-found fault, never `InvalidARNFault`. A REGIONAL kind's ARN without a
/// region is the control: that one IS malformed.
#[tokio::test]
async fn a_global_resource_arn_carries_no_region() {
    let (router, cluster) = seeded().await;

    // (the ARN, the fault it must answer). The global kinds are the two the model
    // declares a multi-Region not-found fault for; the regional kinds are the control.
    let rows: Vec<(String, String, &str)> = vec![
        (
            "a multi-Region cluster, no region".to_string(),
            format!("arn:aws:memorydb::{ACCOUNT}:multiregioncluster/{ABSENT}"),
            "MultiRegionClusterNotFoundFault",
        ),
        (
            "a multi-Region parameter group, no region".to_string(),
            format!("arn:aws:memorydb::{ACCOUNT}:multiregionparametergroup/{ABSENT}"),
            "MultiRegionParameterGroupNotFoundFault",
        ),
        (
            // A region is accepted on a global kind too: it is the only segment the
            // rule relaxes, and a client that carries one still names the resource.
            "a multi-Region cluster with a region".to_string(),
            arn(REGION, ACCOUNT, "multiregioncluster", ABSENT),
            "MultiRegionClusterNotFoundFault",
        ),
        (
            "a cluster with no region".to_string(),
            format!("arn:aws:memorydb::{ACCOUNT}:cluster/{cluster}"),
            "InvalidARNFault",
        ),
        (
            "an ACL with no region".to_string(),
            format!("arn:aws:memorydb::{ACCOUNT}:acl/{OPEN_ACCESS_ACL}"),
            "InvalidARNFault",
        ),
        (
            "a resource type MemoryDB does not tag".to_string(),
            arn(REGION, ACCOUNT, "widget", ABSENT),
            "InvalidARNFault",
        ),
    ];

    for (label, candidate, code) in rows {
        for (operation, payload) in [
            ("ListTags", json!({})),
            ("TagResource", json!({ "Tags": [{ "Key": "k", "Value": "v" }] })),
            ("UntagResource", json!({ "TagKeys": ["k"] })),
        ] {
            let case = format!("{label} / {operation}");
            let mut body = payload;
            body["ResourceArn"] = json!(candidate);
            refused(&router, operation, &body, code, None, &case).await;
        }
    }
}

/// `DeleteCluster` resolves the CLUSTER first: a name the store does not hold answers
/// `ClusterNotFoundFault` whatever else the request carries, and the members this slice
/// cannot honour are still refused for a cluster that does exist.
#[tokio::test]
async fn delete_cluster_resolves_the_cluster_before_what_is_asked_of_it() {
    let (router, cluster) = seeded().await;

    // The rows differ in what accompanies the missing name: nothing, each member this
    // operation also takes, and both of them at once.
    let rows: Vec<(&str, Value)> = vec![
        ("the name alone", json!({ "ClusterName": ABSENT })),
        (
            "with a final snapshot",
            json!({ "ClusterName": ABSENT, "FinalSnapshotName": "farewell" }),
        ),
        (
            "with a multi-Region cluster",
            json!({ "ClusterName": ABSENT, "MultiRegionClusterName": "global" }),
        ),
        (
            "with both",
            json!({
                "ClusterName": ABSENT,
                "FinalSnapshotName": "farewell",
                "MultiRegionClusterName": "global",
            }),
        ),
        (
            // A name is a case-insensitive identifier, so the spelling does not change
            // which fault answers.
            "in another letter case",
            json!({ "ClusterName": ABSENT.to_uppercase(), "FinalSnapshotName": "farewell" }),
        ),
    ];
    for (label, body) in rows {
        let case = format!("missing cluster {label}");
        refused(
            &router,
            "DeleteCluster",
            &body,
            "ClusterNotFoundFault",
            None,
            &case,
        )
        .await;
    }

    // For a cluster that DOES exist the member this slice cannot honour is refused — so
    // the rows above are about the order of the two checks, not about one of them being
    // gone — and the refused delete leaves the cluster in place.
    let case = "a stored cluster with a final snapshot";
    refused(
        &router,
        "DeleteCluster",
        &json!({ "ClusterName": cluster, "FinalSnapshotName": "farewell" }),
        "InvalidParameterValueException",
        None,
        case,
    )
    .await;
    let described = ok(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": cluster }),
        case,
    )
    .await;
    assert_eq!(
        described["Clusters"][0]["Name"],
        json!(cluster),
        "case {case:?}: a refused delete must leave the cluster in place, got {described}"
    );
}

/// `CreateCluster` judges the VALUES it was given before it resolves the resources the
/// request names: an invalid `NodeType` is refused even when the `ACLName` names no ACL,
/// and a node type the service DOES offer gets past that check to the ACL.
#[tokio::test]
async fn create_cluster_judges_its_values_before_the_resources_it_names() {
    let router = router();

    // Node types no MemoryDB cluster runs: the ElastiCache spelling of a real one, a
    // size the family is not offered in, a family nothing publishes, each half of the
    // `db.<family>.<size>` form on its own, and a name that is not of the form at all.
    let invalid: [&str; 7] = [
        "cache.t4g.small",
        "db.t4g.nano",
        "db.no-such-family.large",
        "db.t4g",
        "t4g.small",
        "not-a-node-type",
        "",
    ];
    for node_type in invalid {
        let case = format!("NodeType {node_type:?} with a missing ACL");
        refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": "value-first",
                "NodeType": node_type,
                "ACLName": ABSENT,
            }),
            "InvalidParameterValueException",
            Some(INVALID_NODE_TYPE),
            &case,
        )
        .await;
    }

    // Node types MemoryDB publishes (AWS MemoryDB "Supported node types": the burstable
    // t4g family and the general-purpose / memory-optimized families, including the
    // data-tiering r6gd one) reach the ACL lookup instead, which is the proof that the
    // rows above were refused on the node type and not on the ACL.
    let valid: [&str; 6] = [
        "db.t4g.small",
        "db.t4g.medium",
        "db.r6g.large",
        "db.r6g.4xlarge",
        "db.m7g.16xlarge",
        "db.r6gd.xlarge",
    ];
    for node_type in valid {
        let case = format!("NodeType {node_type:?} with a missing ACL");
        refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": "value-first",
                "NodeType": node_type,
                "ACLName": ABSENT,
            }),
            "ACLNotFoundFault",
            None,
            &case,
        )
        .await;
        // And the same node type with an ACL that exists is accepted, so the catalog
        // above is what the service really offers rather than a list nothing passes.
        let name = format!("ok-{}", node_type.replace('.', "-"));
        let created = ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": name,
                "NodeType": node_type,
                "ACLName": OPEN_ACCESS_ACL,
            }),
            &case,
        )
        .await;
        assert_eq!(
            created["Cluster"]["NodeType"],
            json!(node_type),
            "case {case:?}: the created cluster must report the node type it was given, \
             got {created}"
        );
    }

    // The other references this create carries are resolved before the ACL too: each
    // names a resource this store does not hold, so each answers its own fault rather
    // than the ACL's.
    let references: [(&str, Value, &str); 2] = [
        (
            "a snapshot that does not exist",
            json!({
                "ClusterName": "from-snapshot",
                "NodeType": NODE_TYPE,
                "ACLName": ABSENT,
                "SnapshotName": "no-such-snapshot",
            }),
            "InvalidParameterValueException",
        ),
        (
            "a multi-Region cluster that does not exist",
            json!({
                "ClusterName": "joins-global",
                "NodeType": NODE_TYPE,
                "ACLName": ABSENT,
                "MultiRegionClusterName": "no-such-global-cluster",
            }),
            "MultiRegionClusterNotFoundFault",
        ),
    ];
    for (label, body, code) in references {
        refused(&router, "CreateCluster", &body, code, None, label).await;
    }
}

/// An engine that does not replicate can back no MemoryDB cluster — every one of them is
/// a replicated cluster — which is the modelled `InvalidParameterCombinationException`,
/// while an engine the service simply does not run is the modelled
/// `InvalidParameterValueException`. Both write paths that take an `Engine` judge it the
/// same way.
#[tokio::test]
async fn an_engine_that_does_not_replicate_is_a_parameter_combination_fault() {
    let (router, cluster) = seeded().await;

    // (the engine, the code, the sentence). The rows differ along the reason the engine
    // cannot be used: it does not replicate, it is not an engine this service runs, or
    // it is a published engine in a letter case that is not the published one — `Engine`
    // is an enum-like value rather than a resource name, so it is not folded.
    let rows: [(&str, &str, Option<&str>); 4] = [
        (
            "memcached",
            "InvalidParameterCombinationException",
            Some(NO_REPLICATION),
        ),
        ("no-such-engine", "InvalidParameterValueException", None),
        ("REDIS", "InvalidParameterValueException", None),
        ("Valkey", "InvalidParameterValueException", None),
    ];
    for (engine, code, message) in rows {
        let case = format!("CreateCluster with engine {engine:?}");
        refused(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": "engine-judged",
                "NodeType": NODE_TYPE,
                "ACLName": OPEN_ACCESS_ACL,
                "Engine": engine,
            }),
            code,
            message,
            &case,
        )
        .await;
        let case = format!("UpdateCluster with engine {engine:?}");
        refused(
            &router,
            "UpdateCluster",
            &json!({ "ClusterName": cluster, "Engine": engine }),
            code,
            message,
            &case,
        )
        .await;
    }

    // The engines this service DOES run are accepted on both paths, so what the rows
    // above refuse is the engine and not the member.
    for engine in ["redis", "valkey"] {
        let case = format!("engine {engine:?}");
        ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": format!("engine-{engine}"),
                "NodeType": NODE_TYPE,
                "ACLName": OPEN_ACCESS_ACL,
                "Engine": engine,
            }),
            &case,
        )
        .await;
    }

    // A node type the service does not offer is refused on the update path too, through
    // the same rule the create path reads.
    refused(
        &router,
        "UpdateCluster",
        &json!({ "ClusterName": cluster, "NodeType": "db.t4g.nano" }),
        "InvalidParameterValueException",
        Some(INVALID_NODE_TYPE),
        "UpdateCluster with a node type the service does not offer",
    )
    .await;
}

/// `DescribeEvents` and `DescribeEngineVersions` classify their refusals the same way:
/// a value the service cannot serve is `InvalidParameterValueException`, while members
/// that cannot hold together — a window stated twice or reaching past the history the
/// service keeps — are `InvalidParameterCombinationException`.
#[tokio::test]
async fn a_listing_refusal_states_whether_the_value_or_the_combination_is_wrong() {
    let router = router();
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("the clock is at or after the epoch")
            .as_secs(),
    )
    .expect("the current epoch second fits in an i64");
    // 14 days, from `DescribeEvents` — "you can retrieve up to 14 days' worth of events
    // if necessary" (mirrors `crate::state::EVENT_RETENTION`), and a few seconds of
    // slack so a bound is decidably inside or past the floor the SERVICE computes.
    let retention = 14 * 24 * 60 * 60;
    let slack = 5;

    let events: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "a window reaching past the retention floor",
            json!({ "StartTime": now - retention - slack }),
            "InvalidParameterCombinationException",
            Some("Events occurring more than 14 days in the past are not available."),
        ),
        (
            "a Duration reaching past the retention floor",
            json!({ "Duration": 14 * 24 * 60 + 1 }),
            "InvalidParameterCombinationException",
            Some("Events occurring more than 14 days in the past are not available."),
        ),
        (
            "a window stated twice",
            json!({ "Duration": 60, "StartTime": now - 3_600 }),
            "InvalidParameterCombinationException",
            None,
        ),
        (
            "an interval that ends before it begins",
            json!({ "StartTime": now, "EndTime": now - 3_600 }),
            "InvalidParameterCombinationException",
            None,
        ),
        (
            "a source identifier with no source type",
            json!({ "SourceName": "anything" }),
            "InvalidParameterCombinationException",
            None,
        ),
        (
            "a Duration that is not positive",
            json!({ "Duration": 0 }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "a window starting in the future",
            json!({ "StartTime": now + 3_600 }),
            "InvalidParameterValueException",
            None,
        ),
        (
            "a page size outside the window",
            json!({ "MaxResults": 0 }),
            "InvalidParameterValueException",
            None,
        ),
    ];
    for (label, body, code, message) in events {
        let case = format!("DescribeEvents with {label}");
        refused(&router, "DescribeEvents", &body, code, message, &case).await;
    }

    let versions: Vec<(&str, Value, &str, Option<&str>)> = vec![
        (
            "an engine it does not run",
            json!({ "Engine": "no-such-engine" }),
            "InvalidParameterCombinationException",
            Some("Invalid engine: no-such-engine"),
        ),
        (
            "a version no engine publishes",
            json!({ "EngineVersion": "99.99" }),
            "InvalidParameterValueException",
            Some("Invalid engine version or parameter group family was provided."),
        ),
        (
            "a parameter group family nothing publishes",
            json!({ "ParameterGroupFamily": "memorydb_no-such-family" }),
            "InvalidParameterValueException",
            Some("Invalid engine version or parameter group family was provided."),
        ),
        (
            "a published engine with another engine's version",
            json!({ "Engine": "redis", "EngineVersion": "7.2" }),
            "InvalidParameterValueException",
            Some("Invalid engine version or parameter group family was provided."),
        ),
    ];
    for (label, body, code, message) in versions {
        let case = format!("DescribeEngineVersions with {label}");
        refused(
            &router,
            "DescribeEngineVersions",
            &body,
            code,
            message,
            &case,
        )
        .await;
    }
}

/// `UpdateUser` naming no field to change is refused as an invalid parameter VALUE —
/// the request carries no update at all — while a request naming any one of the members
/// the model declares is served.
#[tokio::test]
async fn an_update_user_with_no_field_to_change_is_an_invalid_parameter_value() {
    let router = router();
    ok(
        &router,
        "CreateUser",
        &json!({
            "UserName": "updated-user",
            "AccessString": "on ~* &* +@all",
            "AuthenticationMode": { "Type": "password", "Passwords": ["Aa1!Aa1!Aa1!Aa1!"] },
        }),
        "seed user",
    )
    .await;

    // The name alone, and the name beside a member this operation does NOT change, are
    // both requests that name no field to update.
    for (label, body) in [
        ("the name alone", json!({ "UserName": "updated-user" })),
        (
            "the name in another letter case",
            json!({ "UserName": "UPDATED-USER" }),
        ),
    ] {
        let case = format!("UpdateUser with {label}");
        refused(
            &router,
            "UpdateUser",
            &body,
            "InvalidParameterValueException",
            Some(EMPTY_USER_UPDATE),
            &case,
        )
        .await;
    }

    // Each member the model declares beside the name is a field to update, so a request
    // carrying any one of them — or both — is served.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "an access string",
            json!({ "UserName": "updated-user", "AccessString": "on ~* +@read" }),
        ),
        (
            "an authentication mode",
            json!({
                "UserName": "updated-user",
                "AuthenticationMode": { "Type": "iam" },
            }),
        ),
        (
            "both",
            json!({
                "UserName": "updated-user",
                "AccessString": "on ~* &* +@all",
                "AuthenticationMode": {
                    "Type": "password",
                    "Passwords": ["Bb2!Bb2!Bb2!Bb2!"],
                },
            }),
        ),
    ];
    for (label, body) in accepted {
        let case = format!("UpdateUser with {label}");
        // The user settles back to `active` between the rows, so each update is judged
        // against a settled user rather than refused for its state.
        std::thread::sleep(std::time::Duration::from_millis(700));
        let updated = ok(&router, "UpdateUser", &body, &case).await;
        assert_eq!(
            updated["User"]["Name"],
            json!("updated-user"),
            "case {case:?}: expected the updated user, got {updated}"
        );
    }

    // A user the store does not hold is answered not-found whatever the request asks of
    // it: the user is resolved before what is asked of it, so an empty update on a
    // missing user is still `UserNotFoundFault`.
    refused(
        &router,
        "UpdateUser",
        &json!({ "UserName": ABSENT }),
        "UserNotFoundFault",
        None,
        "UpdateUser naming no user and no field",
    )
    .await;
}
