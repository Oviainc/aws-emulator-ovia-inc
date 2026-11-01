// SPDX-License-Identifier: CC0-1.0

//! Container-instance registration, listing and describing, driven end to end over the
//! shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test:
//!
//! * `RegisterContainerInstance` stores the instance the agent reports and
//!   `ListContainerInstances` answers with the ARNs of the instances STORED for the
//!   named cluster, filtered by status and paged in a stable order.
//! * `ListContainerInstances$status`: "Filters the container instances by status. ... If
//!   you don't specify this parameter, the default is to include container instances set
//!   to all states other than `INACTIVE`."
//! * `ListContainerInstances$maxResults`: "This value can be between 1 and 100."
//! * `Cluster$registeredContainerInstancesCount`: "The number of container instances
//!   registered into the cluster."

use std::collections::HashSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    count, create_cluster, identity_document, list, ok, refused, register_instance, service,
    string, ACCOUNT, SIGNED_REGION,
};

/// Every value `ContainerInstanceStatus` takes, from the model's enum (smithy_query
/// query="shape" shape="ContainerInstanceStatus": ACTIVE, DRAINING, REGISTERING,
/// DEREGISTERING, REGISTRATION_FAILED). `INACTIVE` is a `ContainerInstance$status` the
/// request member cannot carry, which is why the default filter is expressed as "all
/// states other than INACTIVE".
const CONTAINER_INSTANCE_STATUSES: [&str; 5] = [
    "ACTIVE",
    "DRAINING",
    "REGISTERING",
    "DEREGISTERING",
    "REGISTRATION_FAILED",
];

/// One row of the registration table: the case name, the EC2 instance id the identity
/// document names, the `totalResources`, the `attributes` and the `tags` the agent
/// reports.
type RegistrationRow = (&'static str, &'static str, Value, Value, Value);

