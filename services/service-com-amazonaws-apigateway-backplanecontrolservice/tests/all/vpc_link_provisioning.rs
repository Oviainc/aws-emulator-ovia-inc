// SPDX-License-Identifier: CC0-1.0

//! How a VPC link's provisioning SETTLES, and what deleting one does, asserted on the
//! wire.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateVpcLink`/`GetVpcLink` helpers are this crate's existing harness, reused from
//! `rest_api_lifecycle` and `vpc_links`.
//!
//! The RULES under test:
//!
//! * creating a VPC link is asynchronous, and which state it settles in is decided by
//!   what it TARGETS: a link every target of which names a network load balancer settles
//!   `AVAILABLE`, and one whose targets do not settle `FAILED` with a `statusMessage`
//!   saying which target could not be provisioned — never `AVAILABLE`, which would
//!   report a link that can never carry traffic as healthy;
//! * `DeleteVpcLink` removes the link, so a later read answers the modelled
//!   `NotFoundException`, and is REFUSED while an `Integration` of some API still
//!   reaches the link through its `connectionId` — the link survives that refusal.
//!
//! The tables span what the rules generalise over: which resource a target ARN names
//! (an application load balancer, a VPC endpoint service, a bucket, a load balancer of
//! another vendor's namespace), how many targets a link carries and which of them is
//! unusable, and the forms a delete can be refused or accepted in.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    VPC_LINK_AVAILABLE, VPC_LINK_FAILED, VPC_LINK_PENDING, VPC_LINK_PROVISION_SPAN,
};

use crate::method_integration::{put_method, resource_with_a_method};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Gateway};
use crate::vpc_links::{create_vpc_link, get_vpc_link, target_arn};

/// Slack added to a stored span before asserting the state it is due in, so a loaded
/// machine does not read the record one tick early. The sibling `vpc_links` suite waits
/// the same way.
const MARGIN: Duration = Duration::from_millis(100);

