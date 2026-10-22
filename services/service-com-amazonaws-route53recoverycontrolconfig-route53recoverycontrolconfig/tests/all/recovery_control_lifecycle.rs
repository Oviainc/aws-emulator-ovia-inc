// SPDX-License-Identifier: CC0-1.0

//! The recovery-control lifecycle, end to end over HTTP: cluster -> control panel ->
//! routing control, then back down again.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together — not the handlers' Rust signatures.
//!
//! The tables below span the classes each rule generalises over: every `NetworkType`
//! the model declares plus the omitted case, page sizes from 1 to past the item count,
//! the published bounds of `MaxResults` and one step outside each, and every resource
//! level of the hierarchy for the not-found and conflict rules.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this target shares.
use super::common::{
    call, create_cluster, create_control_panel, create_routing_control, enc, error_code, router,
    REGION,
};

/// The account id every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The ARN prefix AWS documents for recovery-control resources:
/// `arn:aws:route53-recovery-control::<account>:...`.
pub(crate) const ARN_PREFIX: &str = "arn:aws:route53-recovery-control::000000000000:";

/// The `Status` values the model declares, via
/// `smithy_query(query="shape",shape="Status")`: `DEPLOYED | PENDING | PENDING_DELETION`.
pub(crate) const STATUS_PENDING: &str = "PENDING";
pub(crate) const STATUS_DEPLOYED: &str = "DEPLOYED";

/// The Regions a cluster publishes a data-plane endpoint in — five, "one for each
/// supported Amazon Web Services Region" (`CreateCluster`'s own documentation).
pub(crate) const ENDPOINT_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-southeast-2",
    "eu-west-1",
    "us-east-1",
    "us-west-2",
];

/// Longer than the span the service gives a `PENDING` -> `DEPLOYED` transition, so a
/// read after it must observe the deployed resource.
pub(crate) const PAST_DEPLOYMENT: Duration = Duration::from_millis(900);

// The constants above are `pub(crate)` because sibling suites in this same test target
// reuse them rather than keeping a second copy: see `tests/all/update_cluster.rs`.

