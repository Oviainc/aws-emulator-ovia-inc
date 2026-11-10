// SPDX-License-Identifier: CC0-1.0

//! What a managed endpoint is reached THROUGH, end to end: `CreateManagedEndpoint`
//! provisions the gateway, and `DescribeManagedEndpoint` and `ListManagedEndpoints` report
//! it.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client. The create / describe / list helpers are the ones the other endpoint suites
//! drive, so no suite carries its own request path.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. An endpoint reports the connection metadata it was provisioned with — `serverUrl`,
//!    `certificateAuthority`, and the `securityGroup` / `subnetIds` of its placement
//!    (`smithy_query(query="shape", shape="Endpoint")`) — on every read path, identically
//!    in the describe and in the listing, unchanged by repeated reads and by the
//!    `CREATING` -> `ACTIVE` step the clock owes.
//! 2. `serverUrl` is THIS emulator's own base URL followed by the endpoint's own path, so
//!    sending that path back answers the same endpoint; it follows the base the emulator
//!    is reachable at rather than naming a fixed host.
//! 3. `certificateAuthority` carries a REAL certificate: `certificateData` is base64 PEM
//!    that parses as an X.509 certificate issued for the gateway's host, naming the
//!    endpoint, valid from the day it was created. No two endpoints share one.
//! 4. The placement an endpoint reports is the one the service that OWNS the container
//!    cluster reports for it: exactly the `subnetIds` and the `clusterSecurityGroupId` of
//!    the Amazon EKS cluster the virtual cluster is backed by, for every endpoint into that
//!    cluster — and NOTHING when Amazon EKS holds no such cluster, because an id no owner
//!    holds is not one this service reports.
//!
//! For rule 4 Amazon EKS and Amazon EC2 are mounted BESIDE this service on one engine
//! ([`Emr::with_container_clusters`]) and driven through their OWN APIs over the same
//! in-process callback this service resolves a placement through: the subnets a case names
//! were minted by EC2 and the cluster was created by EKS, so no identifier asserted here
//! was written by this suite.

use std::time::Duration;

use serde_json::{json, Map, Value};
use x509_cert::der::oid::AssociatedOid;
use x509_cert::der::{Decode, DecodePem};
use x509_cert::ext::pkix;

use crate::managed_endpoint_listing::{
    create_endpoint, described, describe_endpoint, endpoint_body, list_endpoints, listed,
    listed_record, stored_bodies, wait_for_the_clock,
};
use crate::virtual_cluster_job_run_loop::{field, instant_millis, text, Emr, REGION};

/// ACM's published validity for a certificate it issues: 13 months, 395 days — the window
/// [`assert_certificate_window`] holds the generated material to.
const CERTIFICATE_VALIDITY_DAYS: i64 = 395;

/// The IAM role an Amazon EKS cluster is created with in this suite. No IAM is mounted
/// beside the three services, so nothing resolves it; it is there because `CreateCluster`
/// requires the member.
const EKS_CLUSTER_ROLE: &str = "arn:aws:iam::000000000000:role/eksClusterRole";

/// Long enough past Amazon EKS's own cluster-activation span that the clock owes a fresh
/// cluster `ACTIVE`, which is when it reports the security group it created for it.
const PAST_EKS_ACTIVATION: Duration = Duration::from_millis(750);

