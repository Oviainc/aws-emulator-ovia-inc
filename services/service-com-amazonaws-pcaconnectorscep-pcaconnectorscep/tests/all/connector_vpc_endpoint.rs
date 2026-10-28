// SPDX-License-Identifier: CC0-1.0

//! `CreateConnector`'s `VpcEndpointId` chooses how the connector is reachable, and the SCEP
//! `Endpoint` is where that choice is observable.
//!
//! `VpcEndpointId`'s own doc: "If you don't supply a value, by default Connector for SCEP
//! creates a connector accessible over the public internet. If you provide a VPC endpoint ID,
//! creates a connector accessible only through that specific VPC endpoint." Neither
//! `Connector` nor `ConnectorSummary` models a `VpcEndpointId` member (smithy_query
//! shape=Connector,ConnectorSummary), so the only surface the access path can show up on is
//! `Endpoint`, the HTTPS SCEP URL a client is configured against.
//!
//! The rule these cases assert, over a table that spans the class rather than one recorded
//! request: a connector created WITHOUT a VPC endpoint id reports a SCEP host under the
//! public DNS suffix, and one created WITH one reports a host qualified by that endpoint's id
//! under the PrivateLink suffix — so two connectors differing only in that input report
//! hosts that differ in more than the connector id, every reader of the connector agrees on
//! the host, and the host is stable across reads.
//!
//! Rows differ along what the rule generalises over: whether the input is present, which
//! endpoint id it names (both published `@length` bounds of `VpcEndpointId`, and several
//! distinct ids), and the connector `Type` the same create also chooses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router};

/// A region that is NOT the emulator default, so a host built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The service's endpoint prefix, which is also the label its SCEP hosts carry.
const ARN_SERVICE: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::ARN_SERVICE;

/// DNS suffix of a host reachable over the public internet.
const PUBLIC_SUFFIX: &str = ".amazonaws.com";

/// DNS suffix AWS PrivateLink gives an endpoint-specific DNS name: the `vpce.` label in front
/// of the public suffix.
const PRIVATE_SUFFIX: &str = ".vpce.amazonaws.com";

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern, one per row, so no
/// assertion can pass by echoing a single value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
];

/// An `IntuneConfiguration` body, so a row can choose the Intune `Type` as well as an access
/// path and the two choices can be shown to be independent.
fn intune(application_id: &str, domain: &str) -> Value {
    json!({ "Intune": { "AzureApplicationId": application_id, "Domain": domain } })
}

/// Create a connector and settle it, answering its ARN.
async fn create_active(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    request: Value,
) -> String {
    let created = harness::call(router, REGION, "POST", "/connectors", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending(engine, &arn);
    arn
}

/// The `Endpoint` `GetConnector` reports for `arn`.
async fn get_endpoint(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let read = harness::call(
        router,
        REGION,
        "GET",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200 from GetConnector, got {} / {:?}",
        read.status, read.body
    );
    read.body["Connector"]["Endpoint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string Endpoint in {:?}", read.body))
        .to_string()
}

/// The `Endpoint` `ListConnectors` reports for `arn`.
async fn list_endpoint(router: &MultiServiceRouter, case: &str, arn: &str) -> String {
    let page = harness::call(router, REGION, "GET", "/connectors", None).await;
    assert_eq!(
        page.status, 200,
        "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
        page.status, page.body
    );
    let summaries = page.body["Connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Connectors array in {:?}", page.body));
    let summary = summaries
        .iter()
        .find(|entry| entry["Arn"] == json!(arn))
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectorSummary for {arn:?} in {summaries:?}"));
    summary["Endpoint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string Endpoint in {summary:?}"))
        .to_string()
}

/// The connector id inside a connector ARN.
fn connector_id_of(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("expected a connector ARN ending `connector/<uuid>`, got {arn:?}"))
        .1
}