/// A cluster is answered from the record the create call stored: `DescribeCluster`
/// returns the same name, endpoints, network type and owner, for every `NetworkType`
/// the model declares and for a request that names none.
#[tokio::test]
async fn create_persists_the_cluster_and_describe_answers_the_same_body() {
    let router = router();

    // Rows differ along the member the endpoint list and the echoed network type are
    // projected from. Source for the value set: `smithy_query(query="shape",
    // shape="NetworkType")` -> DUALSTACK | IPV4. The `None` row is the omitted
    // member, which the model gives no default.
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("ipv4-explicit", Some("IPV4"), "IPV4"),
        ("dualstack-explicit", Some("DUALSTACK"), "DUALSTACK"),
        ("network-type-omitted", None, "IPV4"),
    ];

    let mut arns: Vec<String> = Vec::new();
    for (name, requested, expected) in cases {
        let mut body = json!({ "ClusterName": name, "Tags": { "team": "arc" } });
        if let Some(requested) = requested {
            body["NetworkType"] = json!(requested);
        }
        let created = create_cluster(&router, body).await;

        let arn = created["ClusterArn"].as_str().unwrap_or_default().to_string();
        assert!(
            arn.starts_with(ARN_PREFIX) && arn[ARN_PREFIX.len()..].starts_with("cluster/"),
            "case {name}: expected an ARN under {ARN_PREFIX}cluster/, got {arn:?}"
        );
        assert!(
            !arn.contains(REGION),
            "case {name}: a recovery-control ARN carries no Region, got {arn:?}"
        );
        assert_eq!(
            created["Name"], json!(name),
            "case {name}: expected the requested name, got {:?}",
            created["Name"]
        );
        assert_eq!(
            created["Owner"], json!(ACCOUNT),
            "case {name}: expected owner {ACCOUNT}, got {:?}",
            created["Owner"]
        );
        assert_eq!(
            created["NetworkType"], json!(expected),
            "case {name}: expected NetworkType {expected}, got {:?}",
            created["NetworkType"]
        );
        assert_eq!(
            created["Status"], json!(STATUS_PENDING),
            "case {name}: a freshly created cluster is {STATUS_PENDING}, got {:?}",
            created["Status"]
        );

        let endpoints = created["ClusterEndpoints"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let regions: Vec<&str> = endpoints
            .iter()
            .map(|e| e["Region"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            regions, ENDPOINT_REGIONS,
            "case {name}: expected one endpoint per supported Region, got {regions:?}"
        );
        for endpoint in &endpoints {
            let url = endpoint["Endpoint"].as_str().unwrap_or_default();
            let region = endpoint["Region"].as_str().unwrap_or_default();
            assert!(
                url.starts_with("https://") && url.contains(region),
                "case {name}: endpoint for {region} must name its Region, got {url:?}"
            );
        }

        assert!(
            !arns.contains(&arn),
            "case {name}: each create mints a fresh ARN, but {arn} repeated"
        );
        arns.push(arn.clone());

        // Read the cluster back: every member must come from the stored record.
        let (status, read) = call(&router, "GET", &format!("/cluster/{}", enc(&arn)), None).await;
        assert_eq!(
            status, 200,
            "case {name}: expected DescribeCluster 200, got {status} {read:?}"
        );
        let read = read["Cluster"].clone();
        for member in ["ClusterArn", "Name", "Owner", "NetworkType", "ClusterEndpoints"] {
            assert_eq!(
                read[member], created[member],
                "case {name}: DescribeCluster must answer the stored {member}, got {:?} after {:?}",
                read[member], created[member]
            );
        }
    }
}

/// A cluster AWS returns `PENDING` reaches `DEPLOYED` on its own, without any call
/// moving it, and every resource of the hierarchy behaves the same way.
#[tokio::test]
async fn pending_resources_reach_deployed_without_being_written() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "deploying" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let control = create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "rc",
        }),
    )
    .await;
    let control_arn = control["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (level, created) in [
        ("cluster", &cluster),
        ("control panel", &panel),
        ("routing control", &control),
    ] {
        assert_eq!(
            created["Status"], json!(STATUS_PENDING),
            "case {level}: a create answers {STATUS_PENDING}, got {:?}",
            created["Status"]
        );
    }

    // No sleeping inside the handlers: the transition is due at an instant the create
    // recorded, so waiting out that span here is what makes it observable.
    std::thread::sleep(PAST_DEPLOYMENT);

    let reads: &[(&str, String, &str)] = &[
        ("cluster", format!("/cluster/{}", enc(&cluster_arn)), "Cluster"),
        (
            "control panel",
            format!("/controlpanel/{}", enc(&panel_arn)),
            "ControlPanel",
        ),
        (
            "routing control",
            format!("/routingcontrol/{}", enc(&control_arn)),
            "RoutingControl",
        ),
    ];
    for (level, path, member) in reads {
        let (status, read) = call(&router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {level}: expected 200 from {path}, got {status} {read:?}"
        );
        assert_eq!(
            read[member]["Status"], json!(STATUS_DEPLOYED),
            "case {level}: expected {STATUS_DEPLOYED} once the deployment is due, got {:?}",
            read[member]["Status"]
        );
    }
}

/// `RoutingControlCount` is counted from the routing controls actually stored under the
/// control panel: it tracks every create and every delete, at every count.
#[tokio::test]
async fn routing_control_count_tracks_the_stored_controls() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "counting" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "counted-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    assert_eq!(
        panel["RoutingControlCount"], json!(0),
        "case fresh panel: expected 0 routing controls, got {:?}",
        panel["RoutingControlCount"]
    );
    assert_eq!(
        panel["DefaultControlPanel"], json!(false),
        "case fresh panel: a caller-created panel is not the default one, got {:?}",
        panel["DefaultControlPanel"]
    );
    assert_eq!(
        panel["ClusterArn"], json!(cluster_arn),
        "case fresh panel: expected the stored parent ARN, got {:?}",
        panel["ClusterArn"]
    );

    // Create three controls, asserting the count after each one, then delete them in
    // the reverse order, asserting it again: the count follows the store both ways.
    let mut created: Vec<String> = Vec::new();
    for n in 1..=3u32 {
        let control = create_routing_control(
            &router,
            json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": format!("rc-{n}"),
            }),
        )
        .await;
        created.push(
            control["RoutingControlArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        assert_eq!(
            control["ControlPanelArn"], json!(panel_arn),
            "case create rc-{n}: expected the owning panel ARN, got {:?}",
            control["ControlPanelArn"]
        );
        assert_eq!(
            described_count(&router, &panel_arn).await,
            i64::from(n),
            "case create rc-{n}: expected RoutingControlCount {n}"
        );
    }

    for (removed, arn) in created.iter().rev().enumerate() {
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/routingcontrol/{}", enc(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case delete {arn}: expected 200, got {status} {body:?}"
        );
        let expected = 3 - i64::try_from(removed).expect("a small index") - 1;
        assert_eq!(
            described_count(&router, &panel_arn).await,
            expected,
            "case delete {arn}: expected RoutingControlCount {expected}"
        );
        // And the deleted control is gone from both readers.
        let (status, body) = call(
            &router,
            "GET",
            &format!("/routingcontrol/{}", enc(arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case delete {arn}: DescribeRoutingControl must answer 404, got {status} {body:?}"
        );
        let listed = list_routing_controls(&router, &panel_arn, None, None).await;
        assert!(
            !listed.contains(arn),
            "case delete {arn}: ListRoutingControls must not list it, got {listed:?}"
        );
    }
}