/// One row of the describe table: the case name, the ids the request names, the ARNs
/// expected back and the ARNs expected as `MISSING` failures.
type DescribeRow = (&'static str, Vec<String>, Vec<String>, Vec<String>);

/// The ARNs `ListContainerInstances` answers with for one request.
async fn list_instances(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let body = ok(router, "ListContainerInstances", request, case).await;
    list(&body, "containerInstanceArns", case)
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// A `Resource` reduced to what the request asked for: its name, its type and the one
/// value member that type carries (`Resource$type`: "Valid values: INTEGER, DOUBLE,
/// LONG, or STRINGSET").
fn resource_summary(resource: &Value) -> (String, String, String) {
    let name = resource["name"].as_str().unwrap_or_default().to_string();
    let kind = resource["type"].as_str().unwrap_or_default().to_string();
    let value = match kind.as_str() {
        "INTEGER" => resource["integerValue"].to_string(),
        "LONG" => resource["longValue"].to_string(),
        "DOUBLE" => resource["doubleValue"].to_string(),
        "STRINGSET" => resource["stringSetValue"].to_string(),
        _ => String::new(),
    };
    (name, kind, value)
}

/// Every resource of a list, reduced by [`resource_summary`] and sorted so the
/// comparison does not depend on the order the map walked.
fn resource_summaries(resources: &[Value]) -> Vec<(String, String, String)> {
    let mut summaries: Vec<_> = resources.iter().map(resource_summary).collect();
    summaries.sort();
    summaries
}

/// The last segment of a container-instance ARN.
fn arn_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or_default()
}

#[tokio::test]
async fn registered_container_instances_round_trip_through_list_and_describe() {
    let router = service();
    create_cluster(&router, "demo", "round trip").await;

    // The rows differ along what registration generalises over: the EC2 instance the
    // identity document names, how many resources it reported and of which modelled
    // types, how many attributes it carries, and whether it is tagged.
    let cases: [RegistrationRow; 4] = [
        (
            "ec2 instance with cpu, memory and ports",
            "i-0abc1234def567890",
            json!([
                { "name": "CPU", "type": "INTEGER", "integerValue": 2048 },
                { "name": "MEMORY", "type": "INTEGER", "integerValue": 3955 },
                { "name": "PORTS", "type": "STRINGSET", "stringSetValue": ["22", "2376", "51678"] },
            ]),
            json!([
                { "name": "ecs.instance-type", "value": "m5.large" },
                { "name": "ecs.os-type", "value": "linux" },
            ]),
            json!([{ "key": "fleet", "value": "batch" }]),
        ),
        (
            "one resource and an external capability attribute",
            "i-3333333333333333b",
            json!([{ "name": "CPU", "type": "INTEGER", "integerValue": 1024 }]),
            json!([{ "name": "ecs.capability.external" }]),
            json!([]),
        ),
        (
            "long and double resource types",
            "i-1111111111111111f",
            json!([
                { "name": "DISK", "type": "LONG", "longValue": 34359738368_i64 },
                { "name": "GPU_SHARE", "type": "DOUBLE", "doubleValue": 0.5 },
            ]),
            json!([]),
            json!([
                { "key": "fleet", "value": "gpu" },
                { "key": "owner", "value": "team-a" },
            ]),
        ),
        (
            "no resources and no attributes at all",
            "i-2222222222222222a",
            json!([]),
            json!([]),
            json!([]),
        ),
    ];

    let mut registered: Vec<String> = Vec::new();
    for (case, instance_id, resources, attributes, tags) in cases {
        let mut request = json!({
            "cluster": "demo",
            "totalResources": resources.clone(),
            "attributes": attributes.clone(),
            "tags": tags.clone(),
        });
        request["instanceIdentityDocument"] = json!(identity_document(instance_id));
        request["instanceIdentityDocumentSignature"] =
            json!(crate::ecs_wire::identity_document_signature());

        let body = ok(&router, "RegisterContainerInstance", request, case).await;
        let instance = &body["containerInstance"];

        let arn = string(instance, "containerInstanceArn", case);
        let id = arn_id(&arn);
        assert_eq!(
            arn,
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/demo/{id}"),
            "case {case:?}: unexpected containerInstanceArn in {instance}"
        );
        assert!(
            id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "case {case:?}: expected a 32-hex container-instance id, got {id:?}"
        );
        assert!(
            !registered.contains(&arn),
            "case {case:?}: every registration mints a distinct ARN, got {arn:?} twice"
        );

        // "For Amazon EC2 instances, this value is the Amazon EC2 instance ID"
        let ec2_instance_id = string(instance, "ec2InstanceId", case);
        assert_eq!(
            ec2_instance_id, instance_id,
            "case {case:?}: expected the identity document's instanceId, got {instance}"
        );

        assert_eq!(
            string(instance, "status", case),
            "ACTIVE",
            "case {case:?}: a registered instance accepts tasks, so it is ACTIVE: {instance}"
        );
        assert_eq!(
            instance["agentConnected"], json!(true),
            "case {case:?}: the agent that registered the instance is connected: {instance}"
        );
        assert_eq!(
            count(instance, "version", case),
            0,
            "case {case:?}: a first registration is version 0, got {instance}"
        );
        for counter in ["runningTasksCount", "pendingTasksCount"] {
            assert_eq!(
                count(instance, counter, case),
                0,
                "case {case:?}: nothing is placed on a new instance, so {counter} is 0: {instance}"
            );
        }
        assert!(
            instance["registeredAt"].is_number(),
            "case {case:?}: expected a registeredAt timestamp, got {instance}"
        );

        let requested = resources.as_array().cloned().unwrap_or_default();
        assert_eq!(
            resource_summaries(&list(instance, "registeredResources", case)),
            resource_summaries(&requested),
            "case {case:?}: registeredResources must report what the agent registered: {instance}"
        );
        assert_eq!(
            resource_summaries(&list(instance, "remainingResources", case)),
            resource_summaries(&requested),
            "case {case:?}: nothing is allocated out of a new instance, so the remainder is \
             the registered set: {instance}"
        );
        assert_eq!(
            list(instance, "attributes", case),
            attributes.as_array().cloned().unwrap_or_default(),
            "case {case:?}: expected the registered attributes back, got {instance}"
        );
        assert_eq!(
            list(instance, "tags", case),
            tags.as_array().cloned().unwrap_or_default(),
            "case {case:?}: expected the registered tags back, got {instance}"
        );

        // Read back through DescribeContainerInstances, by ARN and by the short id:
        // "The container instance ID or full ARN of the container instance".
        for (reference_case, reference) in [("by arn", arn.clone()), ("by id", id.to_string())] {
            let described = ok(
                &router,
                "DescribeContainerInstances",
                json!({
                    "cluster": "demo",
                    "containerInstances": [reference],
                    "include": ["TAGS", "CONTAINER_INSTANCE_HEALTH"],
                }),
                case,
            )
            .await;
            assert!(
                list(&described, "failures", case).is_empty(),
                "case {case:?} {reference_case}: a stored instance is no failure: {described}"
            );
            let read = &list(&described, "containerInstances", case)[0];
            assert_eq!(
                string(read, "containerInstanceArn", case),
                arn,
                "case {case:?} {reference_case}: describe returned another instance: {read}"
            );
            assert_eq!(
                string(read, "ec2InstanceId", case),
                ec2_instance_id,
                "case {case:?} {reference_case}: describe must report the stored instance id: \
                 {read}"
            );
            assert_eq!(
                resource_summaries(&list(read, "registeredResources", case)),
                resource_summaries(&requested),
                "case {case:?} {reference_case}: describe must report the stored resources: {read}"
            );
            assert_eq!(
                list(read, "tags", case),
                tags.as_array().cloned().unwrap_or_default(),
                "case {case:?} {reference_case}: expected the stored tags back, got {read}"
            );
        }

        // "If this field is omitted, tags aren't included in the response."
        let narrow = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": "demo", "containerInstances": [arn.clone()] }),
            case,
        )
        .await;
        assert_eq!(
            list(&narrow, "containerInstances", case)[0]["tags"],
            Value::Null,
            "case {case:?}: tags are gated by include: {narrow}"
        );

        registered.push(arn);

        // Every registration is visible to the next caller, and the cluster counts it:
        // "This includes container instances in both ACTIVE and DRAINING status."
        let listed = list_instances(&router, json!({ "cluster": "demo" }), case).await;
        let mut want = registered.clone();
        want.sort();
        assert_eq!(
            listed, want,
            "case {case:?}: ListContainerInstances must answer with every stored instance"
        );
        let clusters = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": ["demo"] }),
            case,
        )
        .await;
        assert_eq!(
            count(
                &list(&clusters, "clusters", case)[0],
                "registeredContainerInstancesCount",
                case
            ),
            registered.len() as i64,
            "case {case:?}: the cluster counts the instances registered into it: {clusters}"
        );
    }
}

