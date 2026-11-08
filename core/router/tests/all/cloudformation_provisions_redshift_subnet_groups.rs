// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Redshift::ClusterSubnetGroup` has really
//! created that group — in Amazon Redshift — so a dependent `AWS::Redshift::Cluster` in the
//! same stack resolves it and the stack completes.
//!
//! The rule under test: **a stack reaches its completed status only once the service that
//! owns each declared resource has created it, and the handle the stack publishes for that
//! resource (`Ref`) resolves in that service** — so a sibling resource whose property is a
//! `Ref` to it is configured with a name the owning service really holds. For a cluster
//! subnet group the handle is its `ClusterSubnetGroupName`, and Amazon Redshift resolves a
//! cluster's `ClusterSubnetGroupName` against the groups it HOLDS before it will create a
//! cluster in one (`ClusterSubnetGroupNotFoundFault`) — so a stack declaring both can only
//! complete if the group was created for real. Three crates have to agree for that to hold
//! (CloudFormation makes the calls, Amazon EC2 owns the subnets the group is made of, and
//! Redshift serves the group and the cluster), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: how many subnets the group is made
//! of and how many zones they cover, whether the group's description and tags are declared,
//! and whether a cluster in the same stack references the group through `Ref` or the stack
//! declares the group alone.
//!
//! The deletion half is a row of the same rule: deleting the stack deletes the group it
//! created, so Amazon Redshift reports it absent afterwards.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for a created cluster's own provisioning span (`PROVISIONING_SPAN`, 500 ms) to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_500);

/// The admin password the cluster row declares. It must never be echoed by either service.
const PASSWORD: &str = "Cfn-Redsh1ft-Pass";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
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
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
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

/// Call one query-protocol operation of `service`, at `version`, with these form members.
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

/// The logical id every row declares its subnet group under, and the one the cluster row
/// declares its cluster under.
const GROUP_LOGICAL_ID: &str = "ClusterSubnetGroup";
const CLUSTER_LOGICAL_ID: &str = "RedshiftCluster";

/// The `<ClusterSubnetGroup>` Amazon Redshift reports for `name`, or the refusal it
/// answered.
async fn describe_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    redshift(
        router,
        "DescribeClusterSubnetGroups",
        &[("ClusterSubnetGroupName", name)],
    )
    .await
}

/// One row of the table: a stack declaring a VPC, `subnets` subnets of it and ONE
/// `AWS::Redshift::ClusterSubnetGroup` over them.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// How many subnets the group is made of, each in a zone of its own.
    subnets: usize,
    /// The `Description` the group declares.
    description: &'static str,
    /// The `Tags` document the group declares, or `""` for a group that declares none.
    tags: &'static str,
    /// Whether the stack also declares a cluster whose `ClusterSubnetGroupName` is a `Ref`
    /// to the group.
    with_cluster: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a two-subnet group a cluster in the same stack is launched into",
        stack_name: "redshift-subnets-with-cluster",
        subnets: 2,
        description: "Subnets for the warehouse",
        tags: "",
        with_cluster: true,
    },
    Row {
        case: "a three-subnet tagged group the stack declares alone",
        stack_name: "redshift-subnets-alone",
        subnets: 3,
        description: "Three zones",
        tags: r#","Tags":[{"Key":"team","Value":"analytics"}]"#,
        with_cluster: false,
    },
    Row {
        case: "a single-subnet group a cluster is launched into",
        stack_name: "redshift-subnets-single",
        subnets: 1,
        description: "One zone only",
        tags: "",
        with_cluster: true,
    },
];

/// The Availability Zones a row's subnets are spread over, as the zone SUFFIXES a
/// CloudFormation template names them by (`Fn::Select` over `Fn::GetAZs` would resolve the
/// same list; the suffix keeps the template literal).
const ZONE_LETTERS: [&str; 3] = ["a", "b", "c"];

