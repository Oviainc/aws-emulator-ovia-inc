// SPDX-License-Identifier: CC0-1.0

//! Resource tagging, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`]
//! the production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router_and_engine`, `call`, `call_in`, `ok`) so
//! there is one wire harness for this crate.
//!
//! The assertions are RULES over a table of inputs, not a replay of one recorded call:
//!
//! * the tags a resource reports are exactly the keys `TagResource` merged into it minus
//!   the keys `UntagResource` removed, a repeated key taking the newer value, and
//!   removing a key the resource does not carry is a success that changes nothing;
//! * a tagging call whose `resourceArn` names no resource this account and Region holds
//!   is refused `ResourceNotFoundException` (HTTP 404), naming the ARN asked for and the
//!   resource type it addresses;
//! * a merge that would take a resource past the maximum number of tags it can report is
//!   refused `ServiceQuotaExceededException` (HTTP 402) and writes nothing.
//!
//! The rows vary the resource kind the ARN addresses, the number of tags, the key and
//! value lengths at their modelled bounds, which keys overlap an existing set, and why
//! an ARN resolves to nothing (nothing minted it, the wrong resource kind, the wrong
//! Region, the wrong account, a deleted resource, a resource kind no operation creates).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;

use crate::db_instance_lifecycle::{call, call_in, ok, router_and_engine, ACCOUNT, REGION};

/// A router whose account already holds the AWS Marketplace subscription a DB cluster is
/// gated behind, so a row can create one of every taggable resource kind. A buyer
/// subscribes in AWS Marketplace, out of band of every operation here.
fn tagging_router() -> MultiServiceRouter {
    let (router, engine) = router_and_engine();
    influxdb::state::subscribe_account_to_marketplace_offering(
        &*engine,
        influxdb::state::READ_REPLICAS_OFFERING,
    )
    .expect("the shared engine holds this service's store");
    router
}

/// An ARN of this service, as the modelled `Arn` pattern lays it out:
/// `arn:aws:timestream-influxdb:<region>:<account12>:<resource-type>/<id>`.
fn arn_for(region: &str, account: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:timestream-influxdb:{region}:{account}:{resource_type}/{id}")
}

/// A `tags` request member from a list of pairs.
fn tag_json(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A list of pairs as the map a read must report.
fn tag_set(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// `(key, value)` from string literals, for the tables below.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn list_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> BTreeMap<String, String> {
    let body = ok(
        case,
        call(router, "ListTagsForResource", json!({ "resourceArn": arn })).await,
    );
    let tags = body["tags"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected a `tags` map on the answer, got {body}")
    });
    tags.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a string tag value, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// `TagResource` for `arn`, answered as `(status, body)`.
async fn tag(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) -> (u16, Value) {
    call(
        router,
        "TagResource",
        json!({ "resourceArn": arn, "tags": tag_json(tags) }),
    )
    .await
}

/// `UntagResource` for `arn`, answered as `(status, body)`.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> (u16, Value) {
    call(
        router,
        "UntagResource",
        json!({ "resourceArn": arn, "tagKeys": keys }),
    )
    .await
}

/// The `arn` member of a create answer.
fn arn_of(case: &str, body: &Value) -> String {
    body["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string `arn`, got {body}"))
        .to_string()
}