#[tokio::test]
async fn list_container_instances_filters_by_status_and_pages_over_stored_instances() {
    let router = service();
    create_cluster(&router, "demo", "filters").await;
    create_cluster(&router, "other", "filters").await;
    create_cluster(&router, "default", "filters").await;

    let mut demo: Vec<String> = Vec::new();
    for instance_id in ["i-000000000000000a1", "i-000000000000000b2", "i-000000000000000c3"] {
        demo.push(register_instance(&router, "demo", instance_id, "filters").await);
    }
    demo.sort();
    let other = register_instance(&router, "other", "i-000000000000000d4", "filters").await;
    let fallback = register_instance(&router, "default", "i-000000000000000e5", "filters").await;

    // The status filter spans the whole modelled catalog: a registered instance is
    // ACTIVE, so only that value selects it, and every other value selects nothing.
    for status in CONTAINER_INSTANCE_STATUSES {
        let case = format!("status {status}");
        let listed =
            list_instances(&router, json!({ "cluster": "demo", "status": status }), &case).await;
        let want: Vec<String> = if status == "ACTIVE" {
            demo.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            listed, want,
            "case {case:?}: expected {} ARNs for this status filter",
            want.len()
        );
    }

    // "If you don't specify this parameter, the default is to include container
    // instances set to all states other than INACTIVE."
    assert_eq!(
        list_instances(&router, json!({ "cluster": "demo" }), "no status").await,
        demo,
        "an unfiltered list holds every instance of the cluster"
    );

    // The cluster scopes the answer, and an absent `cluster` means the default one:
    // "If you do not specify a cluster, the default cluster is assumed."
    let scopes: [(&str, Value, Vec<String>); 4] = [
        ("cluster demo", json!({ "cluster": "demo" }), demo.clone()),
        ("cluster other", json!({ "cluster": "other" }), vec![other.clone()]),
        (
            "cluster by arn",
            json!({ "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/other") }),
            vec![other.clone()],
        ),
        ("absent cluster", json!({}), vec![fallback.clone()]),
    ];
    for (case, request, want) in scopes {
        assert_eq!(
            list_instances(&router, request, case).await,
            want,
            "case {case:?}: expected exactly the instances registered into that cluster"
        );
    }

    // Paging walks every instance exactly once, whatever the page size.
    for page_size in [1_i64, 2, 3, 100] {
        let case = format!("page size {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "cluster": "demo", "maxResults": page_size });
            if let Some(next) = &token {
                request["nextToken"] = json!(next);
            }
            let body = ok(&router, "ListContainerInstances", request, &case).await;
            let page = list(&body, "containerInstanceArns", &case);
            assert!(
                page.len() as i64 <= page_size,
                "case {case:?}: a page holds at most {page_size} entries, got {body}"
            );
            seen.extend(
                page.iter()
                    .map(|value| value.as_str().unwrap_or_default().to_string()),
            );
            match body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                // "This value is null when there are no more results to return."
                None => break,
            }
        }
        let unique: HashSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: paging must not repeat an instance, got {seen:?}"
        );
        seen.sort();
        assert_eq!(
            seen, demo,
            "case {case:?}: paging must visit every stored instance exactly once"
        );
    }

    // "This value can be between 1 and 100" — each published bound is accepted and one
    // step past it is refused.
    for max_results in [1_i64, 100] {
        let case = format!("maxResults {max_results}");
        ok(
            &router,
            "ListContainerInstances",
            json!({ "cluster": "demo", "maxResults": max_results }),
            &case,
        )
        .await;
    }
    for max_results in [0_i64, 101, -1] {
        let case = format!("maxResults {max_results}");
        refused(
            &router,
            "ListContainerInstances",
            json!({ "cluster": "demo", "maxResults": max_results }),
            "InvalidParameterException",
            &case,
        )
        .await;
    }

    // A cluster that was never created holds nothing to list: "The specified cluster
    // wasn't found."
    refused(
        &router,
        "ListContainerInstances",
        json!({ "cluster": "nosuchcluster" }),
        "ClusterNotFoundException",
        "unknown cluster",
    )
    .await;

    // `filter` carries a cluster query language statement, which nothing here
    // evaluates, so it is refused rather than silently ignored.
    refused(
        &router,
        "ListContainerInstances",
        json!({ "cluster": "demo", "filter": "attribute:ecs.instance-type == m5.large" }),
        "InvalidParameterException",
        "cluster query language filter",
    )
    .await;
}

