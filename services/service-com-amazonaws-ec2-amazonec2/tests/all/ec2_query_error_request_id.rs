// SPDX-License-Identifier: CC0-1.0

//! The ONE envelope fact about `CreateVpnConnectionRoute` that the shared suites do not
//! cover: its SUCCESS side.
//!
//! [`super::ec2_query_error_envelope`] owns the rule — a refusal reports the answered
//! request in one `<RequestID>`, a success in the lowercase `<requestId>` — and this
//! operation's three refusals are rows in its `refusal_rows` table. Its success rows are
//! other operations', and `super::ec2_query_envelope`'s flag rows likewise, so what is
//! left is this operation's 200: its modelled output is `smithy.api#Unit`, which makes the
//! body the envelope and nothing else — one `<return>true</return>` and one lowercase
//! `<requestId>`, and none of the refusal's capitalised element. The reader, the root name
//! and the element names come from that suite rather than being declared again here.

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::VPN_STATE_AVAILABLE;

use super::ec2_query::call;
use super::ec2_query_error_envelope::{ERROR_REQUEST_ID, SUCCESS_REQUEST_ID};
use super::vpn_connection_routes::seed;
use super::wire::param;

/// The Region the call is signed for.
const REGION: &str = "us-east-1";

/// The root element of this operation's success envelope, which ec2Query names after the
/// operation.
const SUCCESS_ENVELOPE_ROOT: &str = "CreateVpnConnectionRouteResponse";

#[tokio::test]
async fn a_successful_static_route_create_reports_the_request_in_the_success_element() {
    let (router, engine) = super::ec2_query::emulator();
    let vpn_connection_id = seed(&engine, VPN_STATE_AVAILABLE);
    let case = "a static route created on a connection the account holds";

    let params = vec![
        param("VpnConnectionId", &vpn_connection_id),
        param("DestinationCidrBlock", "10.7.0.0/16"),
    ];
    let (status, body) = call(&router, REGION, "CreateVpnConnectionRoute", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    let reported = super::ec2_query_error_envelope::root_children(
        case,
        &body,
        SUCCESS_ENVELOPE_ROOT,
        SUCCESS_REQUEST_ID,
    );
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one <{SUCCESS_REQUEST_ID}> in the success \
         envelope, got {reported:?} in {body}"
    );
    assert!(
        !reported[0].trim().is_empty(),
        "case {case:?}: expected <{SUCCESS_REQUEST_ID}> to name the answered request, got \
         {reported:?} in {body}"
    );
    assert!(
        super::ec2_query_error_envelope::root_children(
            case,
            &body,
            SUCCESS_ENVELOPE_ROOT,
            ERROR_REQUEST_ID,
        )
        .is_empty(),
        "case {case:?}: a success reports the request under <{SUCCESS_REQUEST_ID}> only, \
         got {body}"
    );
    // The modelled output is `smithy.api#Unit`, so the flag is the rest of the body.
    let flags =
        super::ec2_query_error_envelope::root_children(case, &body, SUCCESS_ENVELOPE_ROOT, "return");
    assert_eq!(
        flags,
        vec!["true".to_string()],
        "case {case:?}: expected exactly one <return>true</return>, got {flags:?} in {body}"
    );
}