/// The template one row deploys: a VPC, its subnets, the subnet group over them, and —
/// where the row asks for one — a cluster whose `ClusterSubnetGroupName` is a `Ref` to the
/// group. The group's name is published as an output, which is what `Ref` returns for it.
fn template_of(row: &Row) -> String {
    let mut resources =
        r#""Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.42.0.0/16"}}"#.to_string();
    let mut subnet_refs = Vec::new();
    for index in 0..row.subnets {
        let logical = format!("Subnet{index}");
        resources.push_str(&format!(
            r#","{logical}":{{"Type":"AWS::EC2::Subnet","Properties":{{"VpcId":{{"Ref":"Vpc"}},"CidrBlock":"10.42.{index}.0/24","AvailabilityZone":"{REGION}{}"}}}}"#,
            ZONE_LETTERS[index % ZONE_LETTERS.len()]
        ));
        subnet_refs.push(format!(r#"{{"Ref":"{logical}"}}"#));
    }
    resources.push_str(&format!(
        r#","{GROUP_LOGICAL_ID}":{{"Type":"AWS::Redshift::ClusterSubnetGroup","Properties":{{"Description":"{}","SubnetIds":[{}]{}}}}}"#,
        row.description,
        subnet_refs.join(","),
        row.tags
    ));
    let mut outputs = format!(
        r#""GroupName":{{"Value":{{"Ref":"{GROUP_LOGICAL_ID}"}}}}"#
    );
    if row.with_cluster {
        resources.push_str(&format!(
            r#","{CLUSTER_LOGICAL_ID}":{{"Type":"AWS::Redshift::Cluster","Properties":{{"ClusterType":"single-node","NodeType":"ra3.xlplus","MasterUsername":"admin","MasterUserPassword":"{PASSWORD}","ClusterSubnetGroupName":{{"Ref":"{GROUP_LOGICAL_ID}"}}}}}}"#
        ));
        outputs.push_str(&format!(
            r#","ClusterId":{{"Value":{{"Ref":"{CLUSTER_LOGICAL_ID}"}}}}"#
        ));
    }
    format!(r#"{{"Resources":{{{resources}}},"Outputs":{{{outputs}}}}}"#)
}

#[tokio::test]
async fn a_completed_stack_has_created_its_subnet_group_in_redshift() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &template_of(row)).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body}"
        );

        // The handle the stack publishes for the group: "Ref returns the name of the
        // cluster subnet group", which is what every Redshift call addresses it by.
        let outputs = outputs_of(&body);
        let group_name = outputs
            .get("GroupName")
            .unwrap_or_else(|| panic!("case {case:?}: no GroupName output in {body}"))
            .clone();

        // THE RULE: the group the stack claims to have created is a group Amazon Redshift
        // holds, with the configuration the template declared.
        let (status, reported) = describe_group(&router, &group_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the group the stack created, got \
             {status} with body {reported}"
        );
        let group = block_of(&reported, "ClusterSubnetGroup")
            .unwrap_or_else(|| panic!("case {case:?}: no ClusterSubnetGroup in {reported}"));
        assert_eq!(
            opt_element(group, "Description").as_deref(),
            Some(row.description),
            "case {case:?}: expected the declared description, got {group}"
        );
        // One subnet per declared `SubnetIds` entry, and every one of them a subnet Amazon
        // EC2 really minted — the group could not have been created otherwise, because
        // Redshift resolves each identifier against EC2 before it stores a group.
        let subnet_ids = elements(group, "SubnetIdentifier");
        assert_eq!(
            subnet_ids.len(),
            row.subnets,
            "case {case:?}: expected one subnet per declared entry, got {subnet_ids:?}"
        );
        assert!(
            subnet_ids.iter().all(|id| id.starts_with("subnet-")),
            "case {case:?}: expected every subnet to be one EC2 minted, got {subnet_ids:?}"
        );
        assert!(
            opt_element(group, "VpcId").is_some_and(|vpc| vpc.starts_with("vpc-")),
            "case {case:?}: expected the group to report the VPC its subnets are in, got \
             {group}"
        );

        if !row.with_cluster {
            continue;
        }
        // And the sibling resource that referenced it resolved: the cluster exists in
        // Redshift and is launched into exactly that group.
        let cluster_id = outputs
            .get("ClusterId")
            .unwrap_or_else(|| panic!("case {case:?}: no ClusterId output in {body}"))
            .clone();
        let (status, cluster) = redshift(
            &router,
            "DescribeClusters",
            &[("ClusterIdentifier", cluster_id.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Redshift to report the cluster the stack created, got \
             {status} with body {cluster}"
        );
        assert_eq!(
            element(&cluster, "ClusterSubnetGroupName"),
            group_name.to_ascii_lowercase(),
            "case {case:?}: expected the cluster to be launched into the stack's own group, \
             got {cluster}"
        );
        assert!(
            !cluster.contains(PASSWORD),
            "case {case:?}: the admin password must never reach the wire, got {cluster}"
        );
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_subnet_group_it_created() {
    let case = "delete the stack that created the subnet group";
    let stack_name = "redshift-subnets-deleted";
    let router = build_router();

    let row = Row {
        case,
        stack_name,
        subnets: 2,
        description: "Subnets to be deleted",
        tags: "",
        with_cluster: false,
    };
    let body = create_and_settle(&router, case, stack_name, &template_of(&row)).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let group_name = outputs_of(&body)
        .get("GroupName")
        .unwrap_or_else(|| panic!("case {case:?}: no GroupName output in {body}"))
        .clone();
    let (status, reported) = describe_group(&router, &group_name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the group to exist before the delete, got {status} with \
         body {reported}"
    );

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // The group the stack created is gone from Amazon Redshift, which is what makes the
    // stack's resources the stack's own rather than leftovers.
    let (status, reported) = describe_group(&router, &group_name).await;
    assert_eq!(
        (status, elements(&reported, "Code")),
        (400, vec!["ClusterSubnetGroupNotFoundFault".to_string()]),
        "case {case:?}: expected the group to be gone with the stack, got {status} with body \
         {reported}"
    );
}