/// `DescribeControlPanel`'s `RoutingControlCount` for `panel_arn`.
async fn described_count(router: &MultiServiceRouter, panel_arn: &str) -> i64 {
    let (status, body) = call(
        router,
        "GET",
        &format!("/controlpanel/{}", enc(panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {panel_arn}: expected 200, got {status} {body:?}"
    );
    body["ControlPanel"]["RoutingControlCount"]
        .as_i64()
        .unwrap_or(-1)
}

/// Every routing-control ARN `ListRoutingControls` returns for one page.
async fn list_routing_controls(
    router: &MultiServiceRouter,
    panel_arn: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Vec<String> {
    let (_, _, arns) = list_page(router, panel_arn, max_results, next_token).await;
    arns
}

/// One `ListRoutingControls` page: `(status, NextToken, the page's ARNs)`.
async fn list_page(
    router: &MultiServiceRouter,
    panel_arn: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Option<String>, Vec<String>) {
    let mut query: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("MaxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("NextToken={}", enc(token)));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let path = format!("/controlpanel/{}/routingcontrols{suffix}", enc(panel_arn));
    let (status, body) = call(router, "GET", &path, None).await;
    let arns = body["RoutingControls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rc| rc["RoutingControlArn"].as_str().unwrap_or_default().to_string())
        .collect();
    (
        status,
        body["NextToken"].as_str().map(ToString::to_string),
        arns,
    )
}

/// Paging through the stored routing controls yields each item exactly once, in a
/// stable order, for every page size from one to past the item count.
#[tokio::test]
async fn paging_yields_every_stored_control_exactly_once() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "paging" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "paged-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    const TOTAL: usize = 5;
    let mut expected: Vec<String> = Vec::new();
    for n in 0..TOTAL {
        let control = create_routing_control(
            &router,
            json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": format!("paged-{n}"),
            }),
        )
        .await;
        expected.push(
            control["RoutingControlArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Page sizes spanning the class: one at a time, a size that does not divide the
    // total, the exact total, one past it, and the request that names no MaxResults.
    let cases: &[Option<i32>] = &[Some(1), Some(2), Some(3), Some(5), Some(6), None];
    for max_results in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (status, next, arns) =
                list_page(&router, &panel_arn, *max_results, token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case MaxResults={max_results:?}: expected 200, got {status}"
            );
            if let Some(max) = max_results {
                assert!(
                    arns.len() <= *max as usize,
                    "case MaxResults={max_results:?}: a page holds at most {max} items, got {}",
                    arns.len()
                );
            }
            seen.extend(arns);
            pages += 1;
            assert!(
                pages <= TOTAL + 1,
                "case MaxResults={max_results:?}: paging did not terminate after {pages} pages"
            );
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={max_results:?}: paging must yield every stored control once, \
             in creation order, got {seen:?}"
        );
    }

    // A cursor that names nothing stored cannot be resumed from.
    let (status, body) = call(
        &router,
        "GET",
        &format!(
            "/controlpanel/{}/routingcontrols?NextToken={}",
            enc(&panel_arn),
            enc("arn:aws:route53-recovery-control::000000000000:controlpanel/x/routingcontrol/y")
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case unknown NextToken: expected 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case unknown NextToken: expected ValidationException, got {body:?}"
    );
}

/// A replayed `ClientToken` returns the resource the first call created; a token
/// replayed with different parameters is refused; two different tokens are two
/// resources.
#[tokio::test]
async fn a_replayed_client_token_returns_the_first_resource() {
    let router = router();

    let first = create_cluster(
        &router,
        json!({ "ClusterName": "idem", "ClientToken": "token-a" }),
    )
    .await;
    let replay = create_cluster(
        &router,
        json!({ "ClusterName": "idem", "ClientToken": "token-a" }),
    )
    .await;
    assert_eq!(
        replay["ClusterArn"], first["ClusterArn"],
        "case same token, same body: expected the first cluster back, got {:?} after {:?}",
        replay["ClusterArn"], first["ClusterArn"]
    );

    // Same token, different parameters: not a retry of the same request.
    let (status, body) = call(
        &router,
        "POST",
        "/cluster",
        Some(json!({ "ClusterName": "idem-other", "ClientToken": "token-a" })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case same token, different body: expected 409, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case same token, different body: expected ConflictException, got {body:?}"
    );

    // The same rule holds one level down, for a control panel.
    let cluster_arn = first["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelName": "idem-panel",
            "ClientToken": "token-b",
        }),
    )
    .await;
    let panel_replay = create_control_panel(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelName": "idem-panel",
            "ClientToken": "token-b",
        }),
    )
    .await;
    assert_eq!(
        panel_replay["ControlPanelArn"], panel["ControlPanelArn"],
        "case same token, same body (panel): expected the first panel back, got {:?}",
        panel_replay["ControlPanelArn"]
    );

    // A second cluster under a FRESH token is a second cluster.
    let other = create_cluster(
        &router,
        json!({ "ClusterName": "idem-two", "ClientToken": "token-c" }),
    )
    .await;
    assert_ne!(
        other["ClusterArn"], first["ClusterArn"],
        "case fresh token: expected a second cluster, got {:?}",
        other["ClusterArn"]
    );

    // Deleting the resource releases its token, so the ledger cannot be grown by a
    // create/delete loop and no token reports a resource that is gone.
    let other_arn = other["ClusterArn"].as_str().unwrap_or_default().to_string();
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", enc(&other_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete the token's cluster: expected 200, got {status} {body:?}"
    );
    let recreated = create_cluster(
        &router,
        json!({ "ClusterName": "idem-two", "ClientToken": "token-c" }),
    )
    .await;
    assert_ne!(
        recreated["ClusterArn"], json!(other_arn),
        "case replay after delete: the released token must not name the deleted cluster, \
         got {:?}",
        recreated["ClusterArn"]
    );
}

