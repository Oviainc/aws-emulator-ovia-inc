// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Redshift::Cluster` resources its template declares — in Amazon Redshift.
//!
//! The rule under test: **a stack reaches its completed status only once the service that
//! owns each declared resource has created it, and the handle the stack publishes for that
//! resource (`Ref`) resolves in that service.** For a provisioned cluster the handle is its
//! `ClusterIdentifier`, so after the stack completes `redshift:DescribeClusters` on the
//! stack's own output answers with that cluster and the configuration the template
//! declared — rather than Amazon Redshift answering `ClusterNotFound` for a cluster its own
//! stack just published. Two crates have to agree for that to hold (CloudFormation makes
//! the call, Redshift serves it), so the suite lives here: a crate-local test can only ever
//! see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! cluster or leaves the name to CloudFormation, whether the cluster is single-node or
//! multi-node (and so how many nodes it has), which optional properties the declaration
//! carries, and whether it declares tags of its own. Every expected value is computed from
//! what the row declares, never read back out of the answer being checked.
//!
//! The deletion half is a row of the same rule: deleting the stack deletes the clusters it
//! created, so Amazon Redshift reports them absent afterwards.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's clusters
/// belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// and for the cluster's own provisioning span (`PROVISIONING_SPAN`, 500 ms) to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_250);

/// The admin password every row declares. It must never be echoed by either service.
const PASSWORD: &str = "Cfn-Redsh1ft-Pass";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, at `version`, with these form members.
///
/// The body is percent-encoded by the one encoder this test crate has
/// ([`crate::query_wire::form_encode`]), so a template full of `{`, `"` and `&` survives
/// it exactly as it does for every other awsQuery suite here.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon Redshift operation over its awsQuery wire.
async fn redshift(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "redshift", "2012-12-01", action, members).await
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// The logical id every row declares its cluster under, and so the resource the stack's
/// `Ref` and every assertion below address.
const CLUSTER_LOGICAL_ID: &str = "Warehouse";

/// One row of the table: a stack declaring ONE `AWS::Redshift::Cluster` and publishing its
/// `Ref`, with the members Amazon Redshift must then report for that cluster.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The `Properties` document the template declares, verbatim.
    properties: &'static str,
    /// The `ClusterIdentifier` the template declares, or `None` where CloudFormation
    /// generates the name — the row then only asserts that the published handle resolves.
    declared_identifier: Option<&'static str>,
    /// `(element, value)` pairs Amazon Redshift must report for the created cluster.
    reported: &'static [(&'static str, &'static str)],
}

/// The rows: a named single-node cluster, a multi-node cluster whose name CloudFormation
/// generates, and a tagged cluster carrying the optional properties a declaration may add.
const ROWS: &[Row] = &[
    Row {
        case: "a named single-node cluster",
        stack_name: "redshift-named",
        properties: concat!(
            r#"{"ClusterIdentifier":"cfn-named-warehouse","NodeType":"ra3.xlplus","#,
            r#""MasterUsername":"admin","MasterUserPassword":"Cfn-Redsh1ft-Pass","#,
            r#""ClusterType":"single-node","DBName":"analytics"}"#
        ),
        declared_identifier: Some("cfn-named-warehouse"),
        reported: &[
            ("NodeType", "ra3.xlplus"),
            ("MasterUsername", "admin"),
            ("DBName", "analytics"),
            ("NumberOfNodes", "1"),
        ],
    },
    Row {
        case: "a multi-node cluster CloudFormation names",
        stack_name: "redshift-generated",
        properties: concat!(
            r#"{"NodeType":"ra3.4xlarge","MasterUsername":"analyst","#,
            r#""MasterUserPassword":"Cfn-Redsh1ft-Pass","ClusterType":"multi-node","#,
            r#""NumberOfNodes":4}"#
        ),
        declared_identifier: None,
        reported: &[
            ("NodeType", "ra3.4xlarge"),
            ("MasterUsername", "analyst"),
            ("NumberOfNodes", "4"),
        ],
    },
    Row {
        case: "a tagged cluster carrying optional properties",
        stack_name: "redshift-tagged",
        properties: concat!(
            r#"{"ClusterIdentifier":"cfn-tagged-warehouse","NodeType":"ra3.xlplus","#,
            r#""MasterUsername":"admin","MasterUserPassword":"Cfn-Redsh1ft-Pass","#,
            r#""ClusterType":"single-node","Port":5555,"PubliclyAccessible":false,"#,
            r#""AutomatedSnapshotRetentionPeriod":7,"#,
            r#""PreferredMaintenanceWindow":"wed:04:00-wed:04:30","#,
            r#""Tags":[{"Key":"team","Value":"analytics"}]}"#
        ),
        declared_identifier: Some("cfn-tagged-warehouse"),
        reported: &[
            ("NodeType", "ra3.xlplus"),
            ("AutomatedSnapshotRetentionPeriod", "7"),
            ("PreferredMaintenanceWindow", "wed:04:00-wed:04:30"),
            ("Port", "5555"),
        ],
    },
];

