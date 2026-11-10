// SPDX-License-Identifier: CC0-1.0

//! An EMPTY LIST member on the query wire, over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! The rule this PINS: in the AWS query protocol a list member carrying NO elements is
//! rendered as the member's own key with an EMPTY value and no index (`ExecutableBy=`,
//! `Filter=`), so such a key names a list of ZERO elements — never a one-element list whose
//! element is the empty string, and never a one-element list of an empty structure. A
//! request that carries one is therefore answered exactly as one that omits the member.
//!
//! Already how this service reads such a key; this suite holds it there, because a reading
//! that took `Filter=` for one nameless filter would turn every such request into a refusal.
//! The rows span what the rule generalises over: which operation, which member, and what the
//! list's element is — a string list of ids, a string list of owner/executable-by terms, a
//! list of filter structures, a list of tag-specification structures and a list of health
//! check path structures — because the rendering is the PROTOCOL's and nothing about it
//! belongs to one operation or one element type.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call_raw, API_VERSION};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One case: the operation, the parameters a request carries beside `Action`/`Version`, and
/// the element the response envelope must report.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// The `Action` the request names.
    action: &'static str,
    /// The form fields, already rendered the way a query client renders them.
    params: &'static str,
}

/// Every empty-list rendering this suite drives, one per list member and element type.
///
/// The member keys are the `@xmlName` the model binds each list to — `ExecutableBy` for
/// `DescribeImages$ExecutableUsers`, `Owner` for `$Owners`, `ImageId` for `$ImageIds`,
/// `Filter` for `$Filters`, `HealthCheckPath` for
/// `CreateApplicationStatusCheck$HealthCheckPaths` and `TagSpecification` for
/// `$TagSpecifications` (`smithy_query(query="operation",operation="DescribeImages")`).
const CASES: &[Case] = &[
    // A string list of executable-by terms, alone and beside a selection that must still be
    // read.
    Case {
        action: "DescribeImages",
        params: "ExecutableBy=",
    },
    Case {
        action: "DescribeImages",
        params: "Owner.1=amazon&ExecutableBy=&Filter.1.Name=state&Filter.1.Value.1=available",
    },
    // A string list of owner terms.
    Case {
        action: "DescribeImages",
        params: "Owner=",
    },
    // A string list of resource ids.
    Case {
        action: "DescribeImages",
        params: "ImageId=",
    },
    // A list of filter STRUCTURES.
    Case {
        action: "DescribeImages",
        params: "Filter=",
    },
    Case {
        action: "DescribeImages",
        params: "Owner=&ExecutableBy=&ImageId=&Filter=",
    },
    // The same rendering on the other reporting operations whose list members a client
    // leaves empty.
    Case {
        action: "DescribeClientVpnEndpoints",
        params: "ClientVpnEndpointId=&Filter=",
    },
    Case {
        action: "DescribeCapacityReservationBillingRequests",
        params: "Role=odcr-owner&CapacityReservationId=&Filter=",
    },
    Case {
        action: "DescribeInstances",
        params: "InstanceId=&Filter=",
    },
    Case {
        action: "DescribeVolumes",
        params: "VolumeId=&Filter=",
    },
    Case {
        action: "DescribeSnapshots",
        params: "SnapshotId=&OwnerId=&RestorableBy=&Filter=",
    },
    // A list of TAG-SPECIFICATION structures, and a list of HEALTH CHECK PATH structures,
    // on a mutation that creates a resource from them.
    Case {
        action: "CreateApplicationStatusCheck",
        params: "Protocol=http&Port=80&Path=%2Fhealth&HealthCheckPath=&TagSpecification=",
    },
];

/// One case driven through the router, as its HTTP status and body.
async fn call_case(router: &MultiServiceRouter, case: &Case) -> (u16, String) {
    let body = format!(
        "Action={}&Version={API_VERSION}&{}",
        case.action, case.params
    );
    let (status, _content_type, response) = call_raw(router, REGION, &[], &body).await;
    (status, response)
}

/// An empty-list key is read as an empty list, so every one of these requests is SERVED.
#[tokio::test]
async fn an_empty_list_member_is_read_as_an_empty_list() {
    let (router, _engine) = super::ec2_query::emulator();

    for case in CASES {
        let (status, body) = call_case(&router, case).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an empty list member must be read as an empty list, so the \
             request must be served, got HTTP {status} with body {body}"
        );
        let expected_root = format!("<{}Response", case.action);
        assert!(
            body.contains(&expected_root),
            "case {case:?}: expected a {expected_root}> envelope, got {body}"
        );
    }
}

/// An empty-list key narrows NOTHING: the answer is the one the same request without the
/// member gets, element for element.
#[tokio::test]
async fn an_empty_list_member_narrows_nothing() {
    let (router, _engine) = super::ec2_query::emulator();

    let rows: &[(&str, &str, &str)] = &[
        (
            "DescribeImages",
            "Owner.1=amazon",
            "Owner.1=amazon&ExecutableBy=&ImageId=&Filter=",
        ),
        (
            "DescribeClientVpnEndpoints",
            "",
            "ClientVpnEndpointId=&Filter=",
        ),
        (
            "DescribeCapacityReservationBillingRequests",
            "Role=odcr-owner",
            "Role=odcr-owner&CapacityReservationId=&Filter=",
        ),
    ];

    for (action, without, with_empty) in rows {
        let case = Case {
            action,
            params: without,
        };
        let (plain_status, plain_body) = call_case(&router, &case).await;
        let empty = Case {
            action,
            params: with_empty,
        };
        let (empty_status, empty_body) = call_case(&router, &empty).await;
        assert_eq!(
            (plain_status, empty_status),
            (200, 200),
            "case {action:?}: both renderings must be served, got {plain_status} / \
             {empty_status} with bodies {plain_body} / {empty_body}"
        );
        // The envelope's own request id is per response, so the comparison is over the
        // reported set rather than the whole body.
        let plain_items = super::ec2_query::elements(&plain_body, "item").len();
        let empty_items = super::ec2_query::elements(&empty_body, "item").len();
        assert_eq!(
            plain_items, empty_items,
            "case {action:?}: an empty list member must narrow nothing, so both answers must \
             report the same rows, got {plain_items} and {empty_items}: {plain_body} / \
             {empty_body}"
        );
    }
}

/// A list member that carries an element is still read as that element: the rule above is
/// about an EMPTY value, so it cannot make a populated list read as empty.
#[tokio::test]
async fn a_populated_list_member_is_still_read() {
    let (router, _engine) = super::ec2_query::emulator();

    // An owner term that is no account id and no alias is still refused, which is what
    // proves the term was read rather than dropped.
    let case = Case {
        action: "DescribeImages",
        params: "Owner.1=not-an-owner",
    };
    let (status, body) = call_case(&router, &case).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a malformed owner term must still be refused, got HTTP {status} \
         with body {body}"
    );
    assert!(
        body.contains("InvalidUserID.Malformed"),
        "case {case:?}: expected the InvalidUserID.Malformed code, got {body}"
    );

    // And a filter name the operation does not document is still refused.
    let filtered = Case {
        action: "DescribeImages",
        params: "Filter.1.Name=not-a-filter&Filter.1.Value.1=x",
    };
    let (filter_status, filter_body) = call_case(&router, &filtered).await;
    assert_eq!(
        filter_status, 400,
        "case {filtered:?}: an undocumented filter must still be refused, got HTTP \
         {filter_status} with body {filter_body}"
    );
}