#[tokio::test]
async fn registering_an_instance_again_updates_the_one_already_stored() {
    let router = service();
    create_cluster(&router, "demo", "re-registration").await;

    let document = identity_document("i-0f0f0f0f0f0f0f0f0");
    let signature = crate::ecs_wire::identity_document_signature();
    let first = ok(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": "demo",
            "instanceIdentityDocument": document,
            "instanceIdentityDocumentSignature": signature,
            "totalResources": [{ "name": "CPU", "type": "INTEGER", "integerValue": 1024 }],
        }),
        "first",
    )
    .await;
    let arn = string(&first["containerInstance"], "containerInstanceArn", "first");

    // The same machine registering again, and a registration naming the ARN it was
    // given, are both the SAME container instance: one more version of it, not a second
    // instance. "Every time a container instance experiences a change ... the version
    // counter is incremented."
    let rows: [(&str, Value, i64, i64); 2] = [
        (
            "same identity document",
            json!({
                "cluster": "demo",
                "instanceIdentityDocument": document.clone(),
                "instanceIdentityDocumentSignature": signature.clone(),
                "totalResources": [{ "name": "CPU", "type": "INTEGER", "integerValue": 2048 }],
            }),
            1,
            2048,
        ),
        (
            "previously registered arn",
            json!({
                "cluster": "demo",
                "containerInstanceArn": arn.clone(),
                "instanceIdentityDocument": document.clone(),
                "instanceIdentityDocumentSignature": signature.clone(),
                "totalResources": [{ "name": "CPU", "type": "INTEGER", "integerValue": 4096 }],
            }),
            2,
            4096,
        ),
    ];
    for (case, request, expected_version, expected_cpu) in rows {
        let body = ok(&router, "RegisterContainerInstance", request, case).await;
        let instance = &body["containerInstance"];
        assert_eq!(
            string(instance, "containerInstanceArn", case),
            arn,
            "case {case:?}: re-registration keeps the instance's ARN: {instance}"
        );
        assert_eq!(
            count(instance, "version", case),
            expected_version,
            "case {case:?}: expected version {expected_version}, got {instance}"
        );

        let described = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": "demo", "containerInstances": [arn.clone()] }),
            case,
        )
        .await;
        let read = &list(&described, "containerInstances", case)[0];
        assert_eq!(
            resource_summaries(&list(read, "registeredResources", case)),
            vec![(
                "CPU".to_string(),
                "INTEGER".to_string(),
                expected_cpu.to_string()
            )],
            "case {case:?}: the stored instance carries what the last registration said: {read}"
        );
        assert_eq!(
            list_instances(&router, json!({ "cluster": "demo" }), case).await,
            vec![arn.clone()],
            "case {case:?}: re-registration must not add a second instance"
        );
    }

    // A `containerInstanceArn` is "the ARN of the container instance (if it was
    // previously registered)", so one that names nothing registered here is refused.
    for (case, reference) in [
        ("unknown id", "f".repeat(32)),
        (
            "unknown arn",
            format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/demo/{}",
                "e".repeat(32)
            ),
        ),
    ] {
        refused(
            &router,
            "RegisterContainerInstance",
            json!({ "cluster": "demo", "containerInstanceArn": reference }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // An identity document that leaves a required member blank cannot identify an
    // instance; each row is refused on the first such member (see the member table in
    // [`crate::error_messages`]).
    for (case, bad_document) in [
        ("not json", "not-a-document".to_string()),
        (
            "no instanceId",
            json!({ "accountId": ACCOUNT, "region": SIGNED_REGION }).to_string(),
        ),
        (
            "empty instanceId",
            json!({ "accountId": ACCOUNT, "instanceId": "  " }).to_string(),
        ),
        (
            "instanceId not a string",
            json!({ "accountId": ACCOUNT, "instanceId": 7 }).to_string(),
        ),
        (
            "no accountId",
            json!({ "instanceId": "i-0a0a0a0a0a0a0a0a0" }).to_string(),
        ),
    ] {
        refused(
            &router,
            "RegisterContainerInstance",
            json!({ "cluster": "demo", "instanceIdentityDocument": bad_document }),
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // A cluster that was never created is reported in the OTHER class this operation
    // models: this registration reports a request it cannot read as an
    // `InvalidParameterException` (the identity-document rows above), so the missing
    // cluster has to be distinguishable from it — "Cluster not found." under
    // `ClientException`.
    let body = refused(
        &router,
        "RegisterContainerInstance",
        json!({
            "cluster": "nosuchcluster",
            "instanceIdentityDocument": document,
            "instanceIdentityDocumentSignature": signature,
        }),
        "ClientException",
        "unknown cluster",
    )
    .await;
    assert_eq!(
        body["message"].as_str(),
        Some("Cluster not found."),
        "case \"unknown cluster\": expected the fixed not-found sentence, got {body}"
    );

    // And the cluster is resolved BEFORE the request's shape is read, so a registration
    // that is wrong in both ways reports the cluster rather than the document.
    let body = refused(
        &router,
        "RegisterContainerInstance",
        json!({ "cluster": "nosuchcluster" }),
        "ClientException",
        "unknown cluster and no identity document",
    )
    .await;
    assert_eq!(
        body["message"].as_str(),
        Some("Cluster not found."),
        "case \"unknown cluster and no identity document\": expected the cluster to be \
         resolved first, got {body}"
    );
}

#[tokio::test]
async fn describe_container_instances_reports_ids_that_name_nothing_as_failures() {
    let router = service();
    create_cluster(&router, "demo", "describe").await;
    create_cluster(&router, "other", "describe").await;
    let stored = register_instance(&router, "demo", "i-0909090909090909a", "describe").await;
    let elsewhere = register_instance(&router, "other", "i-0808080808080808b", "describe").await;

    let unknown_id = "a".repeat(32);
    // The ARN a failure echoes for an id that names nothing: the id alone, with no
    // cluster in the resource path — the cluster qualifier belongs to an instance that
    // IS registered. See [`crate::describe_failure_identifiers`].
    let unknown_arn =
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{unknown_id}");

    // Rows: what the request names, which ARNs come back and which are reported MISSING
    // ("the described resource could not be found").
    let rows: [DescribeRow; 4] = [
        (
            "the stored instance",
            vec![stored.clone()],
            vec![stored.clone()],
            vec![],
        ),
        (
            "an id that names nothing",
            vec![unknown_id.clone()],
            vec![],
            vec![unknown_arn.clone()],
        ),
        (
            "the same instance named twice, plus an unknown one",
            vec![
                stored.clone(),
                arn_id(&stored).to_string(),
                unknown_id.clone(),
            ],
            vec![stored.clone()],
            vec![unknown_arn.clone()],
        ),
        (
            "an instance of another cluster",
            vec![elsewhere.clone()],
            vec![],
            vec![elsewhere.clone()],
        ),
    ];
    for (case, named, expected, missing) in rows {
        let body = ok(
            &router,
            "DescribeContainerInstances",
            json!({ "cluster": "demo", "containerInstances": named }),
            case,
        )
        .await;
        let described: Vec<String> = list(&body, "containerInstances", case)
            .iter()
            .map(|instance| string(instance, "containerInstanceArn", case))
            .collect();
        assert_eq!(
            described, expected,
            "case {case:?}: expected {expected:?} described, got {body}"
        );
        let failures: Vec<String> = list(&body, "failures", case)
            .iter()
            .map(|failure| string(failure, "arn", case))
            .collect();
        assert_eq!(
            failures, missing,
            "case {case:?}: expected {missing:?} reported missing, got {body}"
        );
        for failure in list(&body, "failures", case) {
            assert_eq!(
                string(&failure, "reason", case),
                "MISSING",
                "case {case:?}: an id that names nothing is MISSING, got {failure}"
            );
        }
    }

    // One request names at most 100 instances, so its work and its response are bounded.
    let within: Vec<String> = (0..100).map(|_| stored.clone()).collect();
    ok(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": "demo", "containerInstances": within }),
        "100 ids",
    )
    .await;
    let too_many: Vec<String> = (0..101).map(|_| stored.clone()).collect();
    refused(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": "demo", "containerInstances": too_many }),
        "InvalidParameterException",
        "101 ids",
    )
    .await;

    // "The specified cluster wasn't found."
    refused(
        &router,
        "DescribeContainerInstances",
        json!({ "cluster": "nosuchcluster", "containerInstances": [stored] }),
        "ClusterNotFoundException",
        "unknown cluster",
    )
    .await;
}
