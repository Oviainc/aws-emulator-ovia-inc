// SPDX-License-Identifier: CC0-1.0

//! The field vocabulary a `CreateFlowLogs` `LogFormat` draws from, driven over the wire the
//! service speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! ONE rule is under test: a custom record format is a layout of PUBLISHED flow log record
//! fields — "For more information about the available fields, see Flow log records in the
//! Amazon VPC User Guide or Transit Gateway Flow Log records in the Amazon Web Services
//! Transit Gateway Guide" and "Specify the fields using the `${field-id}` format, separated
//! by spaces" (`CreateFlowLogsRequest$LogFormat`) — so a create whose `LogFormat` carries a
//! `${…}` token that names no such field is refused HTTP 400 `InvalidParameter`, naming
//! every unrecognised token, and stores NOTHING; a create whose every token names a
//! published field is served and reports the format back verbatim.
//!
//! The rows differ in which published fields the format names and how many (one, several,
//! fields from each published table, a tag of the monitored resource), and — on the refusal
//! side — in how many tokens are unrecognised, where in the format they sit, whether a
//! token repeats, and how the unrecognised token differs from a real field (a misspelling,
//! a different case, a name of no field at all).

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::flow_logs::cloud_watch_logs_params;

/// The Region every call here is signed for — the flow log store holds the account's flow
/// logs and the seeded default VPC is the one of the emulator's default Region.
const REGION: &str = "us-east-1";

/// Flow log record fields this suite builds accepted formats out of, written out here
/// rather than read from the crate so a catalog that lost or renamed an entry still fails:
/// each is a published field id (Amazon VPC User Guide, "Flow log records" — available
/// fields), and NONE of them is the one field id the default format's first rows are usually
/// shown with.
const PUBLISHED_VPC_FIELDS: [&str; 8] = [
    "log-status",
    "vpc-id",
    "subnet-id",
    "tcp-flags",
    "flow-direction",
    "traffic-path",
    "ecs-cluster-arn",
    "reject-reason",
];

/// Published field ids only a transit gateway record carries (Amazon Web Services Transit
/// Gateway Guide, "Flow log records") — the other published table the member's documentation
/// points at.
const PUBLISHED_TRANSIT_GATEWAY_FIELDS: [&str; 4] = [
    "tgw-id",
    "tgw-attachment-id",
    "tgw-pair-details",
    "packets-lost-no-route",
];

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A VPC created over the wire, by id — so a create can name a VPC the account really has.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// One `CreateFlowLogs` on `vpc_id` carrying `log_format`, as its HTTP status and XML body.
async fn create_with_format(
    router: &MultiServiceRouter,
    vpc_id: &str,
    log_format: &str,
) -> (u16, String) {
    let mut params = cloud_watch_logs_params("VPC", vpc_id, "/aws/vpc/flow-logs");
    params.push(param("LogFormat", log_format));
    super::ec2_query::call(router, REGION, "CreateFlowLogs", &params).await
}

/// The flow log ids the account holds for `vpc_id`, in the order `DescribeFlowLogs` reports
/// them.
async fn flow_log_ids(router: &MultiServiceRouter, case: &str, vpc_id: &str) -> Vec<String> {
    let params = filters(&[("resource-id", &[vpc_id])]);
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeFlowLogs", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeFlowLogs, got {status} with body {body}"
    );
    elements(&body, "flowLogId")
}

/// The `logFormat` the one flow log `flow_log_id` names reports.
async fn reported_format(router: &MultiServiceRouter, case: &str, flow_log_id: &str) -> String {
    let params = indexed("FlowLogId", &[flow_log_id]);
    let (status, body) = super::ec2_query::call(router, REGION, "DescribeFlowLogs", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeFlowLogs, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "logFormat")
        .unwrap_or_else(|| panic!("case {case:?}: no logFormat in {body}"))
}

