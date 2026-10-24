// SPDX-License-Identifier: CC0-1.0

//! `ListServices`, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `ListServices` answers with the ARNs of the services STORED for
//! the cluster the request names (the default cluster when it names none), keeping only
//! those every filter the request sets selects, in a stable order, cut into pages of
//! `maxResults` entries whose `nextToken` resumes the walk where the page stopped.
//!
//! The docs the rows are read off:
//!
//! * `ListServices$cluster`: "The short name or full Amazon Resource Name (ARN) of the
//!   cluster to use when filtering the ListServices results. If you do not specify a
//!   cluster, the default cluster is assumed."
//! * `ListServices$launchType` / `$schedulingStrategy` / `$resourceManagementType`: "The
//!   ... to use when filtering the ListServices results."
//! * `ListServices$maxResults`: "This value can be between 1 and 100. If this parameter
//!   isn't used, then ListServices returns up to 10 results and a nextToken value if
//!   applicable."
//! * `ListServices$nextToken` (output): "This value is null when there are no more
//!   results to return."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    awsvpc_network_configuration, create_cluster, list, ok, refused, register_capacity,
    register_definition, service, string, ACCOUNT, SIGNED_REGION,
};
use crate::task_lifecycle::container;

/// Every value `LaunchType` takes, from the model's enum (smithy_query query="shape"
/// shape="LaunchType": EC2, FARGATE, EXTERNAL, MANAGED_INSTANCES).
const LAUNCH_TYPES: [&str; 4] = ["EC2", "FARGATE", "EXTERNAL", "MANAGED_INSTANCES"];

/// Every value `SchedulingStrategy` takes (smithy_query query="shape"
/// shape="SchedulingStrategy": REPLICA, DAEMON).
const SCHEDULING_STRATEGIES: [&str; 2] = ["REPLICA", "DAEMON"];

/// Every value `ResourceManagementType` takes (smithy_query query="shape"
/// shape="ResourceManagementType": CUSTOMER, ECS). A service a client created through
/// `CreateService` is the customer's; `ECS` names an Express service ECS manages, which
/// this slice holds none of.
const RESOURCE_MANAGEMENT_TYPES: [&str; 2] = ["CUSTOMER", "ECS"];

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one entry.
const MAX_PAGES: usize = 64;

/// The ARN a service of `cluster` named `name` carries
/// (`arn:aws:ecs:<region>:<account>:service/<cluster>/<name>`).
fn service_arn(cluster: &str, name: &str) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/{cluster}/{name}")
}

/// ONE `ListServices` page: the ARNs it carries and the `nextToken` it hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListServices", request, case).await;
    let arns = list(&body, "serviceArns", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string ARN, got {value}"))
                .to_string()
        })
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (arns, next_token)
}

/// Every ARN `request` reaches, walking `nextToken` to the end.
async fn all_pages(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (arns, next) = page(router, this_page, case).await;
        seen.extend(arns);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: ListServices never ran out of pages within {MAX_PAGES} pages");
}

/// Register one revision of `family` that is Fargate-compatible (`awsvpc` plus
/// task-level cpu and memory), so a service may run it under any launch type whose
/// capacity gives a task its own network interface.
async fn register_awsvpc_definition(router: &MultiServiceRouter, family: &str, case: &str) {
    ok(
        router,
        "RegisterTaskDefinition",
        json!({
            "family": family,
            "networkMode": "awsvpc",
            "cpu": "256",
            "memory": "512",
            "containerDefinitions": [container("web", "public.ecr.aws/nginx/nginx:latest")],
        }),
        case,
    )
    .await;
}

/// One service to create: its name, the launch type it asks for (if any), its scheduling
/// strategy, its desired count (absent for a daemon) and whether its task definition is
/// the `awsvpc` one.
struct ServiceRow {
    name: &'static str,
    launch_type: Option<&'static str>,
    scheduling_strategy: &'static str,
    desired: Option<i64>,
    awsvpc: bool,
    capacity_provider: Option<&'static str>,
}