/// A name identifies a resource within its parent, at every level of the hierarchy.
#[tokio::test]
async fn a_duplicate_name_within_the_same_parent_is_a_conflict() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "dup" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let other = create_cluster(&router, json!({ "ClusterName": "dup-other" })).await;
    let other_arn = other["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "dup-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "dup-rc",
        }),
    )
    .await;

    // One row per resource level: the same name under the same parent conflicts.
    let conflicts: &[(&str, &str, Value)] = &[
        ("cluster", "/cluster", json!({ "ClusterName": "dup" })),
        (
            "control panel",
            "/controlpanel",
            json!({ "ClusterArn": cluster_arn, "ControlPanelName": "dup-panel" }),
        ),
        (
            "routing control",
            "/routingcontrol",
            json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": "dup-rc",
            }),
        ),
    ];
    for (level, path, body) in conflicts {
        let (status, answer) = call(&router, "POST", path, Some(body.clone())).await;
        assert_eq!(
            status, 409,
            "case duplicate {level}: expected 409, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case duplicate {level}: expected ConflictException, got {answer:?}"
        );
    }

    // The same name under a DIFFERENT parent is a different resource.
    let elsewhere = create_control_panel(
        &router,
        json!({ "ClusterArn": other_arn, "ControlPanelName": "dup-panel" }),
    )
    .await;
    assert_ne!(
        elsewhere["ControlPanelArn"], panel["ControlPanelArn"],
        "case same name, other cluster: expected a second panel, got {:?}",
        elsewhere["ControlPanelArn"]
    );
}

/// An ARN no resource is stored under is `ResourceNotFoundException` (404) on every
/// read, delete and create that names it as a parent.
#[tokio::test]
async fn an_unknown_arn_is_not_found_everywhere() {
    let router = router();

    let missing_cluster = format!("{ARN_PREFIX}cluster/00000000-0000-0000-0000-000000000000");
    let missing_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let missing_control = format!("{missing_panel}/routingcontrol/11111111-1111-1111-1111-111111111111");

    // Rows span the operation kinds (read, delete, create-under-a-parent, list) and
    // all three resource levels.
    let cases: Vec<(String, &str, String, Option<Value>)> = vec![
        (
            "DescribeCluster".to_string(),
            "GET",
            format!("/cluster/{}", enc(&missing_cluster)),
            None,
        ),
        (
            "DeleteCluster".to_string(),
            "DELETE",
            format!("/cluster/{}", enc(&missing_cluster)),
            None,
        ),
        (
            "DescribeControlPanel".to_string(),
            "GET",
            format!("/controlpanel/{}", enc(&missing_panel)),
            None,
        ),
        (
            "DeleteControlPanel".to_string(),
            "DELETE",
            format!("/controlpanel/{}", enc(&missing_panel)),
            None,
        ),
        (
            "DescribeRoutingControl".to_string(),
            "GET",
            format!("/routingcontrol/{}", enc(&missing_control)),
            None,
        ),
        (
            "DeleteRoutingControl".to_string(),
            "DELETE",
            format!("/routingcontrol/{}", enc(&missing_control)),
            None,
        ),
        (
            "ListRoutingControls".to_string(),
            "GET",
            format!("/controlpanel/{}/routingcontrols", enc(&missing_panel)),
            None,
        ),
        (
            "CreateControlPanel".to_string(),
            "POST",
            "/controlpanel".to_string(),
            Some(json!({ "ClusterArn": missing_cluster, "ControlPanelName": "orphan" })),
        ),
        (
            "CreateRoutingControl".to_string(),
            "POST",
            "/routingcontrol".to_string(),
            Some(json!({ "ClusterArn": missing_cluster, "RoutingControlName": "orphan" })),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, answer) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
    }
}

