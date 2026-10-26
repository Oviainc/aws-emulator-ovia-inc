// SPDX-License-Identifier: CC0-1.0

//! The ORDER a refusal lists an enum's accepted values in.
//!
//! The rule asserted here: when a value outside a modelled enum's value set is
//! refused, the message states that set in the order the SERVICE publishes it,
//! which is a property of the enum shape rather than of the member or the
//! operation the value travelled in — so every member targeting one enum reports
//! the same list in the same order, whatever request carried it. The value set
//! itself, the member path and the `N validation error(s) detected: Value '…' at
//! '…' failed to satisfy constraint: Member must satisfy enum value set: […]`
//! frame are the model's and the framework's; only the order inside the brackets
//! is the service's, and an enum the service publishes no order for keeps the
//! order the model declares.
//!
//! The table spans the operations and members a record type travels in (the
//! queried type of `TestDNSAnswer`, the start position of
//! `ListResourceRecordSets`, the type of a record set inside a change batch),
//! the value refused (a type that does not exist, a modelled type in the wrong
//! case, the DNS query type `ANY` the API does not model), where it travels (the
//! query string, the body) — and, as a control, an enum of ANOTHER shape, which
//! keeps its declared order.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::record_set_validation::message;

/// The zone every refusal below is addressed at.
const ZONE: &str = "example.com";

/// The one input error this service's model declares for an input it cannot use.
const INVALID_INPUT: &str = "InvalidInput";

/// `com.amazonaws.route53#RRType`'s value set, in the order Route 53 states it in
/// when it refuses a value outside it.
///
/// This is NOT the model's declaration order (`smithy_query(query="shape",
/// shape="RRType")` gives `SOA, A, TXT, NS, CNAME, MX, NAPTR, PTR, SRV, SPF,
/// AAAA, CAA, DS, TLSA, SSHFP, SVCB, HTTPS`), which is why the order is asserted
/// here at all: the service publishes the set in its own order and a client
/// reading the accepted types off the message sees that one.
const RR_TYPE_VALUE_SET: &str = "[AAAA, A, SVCB, CAA, NS, SOA, TLSA, SPF, HTTPS, MX, PTR, \
                                  CNAME, DS, TXT, SSHFP, SRV, NAPTR]";

/// The requirement clause an enum's value set states.
fn enum_requirement(value_set: &str) -> String {
    format!("Member must satisfy enum value set: {value_set}")
}

/// One refused request, and the member and value its message must state.
struct RefusalCase {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: String,
    /// The member path the clause names, as the framework spells it.
    member: &'static str,
    /// The value the request sent, which the clause echoes.
    value: &'static str,
    /// The bracketed value set the clause must list.
    value_set: &'static str,
}

/// Send one case and return the reply.
async fn refuse(router: &MultiServiceRouter, case: &RefusalCase) -> Reply {
    call(router, case.method, &case.uri, &case.body).await
}

/// A change batch carrying one record set of type `rr_type`.
fn change_batch(rr_type: &str) -> String {
    format!(
        "<ChangeResourceRecordSetsRequest \
         xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\"><ChangeBatch><Changes>\
         <Change><Action>CREATE</Action><ResourceRecordSet><Name>www.example.com</Name>\
         <Type>{rr_type}</Type><TTL>60</TTL><ResourceRecords><ResourceRecord>\
         <Value>192.0.2.1</Value></ResourceRecord></ResourceRecords></ResourceRecordSet>\
         </Change></Changes></ChangeBatch></ChangeResourceRecordSetsRequest>"
    )
}

#[tokio::test]
async fn a_refused_enum_value_lists_the_accepted_set_in_the_order_the_service_states_it() {
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-value-set-order").await;

    let cases = [
        RefusalCase {
            label: "the queried record type of TestDNSAnswer",
            method: "GET",
            uri: format!(
                "/2013-04-01/testdnsanswer?hostedzoneid={zone_id}\
                 &recordname=example.com&recordtype=NOTAREALTYPE"
            ),
            body: String::new(),
            member: "recordType",
            value: "NOTAREALTYPE",
            value_set: RR_TYPE_VALUE_SET,
        },
        RefusalCase {
            label: "a modelled record type in the wrong case",
            method: "GET",
            uri: format!(
                "/2013-04-01/testdnsanswer?hostedzoneid={zone_id}\
                 &recordname=example.com&recordtype=aaaa"
            ),
            body: String::new(),
            member: "recordType",
            value: "aaaa",
            value_set: RR_TYPE_VALUE_SET,
        },
        RefusalCase {
            label: "the DNS query type ANY, which the API does not model",
            method: "GET",
            uri: format!(
                "/2013-04-01/testdnsanswer?hostedzoneid={zone_id}\
                 &recordname=example.com&recordtype=ANY"
            ),
            body: String::new(),
            member: "recordType",
            value: "ANY",
            value_set: RR_TYPE_VALUE_SET,
        },
        RefusalCase {
            label: "the start position of a record-set listing: the SAME enum, \
                    another operation and member",
            method: "GET",
            uri: format!("/2013-04-01/hostedzone/{zone_id}/rrset?type=ANY"),
            body: String::new(),
            member: "startRecordType",
            value: "ANY",
            value_set: RR_TYPE_VALUE_SET,
        },
        RefusalCase {
            label: "the type of a record set inside a change batch, which travels \
                    in the BODY",
            method: "POST",
            uri: format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
            body: change_batch("NOTAREALTYPE"),
            member: "type",
            value: "NOTAREALTYPE",
            value_set: RR_TYPE_VALUE_SET,
        },
        // The control: another enum of this model, whose order the service
        // publishes nothing of, keeps the order the model declares it in
        // (`HostedZoneType`'s value set is exactly `[PrivateHostedZone]`).
        RefusalCase {
            label: "an enum the service publishes no order for",
            method: "GET",
            uri: "/2013-04-01/hostedzone?hostedzonetype=SharedHostedZone".to_string(),
            body: String::new(),
            member: "hostedZoneType",
            value: "SharedHostedZone",
            value_set: "[PrivateHostedZone]",
        },
    ];

    for case in &cases {
        let reply = refuse(&router, case).await;
        assert_eq!(
            reply.status, 400,
            "case {:?}: expected 400, got {} with body {}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            INVALID_INPUT,
            "case {:?}: expected {INVALID_INPUT}, got body {}",
            case.label,
            reply.body
        );
        let actual = message(&reply);
        let requirement = enum_requirement(case.value_set);
        assert!(
            actual.ends_with(&requirement),
            "case {:?}: the clause must state the accepted set as {requirement:?}, \
             got {actual:?}",
            case.label
        );
        assert!(
            actual.contains(&format!("Value '{}' at '", case.value)),
            "case {:?}: the clause must echo the value the request sent, got {actual:?}",
            case.label
        );
        assert!(
            actual.contains(&format!("at '{}' failed", case.member))
                || actual.contains(&format!(".{}' failed", case.member)),
            "case {:?}: the clause must name the member {} it was about, got {actual:?}",
            case.label,
            case.member
        );
    }

    // One whole message, to pin the frame the clause sits in together with the
    // order inside the brackets.
    let refused = refuse(&router, &cases[0]).await;
    assert_eq!(
        message(&refused),
        format!(
            "1 validation error detected: Value 'NOTAREALTYPE' at 'recordType' failed to \
             satisfy constraint: {}",
            enum_requirement(RR_TYPE_VALUE_SET)
        ),
        "the refusal must state the value, the member path and the accepted set, got {:?}",
        message(&refused)
    );
}