/// The URL with the connector's own id blanked out, so two connectors can be compared on
/// everything EXCEPT the id that is necessarily different between them.
fn without_connector_id(endpoint: &str, connector_id: &str) -> String {
    endpoint.replace(connector_id, "<id>")
}

/// One row: a create request's access path and type, and the host shape it must report.
#[derive(Debug)]
struct Row {
    name: &'static str,
    ca_arn: &'static str,
    vpc_endpoint_id: Option<&'static str>,
    mobile_device_management: Option<Value>,
}

/// The table: present vs absent `VpcEndpointId`, both published `@length` bounds of
/// `VpcEndpointId` (13 and 22, pattern `^vpce-[0-9a-f]{8}([0-9a-f]{9})?$`), several distinct
/// ids, and both connector types.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "public: no VpcEndpointId, general purpose",
            ca_arn: CA_ARNS[0],
            vpc_endpoint_id: None,
            mobile_device_management: None,
        },
        Row {
            name: "public: no VpcEndpointId, intune",
            ca_arn: CA_ARNS[1],
            vpc_endpoint_id: None,
            mobile_device_management: Some(intune(
                "01234567-89ab-cdef-0123-456789abcdef",
                "example.onmicrosoft.com",
            )),
        },
        Row {
            name: "private: 13-character VpcEndpointId (the lower @length bound)",
            ca_arn: CA_ARNS[2],
            vpc_endpoint_id: Some("vpce-0123abcd"),
            mobile_device_management: None,
        },
        Row {
            name: "private: 22-character VpcEndpointId (the upper @length bound)",
            ca_arn: CA_ARNS[3],
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
            mobile_device_management: None,
        },
        Row {
            name: "private: a different 22-character VpcEndpointId, intune",
            ca_arn: CA_ARNS[4],
            vpc_endpoint_id: Some("vpce-fedcba9887654321f"),
            mobile_device_management: Some(intune(
                "fedcba98-7654-3210-fedc-ba9876543210",
                "contoso.example.com",
            )),
        },
    ]
}