/// Create one row's service in `cluster` and return its ARN, as `CreateService` reports
/// it — the value `ListServices` must later answer with.
async fn create_service(
    router: &MultiServiceRouter,
    cluster: &str,
    row: &ServiceRow,
    awsvpc_family: &str,
    bridge_family: &str,
    case: &str,
) -> String {
    let mut request = json!({
        "cluster": cluster,
        "serviceName": row.name,
        "taskDefinition": if row.awsvpc { awsvpc_family } else { bridge_family },
        "schedulingStrategy": row.scheduling_strategy,
    });
    if row.awsvpc {
        request["networkConfiguration"] = awsvpc_network_configuration();
    }
    if let Some(launch_type) = row.launch_type {
        request["launchType"] = json!(launch_type);
    }
    if let Some(desired) = row.desired {
        request["desiredCount"] = json!(desired);
    }
    if let Some(provider) = row.capacity_provider {
        request["capacityProviderStrategy"] = json!([{ "capacityProvider": provider, "weight": 1 }]);
    }
    let created = ok(router, "CreateService", request, case).await;
    string(&created["service"], "serviceArn", case)
}

#[tokio::test]
async fn list_services_reports_the_stored_services_of_one_cluster_narrowed_by_every_filter() {
    let router = service();
    let case = "setup";

    // Two clusters, so a listing that leaked across clusters is visible, plus the
    // `default` cluster a request that names none is assumed to mean.
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "demo", "capacityProviders": ["FARGATE"] }),
        case,
    )
    .await;
    register_capacity(&router, "demo", case).await;
    create_cluster(&router, "other", case).await;
    create_cluster(&router, "default", case).await;

    register_awsvpc_definition(&router, "awsvpcfamily", case).await;
    // A bridge-mode revision is EC2- and EXTERNAL-compatible, which is what lets the
    // table span the launch types `awsvpc` rules out.
    register_definition(&router, "bridgefamily", case).await;

    // The rows differ along what the rule generalises over: the launch type the service
    // was placed with (including a service placed through a capacity-provider strategy,
    // which carries NONE), its scheduling strategy, and its desired count.
    let rows: [ServiceRow; 6] = [
        ServiceRow {
            name: "alpha",
            launch_type: Some("EC2"),
            scheduling_strategy: "REPLICA",
            desired: Some(1),
            awsvpc: true,
            capacity_provider: None,
        },
        ServiceRow {
            name: "bravo",
            launch_type: Some("FARGATE"),
            scheduling_strategy: "REPLICA",
            desired: Some(2),
            awsvpc: true,
            capacity_provider: None,
        },
        ServiceRow {
            name: "charlie",
            launch_type: Some("MANAGED_INSTANCES"),
            scheduling_strategy: "REPLICA",
            desired: Some(1),
            awsvpc: true,
            capacity_provider: None,
        },
        ServiceRow {
            name: "delta",
            launch_type: Some("EXTERNAL"),
            scheduling_strategy: "REPLICA",
            desired: Some(1),
            awsvpc: false,
            capacity_provider: None,
        },
        ServiceRow {
            name: "echo",
            launch_type: Some("EC2"),
            scheduling_strategy: "DAEMON",
            desired: None,
            awsvpc: false,
            capacity_provider: None,
        },
        ServiceRow {
            name: "foxtrot",
            launch_type: None,
            scheduling_strategy: "REPLICA",
            desired: Some(1),
            awsvpc: true,
            capacity_provider: Some("FARGATE"),
        },
    ];

    let mut created_arns: Vec<String> = Vec::new();
    for row in &rows {
        let arn = create_service(
            &router,
            "demo",
            row,
            "awsvpcfamily",
            "bridgefamily",
            row.name,
        )
        .await;
        assert_eq!(
            arn,
            service_arn("demo", row.name),
            "case {:?}: CreateService reported an unexpected ARN",
            row.name
        );
        created_arns.push(arn);
    }
    created_arns.sort();

    // A service of ANOTHER cluster, and one of the `default` cluster, neither of which
    // may appear in a listing of `demo`. The name repeats on purpose: "Service names must
    // be unique within a cluster, but you can have similarly named services in multiple
    // clusters."
    for cluster in ["other", "default"] {
        create_service(
            &router,
            cluster,
            &ServiceRow {
                name: "alpha",
                launch_type: Some("EC2"),
                scheduling_strategy: "REPLICA",
                desired: Some(1),
                awsvpc: false,
                capacity_provider: None,
            },
            "awsvpcfamily",
            "bridgefamily",
            cluster,
        )
        .await;
    }

    let demo = |names: &[&str]| -> Vec<String> {
        let mut arns: Vec<String> = names.iter().map(|name| service_arn("demo", name)).collect();
        arns.sort();
        arns
    };

    // One row per filter the operation models, each on a value the stored set answers
    // differently for, plus the two ways a request may name the cluster and the way it
    // may name none.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no filter at all lists every service of the cluster",
            json!({ "cluster": "demo" }),
            demo(&["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"]),
        ),
        (
            "the cluster named by its full ARN",
            json!({ "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/demo") }),
            demo(&["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"]),
        ),
        (
            "no cluster at all assumes the default cluster",
            json!({}),
            vec![service_arn("default", "alpha")],
        ),
        (
            "another cluster's services are its own",
            json!({ "cluster": "other" }),
            vec![service_arn("other", "alpha")],
        ),
        (
            "launchType EC2",
            json!({ "cluster": "demo", "launchType": LAUNCH_TYPES[0] }),
            demo(&["alpha", "echo"]),
        ),
        (
            "launchType FARGATE",
            json!({ "cluster": "demo", "launchType": LAUNCH_TYPES[1] }),
            demo(&["bravo"]),
        ),
        (
            "launchType EXTERNAL",
            json!({ "cluster": "demo", "launchType": LAUNCH_TYPES[2] }),
            demo(&["delta"]),
        ),
        (
            "launchType MANAGED_INSTANCES",
            json!({ "cluster": "demo", "launchType": LAUNCH_TYPES[3] }),
            demo(&["charlie"]),
        ),
        (
            "schedulingStrategy REPLICA",
            json!({ "cluster": "demo", "schedulingStrategy": SCHEDULING_STRATEGIES[0] }),
            demo(&["alpha", "bravo", "charlie", "delta", "foxtrot"]),
        ),
        (
            "schedulingStrategy DAEMON",
            json!({ "cluster": "demo", "schedulingStrategy": SCHEDULING_STRATEGIES[1] }),
            demo(&["echo"]),
        ),
        (
            "resourceManagementType CUSTOMER",
            json!({ "cluster": "demo", "resourceManagementType": RESOURCE_MANAGEMENT_TYPES[0] }),
            demo(&["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"]),
        ),
        (
            "resourceManagementType ECS matches no customer-created service",
            json!({ "cluster": "demo", "resourceManagementType": RESOURCE_MANAGEMENT_TYPES[1] }),
            Vec::new(),
        ),
        (
            "two filters intersect",
            json!({
                "cluster": "demo",
                "launchType": "EC2",
                "schedulingStrategy": "DAEMON",
            }),
            demo(&["echo"]),
        ),
        (
            "two filters no service satisfies together",
            json!({
                "cluster": "demo",
                "launchType": "FARGATE",
                "schedulingStrategy": "DAEMON",
            }),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in cases {
        let mut listed = all_pages(&router, request, case).await;
        listed.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // ROUND TRIP: every ARN the listing reports is one `CreateService` returned, and
    // `DescribeServices` resolves each of them to the service it names.
    let case = "round trip";
    let mut listed = all_pages(&router, json!({ "cluster": "demo" }), case).await;
    listed.sort();
    assert_eq!(
        listed, created_arns,
        "case {case:?}: the listing must report exactly the ARNs the creates returned"
    );
    for arn in &listed {
        let case = arn.as_str();
        let described = ok(
            &router,
            "DescribeServices",
            json!({ "cluster": "demo", "services": [arn] }),
            case,
        )
        .await;
        let services = list(&described, "services", case);
        assert_eq!(
            services.len(),
            1,
            "case {case:?}: a listed ARN must describe to exactly one service, got {described}"
        );
        assert_eq!(
            &string(&services[0], "serviceArn", case),
            arn,
            "case {case:?}: DescribeServices resolved a listed ARN to another service"
        );
    }
}

#[tokio::test]
async fn list_services_pages_in_a_stable_order_at_max_results() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "paged", case).await;
    register_definition(&router, "pagedfamily", case).await;

    // Eleven services, so the DEFAULT page size (10) leaves a second page too.
    let names: Vec<String> = (1..=11).map(|index| format!("svc-{index:02}")).collect();
    let mut expected: Vec<String> = Vec::new();
    for name in &names {
        let created = ok(
            &router,
            "CreateService",
            json!({
                "cluster": "paged",
                "serviceName": name,
                "taskDefinition": "pagedfamily",
                "desiredCount": 0,
                "launchType": "EC2",
            }),
            name,
        )
        .await;
        expected.push(string(&created["service"], "serviceArn", name));
    }
    expected.sort();

    // The rows differ along the page size: each documented bound (1 and 100), sizes that
    // divide the set and sizes that do not, and the whole set in one page.
    for size in [1_i64, 2, 3, 5, 10, 11, 12, 100] {
        let case = format!("maxResults {size}");
        let case = case.as_str();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "cluster": "paged", "maxResults": size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (arns, next) = page(&router, request, case).await;
            pages += 1;
            assert!(
                arns.len() as i64 <= size,
                "case {case:?}: a page may not exceed maxResults, got {} entries",
                arns.len()
            );
            assert_eq!(
                next.is_some(),
                seen.len() + arns.len() < expected.len(),
                "case {case:?}: a nextToken must be present exactly while results remain, \
                 got {next:?} after {} of {} entries",
                seen.len() + arns.len(),
                expected.len()
            );
            seen.extend(arns);
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
            assert!(
                pages < MAX_PAGES,
                "case {case:?}: ListServices never ran out of pages"
            );
        }
        // The pages concatenate, in order, to the whole set exactly once: nothing is
        // dropped, nothing is repeated and the order does not depend on the page size.
        assert_eq!(
            seen, expected,
            "case {case:?}: expected the pages to walk {expected:?}, got {seen:?}"
        );
        let full_pages = expected.len().div_ceil(usize::try_from(size).expect("a positive size"));
        assert_eq!(
            pages,
            full_pages.max(1),
            "case {case:?}: expected {full_pages} page(s) for {} entries",
            expected.len()
        );
    }

    // "If this parameter isn't used, then ListServices returns up to 10 results and a
    // nextToken value if applicable."
    let case = "no maxResults";
    let (first, token) = page(&router, json!({ "cluster": "paged" }), case).await;
    assert_eq!(
        first.len(),
        10,
        "case {case:?}: expected the documented default of 10 results, got {first:?}"
    );
    let token = token.unwrap_or_else(|| {
        panic!("case {case:?}: expected a nextToken while {} remain", expected.len() - first.len())
    });
    let (second, next) = page(
        &router,
        json!({ "cluster": "paged", "nextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        [first, second].concat(),
        expected,
        "case {case:?}: the default-sized pages must walk the whole set"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken on the last page, got {next:?}"
    );
}

#[tokio::test]
async fn list_services_refuses_an_out_of_range_page_size_and_an_unknown_cluster() {
    let router = service();
    let case = "setup";
    create_cluster(&router, "refusing", case).await;

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListServices",
            json!({ "cluster": "refusing", "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListServices",
            json!({ "cluster": "refusing", "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    let case = "a cluster that does not exist";
    refused(
        &router,
        "ListServices",
        json!({ "cluster": "nosuchcluster" }),
        "ClusterNotFoundException",
        case,
    )
    .await;

    // A request that names no cluster is a request for the `default` cluster, which this
    // engine never created: the same refusal.
    let case = "no cluster and no default cluster";
    refused(
        &router,
        "ListServices",
        json!({}),
        "ClusterNotFoundException",
        case,
    )
    .await;
}

#[tokio::test]
async fn a_deleted_service_leaves_the_listing_but_is_still_describable() {
    let router = service();
    let case = "a deleted service";
    create_cluster(&router, "deleting", case).await;
    register_definition(&router, "deletingfamily", case).await;

    for name in ["kept", "deleted"] {
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": "deleting",
                "serviceName": name,
                "taskDefinition": "deletingfamily",
                "desiredCount": 0,
                "launchType": "EC2",
            }),
            case,
        )
        .await;
    }

    let listed = all_pages(&router, json!({ "cluster": "deleting" }), case).await;
    assert_eq!(
        listed,
        vec![
            service_arn("deleting", "deleted"),
            service_arn("deleting", "kept"),
        ],
        "case {case:?}: both stored services must be listed before the delete"
    );

    // The service wants no task, so nothing has to be cleaned up and the delete takes it
    // straight to INACTIVE.
    let deleted = ok(
        &router,
        "DeleteService",
        json!({ "cluster": "deleting", "service": "deleted" }),
        case,
    )
    .await;
    assert_eq!(
        string(&deleted["service"], "status", case),
        "INACTIVE",
        "case {case:?}: a service with no tasks to clean up is deleted outright"
    );

    let listed = all_pages(&router, json!({ "cluster": "deleting" }), case).await;
    assert_eq!(
        listed,
        vec![service_arn("deleting", "kept")],
        "case {case:?}: a deleted service is no longer one of the cluster's services"
    );

    // "Services in the DRAINING or INACTIVE status can still be viewed with the
    // DescribeServices API operation."
    let described = ok(
        &router,
        "DescribeServices",
        json!({ "cluster": "deleting", "services": ["deleted"] }),
        case,
    )
    .await;
    let services = list(&described, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: DescribeServices must still report the deleted service, got {described}"
    );
    assert_eq!(
        string(&services[0], "status", case),
        "INACTIVE",
        "case {case:?}: the described record keeps the status the delete left"
    );
}