/// A created VPC link's id, asserting the modelled 202 on the way.
async fn created_link(gateway: &Gateway, name: &str, targets: &[String]) -> String {
    let created = create_vpc_link(
        gateway,
        json!({ "name": name, "targetArns": targets }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case CreateVpcLink({name}): expected 202, got {} with {:?}",
        created.status, created.body
    );
    // A create answers while the link is still being provisioned, so the state it
    // settles in is not visible yet.
    assert_eq!(
        created.body["status"].as_str(),
        Some(VPC_LINK_PENDING),
        "case CreateVpcLink({name}): expected {VPC_LINK_PENDING}, got {:?}",
        created.body["status"]
    );
    string_member(&created.body, "id", name)
}

#[tokio::test]
async fn a_vpc_link_settles_failed_unless_every_target_names_a_network_load_balancer() {
    // (case, the targets, the state the link must settle in)
    //
    // The usable rows name network load balancers — the resource `targetArns`
    // documents — in one and in several Regions and accounts; the unusable ones name an
    // APPLICATION load balancer (the other kind Elastic Load Balancing writes under the
    // same `loadbalancer/` prefix), a VPC endpoint service, a bucket, a load balancer
    // ARN with no name after the kind, and a mix where only one target of several is
    // unusable.
    let rows: Vec<(&str, Vec<String>, &str)> = vec![
        (
            "one network load balancer",
            vec![target_arn(0)],
            VPC_LINK_AVAILABLE,
        ),
        (
            "three network load balancers",
            vec![target_arn(1), target_arn(2), target_arn(3)],
            VPC_LINK_AVAILABLE,
        ),
        (
            "a network load balancer in another Region and account",
            vec![
                "arn:aws:elasticloadbalancing:eu-west-1:111122223333:loadbalancer/net/other/abc123"
                    .to_string(),
            ],
            VPC_LINK_AVAILABLE,
        ),
        (
            "an application load balancer",
            vec![
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/app/alb/abc123"
                    .to_string(),
            ],
            VPC_LINK_FAILED,
        ),
        (
            "a gateway load balancer",
            vec![
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/gwy/gwlb/abc123"
                    .to_string(),
            ],
            VPC_LINK_FAILED,
        ),
        (
            "a VPC endpoint service",
            vec![
                "arn:aws:ec2:us-east-1:000000000000:vpc-endpoint-service/vpce-svc-1".to_string(),
            ],
            VPC_LINK_FAILED,
        ),
        (
            "a bucket",
            vec!["arn:aws:s3:::a-bucket".to_string()],
            VPC_LINK_FAILED,
        ),
        (
            "a load balancer ARN naming no load balancer",
            vec![
                "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/"
                    .to_string(),
            ],
            VPC_LINK_FAILED,
        ),
        (
            "two usable targets and one that is not",
            vec![
                target_arn(4),
                "arn:aws:ec2:us-east-1:000000000000:vpc-endpoint-service/vpce-svc-2".to_string(),
                target_arn(5),
            ],
            VPC_LINK_FAILED,
        ),
    ];

    let gateway = Gateway::new();
    let mut links: Vec<(String, String, String)> = Vec::new();
    for (case, targets, expected) in &rows {
        let id = created_link(&gateway, case, targets).await;
        links.push(((*case).to_string(), id, (*expected).to_string()));
    }

    // The transition is one the CLOCK owes: once the provisioning span has passed every
    // link reports the state its targets decided — for every reader, not just the first,
    // because nothing about the read moves it.
    tokio::time::sleep(VPC_LINK_PROVISION_SPAN + MARGIN).await;
    for round in 0..2 {
        for (case, id, expected) in &links {
            let read = get_vpc_link(&gateway, id).await;
            assert_eq!(
                read.status, 200,
                "case {case} (round {round}): expected 200 from GetVpcLink, got {} with {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body["status"].as_str(),
                Some(expected.as_str()),
                "case {case} (round {round}): expected the link to settle {expected}, got {:?}",
                read.body
            );
            // A failed link says WHICH target it could not be provisioned against, and a
            // provisioned one carries no description of its status at all.
            let status_message = read.body["statusMessage"].as_str();
            if expected == VPC_LINK_FAILED {
                let stated = status_message.unwrap_or_default();
                assert!(
                    stated.contains("does not name a network load balancer"),
                    "case {case} (round {round}): expected the reason for the failure, \
                     got {status_message:?}"
                );
            } else {
                assert_eq!(
                    status_message, None,
                    "case {case} (round {round}): expected no status description, got \
                     {status_message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_vpc_link_removes_it_unless_an_integration_still_reaches_it() {
    let gateway = Gateway::new();

    // A link nothing references is removed, and the removal is what makes a later read —
    // and a second delete of the same id — the modelled not-found.
    let id = created_link(&gateway, "the-unreferenced-one", &[target_arn(0)]).await;
    let deleted = delete_vpc_link(&gateway, &id).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteVpcLink(unreferenced): expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_failure(
        &get_vpc_link(&gateway, &id).await,
        404,
        "NotFoundException",
        "GetVpcLink after the delete",
    );
    assert_failure(
        &delete_vpc_link(&gateway, &id).await,
        404,
        "NotFoundException",
        "DeleteVpcLink twice",
    );

    // An id no link ever answered to is the same not-found, in the forms an id can take.
    for case in ["zzzzz99999", "abc", "the-unreferenced-one"] {
        assert_failure(
            &delete_vpc_link(&gateway, case).await,
            404,
            "NotFoundException",
            &format!("DeleteVpcLink({case})"),
        );
    }

    // A link an `Integration` reaches through its `connectionId` is NOT removed: the
    // delete is refused while that reference stands, the refusal names the link and the
    // method whose integration holds it, and the link is still readable afterwards.
    let referenced = created_link(&gateway, "the-referenced-one", &[target_arn(1)]).await;
    let (api, root) = create_api(&gateway, "vpc-link-in-use").await;
    let resource = resource_with_a_method(
        &gateway,
        &api,
        &root,
        "pets",
        "GET",
        serde_json::Value::Null,
    )
    .await;
    put_private_integration(&gateway, &api, &resource, "GET", &referenced).await;

    let refused = delete_vpc_link(&gateway, &referenced).await;
    assert_failure(
        &refused,
        400,
        "BadRequestException",
        "DeleteVpcLink while an integration reaches it",
    );
    let message = refused.body["message"].as_str().unwrap_or_default();
    // The refusal names the link by its ID and the reference as the triple API Gateway
    // states it in — `[RESTApiId:Method:Resource]`; see the `vpc_link_in_use` rule.
    for fragment in [
        referenced.clone(),
        format!("[{api}:GET:{resource}]"),
    ] {
        assert!(
            message.contains(&fragment),
            "case DeleteVpcLink while an integration reaches it: expected the refusal to \
             name {fragment:?}, got {message:?}"
        );
    }
    let read = get_vpc_link(&gateway, &referenced).await;
    assert_eq!(
        read.status, 200,
        "case GetVpcLink after the refused delete: expected 200, got {} with {:?}",
        read.status, read.body
    );

    // Clearing the reference is the caller's own call, and the delete then succeeds —
    // which is what says the refusal was about the reference and not about the link.
    let dropped = gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            None,
        )
        .await;
    assert_eq!(
        dropped.status, 204,
        "case DeleteIntegration: expected 204, got {} with {:?}",
        dropped.status, dropped.body
    );
    let deleted = delete_vpc_link(&gateway, &referenced).await;
    assert_eq!(
        deleted.status, 202,
        "case DeleteVpcLink once the reference is gone: expected 202, got {} with {:?}",
        deleted.status, deleted.body
    );

    // A second method on another resource, so the walk that finds a reference is shown
    // to cover more than one method: the link is blocked while either holds it.
    let blocking = created_link(&gateway, "the-second-referenced-one", &[target_arn(2)]).await;
    put_method(&gateway, &api, &resource, "POST").await;
    put_private_integration(&gateway, &api, &resource, "POST", &blocking).await;
    assert_failure(
        &delete_vpc_link(&gateway, &blocking).await,
        400,
        "BadRequestException",
        "DeleteVpcLink while another method's integration reaches it",
    );
}

/// `DeleteVpcLink` for `vpc_link_id`.
async fn delete_vpc_link(
    gateway: &Gateway,
    vpc_link_id: &str,
) -> crate::rest_api_lifecycle::Answer {
    gateway
        .call("DELETE", &format!("/vpclinks/{vpc_link_id}"), None)
        .await
}

/// `PutIntegration` of a VPC_LINK integration on `verb` of `resource`, which is what
/// makes the method REACH `vpc_link_id`.
async fn put_private_integration(
    gateway: &Gateway,
    api: &str,
    resource: &str,
    verb: &str,
    vpc_link_id: &str,
) {
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
            Some(json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": "http://internal.example/pets",
                "connectionType": "VPC_LINK",
                "connectionId": vpc_link_id,
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutIntegration({verb}, connectionId={vpc_link_id}): expected 201, got {} \
         with {:?}",
        answer.status, answer.body
    );
}
