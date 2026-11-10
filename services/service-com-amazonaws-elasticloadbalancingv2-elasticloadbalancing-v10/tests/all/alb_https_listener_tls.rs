// SPDX-License-Identifier: CC0-1.0

//! What an HTTPS listener serves on the address its load balancer publishes, end to end in
//! one process: the TLS handshake, the certificate presented, and the listener's own
//! answer.
//!
//! The rule under test: **the address a load balancer publishes as its `DNSName` is an
//! address a client can dial, and an HTTPS listener on it completes the TLS handshake
//! presenting the certificate Certificate Manager holds for that listener's
//! `Certificates[].CertificateArn` — the name the `ClientHello` asks for selecting among
//! the listener's certificate list, with the listener's default certificate for a client
//! that asks for a name none of them covers or for no name at all — and then answers the
//! request from that listener's own actions.**
//!
//! Grounded in the model: `LoadBalancer$DNSName` is "the public DNS name of the load
//! balancer" (the address a client sends traffic to),
//! `CreateListenerInput$Certificates` is "[HTTPS and TLS listeners] The default certificate
//! for the listener. You must provide exactly one certificate", and
//! `AddListenerCertificates` "Adds the specified SSL server certificate to the certificate
//! list for the specified HTTPS or TLS listener" — the list a client's own name selects
//! from.
//!
//! Nothing here is staged in a store: every certificate is minted by `rcgen` (something
//! other than the code under test), brought into the emulator through Certificate
//! Manager's OWN `ImportCertificate`, and named on the listener through
//! `CreateListener`/`AddListenerCertificates`. The client is a real `rustls` client that
//! dials the published address in this process, so an answer not computed from the imported
//! material cannot pass: the leaf it is presented is compared byte for byte (its DER) with
//! the one that was imported.
//!
//! The rows span what the selection generalises over: a client that names the default
//! certificate's own name, one that names a certificate-list entry's name, one that names a
//! single label under a WILDCARD entry, one that names a host no certificate covers, and
//! one that names no host at all. Two further cases cover the rest of the rule: the TLS
//! version a listener's security policy decides, and a listener in the clear on the same
//! published name.
//!
//! The clock-owed move to `active` is placed in the store rather than slept for, exactly as
//! `alb_data_plane` places it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::provisioning_loop::{action, call, p, REGION};

/// The body the listener's default action answers with, so the answer is this listener's
/// own rather than anything a client sent.
const SERVED_BODY: &str = "tls-ok";

/// The status that action answers with.
const SERVED_STATUS: &str = "200";

/// How long a case gives one dial and one exchange. A generous bound on work that is all in
/// this process; a case that cannot finish inside it has found a defect, not a slow machine.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// A router carrying this service and Certificate Manager over ONE shared engine, exactly
/// as the assembled router mounts them: the certificate a listener presents is a resource
/// of that service, imported through its own API.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One certificate a case brings: the PEM material a client imports, and the DER of the
/// leaf, which is what the presented certificate is compared against.
struct Minted {
    certificate_pem: String,
    private_key_pem: String,
    der: Vec<u8>,
}

/// Mint a real self-signed leaf covering `names`, with `rcgen`.
fn mint(names: &[&str]) -> Minted {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let owned: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    let mut params = rcgen::CertificateParams::new(owned).expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(
        rcgen::DnType::CommonName,
        *names.first().expect("a certificate covers a name"),
    );
    params.distinguished_name = subject;
    let certificate = params.self_signed(&key).expect("a self-signed certificate");
    Minted {
        certificate_pem: certificate.pem(),
        private_key_pem: key.serialize_pem(),
        der: certificate.der().to_vec(),
    }
}

/// Import one minted certificate through Certificate Manager's own `ImportCertificate`, and
/// answer the ARN it was stored under.
async fn import(router: &MultiServiceRouter, case: &str, minted: &Minted) -> String {
    let body = serde_json::json!({
        "Certificate": aws_smithy_types::base64::encode(minted.certificate_pem.as_bytes()),
        "PrivateKey": aws_smithy_types::base64::encode(minted.private_key_pem.as_bytes()),
    });
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "CertificateManager.ImportCertificate")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/acm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the import body serializes"),
        ))
        .expect("a well-formed import request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value =
        serde_json::from_slice(&response.into_body()).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        status, 200,
        "case {case}: ImportCertificate must answer 200, got {status} {parsed}"
    );
    parsed["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no CertificateArn in {parsed}"))
        .to_string()
}