/// A request outside a modelled constraint is refused with `ValidationException`
/// (400), and a request on the published bounds is accepted.
#[tokio::test]
async fn constraint_violations_are_refused_and_the_bounds_are_accepted() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "bounds" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "bounds-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // `ClusterName`/`ControlPanelName` are len 1..64 pattern ^\S+$, and
    // `MaxResults` is range 1..1000 (`smithy_query(query="operation", ...)`).
    let sixty_five = "a".repeat(65);
    let refusals: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "empty ClusterName",
            "POST",
            "/cluster".to_string(),
            Some(json!({ "ClusterName": "" })),
        ),
        (
            "ClusterName one past the length maximum",
            "POST",
            "/cluster".to_string(),
            Some(json!({ "ClusterName": sixty_five })),
        ),
        (
            "ClusterName with whitespace breaks ^\\S+$",
            "POST",
            "/cluster".to_string(),
            Some(json!({ "ClusterName": "has space" })),
        ),
        (
            "empty ControlPanelName",
            "POST",
            "/controlpanel".to_string(),
            Some(json!({ "ClusterArn": cluster_arn, "ControlPanelName": "" })),
        ),
        (
            "MaxResults one below the range minimum",
            "GET",
            format!("/controlpanel/{}/routingcontrols?MaxResults=0", enc(&panel_arn)),
            None,
        ),
        (
            "MaxResults one above the range maximum",
            "GET",
            format!(
                "/controlpanel/{}/routingcontrols?MaxResults=1001",
                enc(&panel_arn)
            ),
            None,
        ),
    ];
    for (case, method, path, body) in refusals {
        let (status, answer) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
    }

    // Both published `MaxResults` bounds are inside the range and must be served.
    for max in [1, 1000] {
        let (status, _, _) = list_page(&router, &panel_arn, Some(max), None).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max} (a published bound): expected 200, got {status}"
        );
    }

    // A 64-character name is on the length maximum and must be accepted.
    let sixty_four = "b".repeat(64);
    let accepted = create_cluster(&router, json!({ "ClusterName": sixty_four })).await;
    assert_eq!(
        accepted["Name"], json!(sixty_four),
        "case ClusterName on the length maximum: expected it stored, got {:?}",
        accepted["Name"]
    );
}

