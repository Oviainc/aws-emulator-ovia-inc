// SPDX-License-Identifier: CC0-1.0

//! An ACM certificate an Elastic Load Balancing listener serves, across the two services
//! that span: ELBv2 over awsQuery and Certificate Manager over `awsJson1_1`, both served
//! by ONE router over ONE engine, so what is asserted is what a client holding both SDKs
//! sees.
//!
//! RULE: a certificate is presented by the endpoint that terminates the connection, and
//! that endpoint is the LOAD BALANCER — so once a listener of a load balancer serves a
//! certificate, Certificate Manager reports that load balancer's ARN among the resources
//! using the certificate (`CertificateDetail$InUseBy`, "A list of ARNs for the Amazon Web
//! Services resources that are using the certificate") and refuses, with the modelled
//! `ResourceInUseException`, any call that would take the certificate away from it. The
//! reference is given up when the listener stops presenting it, and is accounted for
//! until the teardown it waits on falls due.
//!
//! The association comes from the ELBv2 store and is READ by Certificate Manager, which
//! is why this suite lives here: the certificate is requested through ACM's own
//! operation, the load balancer, target group and listener are created through ELBv2's,
//! and what ACM reports is what ELBv2 recorded rather than a copy of it.
//!
//! The rows differ along which TLS-terminating protocol the listener speaks and whether
//! the certificate is the listener's default one or an SNI entry of its certificate list.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE percent-encoder and element reader this test crate keeps for awsQuery suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{form_encode, opt_element};

/// The region every request below is signed for; the certificate ARN and the load
/// balancer ARN belong to it.
const REGION: &str = "us-west-2";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// How long to wait past a release for the teardown it waits on to fall due. ELBv2 states
/// the span its clock-owed moves take; this is it plus a margin, read from the service
/// rather than restated here.
fn release_window() -> Duration {
    service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::SETTLE_AFTER
        + Duration::from_millis(200)
}

/// One emulator carrying Elastic Load Balancing v2 and Certificate Manager over ONE
/// shared engine, which is what makes the hand-off real: the certificate ARN ACM answered
/// is handed to ELBv2, and the association ELBv2 records is what ACM then reports.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine),
    );
    router
}