/// Create an Application Load Balancer and answer its ARN and the address it published.
async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body));
    let dns_name = reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case create {name}: no DNSName in {}", reply.body));
    (arn, dns_name)
}

/// Create one listener whose default action is the fixed response above, on `protocol` and
/// `port`, naming `certificate_arn` as its default certificate and `ssl_policy` as its
/// security policy when it has them.
async fn create_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    protocol: &str,
    port: &str,
    certificate_arn: Option<&str>,
    ssl_policy: Option<&str>,
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", protocol));
    form.push(p("Port", port));
    if let Some(arn) = certificate_arn {
        form.push(p("Certificates.member.1.CertificateArn", arn));
    }
    if let Some(policy) = ssl_policy {
        form.push(p("SslPolicy", policy));
    }
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        SERVED_STATUS,
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.ContentType",
        "text/plain",
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        SERVED_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {protocol}:{port} listener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ListenerArn in {}", reply.body))
}

/// Add one certificate to a listener's certificate list, through the operation that owns
/// that list.
async fn add_listener_certificate(
    router: &MultiServiceRouter,
    listener_arn: &str,
    certificate_arn: &str,
) {
    let mut form = action("AddListenerCertificates");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("Certificates.member.1.CertificateArn", certificate_arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case add a listener certificate: expected 200, got {} body {}",
        reply.status, reply.body
    );
}

/// Place the instant the load balancer's move to `active` is owed at in the past, so a case
/// serves traffic without sleeping for the span the clock owes.
fn make_active(engine: &Arc<engine::InMemoryEngine>, lb_arn: &str) {
    let state = engine.state::<ElbState>();
    let mut store = state.lock();
    if let Some(record) = store.load_balancers.get_mut(lb_arn) {
        record.active_at = SystemTime::now() - Duration::from_secs(1);
    }
}

/// Whether `published` is an address a client on this machine can DIAL: the `host:port` of
/// a listener this emulator opened, which is the set of values a client dials without
/// resolving a name.
fn is_reachable_authority(published: &str) -> bool {
    published.parse::<std::net::SocketAddr>().is_ok()
}

/// The port of a published address, which is the port a case dials on loopback.
fn published_port(published: &str) -> u16 {
    published
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|error| panic!("the published address {published} is no address: {error}"))
        .port()
}

/// Open a TCP connection to a published address of THIS emulator.
///
/// The core binds its listeners on loopback (`EMULATOR_BIND_ADDRESS`, `127.0.0.1` by
/// default), so the dial below is to this machine — and the assertion above is what pins it
/// there: only the PORT is taken from the published authority.
async fn dial(published: &str) -> tokio::net::TcpStream {
    let port = published_port(published);
    tokio::time::timeout(
        EXCHANGE_TIMEOUT,
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    )
    .await
    .unwrap_or_else(|_| panic!("the published address {published} did not accept a connection"))
    .unwrap_or_else(|error| {
        panic!("the published address {published} refused a connection: {error}")
    })
}

/// The client-side TLS configuration one case dials with.
///
/// The verifier ACCEPTS whatever is presented and the certificate itself is asserted on
/// afterwards, byte for byte: what is under test is WHICH certificate the listener
/// presents, and a client that refused an unexpected one would report a handshake failure
/// instead of naming the certificate it got.
fn client_config() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("the provider supports the default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AssertedAfterwards(provider)))
            .with_no_client_auth(),
    )
}