/// Create a DB instance and answer its ARN. `tags` lands in the create request when it
/// is not empty, so a row can assert the tags a create stored are the resource's tags.
async fn create_db_instance(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({
        "name": name,
        "password": "supersecret1",
        "dbInstanceType": "db.influx.medium",
        "allocatedStorage": 20,
        "vpcSubnetIds": ["subnet-aaa"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    });
    if !tags.is_empty() {
        body["tags"] = tag_json(tags);
    }
    let created = ok(name, call(router, "CreateDbInstance", body).await);
    arn_of(name, &created)
}

/// Create a DB cluster and answer its ARN, read back through `GetDbCluster` — the create
/// answers only the cluster's id.
async fn create_db_cluster(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({
        "name": name,
        "dbInstanceType": "db.influx.medium",
        "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    });
    if !tags.is_empty() {
        body["tags"] = tag_json(tags);
    }
    let created = ok(name, call(router, "CreateDbCluster", body).await);
    let id = created["dbClusterId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: expected a string `dbClusterId`, got {created}"))
        .to_string();
    let record = ok(
        name,
        call(router, "GetDbCluster", json!({ "dbClusterId": id })).await,
    );
    arn_of(name, &record)
}

/// Create a DB parameter group and answer its ARN.
async fn create_db_parameter_group(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let mut body = json!({ "name": name });
    if !tags.is_empty() {
        body["tags"] = tag_json(tags);
    }
    let created = ok(name, call(router, "CreateDbParameterGroup", body).await);
    arn_of(name, &created)
}

/// One of every resource kind this service's `Arn` addresses and an operation creates,
/// each paired with the label the assertions name it by. `suffix` keeps the names unique
/// per row (a name is unique per customer and Region).
async fn taggable_resources(
    router: &MultiServiceRouter,
    suffix: &str,
) -> Vec<(&'static str, String)> {
    vec![
        (
            "db instance",
            create_db_instance(router, &format!("tagged-instance-{suffix}"), &[]).await,
        ),
        (
            "db cluster",
            create_db_cluster(router, &format!("tagged-cluster-{suffix}"), &[]).await,
        ),
        (
            "db parameter group",
            create_db_parameter_group(router, &format!("tagged-group-{suffix}"), &[]).await,
        ),
    ]
}

/// One row of the tagging table: the tags a request carries.
struct TagCase {
    label: &'static str,
    tags: Vec<(String, String)>,
}

/// The tag sets the rule has to hold over. The key and value lengths come from
/// `RequestTagMap`: its key targets `TagKey` (`@length 1..128`) and its value `TagValue`
/// (`@length 0..256`) — `smithy_query(query="shape", shape="RequestTagMap")`.
fn tag_cases() -> Vec<TagCase> {
    vec![
        TagCase {
            label: "a single tag",
            tags: vec![pair("env", "integration")],
        },
        TagCase {
            label: "several tags",
            tags: vec![
                pair("env", "production"),
                pair("team", "timestream"),
                pair("cost-center", "4711"),
            ],
        },
        TagCase {
            label: "a key at the modelled minimum length 1 and a value at the minimum 0",
            tags: vec![pair("k", "")],
        },
        TagCase {
            label: "a key at the modelled maximum length 128 and a value at the maximum 256",
            tags: vec![pair(&"k".repeat(128), &"v".repeat(256))],
        },
        TagCase {
            label: "twelve generated tags",
            tags: (0..12)
                .map(|n| pair(&format!("key-{n:02}"), &format!("value-{n:02}")))
                .collect(),
        },
    ]
}

#[tokio::test]
async fn a_resource_reports_exactly_the_tags_merged_into_it_minus_those_removed() {
    for (row, case) in tag_cases().into_iter().enumerate() {
        let router = tagging_router();
        for (kind, arn) in taggable_resources(&router, &format!("r{row}")).await {
            let case_label = format!("{} / {kind}", case.label);

            // A resource nobody has tagged reports an empty tag set — which is how a
            // caller tells "no tags" from "no such resource".
            let initial = list_tags(&router, &case_label, &arn).await;
            assert!(
                initial.is_empty(),
                "case {case_label:?}: a fresh resource carries no tags, got {initial:?}"
            );

            // Every key the request carries is attached.
            let (status, body) = tag(&router, &arn, &case.tags).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200 from TagResource, got {status} with {body}"
            );
            let after_tag = list_tags(&router, &case_label, &arn).await;
            assert_eq!(
                after_tag,
                tag_set(&case.tags),
                "case {case_label:?}: expected the merged tags, got {after_tag:?}"
            );

            // A second merge overwrites the key it repeats and adds the key it brings.
            let first_key = case.tags[0].0.clone();
            let overlapping = vec![pair(&first_key, "overwritten"), pair("added", "later")];
            let (status, body) = tag(&router, &arn, &overlapping).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200 from the second TagResource, got \
                 {status} with {body}"
            );
            let mut expected = tag_set(&case.tags);
            expected.extend(tag_set(&overlapping));
            let after_merge = list_tags(&router, &case_label, &arn).await;
            assert_eq!(
                after_merge, expected,
                "case {case_label:?}: expected the repeated key to take the newer value, got \
                 {after_merge:?}"
            );

            // Removing a key the resource carries drops it; removing one it does not
            // carry is a success that changes nothing.
            let (status, body) = untag(&router, &arn, &[first_key.as_str(), "no-such-key"]).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200 from UntagResource, got {status} with \
                 {body}"
            );
            expected.remove(&first_key);
            let after_untag = list_tags(&router, &case_label, &arn).await;
            assert_eq!(
                after_untag, expected,
                "case {case_label:?}: expected only the named key removed, got {after_untag:?}"
            );

            // Removing every remaining key leaves the resource with none.
            let remaining: Vec<&str> = expected.keys().map(String::as_str).collect();
            let (status, body) = untag(&router, &arn, &remaining).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200 removing every key, got {status} with \
                 {body}"
            );
            let emptied = list_tags(&router, &case_label, &arn).await;
            assert!(
                emptied.is_empty(),
                "case {case_label:?}: expected no tags left, got {emptied:?}"
            );

            // And a key removed twice is still not an error.
            let (status, body) = untag(&router, &arn, &[first_key.as_str()]).await;
            assert_eq!(
                status, 200,
                "case {case_label:?}: expected HTTP 200 removing an absent key, got {status} \
                 with {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_tags_a_create_request_carried_are_the_resources_tags() {
    let router = tagging_router();
    let tags = vec![pair("env", "integration"), pair("owner", "platform")];

    let created: Vec<(&str, String)> = vec![
        (
            "db instance",
            create_db_instance(&router, "created-with-tags", &tags).await,
        ),
        (
            "db cluster",
            create_db_cluster(&router, "created-tagged-cluster", &tags).await,
        ),
        (
            "db parameter group",
            create_db_parameter_group(&router, "created-tagged-group", &tags).await,
        ),
    ];

    for (kind, arn) in created {
        let listed = list_tags(&router, kind, &arn).await;
        assert_eq!(
            listed,
            tag_set(&tags),
            "case {kind:?}: a create's tags are the resource's tags, got {listed:?}"
        );

        // The tagging operations reach that same set, rather than a second copy beside it.
        let (status, body) = untag(&router, &arn, &["owner"]).await;
        assert_eq!(
            status, 200,
            "case {kind:?}: expected HTTP 200 from UntagResource, got {status} with {body}"
        );
        let listed = list_tags(&router, kind, &arn).await;
        assert_eq!(
            listed,
            tag_set(&tags[..1]),
            "case {kind:?}: expected the create-time tag set minus the removed key, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_not_found_and_changes_nothing() {
    let router = tagging_router();
    let live_arn = create_db_instance(&router, "live-instance", &[]).await;
    let live_id = live_arn
        .rsplit('/')
        .next()
        .expect("the ARN carries an id")
        .to_string();
    let tags = vec![pair("env", "integration")];
    ok("the live instance is tagged", tag(&router, &live_arn, &tags).await);

    // A resource created and then deleted: its record, and with it its tags, is gone.
    let deleted_arn = create_db_instance(&router, "deleted-instance", &[]).await;
    let deleted_id = deleted_arn
        .rsplit('/')
        .next()
        .expect("the ARN carries an id")
        .to_string();
    ok(
        "the instance is deleted",
        call(
            &router,
            "DeleteDbInstance",
            json!({ "identifier": deleted_id }),
        )
        .await,
    );

    // An id as the modelled `Arn` pattern allows one: `[a-zA-Z0-9]{3,64}`.
    let unminted = "z".repeat(20);
    // `resourceType` values, from this service's refusals: the four resource kinds the
    // modelled `Arn` pattern addresses.
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a db-instance ARN nothing minted",
            arn_for(REGION, ACCOUNT, "db-instance", &unminted),
            "DB_INSTANCE",
        ),
        (
            "a db-cluster ARN nothing minted",
            arn_for(REGION, ACCOUNT, "db-cluster", &unminted),
            "DB_CLUSTER",
        ),
        (
            "a db-parameter-group ARN nothing minted",
            arn_for(REGION, ACCOUNT, "db-parameter-group", &unminted),
            "DB_PARAMETER_GROUP",
        ),
        (
            "a db-backup ARN, a resource kind no operation creates yet",
            arn_for(REGION, ACCOUNT, "db-backup", &unminted),
            "DB_BACKUP",
        ),
        (
            "the live instance's id under another resource kind",
            arn_for(REGION, ACCOUNT, "db-cluster", &live_id),
            "DB_CLUSTER",
        ),
        (
            "the live instance's ARN in another Region",
            arn_for("eu-central-1", ACCOUNT, "db-instance", &live_id),
            "DB_INSTANCE",
        ),
        (
            "the live instance's ARN under another account",
            arn_for(REGION, "123456789012", "db-instance", &live_id),
            "DB_INSTANCE",
        ),
        (
            "a deleted instance's ARN",
            deleted_arn.clone(),
            "DB_INSTANCE",
        ),
        (
            // A service-managed parameter group is the service's own catalog entry, not
            // an account resource: it has no ARN, so no ARN addresses it.
            "a service-managed parameter group's name as the id",
            arn_for(REGION, ACCOUNT, "db-parameter-group", "InfluxDBV3Core"),
            "DB_PARAMETER_GROUP",
        ),
    ];

    for (case, arn, resource_type) in cases {
        let answers = vec![
            ("TagResource", tag(&router, &arn, &tags).await),
            ("UntagResource", untag(&router, &arn, &["env"]).await),
            (
                "ListTagsForResource",
                call(
                    &router,
                    "ListTagsForResource",
                    json!({ "resourceArn": &arn }),
                )
                .await,
            ),
        ];
        for (operation, (status, body)) in answers {
            assert_eq!(
                status, 404,
                "case {case:?} / {operation}: expected HTTP 404, got {status} with {body}"
            );
            assert!(
                body["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with("ResourceNotFoundException"),
                "case {case:?} / {operation}: expected a ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(arn.as_str()),
                "case {case:?} / {operation}: the refusal must name the ARN asked for, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(resource_type),
                "case {case:?} / {operation}: expected the resource type the ARN addresses, got \
                 {body}"
            );
        }
    }

    // None of those refusals touched the resource that does exist.
    let still_there = list_tags(&router, "the live instance survives", &live_arn).await;
    assert_eq!(
        still_there,
        tag_set(&tags),
        "case \"the live instance survives\": expected its tags unchanged, got {still_there:?}"
    );
}

#[tokio::test]
async fn an_arn_from_another_region_resolves_in_the_region_that_minted_it() {
    let router = tagging_router();
    // A name is unique per customer and Region, so the same name in two Regions is two
    // resources with two ARNs; each one's tags belong to it alone.
    let name = "cross-region-instance";
    let west = create_db_instance(&router, name, &[]).await;
    let created = ok(
        "the eu-central-1 instance",
        call_in(
            &router,
            "eu-central-1",
            "CreateDbInstance",
            json!({
                "name": name,
                "password": "supersecret1",
                "dbInstanceType": "db.influx.medium",
                "allocatedStorage": 20,
                "vpcSubnetIds": ["subnet-aaa"],
                "vpcSecurityGroupIds": ["sg-aaa"],
            }),
        )
        .await,
    );
    let central = arn_of("the eu-central-1 instance", &created);
    assert_ne!(
        west, central,
        "case \"two Regions\": expected two ARNs, got {west} twice"
    );

    ok(
        "the us-west-2 instance is tagged",
        tag(&router, &west, &[pair("region", "west")]).await,
    );

    let west_tags = list_tags(&router, "the us-west-2 instance", &west).await;
    assert_eq!(
        west_tags,
        tag_set(&[pair("region", "west")]),
        "case \"two Regions\": expected the tagged instance to carry it, got {west_tags:?}"
    );
    let central_tags = list_tags(&router, "the eu-central-1 instance", &central).await;
    assert!(
        central_tags.is_empty(),
        "case \"two Regions\": expected the other Region's instance untouched, got \
         {central_tags:?}"
    );
}

#[tokio::test]
async fn a_merge_past_the_maximum_number_of_tags_is_refused_and_writes_nothing() {
    let router = tagging_router();
    let arn = create_db_instance(&router, "tag-quota-instance", &[]).await;
    let bound = influxdb::state::MAX_TAGS_PER_RESOURCE;

    // AT the bound: as many tags as a resource can report, in one request (the modelled
    // `RequestTagMap` carries up to 200 of them).
    let full: Vec<(String, String)> = (0..bound)
        .map(|n| pair(&format!("key-{n:03}"), &format!("value-{n:03}")))
        .collect();
    let (status, body) = tag(&router, &arn, &full).await;
    assert_eq!(
        status, 200,
        "case \"at the bound of {bound} tags\": expected HTTP 200, got {status} with {body}"
    );
    let stored = list_tags(&router, "at the bound", &arn).await;
    assert_eq!(
        stored.len(),
        bound,
        "case \"at the bound of {bound} tags\": expected {bound} stored tags, got {}",
        stored.len()
    );

    // A key the resource already carries is not a new tag, so re-merging it stays at the
    // bound and is accepted.
    let (status, body) = tag(&router, &arn, &full[..1]).await;
    assert_eq!(
        status, 200,
        "case \"re-merging a key already carried\": expected HTTP 200, got {status} with {body}"
    );

    // One PAST the bound is refused, and the refusal writes nothing.
    let (status, body) = tag(&router, &arn, &[pair("one-too-many", "x")]).await;
    assert_eq!(
        status, 402,
        "case \"one tag past the bound of {bound}\": expected HTTP 402, got {status} with {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ServiceQuotaExceededException"),
        "case \"one tag past the bound of {bound}\": expected a ServiceQuotaExceededException, \
         got {body}"
    );
    let unchanged = list_tags(&router, "after the refusal", &arn).await;
    assert_eq!(
        unchanged, stored,
        "case \"one tag past the bound of {bound}\": expected the stored tags unchanged, got {} \
         tags",
        unchanged.len()
    );

    // Freeing a key makes room for one again.
    ok(
        "a key is removed",
        untag(&router, &arn, &["key-000"]).await,
    );
    let (status, body) = tag(&router, &arn, &[pair("one-too-many", "x")]).await;
    assert_eq!(
        status, 200,
        "case \"a freed key makes room\": expected HTTP 200, got {status} with {body}"
    );
    let after = list_tags(&router, "a freed key makes room", &arn).await;
    assert_eq!(
        after.len(),
        bound,
        "case \"a freed key makes room\": expected {bound} stored tags, got {}",
        after.len()
    );
    assert!(
        after.contains_key("one-too-many") && !after.contains_key("key-000"),
        "case \"a freed key makes room\": expected the new key in place of the freed one, got \
         {after:?}"
    );
}

#[tokio::test]
async fn reading_tags_does_not_change_them() {
    let router = tagging_router();
    let arn = create_db_instance(&router, "read-twice-instance", &[]).await;
    let tags = vec![pair("env", "integration"), pair("team", "timestream")];
    ok("the instance is tagged", tag(&router, &arn, &tags).await);

    let first = list_tags(&router, "the first read", &arn).await;
    let second = list_tags(&router, "the second read", &arn).await;
    assert_eq!(
        first,
        tag_set(&tags),
        "case \"the first read\": expected the stored tags, got {first:?}"
    );
    assert_eq!(
        second, first,
        "case \"the second read\": expected the same tags, got {second:?}"
    );
}
