// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the transit-gateway-registration slice
//! (`RegisterTransitGateway`, `GetTransitGatewayRegistrations`,
//! `DeregisterTransitGateway`), driven over HTTP.
//!
//! The rule under test: a transit gateway registration is a STORED tie between one transit
//! gateway of Amazon VPC and one global network, identified by the global network plus the
//! gateway's ARN and carrying no id of its own. The register stores it `PENDING` and the
//! clock settles it `AVAILABLE`; the reader answers the stored ties of its global network
//! and nothing else; the deregister REMOVES the stored tie, reports the removed one
//! `DELETING`, and leaves a later read with nothing to report, so a repeat of it is a
//! not-found. A gateway is registered with at most ONE global network, and a string that
//! names no transit gateway at all is refused on the member that carried it.
//!
//! A gateway is registered only with a global network of its OWN account, so the ARN's
//! account segment is fixed by the global network and the tables below span what is left
//! free: the ARN's partition, Region and id width. The account rule itself is tabled by
//! `crate::transit_gateway_registration_account_ownership`.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: a registration is only ever read back through
//! `GetTransitGatewayRegistrations`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_tagging::escape;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The resource type a refusal names for a registration, which has no id of its own.
const REGISTRATION_KIND: &str = "transit-gateway-registration";

/// The request member a refusal names when the transit gateway ARN is at fault: Network
/// Manager names the offending member in its own lowerCamelCase spelling.
pub(crate) const ARN_MEMBER: &str = "transitGatewayArn";

/// `RegisterTransitGateway` on `network` for `arn`.
pub(crate) async fn register(emulator: &Emulator, network: &str, arn: &str) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/transit-gateway-registrations"),
            json!({ "TransitGatewayArn": arn }),
        )
        .await
}

/// `DeregisterTransitGateway` for `arn` on `network`. The ARN travels in a NON-greedy
/// `@httpLabel`, so a client sends its `:` and `/` percent-encoded.
pub(crate) async fn deregister(emulator: &Emulator, network: &str, arn: &str) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/transit-gateway-registrations/{}",
            escape(arn)
        ))
        .await
}

/// `GetTransitGatewayRegistrations` on `network` with `query` appended to the URI.
pub(crate) async fn get_registrations(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/transit-gateway-registrations")
    } else {
        format!("/global-networks/{network}/transit-gateway-registrations?{query}")
    };
    emulator.get(&uri).await
}

/// The `TransitGatewayArn`s a reader page lists, in the order returned.
pub(crate) fn arns_of(body: &Value) -> Vec<String> {
    body["TransitGatewayRegistrations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a TransitGatewayRegistrations array in {body}"))
        .iter()
        .map(|registration| {
            registration["TransitGatewayArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected TransitGatewayArn in {registration}"))
                .to_string()
        })
        .collect()
}

/// The state code one registration reports, or `<none>` when it carries none.
fn state_code(registration: &Value) -> &str {
    registration["State"]["Code"].as_str().unwrap_or("<none>")
}

/// The one registration the reader answers for `arn`, or `None` when it answers none.
async fn read_one(emulator: &Emulator, network: &str, arn: &str) -> Option<Value> {
    let (status, body) = get_registrations(
        emulator,
        network,
        &format!("transitGatewayArns={}", escape(arn)),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetTransitGatewayRegistrations({arn}): expected 200, got {status} with {body}"
    );
    let page = body["TransitGatewayRegistrations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a TransitGatewayRegistrations array in {body}"))
        .clone();
    assert!(
        page.len() <= 1,
        "case read {arn}: one ARN selects at most one registration, got {body}"
    );
    page.into_iter().next()
}

/// One row of the round-trip table: the transit gateway being registered.
struct Row {
    name: &'static str,
    arn: String,
}