/// See [`client_config`].
#[derive(Debug)]
struct AssertedAfterwards(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AssertedAfterwards {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// What one TLS exchange observed: the leaf the listener presented, the TLS version it
/// negotiated, and the answer it sent.
struct Presented {
    leaf: Vec<u8>,
    version: Option<rustls::ProtocolVersion>,
    answer: String,
}

/// Dial the published address, complete a TLS handshake asking for `server_name` (or asking
/// for no name at all, which is what a client dialling an address does), send one request
/// and read the answer.
async fn tls_exchange(case: &str, published: &str, server_name: Option<&str>) -> Presented {
    let conn = dial(published).await;
    // A client that asks for a name sends it as the `ClientHello`'s `server_name`; one that
    // asks for none dials the address itself, which carries no such extension.
    let asked_for = match server_name {
        Some(name) => rustls::pki_types::ServerName::try_from(name.to_string())
            .unwrap_or_else(|error| panic!("case {case}: {name} is no server name: {error}")),
        None => rustls::pki_types::ServerName::from(std::net::Ipv4Addr::LOCALHOST),
    };
    let connector = tokio_rustls::TlsConnector::from(client_config());
    let mut stream = tokio::time::timeout(EXCHANGE_TIMEOUT, connector.connect(asked_for, conn))
        .await
        .unwrap_or_else(|_| panic!("case {case}: the handshake did not complete in time"))
        .unwrap_or_else(|error| panic!("case {case}: the handshake must complete, got {error}"));
    let leaf = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first().map(|leaf| leaf.as_ref().to_vec()))
        .unwrap_or_else(|| panic!("case {case}: the listener presented no certificate"));
    let version = stream.get_ref().1.protocol_version();
    // The host a client of a published address sends is the name it asked for, and the
    // address itself when it asked for none.
    let host = server_name.unwrap_or(published);
    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case}: the request was not sent: {error}"));
    let mut answer = Vec::new();
    tokio::time::timeout(EXCHANGE_TIMEOUT, stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("case {case}: the listener did not answer in time"))
        .unwrap_or_else(|error| panic!("case {case}: the answer was not read: {error}"));
    Presented {
        leaf,
        version,
        answer: String::from_utf8_lossy(&answer).to_string(),
    }
}

/// RULE: an HTTPS listener on the address its load balancer published completes the
/// handshake presenting the certificate Certificate Manager holds for it — selected by the
/// name the client asked for, defaulting to the listener's own default certificate — and
/// then answers the request from the listener's actions.
#[tokio::test]
async fn an_https_listener_presents_the_certificate_the_client_asked_for() {
    let (router, engine) = emulator();

    // Three certificates a client imported: the listener's default, one added to its
    // certificate list, and a WILDCARD one added to that list.
    let default_leaf = mint(&["default.listener.example.com"]);
    let listed_leaf = mint(&["listed.listener.example.net"]);
    let wildcard_leaf = mint(&["*.apps.listener.example.org"]);
    let default_arn = import(&router, "the default certificate", &default_leaf).await;
    let listed_arn = import(&router, "a certificate-list entry", &listed_leaf).await;
    let wildcard_arn = import(&router, "a wildcard certificate", &wildcard_leaf).await;

    let (lb_arn, published) = create_load_balancer(&router, "tls-listener-lb").await;
    assert!(
        is_reachable_authority(&published),
        "expected the load balancer to publish an address a client can dial, got {published:?}"
    );
    let listener_arn =
        create_listener(&router, &lb_arn, "HTTPS", "443", Some(&default_arn), None).await;
    add_listener_certificate(&router, &listener_arn, &listed_arn).await;
    add_listener_certificate(&router, &listener_arn, &wildcard_arn).await;
    make_active(&engine, &lb_arn);

    // The rows: which name the client asks for, and which imported leaf the listener must
    // then present.
    let cases: &[(&str, Option<&str>, &Minted)] = &[
        (
            "the default certificate's own name",
            Some("default.listener.example.com"),
            &default_leaf,
        ),
        (
            "a name the certificate list covers",
            Some("listed.listener.example.net"),
            &listed_leaf,
        ),
        (
            "a single label under a wildcard entry",
            Some("api.apps.listener.example.org"),
            &wildcard_leaf,
        ),
        (
            "a name no certificate of this listener covers",
            Some("unknown.listener.example.com"),
            &default_leaf,
        ),
        ("no name at all", None, &default_leaf),
    ];
    for (case, server_name, expected) in cases {
        let presented = tls_exchange(case, &published, *server_name).await;
        assert_eq!(
            presented.leaf,
            expected.der,
            "case {case}: expected the listener to present the imported leaf ({} bytes), got {} \
             bytes",
            expected.der.len(),
            presented.leaf.len()
        );
        assert!(
            presented.answer.starts_with("HTTP/1.1 200"),
            "case {case}: expected the listener's own answer, got {:?}",
            presented.answer
        );
        assert!(
            presented.answer.contains(SERVED_BODY),
            "case {case}: expected the body {SERVED_BODY:?}, got {:?}",
            presented.answer
        );
        // The listener names no security policy, so it negotiates under the default one,
        // whose published `SslProtocols` stop at TLS 1.2.
        assert_eq!(
            presented.version,
            Some(rustls::ProtocolVersion::TLSv1_2),
            "case {case}: expected the default policy's highest version, got {:?}",
            presented.version
        );
    }
}

