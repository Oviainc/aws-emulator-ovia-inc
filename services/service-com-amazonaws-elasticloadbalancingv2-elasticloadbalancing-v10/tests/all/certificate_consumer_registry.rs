// SPDX-License-Identifier: CC0-1.0

//! Which load balancer PRESENTS a Certificate Manager certificate, as the service that
//! holds that certificate sees it.
//!
//! RULE: a listener that terminates TLS serves certificates this service does not own, so
//! the load balancer the listener belongs to is recorded as the CONSUMER of every
//! Certificate Manager certificate its listeners present — the default certificate of
//! each and every entry of their certificate lists — in the one shared directory the
//! holding service reads; and the whole set is rewritten by every call that changes it, a
//! reference the load balancer gives up staying accounted for until the teardown it waits
//! on falls due.
//!
//! The directory is read here exactly as its reader reads it: the releases the clock has
//! already made due are applied first, and the question is asked from the CERTIFICATE
//! side ([`consumers`]).
//!
//! The rows span what the rule generalises over: which TLS-terminating protocol the
//! listener speaks, whether the certificate is the listener's default one or an SNI list
//! entry, how many listeners of one load balancer present it, how many load balancers
//! present the same certificate, how many certificates one load balancer presents, and —
//! for the release — which call gives the reference up.
//!
//! Every ARN asserted on is one the service answered, and the certificate ARNs are built
//! from the two forms `Certificate$CertificateArn` carries: a Certificate Manager
//! certificate, which this directory is about, and a server certificate uploaded to IAM,
//! which is a resource of another service and so no entry of it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::SETTLE_AFTER;
use service_common::acm_certificate_associations::CertificateAssociationDirectory;
use smithy_server_runtime::MultiServiceRouter;

use crate::listener_rules::create_load_balancer;
use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, p, Reply, REGION,
};

/// A Certificate Manager certificate ARN, numbered — the form
/// `service_common::arn::ACM_CERTIFICATE` admits
/// (`arn:aws:acm:<region>:<account>:certificate/<id>`), built for the Region and account
/// this suite's requests are signed for.
fn acm_certificate(index: u8) -> String {
    format!(
        "arn:aws:acm:{REGION}:{}:certificate/{index:08x}-1111-1111-1111-111111111111",
        account()
    )
}

/// A server certificate uploaded to IAM, the OTHER form a listener's certificate ARN
/// carries. It names a resource of IAM rather than of Certificate Manager, so no load
/// balancer presenting it is a consumer in this directory.
fn iam_server_certificate(name: &str) -> String {
    format!("arn:aws:iam::{}:server-certificate/{name}", account())
}

/// The resources the shared directory reports as using `certificate_arn`, read the way
/// its reader reads it: every release the clock has already made due applied first, then
/// the question asked from the certificate side.
fn consumers(engine: &Arc<engine::InMemoryEngine>, certificate_arn: &str) -> Vec<String> {
    let directory = engine.state::<CertificateAssociationDirectory>();
    let mut directory = directory.lock();
    directory.materialize_due(SystemTime::now());
    directory.resources_using(certificate_arn)
}

/// Create a TLS-terminating listener carrying `certificate_arn` as its default
/// certificate and answer the reply, whatever it says.
async fn create_tls_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    tg_arn: &str,
    protocol: &str,
    port: i32,
    certificate_arn: &str,
) -> Reply {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", protocol));
    form.push(p("Port", &port.to_string()));
    form.push(p("Certificates.member.1.CertificateArn", certificate_arn));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    call(router, &form).await
}