/// The table: rows differing along everything the rule leaves free — the ARN's partition,
/// Region and the width of the gateway id, whose published bounds are 8 and 17 hex digits
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpc\/vpc-[0-9a-f]{8,17}$` for `VpcArn`,
/// `smithy_query shape=VpcArn`, is the skeleton every EC2 ARN of this model spells). The
/// account is the one that owns the global network on every row, because that is the only
/// account a gateway may be registered from.
fn rows() -> Vec<Row> {
    let account = own_account();
    vec![
        Row {
            name: "the shortest published id",
            arn: transit_gateway_arn("aws", "us-west-2", &account, 1, 8),
        },
        Row {
            name: "the longest published id",
            arn: transit_gateway_arn("aws", "eu-central-1", &account, 2, 17),
        },
        Row {
            name: "another partition and Region",
            arn: transit_gateway_arn("aws-cn", "cn-north-1", &account, 3, 12),
        },
        Row {
            name: "a third Region",
            arn: transit_gateway_arn("aws", "ap-southeast-2", &account, 4, 10),
        },
        Row {
            name: "a third partition",
            arn: transit_gateway_arn("aws-us-gov", "us-gov-west-1", &account, 5, 16),
        },
    ]
}

#[tokio::test]
async fn transit_gateway_registrations_round_trip_through_stored_state() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("transit gateways").await;
    let rows = rows();

    for row in &rows {
        let case = row.name;
        let (status, created) = register(&emulator, &network, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from RegisterTransitGateway, got {status} with {created}"
        );
        let registration = &created["TransitGatewayRegistration"];
        assert_eq!(
            registration["TransitGatewayArn"],
            json!(row.arn),
            "case {case}: the registration must name the gateway asked for, got {created}"
        );
        assert_eq!(
            registration["GlobalNetworkId"],
            json!(network),
            "case {case}: the registration must name its global network, got {created}"
        );
        // A resource this service reports in a non-terminal state owes the clock one
        // transition, so a caller acting straight after the register still sees PENDING.
        assert_eq!(
            state_code(registration),
            "PENDING",
            "case {case}: a fresh registration must be PENDING, got {created}"
        );

        // The stored tie — not the answer above — is what the reader answers from.
        let stored = read_one(&emulator, &network, &row.arn)
            .await
            .unwrap_or_else(|| panic!("case {case}: the reader must list the registration"));
        assert_eq!(
            stored["GlobalNetworkId"],
            json!(network),
            "case {case}: the reader must answer the stored global network, got {stored}"
        );
    }

    // Every row is listed together once the unfiltered read is taken, in one stable order,
    // and the state the clock owed has fallen due for all of them by now.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, page) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case unfiltered read: expected 200, got {status} with {page}"
    );
    let mut expected: Vec<String> = rows.iter().map(|row| row.arn.clone()).collect();
    expected.sort();
    assert_eq!(
        arns_of(&page),
        expected,
        "case unfiltered read: expected every stored registration, got {page}"
    );
    for registration in page["TransitGatewayRegistrations"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        assert_eq!(
            state_code(registration),
            "AVAILABLE",
            "case settled read: the transition the clock owed must have fallen due, got {registration}"
        );
    }

    // Reading twice changes nothing a later caller sees.
    let (status, again) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case repeated read: expected 200, got {status} with {again}"
    );
    assert_eq!(
        arns_of(&again),
        expected,
        "case repeated read: a read must not change what a later caller sees, got {again}"
    );

    // A page size walks the same listing one registration at a time, and the tokens reach
    // every one of them exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=rows.len() {
        let query = match &token {
            Some(next) => format!("maxResults=1&nextToken={}", escape(next)),
            None => "maxResults=1".to_string(),
        };
        let (status, body) = get_registrations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case paged read {step}: expected 200, got {status} with {body}"
        );
        let listed = arns_of(&body);
        assert!(
            listed.len() <= 1,
            "case paged read {step}: a page of one must hold at most one item, got {body}"
        );
        walked.extend(listed);
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, expected,
        "case paged read: following every token must reach each registration once, got {walked:?}"
    );

    // DEREGISTER: the removed registration is reported DELETING, a later read no longer
    // lists it, and a repeat of the same call is the not-found this operation models.
    for row in &rows {
        let case = row.name;
        let (status, removed) = deregister(&emulator, &network, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DeregisterTransitGateway, got {status} with {removed}"
        );
        let registration = &removed["TransitGatewayRegistration"];
        assert_eq!(
            registration["TransitGatewayArn"],
            json!(row.arn),
            "case {case}: the removal must report the gateway asked for, got {removed}"
        );
        assert_eq!(
            registration["GlobalNetworkId"],
            json!(network),
            "case {case}: the removal must report the stored global network, got {removed}"
        );
        assert_eq!(
            state_code(registration),
            "DELETING",
            "case {case}: the removed registration must be reported DELETING, got {removed}"
        );

        assert!(
            read_one(&emulator, &network, &row.arn).await.is_none(),
            "case {case}: a deregistered gateway must not be listed any more"
        );

        let (status, body) = deregister(&emulator, &network, &row.arn).await;
        assert_eq!(
            status, 404,
            "case {case} repeated: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case} repeated: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(REGISTRATION_KIND),
            "case {case} repeated: the refusal must name the registration kind, got {body}"
        );
    }

    let (status, empty) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after every deregister: expected 200, got {status} with {empty}"
    );
    assert_eq!(
        empty["TransitGatewayRegistrations"],
        json!([]),
        "case read after every deregister: expected an empty page, got {empty}"
    );
}

#[tokio::test]
async fn a_transit_gateway_is_registered_with_at_most_one_global_network() {
    let emulator = Emulator::new();
    let first = emulator.create_global_network("first network").await;
    let second = emulator.create_global_network("second network").await;

    for row in &rows() {
        let case = row.name;
        let (status, created) = register(&emulator, &first, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from the first register, got {status} with {created}"
        );

        // The identity of the registration is the global network plus the gateway, so the
        // SAME request again reports the stored registration and restarts nothing: once the
        // settle span has passed, a repeat still answers AVAILABLE.
        tokio::time::sleep(PAST_SETTLE).await;
        let (status, repeated) = register(&emulator, &first, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case} repeated: expected 200, got {status} with {repeated}"
        );
        assert_eq!(
            state_code(&repeated["TransitGatewayRegistration"]),
            "AVAILABLE",
            "case {case} repeated: a retry must not restart the settle span, got {repeated}"
        );

        // The same gateway offered to a SECOND global network is the modelled conflict, and
        // stores nothing: the other global network still holds no registration for it.
        let (status, conflict) = register(&emulator, &second, &row.arn).await;
        assert_eq!(
            status, 409,
            "case {case} second network: expected 409, got {status} with {conflict}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {case} second network: expected ConflictException, got {conflict}"
        );
        assert_eq!(
            conflict["ResourceType"],
            json!(REGISTRATION_KIND),
            "case {case} second network: the refusal must name the registration kind, got {conflict}"
        );
        assert!(
            read_one(&emulator, &second, &row.arn).await.is_none(),
            "case {case} second network: a refused register must store nothing"
        );

        // A registration made in one global network is not addressable through another, so
        // the deregister there is a not-found and leaves the stored one alone.
        let (status, body) = deregister(&emulator, &second, &row.arn).await;
        assert_eq!(
            status, 404,
            "case {case} deregister elsewhere: expected 404, got {status} with {body}"
        );
        assert!(
            read_one(&emulator, &first, &row.arn).await.is_some(),
            "case {case} deregister elsewhere: the stored registration must survive"
        );

        // Deregistering frees the gateway, so the second global network can register it.
        let (status, removed) = deregister(&emulator, &first, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case} deregister: expected 200, got {status} with {removed}"
        );
        let (status, moved) = register(&emulator, &second, &row.arn).await;
        assert_eq!(
            status, 200,
            "case {case} register after deregister: expected 200, got {status} with {moved}"
        );
        assert_eq!(
            moved["TransitGatewayRegistration"]["GlobalNetworkId"],
            json!(second),
            "case {case} register after deregister: the registration must name the new global network, got {moved}"
        );
        assert!(
            read_one(&emulator, &first, &row.arn).await.is_none(),
            "case {case} register after deregister: the first global network must list nothing"
        );
    }
}

/// One row of the malformed-ARN table: a string that names no transit gateway.
struct Malformed {
    name: &'static str,
    value: String,
}

#[tokio::test]
async fn a_string_that_names_no_transit_gateway_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("malformed gateways").await;

    // The rows span the ways the EC2 ARN skeleton can fail: not an ARN at all, another
    // service's namespace, another resource type, and each segment the skeleton requires
    // left empty.
    let cases = [
        Malformed {
            name: "a bare gateway id",
            value: "tgw-0123456789abcdef1".to_string(),
        },
        Malformed {
            name: "another service's namespace",
            value: "arn:aws:directconnect:us-west-2:123456789012:transit-gateway/tgw-0123abcd"
                .to_string(),
        },
        Malformed {
            name: "another resource type",
            value: "arn:aws:ec2:us-west-2:123456789012:vpc/vpc-0123abcd".to_string(),
        },
        Malformed {
            name: "no partition",
            value: "arn::ec2:us-west-2:123456789012:transit-gateway/tgw-0123abcd".to_string(),
        },
        Malformed {
            name: "no Region",
            value: "arn:aws:ec2::123456789012:transit-gateway/tgw-0123abcd".to_string(),
        },
        Malformed {
            name: "no resource id",
            value: "arn:aws:ec2:us-west-2:123456789012:transit-gateway/".to_string(),
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, refused) = register(&emulator, &network, &case.value).await;
        assert_eq!(
            status, 400,
            "case {name} register: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name} register: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {name} register: the refusal must name the member at fault, got {refused}"
        );

        let (status, refused) = deregister(&emulator, &network, &case.value).await;
        assert_eq!(
            status, 400,
            "case {name} deregister: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {name} deregister: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {name} deregister: the refusal must name the member at fault, got {refused}"
        );
    }

    let (status, empty) = get_registrations(&emulator, &network, "").await;
    assert_eq!(
        status, 200,
        "case read after the refusals: expected 200, got {status} with {empty}"
    );
    assert_eq!(
        empty["TransitGatewayRegistrations"],
        json!([]),
        "case read after the refusals: a refused register must store nothing, got {empty}"
    );
}

#[tokio::test]
async fn a_global_network_that_is_not_stored_is_refused_on_every_registration_operation() {
    let emulator = Emulator::new();
    let absent = "global-network-0000000000000dead";
    let arn = transit_gateway_arn("aws", "us-west-2", &own_account(), 9, 17);

    // The REPORTING and the DEREGISTERING calls are addressed at state the store already
    // holds, so a global network it does not hold is the missing-resource answer.
    let calls: [(&str, (u16, Value)); 2] = [
        (
            "GetTransitGatewayRegistrations",
            get_registrations(&emulator, absent, "").await,
        ),
        (
            "DeregisterTransitGateway",
            deregister(&emulator, absent, &arn).await,
        ),
    ];

    for (case, (status, body)) in &calls {
        assert_eq!(
            *status, 404,
            "case {case}: expected 404 for an unknown global network, got {status} with {body}"
        );
        assert_eq!(
            error_code(body),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("global-network"),
            "case {case}: the refusal must name the global network, got {body}"
        );
    }

    // REGISTERING stores a new record under the global network the path addresses, so that
    // parent reference is validated before anything is stored: the refusal is the field
    // failure the creates under a global network answer, naming the member at fault.
    let (status, body) = register(&emulator, absent, &arn).await;
    assert_eq!(
        status, 400,
        "case RegisterTransitGateway: expected 400 for an unknown global network, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case RegisterTransitGateway: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("FieldValidationFailed"),
        "case RegisterTransitGateway: expected Reason FieldValidationFailed, got {body}"
    );
    assert_eq!(
        body["Fields"],
        json!([{ "Name": "globalNetworkId", "Message": "Global network not found." }]),
        "case RegisterTransitGateway: expected one Fields entry naming globalNetworkId, got {body}"
    );

    // The combined case: a request whose gateway ARN names no transit gateway at all is a
    // FIELD failure, decided before any stored state is consulted — the same order the
    // customer-gateway slice of this service resolves its own free-form ARN in — so it
    // answers 400 even under a global network the store does not hold.
    let malformed = "tgw-0123456789abcdef1";
    let combined: [(&str, (u16, Value)); 2] = [
        (
            "RegisterTransitGateway",
            register(&emulator, absent, malformed).await,
        ),
        (
            "DeregisterTransitGateway",
            deregister(&emulator, absent, malformed).await,
        ),
    ];
    for (case, (status, body)) in &combined {
        assert_eq!(
            *status, 400,
            "case {case} with a malformed ARN: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(body),
            "ValidationException",
            "case {case} with a malformed ARN: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"],
            json!(ARN_MEMBER),
            "case {case} with a malformed ARN: the refusal must name the member at fault, got {body}"
        );
    }
}
