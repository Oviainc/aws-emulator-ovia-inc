// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AuthorizedPrincipal$PrincipalType`: the words the service
//! answers for the KIND of principal a VPC endpoint access grant was made for.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the two calls from [`super::vpc_endpoint_access`].
//!
//! RULE: the type answered names the KIND of principal, in the words the service answers
//! that kind in — `AWS_ACCOUNT` for an account grantee, and `AWS Service` for a service
//! grantee, which is NOT how the `PrincipalType` enum declares that kind
//! (`smithy_query(query="shape",shape="PrincipalType")` declares `AWS_ACCOUNT,
//! AWS_SERVICE`). It is the same value wherever the record is read back: the grant's own
//! answer and the listing report ONE record, so a client switching on the string, or
//! round-tripping it into a grant list, sees one value per kind.
//!
//! The table varies what the rule generalises over: the kind of principal, the identity
//! that names it (account ids of different lengths and digits, the one service principal
//! the model admits), the service options carried with the grant, how many principals the
//! domain already holds, and whether the grant is the principal's first or a re-grant
//! that replaces its record. Each row is read back through BOTH paths.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PRINCIPAL_TYPE_AWS_ACCOUNT, PRINCIPAL_TYPE_AWS_SERVICE,
};

use super::domain_lifecycle::Emulator;
use super::vpc_endpoint_access::{
    authorize, created_domain, list, principal_list, service_principal,
};

/// The value the service answers for an ACCOUNT grantee, spelled out here rather than
/// taken from the crate, so a change to either one fails this suite.
const ACCOUNT_TYPE: &str = "AWS_ACCOUNT";

/// The value the service answers for a SERVICE grantee: two words, space-separated.
const SERVICE_TYPE: &str = "AWS Service";

/// One grant to make: the body it carries, and the type the record must be answered
/// with.
struct Row {
    case: &'static str,
    /// The request body, which carries everything but the domain name.
    body: Value,
    /// The principal the record is addressed by, which is the identity the body named.
    principal: String,
    /// The `PrincipalType` both read paths must answer.
    expected_type: &'static str,
}

/// Every row of the table, in the order they are granted on ONE domain — so each later
/// row is granted on a domain that already holds principals, and the last pair re-grants
/// a principal already in the set.
fn rows() -> Vec<Row> {
    let service = service_principal().to_string();
    vec![
        Row {
            case: "an account grantee, the first principal of the domain",
            body: json!({ "Account": "123456789012" }),
            principal: "123456789012".to_owned(),
            expected_type: ACCOUNT_TYPE,
        },
        Row {
            case: "a service grantee beside it",
            body: json!({ "Service": service.clone() }),
            principal: service.clone(),
            expected_type: SERVICE_TYPE,
        },
        Row {
            case: "a one-digit account id",
            body: json!({ "Account": "7" }),
            principal: "7".to_owned(),
            expected_type: ACCOUNT_TYPE,
        },
        Row {
            case: "an account id past twelve digits, with service options",
            body: json!({
                "Account": "98765432109876543210",
                "ServiceOptions": { "SupportedRegions": ["eu-west-1", "us-east-1"] },
            }),
            principal: "98765432109876543210".to_owned(),
            expected_type: ACCOUNT_TYPE,
        },
        Row {
            case: "a re-grant of the service grantee, with service options this time",
            body: json!({
                "Service": service.clone(),
                "ServiceOptions": { "SupportedRegions": ["ap-south-1"] },
            }),
            principal: service.clone(),
            expected_type: SERVICE_TYPE,
        },
        Row {
            case: "a re-grant of the first account, with empty service options",
            body: json!({ "Account": "123456789012", "ServiceOptions": {} }),
            principal: "123456789012".to_owned(),
            expected_type: ACCOUNT_TYPE,
        },
    ]
}

/// The `PrincipalType` of the entry `principal` is reported under, with the case named
/// on failure.
fn reported_type(entries: &[Value], principal: &str, case: &str) -> String {
    let entry = entries
        .iter()
        .find(|entry| entry["Principal"].as_str() == Some(principal))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {principal:?} in the reported set, got {entries:?}")
        });
    entry["PrincipalType"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a string PrincipalType, got {entry:?}")
        })
        .to_owned()
}

#[tokio::test]
async fn the_principal_type_answered_names_the_kind_of_principal_granted() {
    let emulator = Emulator::new();
    let domain = "principal-type";
    created_domain(&emulator, domain).await;

    for row in rows() {
        let case = row.case;
        let granted = authorize(&emulator, domain, row.body.clone()).await;
        assert_eq!(
            granted.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            granted.status, granted.body
        );

        // The grant's OWN answer.
        let answered = granted.body["AuthorizedPrincipal"]["PrincipalType"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            answered, row.expected_type,
            "case {case:?}: expected the grant to answer PrincipalType {:?}, got {:?}",
            row.expected_type, granted.body
        );

        // And the listing, which reports the same ONE record.
        let listed = list(&emulator, domain, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the listing to be served with 200, got {} with {:?}",
            listed.status, listed.body
        );
        let entries = principal_list(&listed, case);
        let read_back = reported_type(&entries, &row.principal, case);
        assert_eq!(
            read_back, row.expected_type,
            "case {case:?}: expected the listing to report PrincipalType {:?} for {:?}, got \
             {entries:?}",
            row.expected_type, row.principal
        );
    }
}

#[tokio::test]
async fn the_two_kinds_are_answered_with_different_values_on_every_domain() {
    let emulator = Emulator::new();

    // One row per domain the pair is granted on, so the value belongs to the KIND rather
    // than to the domain or to the order the two grants were made in.
    let domains = [
        ("the service grantee granted first", "principal-kinds-a", true),
        ("the account grantee granted first", "principal-kinds-b", false),
    ];
    for (case, domain, service_first) in domains {
        created_domain(&emulator, domain).await;
        let account = json!({ "Account": "210987654321" });
        let service = json!({ "Service": service_principal() });
        let order = if service_first {
            [service.clone(), account.clone()]
        } else {
            [account.clone(), service.clone()]
        };
        for body in order {
            let granted = authorize(&emulator, domain, body.clone()).await;
            assert_eq!(
                granted.status, 200,
                "case {case:?}: expected 200 for {body:?}, got {} with {:?}",
                granted.status, granted.body
            );
        }

        let listed = list(&emulator, domain, None).await;
        let entries = principal_list(&listed, case);
        let for_account = reported_type(&entries, "210987654321", case);
        let for_service = reported_type(&entries, service_principal(), case);
        assert_eq!(
            for_account, PRINCIPAL_TYPE_AWS_ACCOUNT,
            "case {case:?}: expected the account grantee to read {PRINCIPAL_TYPE_AWS_ACCOUNT:?}, \
             got {entries:?}"
        );
        assert_eq!(
            for_service, PRINCIPAL_TYPE_AWS_SERVICE,
            "case {case:?}: expected the service grantee to read {PRINCIPAL_TYPE_AWS_SERVICE:?}, \
             got {entries:?}"
        );
        assert_ne!(
            for_account, for_service,
            "case {case:?}: the two kinds are answered with different values, got {entries:?}"
        );
    }
}