/// Create that listener and require the 200, answering its ARN.
async fn tls_listener(
    router: &MultiServiceRouter,
    case: &str,
    lb_arn: &str,
    tg_arn: &str,
    protocol: &str,
    port: i32,
    certificate_arn: &str,
) -> String {
    let reply =
        create_tls_listener(router, lb_arn, tg_arn, protocol, port, certificate_arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: CreateListener must answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case {case:?}: no ListenerArn in {}", reply.body))
}

/// Add `certificate_arns` to one listener's certificate list, requiring the 200.
async fn add_listener_certificates(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
    certificate_arns: &[String],
) {
    let mut form = action("AddListenerCertificates");
    form.push(p("ListenerArn", listener_arn));
    for (index, arn) in certificate_arns.iter().enumerate() {
        form.push(p(
            &format!("Certificates.member.{}.CertificateArn", index + 1),
            arn,
        ));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: AddListenerCertificates must answer 200, got {} {}",
        reply.status, reply.body
    );
}

/// How one row attaches the certificate under test to its listener.
#[derive(Clone, Copy, Debug)]
enum Attached {
    /// As the listener's DEFAULT certificate — "[HTTPS and TLS listeners] The default
    /// certificate for the listener."
    AsTheDefault,
    /// As an entry of the listener's CERTIFICATE LIST, which `AddListenerCertificates`
    /// puts there for SNI.
    OnTheCertificateList,
}

/// One row of the consumer table.
struct ConsumerRow {
    case: &'static str,
    /// The TLS-terminating protocol each listener of the row speaks.
    protocol: &'static str,
    /// How the certificate under test is attached.
    attached: Attached,
    /// How many load balancers present it.
    load_balancers: usize,
    /// How many listeners of EACH of those load balancers present it.
    listeners_each: usize,
}

/// RULE: every load balancer whose listeners present a Certificate Manager certificate is
/// recorded as a consumer of it, once, and a load balancer presenting none is recorded for
/// none.
#[tokio::test]
async fn a_load_balancer_presenting_a_certificate_is_recorded_as_its_consumer() {
    // Rows differ along the protocol, where the certificate is attached, how many load
    // balancers present it and how many listeners of each do.
    let rows: &[ConsumerRow] = &[
        ConsumerRow {
            case: "an HTTPS listener's default certificate",
            protocol: "HTTPS",
            attached: Attached::AsTheDefault,
            load_balancers: 1,
            listeners_each: 1,
        },
        ConsumerRow {
            case: "a TLS listener's default certificate",
            protocol: "TLS",
            attached: Attached::AsTheDefault,
            load_balancers: 1,
            listeners_each: 1,
        },
        ConsumerRow {
            case: "an SNI entry of an HTTPS listener's certificate list",
            protocol: "HTTPS",
            attached: Attached::OnTheCertificateList,
            load_balancers: 1,
            listeners_each: 1,
        },
        ConsumerRow {
            case: "two listeners of one load balancer presenting the same certificate",
            protocol: "HTTPS",
            attached: Attached::AsTheDefault,
            load_balancers: 1,
            listeners_each: 2,
        },
        ConsumerRow {
            case: "two load balancers presenting the same certificate",
            protocol: "HTTPS",
            attached: Attached::OnTheCertificateList,
            load_balancers: 2,
            listeners_each: 1,
        },
    ];

    for row in rows {
        let case = row.case;
        // One engine per row, so each row's assertions are about its own store.
        let (router, engine) = emulator();
        let tg_arn = create_http_target_group(&router, "consumer-tg").await;
        // The certificate under test, the one every listener of the row also holds as
        // its default when the row attaches the subject to the list instead, and a
        // certificate nothing presents at all.
        let subject = acm_certificate(1);
        let default_filler = acm_certificate(2);
        let unused = acm_certificate(9);

        assert!(
            consumers(&engine, &subject).is_empty(),
            "case {case:?}: nothing presents the certificate yet, got {:?}",
            consumers(&engine, &subject)
        );

        let mut expected: Vec<String> = Vec::new();
        for which in 0..row.load_balancers {
            let lb_arn =
                create_load_balancer(&router, &format!("consumer-alb-{which}")).await;
            for listener in 0..row.listeners_each {
                let port = 443 + listener as i32;
                match row.attached {
                    Attached::AsTheDefault => {
                        tls_listener(
                            &router, case, &lb_arn, &tg_arn, row.protocol, port, &subject,
                        )
                        .await;
                    }
                    Attached::OnTheCertificateList => {
                        let listener_arn = tls_listener(
                            &router,
                            case,
                            &lb_arn,
                            &tg_arn,
                            row.protocol,
                            port,
                            &default_filler,
                        )
                        .await;
                        add_listener_certificates(
                            &router,
                            case,
                            &listener_arn,
                            std::slice::from_ref(&subject),
                        )
                        .await;
                    }
                }
            }
            expected.push(lb_arn);
        }
        expected.sort();

        assert_eq!(
            consumers(&engine, &subject),
            expected,
            "case {case:?}: expected the load balancers presenting it, got {:?}",
            consumers(&engine, &subject)
        );
        assert!(
            consumers(&engine, &unused).is_empty(),
            "case {case:?}: a certificate no listener names must have no consumer, got {:?}",
            consumers(&engine, &unused)
        );
    }
}

/// RULE, read from the other side: a certificate ARN that names a resource of IAM rather
/// than of Certificate Manager records no consumer in this directory, while the
/// Certificate Manager certificate the SAME listener presents does.
#[tokio::test]
async fn only_a_certificate_manager_certificate_records_a_consumer() {
    let (router, engine) = emulator();
    let tg_arn = create_http_target_group(&router, "kind-tg").await;
    let lb_arn = create_load_balancer(&router, "kind-alb").await;

    // The listener's default certificate is the server certificate uploaded to IAM; the
    // Certificate Manager certificate sits on its list, so one listener presents one of
    // each.
    let iam = iam_server_certificate("kind-server-certificate");
    let acm = acm_certificate(3);
    let listener_arn =
        tls_listener(&router, "an IAM server certificate", &lb_arn, &tg_arn, "HTTPS", 443, &iam)
            .await;
    add_listener_certificates(
        &router,
        "an IAM server certificate",
        &listener_arn,
        std::slice::from_ref(&acm),
    )
    .await;

    assert!(
        consumers(&engine, &iam).is_empty(),
        "case an IAM server certificate: expected no consumer of a certificate another \
         service holds, got {:?}",
        consumers(&engine, &iam)
    );
    assert_eq!(
        consumers(&engine, &acm),
        vec![lb_arn.clone()],
        "case an IAM server certificate: the Certificate Manager certificate on the same \
         listener must still name the load balancer, got {:?}",
        consumers(&engine, &acm)
    );
}

/// How one row of the release table gives the reference up.
#[derive(Clone, Copy, Debug)]
enum Released {
    /// `ModifyListener` points the listener at another default certificate.
    ModifyingTheListener,
    /// `RemoveListenerCertificates` takes the certificate off the list.
    RemovingTheListenerCertificate,
    /// `DeleteListener` removes the listener that presented it.
    DeletingTheListener,
    /// `DeleteLoadBalancer` removes the load balancer and its listeners with it.
    DeletingTheLoadBalancer,
}

impl Released {
    /// Whether the row attaches the certificate under test to the listener's certificate
    /// LIST rather than as its default — the removal row does, because the default
    /// certificate is not what `RemoveListenerCertificates` removes.
    fn on_the_certificate_list(self) -> bool {
        matches!(self, Released::RemovingTheListenerCertificate)
    }
}

/// RULE: a reference a load balancer gives up is still accounted for until the teardown
/// it waits on falls due, and is gone once it has — whichever call gives it up.
#[tokio::test]
async fn a_reference_a_load_balancer_gives_up_is_held_until_its_release_falls_due() {
    let rows: &[(&str, Released)] = &[
        (
            "the listener pointed at another certificate",
            Released::ModifyingTheListener,
        ),
        (
            "the certificate taken off the listener's list",
            Released::RemovingTheListenerCertificate,
        ),
        ("the listener deleted", Released::DeletingTheListener),
        (
            "the load balancer deleted",
            Released::DeletingTheLoadBalancer,
        ),
    ];

    for (case, released) in rows {
        let (router, engine) = emulator();
        let tg_arn = create_http_target_group(&router, "release-tg").await;
        let lb_arn = create_load_balancer(&router, "release-alb").await;
        let subject = acm_certificate(4);
        let successor = acm_certificate(5);

        let listener_arn = if released.on_the_certificate_list() {
            let listener_arn = tls_listener(
                &router, case, &lb_arn, &tg_arn, "HTTPS", 443, &successor,
            )
            .await;
            add_listener_certificates(
                &router,
                case,
                &listener_arn,
                std::slice::from_ref(&subject),
            )
            .await;
            listener_arn
        } else {
            tls_listener(&router, case, &lb_arn, &tg_arn, "HTTPS", 443, &subject).await
        };
        assert_eq!(
            consumers(&engine, &subject),
            vec![lb_arn.clone()],
            "case {case:?}: the load balancer must be presenting it before the release, got {:?}",
            consumers(&engine, &subject)
        );

        let reply = match released {
            Released::ModifyingTheListener => {
                let mut form = action("ModifyListener");
                form.push(p("ListenerArn", &listener_arn));
                form.push(p("Certificates.member.1.CertificateArn", &successor));
                call(&router, &form).await
            }
            Released::RemovingTheListenerCertificate => {
                let mut form = action("RemoveListenerCertificates");
                form.push(p("ListenerArn", &listener_arn));
                form.push(p("Certificates.member.1.CertificateArn", &subject));
                call(&router, &form).await
            }
            Released::DeletingTheListener => {
                let mut form = action("DeleteListener");
                form.push(p("ListenerArn", &listener_arn));
                call(&router, &form).await
            }
            Released::DeletingTheLoadBalancer => {
                let mut form = action("DeleteLoadBalancer");
                form.push(p("LoadBalancerArn", &lb_arn));
                call(&router, &form).await
            }
        };
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the call giving the reference up must answer 200, got {} {}",
            reply.status, reply.body
        );

        // The release is ACCEPTED, not finished: until the work it waits on falls due the
        // load balancer is still presenting the certificate.
        assert_eq!(
            consumers(&engine, &subject),
            vec![lb_arn.clone()],
            "case {case:?}: a release not yet due must leave the certificate in use, got {:?}",
            consumers(&engine, &subject)
        );
        tokio::time::sleep(SETTLE_AFTER + Duration::from_millis(100)).await;
        assert!(
            consumers(&engine, &subject).is_empty(),
            "case {case:?}: a release that fell due must free the certificate, got {:?}",
            consumers(&engine, &subject)
        );

        // And the certificate the load balancer is left presenting — the successor, on
        // every row that left a listener behind — is untouched by the release.
        let still_presented = !matches!(
            released,
            Released::DeletingTheListener | Released::DeletingTheLoadBalancer
        );
        let expected: Vec<String> = if still_presented {
            vec![lb_arn.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            consumers(&engine, &successor),
            expected,
            "case {case:?}: expected {expected:?} to present the successor, got {:?}",
            consumers(&engine, &successor)
        );
    }
}

/// RULE: a listener left on a protocol that terminates no TLS presents no certificate at
/// all — the model: "Changing the protocol from HTTPS to HTTP, or from TLS to TCP,
/// removes the security policy and default certificate properties" — so the load balancer
/// stops being a consumer of both its default certificate and its list entries.
#[tokio::test]
async fn a_listener_moved_off_tls_presents_no_certificate() {
    let (router, engine) = emulator();
    let tg_arn = create_http_target_group(&router, "downgrade-tg").await;
    let lb_arn = create_load_balancer(&router, "downgrade-alb").await;
    let case = "an HTTPS listener modified to HTTP";
    let default_certificate = acm_certificate(6);
    let sni_certificate = acm_certificate(7);

    let listener_arn = tls_listener(
        &router,
        case,
        &lb_arn,
        &tg_arn,
        "HTTPS",
        443,
        &default_certificate,
    )
    .await;
    add_listener_certificates(
        &router,
        case,
        &listener_arn,
        std::slice::from_ref(&sni_certificate),
    )
    .await;
    for arn in [&default_certificate, &sni_certificate] {
        assert_eq!(
            consumers(&engine, arn),
            vec![lb_arn.clone()],
            "case {case:?}: the load balancer must present {arn} before the change, got {:?}",
            consumers(&engine, arn)
        );
    }

    let mut form = action("ModifyListener");
    form.push(p("ListenerArn", &listener_arn));
    form.push(p("Protocol", "HTTP"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ModifyListener must answer 200, got {} {}",
        reply.status, reply.body
    );

    tokio::time::sleep(SETTLE_AFTER + Duration::from_millis(100)).await;
    for arn in [&default_certificate, &sni_certificate] {
        assert!(
            consumers(&engine, arn).is_empty(),
            "case {case:?}: a listener that terminates no TLS presents no certificate, so \
             {arn} must be free, got {:?}",
            consumers(&engine, arn)
        );
    }
}