/// A connector created without a `VpcEndpointId` serves on a public host; one created with
/// one serves on a host qualified by that endpoint id under the PrivateLink suffix. Every
/// reader of the connector reports the same host, and a second read reports it again.
#[tokio::test]
async fn endpoint_reflects_the_access_path_the_create_chose() {
    for row in rows() {
        let (router, engine) = new_router();
        let mut request = json!({ "CertificateAuthorityArn": row.ca_arn });
        if let Some(vpce) = row.vpc_endpoint_id {
            request["VpcEndpointId"] = json!(vpce);
        }
        if let Some(mdm) = &row.mobile_device_management {
            request["MobileDeviceManagement"] = mdm.clone();
        }

        let arn = create_active(&router, &engine, row.name, request).await;
        let id = connector_id_of(&arn).to_string();
        let endpoint = get_endpoint(&router, row.name, &arn).await;

        assert!(
            endpoint.starts_with("https://") && endpoint.contains(&id),
            "case {row:?}: the Endpoint must be an HTTPS URL derived from the connector id \
             {id:?}, got {endpoint:?}"
        );

        match row.vpc_endpoint_id {
            None => {
                assert!(
                    endpoint.ends_with(PUBLIC_SUFFIX) && !endpoint.ends_with(PRIVATE_SUFFIX),
                    "case {row:?}: a connector with no VpcEndpointId is reachable over the \
                     public internet, so its Endpoint host must sit under {PUBLIC_SUFFIX:?} and \
                     not under {PRIVATE_SUFFIX:?}, got {endpoint:?}"
                );
                assert!(
                    !endpoint.contains("vpce-"),
                    "case {row:?}: a public connector's Endpoint must name no VPC endpoint, got \
                     {endpoint:?}"
                );
            }
            Some(vpce) => {
                assert!(
                    endpoint.ends_with(PRIVATE_SUFFIX),
                    "case {row:?}: a connector restricted to one VPC endpoint must report a \
                     private host under {PRIVATE_SUFFIX:?}, got {endpoint:?}"
                );
                assert!(
                    endpoint.contains(&format!(".{vpce}.")),
                    "case {row:?}: the Endpoint host must be qualified by the VPC endpoint \
                     {vpce:?} it is reachable through, got {endpoint:?}"
                );
            }
        }

        // The region the request was signed for is still part of the host, whichever access
        // path the create chose.
        assert!(
            endpoint.contains(&format!(".{ARN_SERVICE}.{REGION}.")),
            "case {row:?}: the Endpoint host must carry the service and the Region the request \
             was signed for, got {endpoint:?}"
        );

        // Every reader agrees, and the host does not move between reads.
        let again = get_endpoint(&router, row.name, &arn).await;
        assert_eq!(
            again, endpoint,
            "case {row:?}: the Endpoint must be stable across reads, got {again:?} then \
             {endpoint:?}"
        );
        let listed = list_endpoint(&router, row.name, &arn).await;
        assert_eq!(
            listed, endpoint,
            "case {row:?}: ListConnectors must report the same Endpoint as GetConnector, got \
             {listed:?} vs {endpoint:?}"
        );

        // An Intune connector's OIDC issuer is the service's identity plane, which the Entra
        // tenant fetches over the public internet: it is NOT the SCEP URL and it does not
        // follow the access path the create chose, so restricting the data plane to one VPC
        // endpoint cannot hide the issuer behind a host only that VPC can resolve.
        if row.mobile_device_management.is_some() {
            let read = harness::call(
                &router,
                REGION,
                "GET",
                &format!("/connectors/{}", label(&arn)),
                None,
            )
            .await;
            let issuer = read.body["Connector"]["OpenIdConfiguration"]["Issuer"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_ne!(
                issuer, endpoint,
                "case {row:?}: the OIDC issuer must not be the connector's SCEP URL, got {:?}",
                read.body
            );
            assert!(
                issuer.starts_with("https://")
                    && !issuer.contains(PRIVATE_SUFFIX)
                    && issuer.contains(&format!(".{ARN_SERVICE}.{REGION}{PUBLIC_SUFFIX}/")),
                "case {row:?}: the OIDC issuer must be an HTTPS URL on this service's public \
                 regional domain whatever access path the SCEP endpoint took, got {issuer:?}"
            );
        }
    }
}

/// Two connectors that differ ONLY in `VpcEndpointId` must not be indistinguishable: their
/// hosts differ in more than the connector id, and two different VPC endpoints give two
/// different hosts.
#[tokio::test]
async fn connectors_differing_only_in_vpc_endpoint_report_different_hosts() {
    let (router, engine) = new_router();
    let ca_arn = CA_ARNS[0];

    // Same CA, same (absent) MDM, same Region: the ONLY difference between these creates is
    // the access path.
    let paths: &[Option<&str>] = &[
        None,
        Some("vpce-0123abcd"),
        Some("vpce-0123abcd456789abc"),
        Some("vpce-fedcba9887654321f"),
    ];

    let mut templates: Vec<(Option<&str>, String)> = Vec::new();
    for path in paths {
        let case = format!("VpcEndpointId={path:?}");
        let mut request = json!({ "CertificateAuthorityArn": ca_arn });
        if let Some(vpce) = path {
            request["VpcEndpointId"] = json!(vpce);
        }
        let arn = create_active(&router, &engine, &case, request).await;
        let endpoint = get_endpoint(&router, &case, &arn).await;
        templates.push((*path, without_connector_id(&endpoint, connector_id_of(&arn))));
    }

    for (index, (path, template)) in templates.iter().enumerate() {
        for (other_path, other) in templates.iter().skip(index + 1) {
            assert_ne!(
                template, other,
                "case VpcEndpointId={path:?} vs {other_path:?}: two connectors whose creates \
                 named different access paths must report different SCEP hosts, but both \
                 reported {template:?} once the connector id is factored out"
            );
        }
    }
}