/// RULE (the security policy half): a listener negotiates the TLS versions the policy it
/// was set to publishes — a policy whose `SslProtocols` list TLS 1.3 negotiates TLS 1.3,
/// where the default policy stops at TLS 1.2.
#[tokio::test]
async fn a_listener_negotiates_under_the_security_policy_it_was_set_to() {
    let (router, engine) = emulator();
    let leaf = mint(&["policy.listener.example.com"]);
    let arn = import(&router, "a certificate for a policy row", &leaf).await;
    let (lb_arn, published) = create_load_balancer(&router, "tls-policy-lb").await;
    // A policy whose published protocol list carries TLS 1.3 beside TLS 1.2
    // (`DescribeSSLPolicies` reports both for it).
    create_listener(
        &router,
        &lb_arn,
        "HTTPS",
        "443",
        Some(&arn),
        Some("ELBSecurityPolicy-TLS13-1-2-2021-06"),
    )
    .await;
    make_active(&engine, &lb_arn);

    let case = "a policy that publishes TLS 1.3";
    let presented = tls_exchange(case, &published, Some("policy.listener.example.com")).await;
    assert_eq!(
        presented.version,
        Some(rustls::ProtocolVersion::TLSv1_3),
        "case {case}: expected the policy's highest version, got {:?}",
        presented.version
    );
    assert_eq!(
        presented.leaf, leaf.der,
        "case {case}: expected the imported leaf, got {} bytes",
        presented.leaf.len()
    );
    assert!(
        presented.answer.starts_with("HTTP/1.1 200") && presented.answer.contains(SERVED_BODY),
        "case {case}: expected the listener's own answer, got {:?}",
        presented.answer
    );
}

/// RULE (the other half of the published address): a listener in the clear answers on the
/// same address, out of its own actions, for a client that dials it without TLS.
#[tokio::test]
async fn a_listener_in_the_clear_answers_on_the_published_address() {
    let (router, engine) = emulator();
    let (lb_arn, published) = create_load_balancer(&router, "clear-listener-lb").await;
    assert!(
        is_reachable_authority(&published),
        "expected the load balancer to publish an address a client can dial, got {published:?}"
    );
    create_listener(&router, &lb_arn, "HTTP", "80", None, None).await;
    make_active(&engine, &lb_arn);

    let mut stream = dial(&published).await;
    let request = format!("GET / HTTP/1.1\r\nHost: {published}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("the request was not sent: {error}"));
    let mut answer = Vec::new();
    tokio::time::timeout(EXCHANGE_TIMEOUT, stream.read_to_end(&mut answer))
        .await
        .unwrap_or_else(|_| panic!("the published address {published} did not answer in time"))
        .unwrap_or_else(|error| panic!("the answer was not read: {error}"));
    let answer = String::from_utf8_lossy(&answer).to_string();
    assert!(
        answer.starts_with("HTTP/1.1 200") && answer.contains(SERVED_BODY),
        "case a listener in the clear: expected the listener's own answer, got {answer:?}"
    );
}