/// An Amazon EKS client over the in-process callback the emulator routes on: no socket is
/// opened, and the endpoint URL is only the address that transport routes by.
///
/// The very seam this service's own placement resolution reaches Amazon EKS through, so a
/// cluster this suite creates is created exactly where a create will look for it.
fn eks(emr: &Emr, region: &str) -> ::aws_sdk_eks::Client {
    let config = ::aws_sdk_eks::Config::builder()
        .behavior_version(::aws_sdk_eks::config::BehaviorVersion::latest())
        .region(::aws_sdk_eks::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://eks.{region}.amazonaws.com"))
        .credentials_provider(emr.callback().aws_credentials_provider())
        .http_client(emr.callback().http_client())
        .retry_config(::aws_sdk_eks::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eks::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eks::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_eks::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_eks::Client::from_conf(config)
}

/// An Amazon EC2 client over the same seam: the service that owns subnets, asked for the
/// ones the account has.
fn ec2(emr: &Emr, region: &str) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        .credentials_provider(emr.callback().aws_credentials_provider())
        .http_client(emr.callback().http_client())
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// `count` subnets the ACCOUNT has in `region`, as Amazon EC2 reports them — never derived
/// here: a suite that needs a subnet id asks the owner for one.
async fn account_subnets(emr: &Emr, region: &str, count: usize) -> Vec<String> {
    let described = ec2(emr, region)
        .describe_subnets()
        .send()
        .await
        .expect("EC2 must report the account's subnets");
    let mut reported: Vec<(String, String)> = described
        .subnets()
        .iter()
        .filter_map(|subnet| {
            Some((
                subnet.availability_zone()?.to_string(),
                subnet.subnet_id()?.to_string(),
            ))
        })
        .filter(|(zone, _)| zone.starts_with(region))
        .collect();
    reported.sort();
    assert!(
        reported.len() >= count,
        "the account must have at least {count} subnets in {region}, got {reported:?}"
    );
    reported
        .into_iter()
        .take(count)
        .map(|(_, subnet_id)| subnet_id)
        .collect()
}

/// The network Amazon EKS reports for one of its clusters: the subnets of its VPC
/// configuration and the security group it created for the cluster.
struct ClusterNetwork {
    subnet_ids: Vec<String>,
    security_group: String,
}

/// Create one Amazon EKS cluster named `name`, in `count` of the account's subnets, and
/// return the network EKS reports for it once it is ACTIVE.
///
/// Everything here goes through Amazon EKS's own API: the cluster exists because EKS
/// created it, and the ids below are the ones EKS answers with.
async fn create_container_cluster(emr: &Emr, name: &str, count: usize) -> ClusterNetwork {
    let subnets = account_subnets(emr, REGION, count).await;
    eks(emr, REGION)
        .create_cluster()
        .name(name)
        .role_arn(EKS_CLUSTER_ROLE)
        .resources_vpc_config(
            ::aws_sdk_eks::types::VpcConfigRequest::builder()
                .set_subnet_ids(Some(subnets))
                .build(),
        )
        .send()
        .await
        .expect("Amazon EKS must create the container cluster");
    // The group EKS creates for a cluster is reported with the rest of what provisioning
    // produced, so the fixture waits for the clock EKS owes it.
    std::thread::sleep(PAST_EKS_ACTIVATION);
    let described = eks(emr, REGION)
        .describe_cluster()
        .name(name)
        .send()
        .await
        .expect("Amazon EKS must report the cluster it created");
    let vpc_config = described
        .cluster()
        .and_then(::aws_sdk_eks::types::Cluster::resources_vpc_config)
        .expect("the cluster must report its VPC configuration");
    ClusterNetwork {
        subnet_ids: vpc_config.subnet_ids().to_vec(),
        security_group: vpc_config
            .cluster_security_group_id()
            .expect("Amazon EKS must report the security group it created for the cluster")
            .to_string(),
    }
}

/// Create one virtual cluster backed by the container cluster `container_cluster`, and
/// return its id.
///
/// The harness's own cluster helper pins one container cluster for every cluster it makes,
/// and the placement an endpoint reports is the CONTAINER cluster's, so these suites need
/// clusters that differ in exactly that member.
async fn cluster_on(emr: &Emr, name: &str, token: &str, container_cluster: &str) -> String {
    let answer = emr
        .send(
            "POST",
            "/virtualclusters",
            Some(json!({
                "name": name,
                "clientToken": token,
                "containerProvider": {"id": container_cluster, "type": "EKS"},
            })),
        )
        .await;
    text(answer.ok(name), "/id", name)
}

/// The same, signed for `region`: an endpoint is provisioned by the Region's own endpoint.
async fn cluster_on_in(
    emr: &Emr,
    region: &str,
    name: &str,
    token: &str,
    container_cluster: &str,
) -> String {
    let answer = emr
        .send_in(
            region,
            "POST",
            "/virtualclusters",
            Some(json!({
                "name": name,
                "clientToken": token,
                "containerProvider": {"id": container_cluster, "type": "EKS"},
            })),
        )
        .await;
    text(answer.ok(name), "/id", name)
}

/// `subnetIds`, in the order the endpoint reports them, and empty when it reports none.
fn subnet_ids(endpoint: &Value, case: &str) -> Vec<String> {
    let Some(reported) = endpoint.get("subnetIds") else {
        return Vec::new();
    };
    reported
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: subnetIds is not a list: {endpoint}"))
        .iter()
        .map(|subnet| {
            subnet
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a subnet id is not a string: {endpoint}"))
                .to_string()
        })
        .collect()
}

/// The connection members of one reported endpoint, as one comparable value: what every
/// read of that endpoint has to agree on, including which of them are absent.
fn connection(endpoint: &Value) -> Value {
    let mut members = Map::new();
    for member in ["serverUrl", "certificateAuthority", "securityGroup", "subnetIds"] {
        if let Some(value) = endpoint.get(member) {
            members.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(members)
}

/// The PEM text `certificateData` carries, decoded from the base64 the member is declared
/// in (`Base64Encoded`, max 5000 characters).
fn certificate_pem(endpoint: &Value, case: &str) -> String {
    let encoded = text(endpoint, "/certificateAuthority/certificateData", case);
    assert!(
        encoded.len() <= 5000,
        "case {case:?}: certificateData is {} characters, past the 5000 the member holds",
        encoded.len()
    );
    let decoded = ::aws_smithy_types::base64::decode(&encoded).unwrap_or_else(|error| {
        panic!("case {case:?}: certificateData is not base64: {error} in {encoded:?}")
    });
    let pem = String::from_utf8(decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: the decoded material is not text: {error}"));
    assert!(
        pem.starts_with("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: expected PEM certificate data, got {pem:?}"
    );
    pem
}

/// The DNS names of a certificate's Subject Alternative Name extension.
fn dns_names(certificate: &x509_cert::Certificate) -> Vec<String> {
    certificate
        .tbs_certificate
        .extensions
        .iter()
        .flatten()
        .find(|extension| extension.extn_id == pkix::SubjectAltName::OID)
        .and_then(|extension| pkix::SubjectAltName::from_der(extension.extn_value.as_bytes()).ok())
        .map(|names| {
            names
                .0
                .iter()
                .filter_map(|name| match name {
                    pkix::name::GeneralName::DnsName(dns) => Some(dns.as_str().to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The certificate is valid from the day the endpoint was created, over the window a
/// certificate for an endpoint's address is issued for.
fn assert_certificate_window(
    certificate: &x509_cert::Certificate,
    created_at_secs: i64,
    case: &str,
) {
    let day = 24 * 60 * 60;
    let bound = |at: &x509_cert::time::Time| {
        i64::try_from(at.to_unix_duration().as_secs()).expect("a validity bound fits an i64")
    };
    let not_before = bound(&certificate.tbs_certificate.validity.not_before);
    let not_after = bound(&certificate.tbs_certificate.validity.not_after);
    assert!(
        not_before <= created_at_secs && created_at_secs - not_before < day,
        "case {case:?}: expected a window opening on the creation day ({created_at_secs}), \
         got notBefore {not_before}"
    );
    assert_eq!(
        not_after - not_before,
        CERTIFICATE_VALIDITY_DAYS * day,
        "case {case:?}: expected a {CERTIFICATE_VALIDITY_DAYS}-day window, got \
         notBefore {not_before} and notAfter {not_after}"
    );
}

/// RULES 1 and 2: every stored endpoint reports the gateway it was provisioned with, the
/// same on both read paths and across the `CREATING` -> `ACTIVE` step, and `serverUrl`
/// leads back to this emulator — sending the path it names answers that same endpoint.
///
/// The rows span the forms an endpoint takes (which optional members it carries, its
/// `type`, how deep its `configurationOverrides` goes) across two virtual clusters.
#[tokio::test]
async fn an_endpoint_reports_the_gateway_it_was_provisioned_with() {
    let emr = Emr::new();
    let base = smithy_server_runtime::UNSERVED_URL_BASE;
    let clusters = [
        cluster_on(&emr, "gateway-a", "vc-gw-a", "eks-gateway-a").await,
        cluster_on(&emr, "gateway-b", "vc-gw-b", "eks-gateway-b").await,
    ];

    let mut stored: Vec<(String, String, String)> = Vec::new();
    for cluster in &clusters {
        for (label, body) in stored_bodies() {
            let case = format!("{label} in {cluster}");
            let created = create_endpoint(&emr, cluster, body).await;
            let id = text(created.ok(&case), "/id", &case);
            stored.push((case, cluster.clone(), id));
        }
    }

    let mut certificates: Vec<String> = Vec::new();
    for (case, cluster, id) in &stored {
        let endpoint = described(describe_endpoint(&emr, cluster, id).await.ok(case), case);

        // RULE 2: the address is this emulator's base plus the endpoint's own path.
        let server_url = text(&endpoint, "/serverUrl", case);
        let expected = format!("{base}/virtualclusters/{cluster}/endpoints/{id}");
        assert_eq!(
            server_url, expected,
            "case {case:?}: expected the gateway at {expected:?}, got {server_url:?}"
        );
        // And it is reachable HERE: the path it names describes this very endpoint.
        let path = server_url
            .strip_prefix(base)
            .unwrap_or_else(|| panic!("case {case:?}: {server_url:?} is not on {base:?}"));
        let answered = described(emr.send("GET", path, None).await.ok(case), case);
        assert_eq!(
            text(&answered, "/id", case),
            *id,
            "case {case:?}: the address {server_url:?} answered another endpoint: {answered}"
        );

        // RULE 1: the connection members are all there, and the listing agrees.
        certificates.push(certificate_pem(&endpoint, case));
        let before = connection(&endpoint);
        assert_eq!(
            connection(&listed_record(&emr, cluster, id, case).await),
            before,
            "case {case:?}: the listed gateway differs from the described one"
        );
        // A second read answers the same gateway: nothing about it is minted per read.
        let again = described(describe_endpoint(&emr, cluster, id).await.ok(case), case);
        assert_eq!(
            connection(&again),
            before,
            "case {case:?}: a second read reported another gateway"
        );
    }

    // RULE 3: each endpoint was issued its OWN certificate.
    for (index, material) in certificates.iter().enumerate() {
        for (other, against) in certificates.iter().enumerate().skip(index + 1) {
            let case = format!("the certificates of endpoints {index} and {other}");
            assert_ne!(
                material, against,
                "case {case:?}: two endpoints must not share one certificate"
            );
        }
    }

    // RULE 1 across the transition: the step moves `state` and leaves the gateway alone.
    let mut before: Vec<Value> = Vec::new();
    for (case, cluster, id) in &stored {
        let endpoint = described(describe_endpoint(&emr, cluster, id).await.ok(case), case);
        before.push(connection(&endpoint));
    }
    wait_for_the_clock();
    for ((case, cluster, id), was) in stored.iter().zip(before) {
        let endpoint = described(describe_endpoint(&emr, cluster, id).await.ok(case), case);
        let state_now = text(&endpoint, "/state", case);
        assert_eq!(
            state_now, "ACTIVE",
            "case {case:?}: expected the endpoint ACTIVE once its step came due, got {state_now:?}"
        );
        assert_eq!(
            connection(&endpoint),
            was,
            "case {case:?}: the transition must leave the gateway it was provisioned with"
        );
    }
}

/// RULE 2: the gateway's address follows the base the emulator is reachable at
/// (`ctx.endpoint()`), so an address that named a fixed host would fail on every row but
/// one.
///
/// The rows are the bases an emulator reports: the unserved default, a host with a port,
/// and an address literal.
#[tokio::test]
async fn the_gateway_address_follows_the_emulators_own_base() {
    for base in [
        smithy_server_runtime::UNSERVED_URL_BASE,
        "https://emr-on-eks.test.invalid:9297",
        "https://127.0.0.1:9297",
    ] {
        let emr = if base == smithy_server_runtime::UNSERVED_URL_BASE {
            Emr::new()
        } else {
            Emr::with_url_base(base)
        };
        let cluster = cluster_on(&emr, "base-row", "vc-base-row", "eks-base-row").await;
        let case = format!("the emulator reachable at {base}");
        let created = create_endpoint(
            &emr,
            &cluster,
            endpoint_body("base-row", "ep-tok-base-row", "JUPYTER_ENTERPRISE_GATEWAY"),
        )
        .await;
        let id = text(created.ok(&case), "/id", &case);
        let endpoint = described(describe_endpoint(&emr, &cluster, &id).await.ok(&case), &case);

        let server_url = text(&endpoint, "/serverUrl", &case);
        let expected = format!("{base}/virtualclusters/{cluster}/endpoints/{id}");
        assert_eq!(
            server_url, expected,
            "case {case:?}: expected the gateway at {expected:?}, got {server_url:?}"
        );

        // The certificate secures THAT address: it is issued for the host the base names.
        let host = base
            .trim_start_matches("https://")
            .split(':')
            .next()
            .expect("a base URL names a host");
        let pem = certificate_pem(&endpoint, &case);
        let parsed = x509_cert::Certificate::from_pem(pem.as_bytes())
            .unwrap_or_else(|error| panic!("case {case:?}: the certificate parses: {error}"));
        let names = dns_names(&parsed);
        // An address literal is carried as an IP address rather than as a DNS name, so a
        // base naming one carries no DNS name at all.
        if host.parse::<std::net::IpAddr>().is_err() {
            assert!(
                names.iter().any(|name| name == host),
                "case {case:?}: expected the certificate issued for {host:?}, got {names:?}"
            );
        } else {
            assert!(
                names.is_empty(),
                "case {case:?}: expected no DNS name for an address literal, got {names:?}"
            );
        }
    }
}

/// RULE 3: `certificateAuthority` carries a real certificate issued for the endpoint.
///
/// The rows span the forms a `ResourceNameString` takes — the characters the pattern
/// `^[\.\-_/#A-Za-z0-9]+$` allows and both its length bounds — because the endpoint's name
/// is what the certificate's subject carries, and they run in two REGIONS, because an
/// endpoint is provisioned by the Region's own endpoint.
#[tokio::test]
async fn the_endpoint_certificate_is_issued_for_the_endpoint() {
    // `ResourceNameString` is len 1..64, pattern ^[\.\-_/#A-Za-z0-9]+$ (smithy_query:
    // operation `CreateManagedEndpoint`, input `name`).
    let names = [
        ("the shortest name the model allows", "a".to_string()),
        ("the longest name the model allows", "n".repeat(64)),
        (
            "every character class the pattern allows",
            "Studio.jeg_1/2#3-4".to_string(),
        ),
    ];
    for region in [REGION, "eu-central-1"] {
        let emr = Emr::new();
        let cluster = cluster_on_in(
            &emr,
            region,
            "certificate-rows",
            "vc-cert-rows",
            "eks-certificate-rows",
        )
        .await;
        for (index, (label, name)) in names.iter().enumerate() {
            let case = format!("{label} in {region}");
            let created = emr
                .send_in(
                    region,
                    "POST",
                    &format!("/virtualclusters/{cluster}/endpoints"),
                    Some(json!({
                        "name": name,
                        "clientToken": format!("ep-tok-cert-{index}"),
                        "type": "JUPYTER_ENTERPRISE_GATEWAY",
                        "releaseLabel": "emr-6.10.0-latest",
                        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
                    })),
                )
                .await;
            let id = text(created.ok(&case), "/id", &case);
            let answer = emr
                .send_in(
                    region,
                    "GET",
                    &format!("/virtualclusters/{cluster}/endpoints/{id}"),
                    None,
                )
                .await;
            let endpoint = described(answer.ok(&case), &case);

            let pem = certificate_pem(&endpoint, &case);
            let parsed = x509_cert::Certificate::from_pem(pem.as_bytes())
                .unwrap_or_else(|error| panic!("case {case:?}: the certificate parses: {error}"));
            let subject = parsed.tbs_certificate.subject.to_string();
            assert!(
                subject.contains(name.as_str()),
                "case {case:?}: expected the endpoint named in the subject, got {subject:?}"
            );
            let created_at_secs = instant_millis(&endpoint, "/createdAt", &case) / 1_000;
            assert_certificate_window(&parsed, created_at_secs, &case);
        }
    }
}

/// RULE 4: the placement is the container cluster's, as the service that OWNS that cluster
/// reports it — and nothing at all when no owner reports one.
///
/// The rows differ along what the rule generalises over: how many subnets the container
/// cluster spans, how many virtual clusters are backed by it, how many endpoints each of
/// them has, and whether Amazon EKS holds the container cluster the virtual cluster names
/// at all. Every subnet id asserted was minted by Amazon EC2 and every group by Amazon EKS.
#[tokio::test]
async fn the_placement_is_the_one_the_container_clusters_owner_reports() {
    let emr = Emr::with_container_clusters();

    // (the container cluster, how many of the account's subnets it spans, how many virtual
    // clusters are backed by it, how many endpoints each of those has)
    let rows = [
        ("eks-placement-one", 2_usize, 1_usize, 2_usize),
        ("eks-placement-two", 3, 2, 1),
    ];
    for (container_cluster, subnets, cluster_count, endpoints) in rows {
        let network = create_container_cluster(&emr, container_cluster, subnets).await;
        assert_eq!(
            network.subnet_ids.len(),
            subnets,
            "case {container_cluster:?}: Amazon EKS must report the {subnets} subnets the \
             cluster was created in, got {:?}",
            network.subnet_ids
        );

        for nth in 0..cluster_count {
            let cluster = cluster_on(
                &emr,
                &format!("{container_cluster}-{nth}"),
                &format!("vc-{container_cluster}-{nth}"),
                container_cluster,
            )
            .await;
            for endpoint in 0..endpoints {
                let case = format!("endpoint {endpoint} of {cluster} on {container_cluster}");
                let created = create_endpoint(
                    &emr,
                    &cluster,
                    endpoint_body(
                        &format!("placement-{nth}-{endpoint}"),
                        &format!("ep-tok-{container_cluster}-{nth}-{endpoint}"),
                        "JUPYTER_SERVER",
                    ),
                )
                .await;
                let id = text(created.ok(&case), "/id", &case);
                let reported =
                    described(describe_endpoint(&emr, &cluster, &id).await.ok(&case), &case);

                let security_group = text(&reported, "/securityGroup", &case);
                assert_eq!(
                    security_group, network.security_group,
                    "case {case:?}: expected the group Amazon EKS created for the cluster \
                     ({:?}), got {security_group:?}",
                    network.security_group
                );
                let placed_in = subnet_ids(&reported, &case);
                assert_eq!(
                    placed_in, network.subnet_ids,
                    "case {case:?}: expected the subnets Amazon EKS reports for the cluster \
                     ({:?}), got {placed_in:?}",
                    network.subnet_ids
                );
                // The listing reports the same placement, from the same record.
                assert_eq!(
                    connection(&listed_record(&emr, &cluster, &id, &case).await),
                    connection(&reported),
                    "case {case:?}: the listed placement differs from the described one"
                );
            }
        }
    }

    // The contrasting rows: a virtual cluster backed by a container cluster Amazon EKS
    // holds nothing for. There is no placement to report, so NEITHER member is reported —
    // while the address and the certificate, which are this service's own, still are.
    for (label, container_cluster) in [
        ("a container cluster that was never created", "eks-absent"),
        (
            "a container cluster whose name is another cluster's with a suffix",
            "eks-placement-one-also",
        ),
    ] {
        let cluster = cluster_on(
            &emr,
            &format!("unheld-{container_cluster}"),
            &format!("vc-unheld-{container_cluster}"),
            container_cluster,
        )
        .await;
        let case = format!("{label} ({container_cluster})");
        let created = create_endpoint(
            &emr,
            &cluster,
            endpoint_body(
                "unheld-placement",
                &format!("ep-tok-unheld-{container_cluster}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await;
        let id = text(created.ok(&case), "/id", &case);
        let reported = described(describe_endpoint(&emr, &cluster, &id).await.ok(&case), &case);

        assert_eq!(
            reported.get("securityGroup"),
            None,
            "case {case:?}: no owner reports a group for this cluster, so none may be \
             reported: {reported}"
        );
        assert_eq!(
            reported.get("subnetIds"),
            None,
            "case {case:?}: no owner reports subnets for this cluster, so none may be \
             reported: {reported}"
        );
        assert!(
            text(&reported, "/serverUrl", &case).ends_with(&format!("/endpoints/{id}")),
            "case {case:?}: the endpoint's own address is still reported: {reported}"
        );
        certificate_pem(&reported, &case);
    }
}

/// RULE 1 on the LISTING: every endpoint a page reports carries the gateway it was
/// provisioned with, so a client that reaches an endpoint from the listing needs no
/// describe.
///
/// The rows are page sizes, so the members are asserted on a page that holds one endpoint
/// and on one that holds several.
#[tokio::test]
async fn every_listed_endpoint_carries_its_gateway() {
    let emr = Emr::new();
    let cluster = cluster_on(&emr, "listed-gateways", "vc-listed-gw", "eks-listed").await;
    for nth in 0..3 {
        let case = format!("endpoint {nth}");
        create_endpoint(
            &emr,
            &cluster,
            endpoint_body(
                &format!("listed-{nth}"),
                &format!("ep-tok-listed-{nth}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            ),
        )
        .await
        .ok(&case);
    }

    for max_results in [1_usize, 2, 10] {
        let case = format!("pages of {max_results}");
        let answer = list_endpoints(&emr, &cluster, &format!("?maxResults={max_results}")).await;
        let page = listed(answer.ok(&case), &case);
        assert!(
            !page.is_empty(),
            "case {case:?}: the listing reported no endpoint"
        );
        for endpoint in page {
            let id = text(&endpoint, "/id", &case);
            let row = format!("{case}: endpoint {id}");
            let server_url = text(&endpoint, "/serverUrl", &row);
            assert_eq!(
                server_url,
                format!(
                    "{}/virtualclusters/{cluster}/endpoints/{id}",
                    smithy_server_runtime::UNSERVED_URL_BASE
                ),
                "case {row:?}: the listed address is not this endpoint's: {server_url:?}"
            );
            certificate_pem(&endpoint, &row);
            // No Amazon EKS is mounted beside this service here, so no owner reports a
            // placement and none is reported — the rule's other half is the suite above.
            let authority = field(&endpoint, "/certificateAuthority", &row);
            assert!(
                authority.get("certificateArn").is_none(),
                "case {row:?}: no ACM record names this material, so no ARN may: {authority}"
            );
        }
    }
}