/// A routing control created without a `ControlPanelArn` lands in the cluster's default
/// control panel — the one the cluster was created with.
#[tokio::test]
async fn a_routing_control_without_a_panel_lands_in_the_default_panel() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "defaulting" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();

    let control = create_routing_control(
        &router,
        json!({ "ClusterArn": cluster_arn, "RoutingControlName": "implicit" }),
    )
    .await;
    let panel_arn = control["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        panel_arn.starts_with(&format!("{ARN_PREFIX}controlpanel/")),
        "case implicit panel: expected a control-panel ARN, got {panel_arn:?}"
    );

    let (status, body) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}", enc(&panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case implicit panel: expected DescribeControlPanel 200, got {status} {body:?}"
    );
    let panel = body["ControlPanel"].clone();
    assert_eq!(
        panel["DefaultControlPanel"], json!(true),
        "case implicit panel: expected the cluster's DEFAULT panel, got {:?}",
        panel["DefaultControlPanel"]
    );
    assert_eq!(
        panel["ClusterArn"], json!(cluster_arn),
        "case implicit panel: expected the panel of this cluster, got {:?}",
        panel["ClusterArn"]
    );
    assert_eq!(
        panel["RoutingControlCount"], json!(1),
        "case implicit panel: expected the new control counted, got {:?}",
        panel["RoutingControlCount"]
    );

    // A control panel belonging to ANOTHER cluster is a contradictory request.
    let other = create_cluster(&router, json!({ "ClusterName": "defaulting-other" })).await;
    let other_arn = other["ClusterArn"].as_str().unwrap_or_default().to_string();
    let (status, answer) = call(
        &router,
        "POST",
        "/routingcontrol",
        Some(json!({
            "ClusterArn": other_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "crossed",
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case panel of another cluster: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case panel of another cluster: expected ValidationException, got {answer:?}"
    );
}

/// The store is bounded: a caller cannot grow it without limit, and the create refuses
/// past the ceiling with the error the model declares for it.
#[tokio::test]
async fn a_full_store_refuses_another_create_with_the_modelled_quota_error() {
    let router = router();

    // The ceiling this service holds the cluster store to.
    let max_clusters =
        service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::state::MAX_CLUSTERS;

    let mut arns: Vec<String> = Vec::new();
    for n in 0..max_clusters {
        let created = create_cluster(&router, json!({ "ClusterName": format!("bounded-{n}") })).await;
        arns.push(
            created["ClusterArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    let (status, body) = call(
        &router,
        "POST",
        "/cluster",
        Some(json!({ "ClusterName": "one-too-many" })),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one past the ceiling of {max_clusters}: expected 402, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case one past the ceiling: expected ServiceQuotaExceededException, got {body:?}"
    );

    // Deleting one makes room again, so the bound is a ceiling and not a dead end.
    let freed = arns.first().expect("at least one cluster was created").clone();
    let (status, body) = call(&router, "DELETE", &format!("/cluster/{}", enc(&freed)), None).await;
    assert_eq!(
        status, 200,
        "case delete to make room: expected 200, got {status} {body:?}"
    );
    let (status, body) = call(
        &router,
        "POST",
        "/cluster",
        Some(json!({ "ClusterName": "one-too-many" })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case create after making room: expected 200, got {status} {body:?}"
    );
}

#[tokio::test]
async fn a_parent_with_children_cannot_be_deleted() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "teardown" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "teardown-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let control = create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "teardown-rc",
        }),
    )
    .await;
    let control_arn = control["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // The default control panel of this cluster, reached through a routing control
    // created without a panel.
    let implicit = create_routing_control(
        &router,
        json!({ "ClusterArn": cluster_arn, "RoutingControlName": "implicit" }),
    )
    .await;
    let default_panel_arn = implicit["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let implicit_arn = implicit["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: a cluster with a control panel, a control panel with a routing control,
    // and the default control panel, which lives and dies with its cluster.
    let conflicts: &[(&str, String)] = &[
        ("cluster with a control panel", format!("/cluster/{}", enc(&cluster_arn))),
        (
            "control panel with a routing control",
            format!("/controlpanel/{}", enc(&panel_arn)),
        ),
        (
            "default control panel",
            format!("/controlpanel/{}", enc(&default_panel_arn)),
        ),
    ];
    for (case, path) in conflicts {
        let (status, answer) = call(&router, "DELETE", path, None).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case}: expected ConflictException, got {answer:?}"
        );
    }

    // Tear down from the leaves up; each step then succeeds.
    for (case, path) in [
        (
            "routing control",
            format!("/routingcontrol/{}", enc(&control_arn)),
        ),
        (
            "implicit routing control",
            format!("/routingcontrol/{}", enc(&implicit_arn)),
        ),
        ("control panel", format!("/controlpanel/{}", enc(&panel_arn))),
        ("cluster", format!("/cluster/{}", enc(&cluster_arn))),
    ] {
        let (status, answer) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case delete {case}: expected 200, got {status} {answer:?}"
        );
    }

    // The cluster, and the default control panel that belonged to it, are both gone.
    for (case, path) in [
        ("cluster", format!("/cluster/{}", enc(&cluster_arn))),
        (
            "default control panel",
            format!("/controlpanel/{}", enc(&default_panel_arn)),
        ),
    ] {
        let (status, answer) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case read deleted {case}: expected 404, got {status} {answer:?}"
        );
    }

    // And the name is free again once the cluster is gone.
    let reused = create_cluster(&router, json!({ "ClusterName": "teardown" })).await;
    assert_ne!(
        reused["ClusterArn"], cluster["ClusterArn"],
        "case recreate after delete: expected a fresh ARN, got {:?}",
        reused["ClusterArn"]
    );
}

