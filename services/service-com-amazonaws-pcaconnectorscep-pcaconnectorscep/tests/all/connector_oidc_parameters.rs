// SPDX-License-Identifier: CC0-1.0

//! The OIDC federation parameters an Intune connector publishes.
//!
//! `Connector$OpenIdConfiguration` / `ConnectorSummary$OpenIdConfiguration` (smithy_query
//! shape=OpenIdConfiguration) are the "issuer", "subject" and "audience" value "to copy into
//! your Microsoft Entra app registration's OIDC" — the three halves of a federated
//! credential the caller pastes into Entra.
//!
//! The rule these cases assert, over a table rather than one recorded request: every member
//! of a connector's published federation parameters is minted by the service from the
//! connector's own identity, its Region and the federation protocol — the audience is the
//! single audience Microsoft Entra workload identity federation accepts in an exchanged
//! token, the issuer is an HTTPS OIDC issuer URL on the service's own public regional
//! domain naming the connector, and the subject identifies the connector inside that
//! issuer's namespace — so none of the three is taken from, or contains, anything the
//! request carried (the caller's Entra tenant id, its primary domain, the CA ARN), none is
//! the connector's SCEP data-plane URL or its ARN, every reader agrees, and two connectors
//! differ in issuer and subject while sharing the audience.
//!
//! Rows differ along what the rule generalises over: the Region the call is signed for, the
//! shape and length of `AzureApplicationId` (both ends of what its own pattern allows), the
//! `Domain`, the CA, and whether the create restricted the SCEP data plane to a VPC
//! endpoint.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router};

/// The audience a connector publishes: the one audience Microsoft Entra workload identity
/// federation accepts in a token presented for token exchange, which is why it is the same
/// for every connector and for every tenant.
const AUDIENCE: &str = "api://AzureADTokenExchange";

/// The service's endpoint prefix, which is also the label its hosts carry.
const ARN_SERVICE: &str =
    service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state::ARN_SERVICE;

/// DNS suffix of a host reachable over the public internet.
const PUBLIC_SUFFIX: &str = ".amazonaws.com";

/// DNS suffix AWS PrivateLink gives an endpoint-specific DNS name: a host under it resolves
/// only inside the VPC, so an issuer a remote identity provider must fetch cannot sit there.
const PRIVATE_SUFFIX: &str = ".vpce.amazonaws.com";

/// One `CreateConnector` request for an Intune connector, and the Region it is signed for.
#[derive(Debug)]
struct Row {
    name: &'static str,
    region: &'static str,
    ca_arn: &'static str,
    /// Matches `AzureApplicationId`'s own `@length(15, 100)` and pattern
    /// `^[a-zA-Z0-9]{2,15}(-[a-zA-Z0-9]{2,15}){4}$` (smithy_query shape=IntuneConfiguration).
    application_id: &'static str,
    /// Matches `AzureDomain`'s `@length(1, 256)` and pattern `^[a-zA-Z0-9._-]+$`.
    domain: &'static str,
    vpc_endpoint_id: Option<&'static str>,
}

/// Create a connector, settle it and answer its ARN.
async fn create_active(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    region: &str,
    case: &str,
    request: Value,
) -> String {
    let created = harness::call(router, region, "POST", "/connectors", Some(request)).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.text("ConnectorArn");
    harness::release_pending_if_owed(engine, &arn);
    arn
}