/// One awsQuery call to the ELBv2 model, answered with its status and XML body.
async fn elbv2(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={ELB_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("elasticloadbalancingv2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}\
                 /elasticloadbalancingv2/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One ELBv2 call that must succeed, answered with its XML body.
async fn elbv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = elbv2(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} must answer 200, got {status} {body}"
    );
    body
}

/// One `awsJson1_1` call to Certificate Manager, answered with its status and parsed body.
async fn acm(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDACMTEST/20260101/{REGION}\
                 /acm/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Request one certificate from ACM and answer the ARN it minted.
async fn request_certificate(router: &MultiServiceRouter, case: &str, domain: &str) -> String {
    let (status, body) = acm(
        router,
        "RequestCertificate",
        &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RequestCertificate must answer 200, got {status} {body}"
    );
    body["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {body}"))
        .to_string()
}

/// The resources `DescribeCertificate` reports as using `certificate_arn`.
async fn in_use_by(router: &MultiServiceRouter, case: &str, certificate_arn: &str) -> Vec<String> {
    let (status, body) = acm(
        router,
        "DescribeCertificate",
        &json!({ "CertificateArn": certificate_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCertificate must answer 200, got {status} {body}"
    );
    body["Certificate"]["InUseBy"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no InUseBy in {body}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Where the certificate under test sits on the listener.
#[derive(Clone, Copy, Debug)]
enum Attached {
    /// The listener's DEFAULT certificate, which `CreateListener` sets.
    AsTheDefault,
    /// An SNI entry of its certificate LIST, which `AddListenerCertificates` adds.
    OnTheCertificateList,
}

/// RULE: a load balancer whose listener serves an ACM certificate is reported by ACM
/// among the resources using it, protects it from deletion while it does, and gives the
/// reference up once the listener stops serving it.
#[tokio::test]
async fn acm_reports_the_load_balancer_whose_listener_serves_the_certificate() {
    // The rows span the TLS-terminating protocols a listener's certificate is bracketed
    // to and both places a listener carries one.
    let rows: &[(&str, &str, Attached)] = &[
        (
            "an HTTPS listener's default certificate",
            "HTTPS",
            Attached::AsTheDefault,
        ),
        (
            "a TLS listener's default certificate",
            "TLS",
            Attached::AsTheDefault,
        ),
        (
            "an SNI entry of an HTTPS listener's certificate list",
            "HTTPS",
            Attached::OnTheCertificateList,
        ),
    ];

    for (case, protocol, attached) in rows {
        // One emulator per row, so each row's assertions are about its own store.
        let router = emulator();
        let subject = request_certificate(&router, case, "in-use.example.com").await;
        // The certificate the listener holds as its default on the row that attaches the
        // subject to the list instead, so the subject is never the listener's only one.
        let filler = request_certificate(&router, case, "filler.example.com").await;

        // Nothing is serving it yet.
        assert!(
            in_use_by(&router, case, &subject).await.is_empty(),
            "case {case:?}: a certificate no listener serves has no consumer, got {:?}",
            in_use_by(&router, case, &subject).await
        );

        let created = elbv2_ok(
            &router,
            case,
            "CreateLoadBalancer",
            &[
                ("Name", "in-use-alb"),
                ("Subnets.member.1", "subnet-11111111"),
                ("Subnets.member.2", "subnet-22222222"),
            ],
        )
        .await;
        let lb_arn = opt_element(&created, "LoadBalancerArn")
            .unwrap_or_else(|| panic!("case {case:?}: no LoadBalancerArn in {created}"));
        let created = elbv2_ok(
            &router,
            case,
            "CreateTargetGroup",
            &[
                ("Name", "in-use-tg"),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("TargetType", "ip"),
                ("VpcId", "vpc-12345678"),
            ],
        )
        .await;
        let tg_arn = opt_element(&created, "TargetGroupArn")
            .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {created}"));

        let default_certificate = match attached {
            Attached::AsTheDefault => subject.as_str(),
            Attached::OnTheCertificateList => filler.as_str(),
        };
        let created = elbv2_ok(
            &router,
            case,
            "CreateListener",
            &[
                ("LoadBalancerArn", &lb_arn),
                ("Protocol", protocol),
                ("Port", "443"),
                ("SslPolicy", "ELBSecurityPolicy-TLS13-1-2-2021-06"),
                ("Certificates.member.1.CertificateArn", default_certificate),
                ("DefaultActions.member.1.Type", "forward"),
                ("DefaultActions.member.1.TargetGroupArn", &tg_arn),
            ],
        )
        .await;
        let listener_arn = opt_element(&created, "ListenerArn")
            .unwrap_or_else(|| panic!("case {case:?}: no ListenerArn in {created}"));
        if matches!(attached, Attached::OnTheCertificateList) {
            elbv2_ok(
                &router,
                case,
                "AddListenerCertificates",
                &[
                    ("ListenerArn", &listener_arn),
                    ("Certificates.member.1.CertificateArn", &subject),
                ],
            )
            .await;
        }

        // The load balancer — not the listener — is the resource ACM names.
        assert_eq!(
            in_use_by(&router, case, &subject).await,
            vec![lb_arn.clone()],
            "case {case:?}: expected the load balancer ARN, got {:?}",
            in_use_by(&router, case, &subject).await
        );

        // And while it is serving the certificate, ACM will not let it be taken away.
        let (status, body) = acm(
            &router,
            "DeleteCertificate",
            &json!({ "CertificateArn": subject }),
        )
        .await;
        let code = body["__type"].as_str().unwrap_or_default();
        assert!(
            status == 400 && code.ends_with("ResourceInUseException"),
            "case {case:?}: DeleteCertificate must be refused while a load balancer serves \
             the certificate, got {status} {body}"
        );

        // The listener gone, the reference is given up: still accounted for until the
        // teardown falls due, and free after it.
        elbv2_ok(
            &router,
            case,
            "DeleteListener",
            &[("ListenerArn", &listener_arn)],
        )
        .await;
        assert_eq!(
            in_use_by(&router, case, &subject).await,
            vec![lb_arn.clone()],
            "case {case:?}: a release not yet due must leave the certificate in use, got {:?}",
            in_use_by(&router, case, &subject).await
        );
        tokio::time::sleep(release_window()).await;
        assert!(
            in_use_by(&router, case, &subject).await.is_empty(),
            "case {case:?}: a release that fell due must free the certificate, got {:?}",
            in_use_by(&router, case, &subject).await
        );

        // With nothing serving it, the delete ACM refused above goes through.
        let (status, body) = acm(
            &router,
            "DeleteCertificate",
            &json!({ "CertificateArn": subject }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteCertificate must answer 200 once nothing serves the \
             certificate, got {status} {body}"
        );
    }
}