/// One `ListControlPanels` page: `(status, NextToken, the page's `ControlPanel` bodies)`.
///
/// `ClusterArn`, `MaxResults` and `NextToken` are all `httpQuery` members
/// (`smithy_query(query="operation", operation="ListControlPanels")`).
async fn list_control_panels_page(
    router: &MultiServiceRouter,
    cluster_arn: Option<&str>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (u16, Option<String>, Vec<Value>) {
    let mut query: Vec<String> = Vec::new();
    if let Some(cluster) = cluster_arn {
        query.push(format!("ClusterArn={}", enc(cluster)));
    }
    if let Some(max) = max_results {
        query.push(format!("MaxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("NextToken={}", enc(token)));
    }
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let (status, body) = call(router, "GET", &format!("/controlpanels{suffix}"), None).await;
    let panels = body["ControlPanels"].as_array().cloned().unwrap_or_default();
    (
        status,
        body["NextToken"].as_str().map(ToString::to_string),
        panels,
    )
}

/// Page right through `ListControlPanels` for one scope and return every
/// `ControlPanelArn` it yielded, asserting each page honours `MaxResults`.
async fn drain_control_panels(
    router: &MultiServiceRouter,
    cluster_arn: Option<&str>,
    max_results: Option<i32>,
    expected_total: usize,
) -> Vec<String> {
    let case = format!("ClusterArn={cluster_arn:?} MaxResults={max_results:?}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let (status, next, panels) =
            list_control_panels_page(router, cluster_arn, max_results, token.as_deref()).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status}");
        if let Some(max) = max_results {
            assert!(
                panels.len() <= max as usize,
                "case {case}: a page holds at most {max} panels, got {}",
                panels.len()
            );
        }
        seen.extend(
            panels
                .iter()
                .map(|p| p["ControlPanelArn"].as_str().unwrap_or_default().to_string()),
        );
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case}: paging did not terminate after {pages} pages"
        );
        match next {
            None => break,
            Some(next) => token = Some(next),
        }
    }
    seen
}

