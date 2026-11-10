// SPDX-License-Identifier: CC0-1.0

//! The `Protocol` of an application status check, read the way EC2 reads it, over the wire the
//! service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML;
//! README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a request that spells one of the member's modelled values in another
//! ASCII case names that value, so the create is SERVED and the check is stored with the
//! spelling the model declares — while a value that is no declared one, in any case, is still
//! refused for the value rather than created.
//!
//! The rows span the class: every member of the model's own catalog
//! (`NETWORK_PROTOCOL_ENUM_VALUES`) in its declared spelling, upper case, title case and mixed
//! case, with the check read back through `DescribeApplicationStatusChecks` so the stored
//! spelling is what a later reader sees; a refusal table of values outside the catalog,
//! including one that differs from a declared value by more than case; and the member of the
//! same NAME on another operation's input, which keeps the value the client sent.

use ec2::ops::types::NETWORK_PROTOCOL_ENUM_VALUES;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code};

/// The Region every call here is signed for. A check's configuration is a fact of the check
/// rather than of the Region.
const REGION: &str = "us-west-2";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `CreateApplicationStatusCheck` naming `protocol`, as its HTTP status and body.
async fn create(router: &MultiServiceRouter, protocol: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "CreateApplicationStatusCheck",
        &[p("Protocol", protocol), p("Port", "80"), p("Path", "/health")],
    )
    .await
}

/// Every spelling of `declared` a client may send it in: as the model declares it, upper case,
/// title case, and one alternating case — the class the rule generalises over.
fn spellings(declared: &str) -> Vec<String> {
    let upper = declared.to_ascii_uppercase();
    let mut title = declared.to_string();
    title[..1].make_ascii_uppercase();
    let alternating: String = declared
        .chars()
        .enumerate()
        .map(|(index, character)| {
            if index % 2 == 0 {
                character.to_ascii_uppercase()
            } else {
                character
            }
        })
        .collect();
    vec![declared.to_string(), upper, title, alternating]
}

/// Each spelling of each declared protocol is served, and the check is stored with the declared
/// spelling — which is what a later describe reports.
#[tokio::test]
async fn a_protocol_spelled_in_another_case_names_the_declared_value() {
    let (router, _engine) = super::ec2_query::emulator();

    for declared in NETWORK_PROTOCOL_ENUM_VALUES {
        for spelling in spellings(declared) {
            let case = format!("{declared} sent as {spelling:?}");
            let (status, body) = create(&router, &spelling).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a protocol spelled in another case must be served, got HTTP \
                 {status} with body {body}"
            );
            assert_eq!(
                element(&body, "protocol").as_deref(),
                Some(*declared),
                "case {case:?}: the created check must carry the declared spelling, got {body}"
            );
            let id = element(&body, "applicationStatusCheckId").unwrap_or_else(|| {
                panic!("case {case:?}: the create must mint an id: {body}")
            });

            // Read it back: the stored spelling is what a later reader sees.
            let (described_status, described) = call(
                &router,
                REGION,
                "DescribeApplicationStatusChecks",
                &[p("ApplicationStatusCheckId.1", id.as_str())],
            )
            .await;
            assert_eq!(
                described_status, 200,
                "case {case:?}: the check must read back, got HTTP {described_status} with body \
                 {described}"
            );
            assert_eq!(
                elements(&described, "protocol"),
                vec![(*declared).to_string()],
                "case {case:?}: the stored protocol must be the declared spelling, got \
                 {described}"
            );
        }
    }
}

/// The member this service reads without regard to case is a member OF ONE INPUT SHAPE: a
/// member of the same name on another operation's input keeps the value the client sent.
///
/// `CreateNetworkAclEntryRequest$Protocol` is a free-text `String` holding a protocol NUMBER,
/// so a respelling there would refuse a value the caller never sent. The refusal must echo the
/// value as it arrived.
#[tokio::test]
async fn a_member_of_the_same_name_on_another_input_is_untouched() {
    let (router, _engine) = super::ec2_query::emulator();

    let (status, body) = call(&router, REGION, "CreateVpc", &[p("CidrBlock", "10.9.0.0/16")]).await;
    assert_eq!(
        status, 200,
        "the VPC the ACL belongs to must be created, got {status} with body {body}"
    );
    let vpc_id = element(&body, "vpcId")
        .unwrap_or_else(|| panic!("CreateVpc must mint a vpc id: {body}"));

    let (status, body) = call(&router, REGION, "CreateNetworkAcl", &[p("VpcId", &vpc_id)]).await;
    assert_eq!(
        status, 200,
        "the network ACL must be created, got {status} with body {body}"
    );
    let acl_id = element(&body, "networkAclId")
        .unwrap_or_else(|| panic!("CreateNetworkAcl must mint an id: {body}"));

    // The same wire key, the same value: here it is no declared enum value, and the refusal
    // must name it exactly as sent.
    for sent in ["HTTP", "Http"] {
        let (status, body) = call(
            &router,
            REGION,
            "CreateNetworkAclEntry",
            &[
                p("NetworkAclId", &acl_id),
                p("RuleNumber", "100"),
                p("Egress", "false"),
                p("RuleAction", "allow"),
                p("CidrBlock", "0.0.0.0/0"),
                p("Protocol", sent),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {sent:?}: a protocol that is no number must be refused, got HTTP {status} \
             with body {body}"
        );
        assert!(
            body.contains(&format!("({sent})")),
            "case {sent:?}: the refusal must name the value as sent, got {body}"
        );
    }
}

/// A value that is no declared one is refused for the VALUE, in every case it is sent in, and
/// nothing is created for it.
#[tokio::test]
async fn a_protocol_outside_the_catalog_is_still_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // Values outside `NETWORK_PROTOCOL_ENUM_VALUES`: another protocol word, a declared value
    // with something added, a prefix of one, and the empty value.
    for outside in ["tcp", "TCP", "httpx", "HTTPX", "htt", "HTTP ", ""] {
        let (status, body) = create(&router, outside).await;
        assert_eq!(
            status, 400,
            "case {outside:?}: a value outside the catalog must be refused, got HTTP {status} \
             with body {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {outside:?}: the refusal must carry an error code, got {body}"
        );
    }

    let held = engine
        .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
        .lock()
        .application_status_checks
        .len();
    assert_eq!(
        held, 0,
        "a refused create must store no check, got {held} stored"
    );
}