/// The template one row deploys: its cluster under [`CLUSTER_LOGICAL_ID`], publishing the
/// identifier its `Ref` returns.
fn template_of(properties: &str) -> String {
    format!(
        r#"{{"Resources":{{"{CLUSTER_LOGICAL_ID}":{{"Type":"AWS::Redshift::Cluster","Properties":{properties}}}}},
             "Outputs":{{"ClusterId":{{"Value":{{"Ref":"{CLUSTER_LOGICAL_ID}"}}}}}}}}"#
    )
}

/// The `<Cluster>` Amazon Redshift reports for `identifier`, or the refusal it answered.
async fn describe_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, String) {
    redshift(
        router,
        "DescribeClusters",
        &[("ClusterIdentifier", identifier)],
    )
    .await
}

#[tokio::test]
async fn a_completed_stack_has_created_every_cluster_it_declares_in_redshift() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &template_of(row.properties),
        )
        .await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // The handle the stack publishes for the resource: "Ref returns the name of the
        // cluster", which is the identifier every Redshift call addresses it by.
        let identifier = outputs_of(&body)
            .get("ClusterId")
            .unwrap_or_else(|| panic!("case {case:?}: no ClusterId output in {body}"))
            .clone();
        if let Some(declared) = row.declared_identifier {
            assert_eq!(
                identifier, declared,
                "case {case:?}: expected the declared identifier to be the published handle, \
                 got {identifier:?}"
            );
        } else {
            // A name CloudFormation generated is still one Amazon Redshift accepts: the
            // identifier's documented syntax admits only lowercase letters, digits and
            // hyphens, and it must start with a letter.
            assert!(
                identifier.starts_with(|first: char| first.is_ascii_lowercase())
                    && identifier
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "case {case:?}: expected a generated identifier inside Redshift's documented \
                 syntax, got {identifier:?}"
            );
        }

        // THE RULE: the cluster the stack claims to have created is a cluster Amazon
        // Redshift holds, with the configuration the template declared.
        let (status, reported) = describe_cluster(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the cluster the stack created, got \
             {status} with body {reported}"
        );
        assert_eq!(
            element(&reported, "ClusterIdentifier"),
            identifier,
            "case {case:?}: expected the cluster to be the published handle's, got {reported}"
        );
        for (member, expected) in row.reported {
            assert_eq!(
                opt_element(&reported, member).as_deref(),
                Some(*expected),
                "case {case:?}: expected {member}={expected:?} on the created cluster, got {:?}",
                opt_element(&reported, member)
            );
        }
        assert!(
            !reported.contains(PASSWORD),
            "case {case:?}: the admin password must never reach the wire, got {reported}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_cluster_it_created() {
    let case = "delete the stack that created the cluster";
    let stack_name = "redshift-deleted";
    let identifier = "cfn-deleted-warehouse";
    let router = build_router();

    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &template_of(&format!(
            r#"{{"ClusterIdentifier":"{identifier}","NodeType":"ra3.xlplus","MasterUsername":"admin","MasterUserPassword":"{PASSWORD}","ClusterType":"single-node"}}"#
        )),
    )
    .await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let (status, reported) = describe_cluster(&router, identifier).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the cluster to exist before the delete, got {status} with \
         body {reported}"
    );

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // The cluster the stack created is gone from Amazon Redshift, which is what makes the
    // stack's resources the stack's own rather than leftovers.
    let (status, reported) = describe_cluster(&router, identifier).await;
    assert_eq!(
        (status, elements(&reported, "Code")),
        (404, vec!["ClusterNotFound".to_string()]),
        "case {case:?}: expected the cluster to be gone with the stack, got {status} with \
         body {reported}"
    );
}