/// `ListControlPanels` answers from the stored panels: every panel of the account, or
/// only those of the cluster `ClusterArn` names, in creation order, for every page size
/// from one to past the item count — and each listed body is the one
/// `DescribeControlPanel` answers with.
#[tokio::test]
async fn listing_control_panels_pages_through_every_stored_panel_once() {
    let router = router();

    // Two clusters, each created with its own default control panel, then
    // caller-created panels in a known order: the expected order is creation order.
    let first = create_cluster(&router, json!({ "ClusterName": "listing-first" })).await;
    let first_arn = first["ClusterArn"].as_str().unwrap_or_default().to_string();
    let second = create_cluster(&router, json!({ "ClusterName": "listing-second" })).await;
    let second_arn = second["ClusterArn"].as_str().unwrap_or_default().to_string();

    // A routing control created without a panel lands in its cluster's DEFAULT panel,
    // which is how the test learns that panel's ARN without listing.
    let default_of = |cluster: String| {
        let router = &router;
        async move {
            let control = create_routing_control(
                router,
                json!({ "ClusterArn": cluster, "RoutingControlName": "default-probe" }),
            )
            .await;
            control["ControlPanelArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
    };
    let default_first = default_of(first_arn.clone()).await;
    let default_second = default_of(second_arn.clone()).await;

    let mut created_first: Vec<String> = Vec::new();
    for name in ["first-a", "first-b"] {
        let panel = create_control_panel(
            &router,
            json!({ "ClusterArn": first_arn, "ControlPanelName": name }),
        )
        .await;
        created_first.push(
            panel["ControlPanelArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    let second_panel = create_control_panel(
        &router,
        json!({ "ClusterArn": second_arn, "ControlPanelName": "second-a" }),
    )
    .await;
    let second_panel_arn = second_panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let expected_first: Vec<String> = std::iter::once(default_first.clone())
        .chain(created_first.iter().cloned())
        .collect();
    let expected_second: Vec<String> =
        vec![default_second.clone(), second_panel_arn.clone()];
    // Account-wide: both default panels (minted with their clusters) precede the
    // caller-created ones, in the order the calls were made.
    let expected_all: Vec<String> = vec![
        default_first.clone(),
        default_second.clone(),
        created_first[0].clone(),
        created_first[1].clone(),
        second_panel_arn.clone(),
    ];

    // Rows span the scope (no filter, each cluster) and page sizes from one at a time,
    // through a size that does not divide the total, the exact total, one past it, and
    // the request that names no MaxResults.
    let scopes: &[(Option<&str>, &Vec<String>)] = &[
        (None, &expected_all),
        (Some(first_arn.as_str()), &expected_first),
        (Some(second_arn.as_str()), &expected_second),
    ];
    let page_sizes: &[Option<i32>] = &[Some(1), Some(2), Some(3), Some(5), Some(6), None];
    for (scope, expected) in scopes {
        for max_results in page_sizes {
            let seen = drain_control_panels(&router, *scope, *max_results, expected.len()).await;
            assert_eq!(
                &seen, *expected,
                "case ClusterArn={scope:?} MaxResults={max_results:?}: paging must yield every \
                 stored panel once, in creation order, got {seen:?}"
            );
        }
    }

    // Every listed body is the stored record, so it matches the single-resource reader
    // member for member — including the counted RoutingControlCount.
    let (_, _, listed) = list_control_panels_page(&router, None, None, None).await;
    for panel in &listed {
        let arn = panel["ControlPanelArn"].as_str().unwrap_or_default();
        let (status, body) = call(&router, "GET", &format!("/controlpanel/{}", enc(arn)), None).await;
        assert_eq!(
            status, 200,
            "case listed {arn}: expected DescribeControlPanel 200, got {status} {body:?}"
        );
        assert_eq!(
            &body["ControlPanel"], panel,
            "case listed {arn}: the list body must be the stored record, got {:?} after {panel:?}",
            body["ControlPanel"]
        );
    }
    // The default panels carry the probe control created above; the caller-created ones
    // carry none.
    for (arn, expected_count) in [
        (default_first.as_str(), 1),
        (default_second.as_str(), 1),
        (created_first[0].as_str(), 0),
        (second_panel_arn.as_str(), 0),
    ] {
        let listed_count = listed
            .iter()
            .find(|p| p["ControlPanelArn"].as_str() == Some(arn))
            .map(|p| p["RoutingControlCount"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            listed_count, json!(expected_count),
            "case listed {arn}: expected RoutingControlCount {expected_count}, got {listed_count:?}"
        );
    }

    // A deleted panel leaves the listing, and the list stays a pure read: listing twice
    // yields the same page.
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/controlpanel/{}", enc(&second_panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete a listed panel: expected 200, got {status} {body:?}"
    );
    let after = drain_control_panels(&router, Some(&second_arn), None, 2).await;
    assert_eq!(
        after,
        vec![default_second.clone()],
        "case after delete: the deleted panel must be gone, got {after:?}"
    );
    let again = drain_control_panels(&router, Some(&second_arn), None, 2).await;
    assert_eq!(
        again, after,
        "case repeated read: listing must not change what a later caller sees, got {again:?}"
    );
}

/// `ListControlPanels` refuses what it cannot serve: a `ClusterArn` no cluster is stored
/// under is 404, a cursor outside the selected set is 400, and `MaxResults` outside its
/// modelled range is 400 — while both published bounds are served.
#[tokio::test]
async fn listing_control_panels_refuses_an_unknown_scope_or_cursor() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "refusing" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let other = create_cluster(&router, json!({ "ClusterName": "refusing-other" })).await;
    let other_arn = other["ClusterArn"].as_str().unwrap_or_default().to_string();
    let other_panel = create_control_panel(
        &router,
        json!({ "ClusterArn": other_arn, "ControlPanelName": "other-panel" }),
    )
    .await;
    let other_panel_arn = other_panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A cluster ARN of the right shape that no create ever minted.
    let missing_cluster = format!("{ARN_PREFIX}cluster/00000000-0000-0000-0000-000000000000");
    let (status, body) = call(
        &router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&missing_cluster)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case ClusterArn naming no stored cluster: expected 404, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case ClusterArn naming no stored cluster: expected ResourceNotFoundException, got {body:?}"
    );

    // Cursor rows: a token naming nothing stored, in both scopes, and a token naming a
    // panel that IS stored but lies outside the filtered set.
    let unknown_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let cursors: &[(&str, Option<&str>, &str)] = &[
        ("unknown cursor, account-wide", None, unknown_panel.as_str()),
        (
            "unknown cursor, filtered",
            Some(cluster_arn.as_str()),
            unknown_panel.as_str(),
        ),
        (
            "cursor naming a panel of another cluster",
            Some(cluster_arn.as_str()),
            other_panel_arn.as_str(),
        ),
    ];
    for (case, scope, token) in cursors {
        let (status, _, _) = list_control_panels_page(&router, *scope, None, Some(token)).await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status}");
        let mut query = format!("NextToken={}", enc(token));
        if let Some(scope) = scope {
            query = format!("ClusterArn={}&{query}", enc(scope));
        }
        let (_, body) = call(&router, "GET", &format!("/controlpanels?{query}"), None).await;
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body:?}"
        );
    }

    // `MaxResults` is modelled `range 1..1000`: one step outside each bound is refused,
    // and each bound itself is served.
    for (case, max) in [
        ("MaxResults one below the range minimum", 0),
        ("MaxResults one above the range maximum", 1001),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/controlpanels?MaxResults={max}"),
            None,
        )
        .await;
        assert_eq!(status, 400, "case {case}: expected 400, got {status} {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: expected ValidationException, got {body:?}"
        );
    }
    for max in [1, 1000] {
        let (status, _, _) = list_control_panels_page(&router, None, Some(max), None).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max} (a published bound): expected 200, got {status}"
        );
    }
}