/// The whole `Connector` `GetConnector` reports for `arn`.
async fn get_connector(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    arn: &str,
) -> Value {
    let read = harness::call(
        router,
        region,
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
    read.body["Connector"].clone()
}

/// The `ConnectorSummary` `ListConnectors` reports for `arn`.
async fn list_connector(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    arn: &str,
) -> Value {
    let page = harness::call(router, region, "GET", "/connectors", None).await;
    assert_eq!(
        page.status, 200,
        "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
        page.status, page.body
    );
    let summaries = page.body["Connectors"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Connectors array in {:?}", page.body))
        .clone();
    summaries
        .into_iter()
        .find(|entry| entry["Arn"] == json!(arn))
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectorSummary for {arn:?}"))
}

/// The connector id of an ARN: everything after the `connector/` resource segment.
fn connector_id_of(arn: &str) -> &str {
    arn.rsplit_once("connector/")
        .map(|(_, id)| id)
        .unwrap_or_else(|| panic!("expected a connector ARN, got {arn:?}"))
}

/// The authority (host) of an `https://` URL.
fn authority_of(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("expected an https URL, got {url:?}"));
    rest.split_once('/').map(|(host, _)| host).unwrap_or(rest)
}

/// One member of the published parameters, as a string.
fn member(oidc: &Value, name: &str) -> String {
    oidc[name]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string `{name}` in {oidc:?}"))
        .to_string()
}

/// Every published member satisfies the rule: the audience is the federation's own, the
/// issuer is an HTTPS OIDC issuer URL of this service in the connector's Region naming the
/// connector, the subject names the connector inside that issuer, and nothing the request
/// carried appears in any of them.
#[tokio::test]
async fn intune_connector_publishes_service_minted_oidc_parameters() {
    let rows = [
        Row {
            name: "shortest application id the pattern allows",
            region: "us-east-1",
            ca_arn: "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
            // 15 characters: `AzureApplicationId`'s published `@length` minimum.
            application_id: "ab-cd-ef-gh-ijk",
            domain: "example.onmicrosoft.com",
            vpc_endpoint_id: None,
        },
        Row {
            name: "tenant-id shaped application id in a non-default region",
            region: "eu-west-2",
            ca_arn: "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            application_id: "01234567-89ab-cdef-0123-456789abcdef",
            domain: "contoso.example.com",
            vpc_endpoint_id: None,
        },
        Row {
            name: "longest application id the pattern allows, single-label domain",
            region: "ap-southeast-2",
            ca_arn: "arn:aws:acm-pca:ap-southeast-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
            // Five groups of fifteen: the longest string the member's pattern accepts.
            application_id: "abcdefghij12345-klmnopqrst67890-uvwxyzABCDE1234-FGHIJKLMNOP5678-QRSTUVWXYZ9012",
            domain: "localdomain",
            vpc_endpoint_id: None,
        },
        Row {
            name: "data plane restricted to a VPC endpoint",
            region: "us-west-2",
            ca_arn: "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
            application_id: "fedcba98-7654-3210-fedc-ba9876543210",
            domain: "scep.example.onmicrosoft.com",
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
        },
        Row {
            name: "domain made only of the punctuation the pattern allows",
            region: "eu-central-1",
            ca_arn: "arn:aws:acm-pca:eu-central-1:000000000000:certificate-authority/99999999-8888-7777-6666-555544443333",
            application_id: "AA11-bb22-CC33-dd44-EE55",
            domain: "a_b-c.d",
            vpc_endpoint_id: Some("vpce-0123abcd"),
        },
    ];

    for row in &rows {
        let (router, engine) = new_router();
        let mut request = json!({
            "CertificateAuthorityArn": row.ca_arn,
            "MobileDeviceManagement": {
                "Intune": { "AzureApplicationId": row.application_id, "Domain": row.domain },
            },
        });
        if let Some(vpce) = row.vpc_endpoint_id {
            request["VpcEndpointId"] = json!(vpce);
        }

        let arn = create_active(&router, &engine, row.region, row.name, request).await;
        let id = connector_id_of(&arn).to_string();
        let connector = get_connector(&router, row.region, row.name, &arn).await;
        let endpoint = member(&connector, "Endpoint");
        let oidc = connector["OpenIdConfiguration"].clone();

        let audience = member(&oidc, "Audience");
        assert_eq!(
            audience, AUDIENCE,
            "case {row:?}: the audience must be the Entra token-exchange audience, got \
             {audience:?}"
        );

        let issuer = member(&oidc, "Issuer");
        assert!(
            issuer.starts_with("https://"),
            "case {row:?}: the issuer must be an HTTPS OIDC issuer URL, got {issuer:?}"
        );
        let host = authority_of(&issuer);
        assert!(
            host.ends_with(PUBLIC_SUFFIX) && !host.ends_with(PRIVATE_SUFFIX),
            "case {row:?}: the issuer must be served on a host a remote identity provider \
             can resolve over the public internet, got {host:?}"
        );
        assert!(
            host.ends_with(&format!(".{ARN_SERVICE}.{}{PUBLIC_SUFFIX}", row.region)),
            "case {row:?}: the issuer host must be this service's own domain in the Region \
             the call was signed for, got {host:?}"
        );
        assert!(
            issuer.contains(&id),
            "case {row:?}: the issuer must name the connector {id:?}, got {issuer:?}"
        );
        assert_ne!(
            issuer, endpoint,
            "case {row:?}: the issuer must be the service's identity plane, not the \
             connector's SCEP URL, got {issuer:?}"
        );

        let subject = member(&oidc, "Subject");
        assert!(
            subject.contains(&id),
            "case {row:?}: the subject must identify the connector {id:?}, got {subject:?}"
        );
        assert_ne!(
            subject, arn,
            "case {row:?}: the subject must be minted inside the issuer's namespace rather \
             than being the connector ARN, got {subject:?}"
        );
        assert!(
            !subject.starts_with("arn:"),
            "case {row:?}: the subject must not be an ARN, got {subject:?}"
        );

        // Nothing the caller sent may appear in a value the service publishes: the tenant
        // id and the primary domain are what the caller already has, and neither is an
        // OIDC audience, issuer or subject.
        let supplied: Vec<(&str, &str)> = vec![
            ("AzureApplicationId", row.application_id),
            ("Domain", row.domain),
            ("CertificateAuthorityArn", row.ca_arn),
        ];
        for (name, value) in supplied {
            for published in ["Issuer", "Subject", "Audience"] {
                let actual = member(&oidc, published);
                assert!(
                    !actual.contains(value),
                    "case {row:?}: {published} must not carry the request's {name} {value:?}, \
                     got {actual:?}"
                );
            }
        }

        // Every reader reports the same parameters, and a read does not change them.
        let again = get_connector(&router, row.region, row.name, &arn).await;
        assert_eq!(
            again["OpenIdConfiguration"], oidc,
            "case {row:?}: the published parameters must be stable across reads, got {:?} \
             then {oidc:?}",
            again["OpenIdConfiguration"]
        );
        let summary = list_connector(&router, row.region, row.name, &arn).await;
        assert_eq!(
            summary["OpenIdConfiguration"], oidc,
            "case {row:?}: ListConnectors must publish the same parameters as GetConnector, \
             got {:?} vs {oidc:?}",
            summary["OpenIdConfiguration"]
        );
    }
}

/// Two Intune connectors created from the SAME request in the same Region are two federated
/// identities: their issuers and subjects differ, while the audience — a property of the
/// federation rather than of a connector — is identical.
#[tokio::test]
async fn two_intune_connectors_publish_distinct_identities() {
    let region = "eu-west-2";
    let case = "two connectors from one request";
    let (router, engine) = new_router();
    let request = json!({
        "CertificateAuthorityArn":
            "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/abcdef01-2345-6789-abcd-ef0123456789",
        "MobileDeviceManagement": {
            "Intune": {
                "AzureApplicationId": "01234567-89ab-cdef-0123-456789abcdef",
                "Domain": "example.onmicrosoft.com",
            },
        },
    });

    let first_arn = create_active(&router, &engine, region, case, request.clone()).await;
    let second_arn = create_active(&router, &engine, region, case, request).await;
    assert_ne!(
        first_arn, second_arn,
        "case {case:?}: two creates without a ClientToken must mint two connectors, got \
         {first_arn:?} twice"
    );

    let first = get_connector(&router, region, case, &first_arn).await["OpenIdConfiguration"]
        .clone();
    let second = get_connector(&router, region, case, &second_arn).await["OpenIdConfiguration"]
        .clone();

    assert_ne!(
        member(&first, "Issuer"),
        member(&second, "Issuer"),
        "case {case:?}: two connectors must publish two issuers, both got {:?}",
        member(&first, "Issuer")
    );
    assert_ne!(
        member(&first, "Subject"),
        member(&second, "Subject"),
        "case {case:?}: two connectors must publish two subjects, both got {:?}",
        member(&first, "Subject")
    );
    assert_eq!(
        member(&first, "Audience"),
        member(&second, "Audience"),
        "case {case:?}: the audience is the federation's, so both connectors must publish \
         the same one, got {:?} vs {:?}",
        member(&first, "Audience"),
        member(&second, "Audience")
    );
}

/// OIDC parameters exist for Microsoft Intune, so a general-purpose connector — one created
/// without `MobileDeviceManagement` — publishes none, on either reader.
#[tokio::test]
async fn general_purpose_connector_publishes_no_oidc_parameters() {
    let region = "us-east-1";
    let case = "general purpose connector";
    let (router, engine) = new_router();
    let arn = create_active(
        &router,
        &engine,
        region,
        case,
        json!({
            "CertificateAuthorityArn":
                "arn:aws:acm-pca:us-east-1:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
        }),
    )
    .await;

    for (reader, connector) in [
        ("GetConnector", get_connector(&router, region, case, &arn).await),
        ("ListConnectors", list_connector(&router, region, case, &arn).await),
    ] {
        assert_eq!(
            connector["OpenIdConfiguration"],
            Value::Null,
            "case {case:?}: {reader} must publish no OIDC parameters for a connector with no \
             Intune configuration, got {:?}",
            connector["OpenIdConfiguration"]
        );
    }
}