/// A `LogFormat` naming only published fields is SERVED, and the flow log it stores reports
/// that format back verbatim. The rows differ in how many fields the format names and which
/// published table they come from — one field, several, fields of a transit gateway record,
/// a field that reads a tag of the monitored resource, and every field this suite knows at
/// once.
#[tokio::test]
async fn a_log_format_of_published_fields_is_served_and_read_back_verbatim() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.231.0.0/16").await;

    let one_field = format!("${{{}}}", PUBLISHED_VPC_FIELDS[0]);
    let token = |field: &str| format!("${{{field}}}");
    let join = |fields: &[&str]| {
        fields
            .iter()
            .map(|field| token(field))
            .collect::<Vec<String>>()
            .join(" ")
    };
    let every_field: Vec<&str> = PUBLISHED_VPC_FIELDS
        .iter()
        .chain(PUBLISHED_TRANSIT_GATEWAY_FIELDS.iter())
        .copied()
        .collect();

    // Each row: the format the create asks for.
    let cases: Vec<(&str, String)> = vec![
        ("one published field", one_field),
        (
            "three fields of a VPC record",
            join(&PUBLISHED_VPC_FIELDS[1..4]),
        ),
        (
            "the fields of a transit gateway record",
            join(&PUBLISHED_TRANSIT_GATEWAY_FIELDS),
        ),
        (
            "a published field beside a tag of the monitored resource",
            format!("{} ${{resource-tag:Name}}", token("srcaddr")),
        ),
        ("every field this suite knows", join(&every_field)),
    ];

    for (case, log_format) in &cases {
        let (status, body) = create_with_format(&router, &vpc_id, log_format).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for the format {log_format:?}, got {status} \
             with body {body}"
        );
        let created = elements(&body, "item");
        assert_eq!(
            created.len(),
            1,
            "case {case:?}: expected exactly one flow log id, got {created:?} in {body}"
        );
        let reported = reported_format(&router, case, &created[0]).await;
        assert_eq!(
            &reported, log_format,
            "case {case:?}: expected the format {log_format:?} read back, got {reported:?}"
        );
    }

    // Every served row stored its flow log, and nothing else did.
    let stored = flow_log_ids(&router, "every served format", &vpc_id).await;
    assert_eq!(
        stored.len(),
        cases.len(),
        "every served format: expected {expected} flow logs, got {stored:?}",
        expected = cases.len()
    );
}

/// A `LogFormat` carrying a `${…}` token that names no published field is REFUSED — HTTP
/// 400 `InvalidParameter`, with every unrecognised token named and quoted in the order the
/// format named them — and the monitored resource is left with exactly the flow logs it
/// already carried. The rows differ in how many tokens are unrecognised, where they sit,
/// whether one repeats, and how an unrecognised token differs from a real field.
#[tokio::test]
async fn a_log_format_naming_a_field_that_does_not_exist_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.232.0.0/16").await;

    // One flow log the account already holds, so a refusal that created something would
    // show up as a second one.
    let (status, body) = create_with_format(&router, &vpc_id, "${srcaddr} ${dstaddr}").await;
    assert_eq!(
        status, 200,
        "seeding one flow log: expected HTTP 200, got {status} with body {body}"
    );
    let before = flow_log_ids(&router, "before the refusals", &vpc_id).await;
    assert_eq!(
        before.len(),
        1,
        "before the refusals: expected the one seeded flow log, got {before:?}"
    );

    // Each row: the format the create asks for, and the tokens the refusal must name.
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a name of no field at all",
            "${not-a-field}",
            vec!["not-a-field"],
        ),
        (
            "an unrecognised token after a published one",
            "${srcaddr} ${not-a-field}",
            vec!["not-a-field"],
        ),
        (
            "an unrecognised token before a published one",
            "${not-a-field} ${srcaddr}",
            vec!["not-a-field"],
        ),
        (
            "a misspelled field id",
            "${srcadr} ${dstaddr}",
            vec!["srcadr"],
        ),
        (
            "a published field id in another case",
            "${SrcAddr}",
            vec!["SrcAddr"],
        ),
        (
            "a field of another service's record",
            "${http-method}",
            vec!["http-method"],
        ),
        (
            "two unrecognised tokens among published ones",
            "${version} ${alpha} ${srcaddr} ${bravo}",
            vec!["alpha", "bravo"],
        ),
        (
            "one unrecognised token, named twice",
            "${alpha} ${srcaddr} ${alpha}",
            vec!["alpha"],
        ),
    ];

    for (case, log_format, unknown) in &cases {
        let (status, body) = create_with_format(&router, &vpc_id, log_format).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for the format {log_format:?}, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameter",
            "case {case:?}: expected InvalidParameter, got {body}"
        );
        let expected = format!(
            "Unknown fields provided: {fields}",
            fields = unknown
                .iter()
                .map(|field| format!("'{field}'"))
                .collect::<Vec<String>>()
                .join(", "),
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
        // Nothing was created: the resource still carries only the seeded flow log.
        let after = flow_log_ids(&router, case, &vpc_id).await;
        assert_eq!(
            &after, &before,
            "case {case:?}: a refused create must store nothing, but the resource now \
             carries {after:?}"
        );
    }

    assert_eq!(
        engine
            .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
            .lock()
            .flow_logs
            .len(),
        1,
        "a refused create must leave the store exactly as it was"
    );
}
