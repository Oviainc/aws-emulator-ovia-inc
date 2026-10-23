// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for EC2's `ec2Query` RESPONSE ENVELOPE — the parts of a response that
//! belong to the protocol rather than to any one operation. Driven over the wire the
//! service actually speaks, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. THE ENVELOPE CARRIES THE SUCCESS FLAG `<return>true</return>` on a SUCCESSFUL
//!    response that reports no result of its own. "No result of its own" is what the model says: an
//!    operation whose output shape is `smithy.api#Unit` declares no members at all, and
//!    every EC2 operation modelled that way is a mutation (no `Describe*` or `Get*` has a
//!    Unit output). Beside those sit the operations the EC2 API Reference documents a
//!    `return` response element on even though the model declares other members instead —
//!    `CreateSecurityGroup`, whose documented response elements are `requestId`, `return`
//!    and `groupId`. Exactly ONE such element is carried, so an operation whose modelled
//!    output declares `return` itself (`DeleteSecurityGroup`) is not given a second one,
//!    and an operation that reports the resource it minted (`CreateVpc` answers `<vpc>`)
//!    or that only reports (`DescribeVpcs`) carries none.
//! 2. EVERY EC2 `ec2Query` RESPONSE DECLARES `content-type: text/xml;charset=UTF-8` —
//!    success and modelled refusal alike.
//!
//! The flag reports the success of the call, so a call that FAILED carries none: a refusal
//! answers the error envelope, whatever its operation's output shape is.
//!
//! The rows span the class rather than one recorded request: a tag write, a tag write
//! naming several resources and several tags at once, two deletes, three attribute
//! modifications (one of them a repeat that changes nothing), a create that also reports an
//! id, a mutation on a wholly different resource kind (an EBS volume), the
//! handler-owned-flag branch, a create that reports its resource, a describe, and two
//! modelled 400s — one of them on an operation whose output shape is `smithy.api#Unit`, the
//! very shape rule 1 writes the flag for.

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call_with_content_type, element, error_code};
use super::volume_lifecycle::{zone_name, REGION as VOLUME_REGION};
use super::wire::{document, param, Param};

/// The Region the VPC, subnet and security-group rows are signed for.
const REGION: &str = "us-east-1";

/// The media type every EC2 `ec2Query` response declares.
///
/// Recorded behaviour: the Smithy model declares the `ec2Query` protocol but not the media
/// type's `charset` parameter, and AWS sends the parameter on a success and on a refusal
/// alike, where the protocol's own default is the bare `text/xml`.
const RESPONSE_CONTENT_TYPE: &str = "text/xml;charset=UTF-8";

/// A well-formed VPC id in the long (17 hexadecimal digit) form that the account holds no
/// VPC for — the row that makes the service answer a modelled refusal.
const UNKNOWN_VPC_ID: &str = "vpc-0ffffffffffffffff";

/// The XML namespace every EC2 envelope is stamped with — the service shape's
/// `@xmlNamespace` URI:
///
/// ```text
/// smithy_query(query="service_info",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// Asserted on every row below, so a response envelope that lost it — or carries one that
/// drifted from the model — fails here rather than silently reaching a client.
const XML_NAMESPACE: &str = "http://ec2.amazonaws.com/doc/2016-11-15";

/// One row's response: the operation it called (which names the response's root element),
/// the HTTP status, the response's `content-type` and the XML body.
struct Answer {
    action: &'static str,
    status: u16,
    content_type: Option<String>,
    body: String,
}

/// What a row calls, and the resources it needs in place first.
#[derive(Clone, Copy, Debug)]
enum Call {
    /// `CreateTags` on a VPC: a `smithy.api#Unit` output, the operation the recordings
    /// name first.
    CreateTagsOnAVpc,
    /// `DeleteVpc` on a VPC with nothing in it: a `smithy.api#Unit` output.
    DeleteAnEmptyVpc,
    /// `DeleteSubnet` on an empty subnet: a `smithy.api#Unit` output.
    DeleteAnEmptySubnet,
    /// `ModifySubnetAttribute` setting one attribute: a `smithy.api#Unit` output.
    ModifyASubnetAttribute,
    /// `ModifyVpcAttribute` setting one attribute: a `smithy.api#Unit` output.
    ModifyAVpcAttribute,
    /// `CreateTags` naming TWO VPCs and THREE tags in ONE call: the flag is the
    /// envelope's, so however many resources and tags a mutation touches, it answers
    /// exactly one.
    CreateTagsOnTwoVpcsWithThreeTags,
    /// `ModifyVpcAttribute` setting an attribute to the value it already holds: a repeat
    /// that changes nothing is still a successful mutation and still answers the flag.
    ModifyAVpcAttributeTwice,
    /// `CreateSecurityGroup`: a modelled output (`groupId`) that the documented `return`
    /// element sits beside.
    CreateASecurityGroup,
    /// `EnableVolumeIO` on a volume whose I/O has been found inconsistent: a
    /// `smithy.api#Unit` output on a resource kind no recording in this issue cites.
    EnableIoOnAVolume,
    /// `DeleteSecurityGroup`: the branch where the model declares `return` itself, so the
    /// handler owns the value and the envelope must add nothing.
    DeleteASecurityGroup,
    /// `CreateVpc`: a mutation that reports the resource it minted and no flag.
    CreateAVpc,
    /// `DescribeVpcs`: a reporting operation, which carries no flag.
    DescribeTheVpcs,
    /// `DescribeVpcs` for a VPC the account does not hold: a modelled refusal.
    DescribeAVpcTheAccountDoesNotHold,
    /// `DeleteVpc` for a VPC the account does not hold: a modelled refusal from an
    /// operation whose modelled output is `smithy.api#Unit`, so the flag must not be
    /// written for a call that did not succeed.
    DeleteAVpcTheAccountDoesNotHold,
}

/// One `action` call signed for `region`, as an [`Answer`].
async fn answer(
    router: &MultiServiceRouter,
    region: &str,
    action: &'static str,
    params: &[Param],
) -> Answer {
    let (status, content_type, body) =
        call_with_content_type(router, region, action, params).await;
    Answer {
        action,
        status,
        content_type,
        body,
    }
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    action: &'static str,
    params: &[Param],
) -> String {
    let answered = answer(router, region, action, params).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from {action} while setting the row up, got {} \
         with body {}",
        answered.status, answered.body
    );
    answered.body
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(
        case,
        router,
        REGION,
        "CreateVpc",
        &[param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        REGION,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    element(&body, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// A security group named `group_name` in `vpc_id`, as its id.
async fn create_security_group(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    group_name: &str,
) -> String {
    let body = ok(
        case,
        router,
        REGION,
        "CreateSecurityGroup",
        &security_group_params(vpc_id, group_name),
    )
    .await;
    element(&body, "groupId").unwrap_or_else(|| panic!("case {case:?}: no <groupId> in {body}"))
}

/// The form fields of a `CreateSecurityGroup` in `vpc_id`.
fn security_group_params(vpc_id: &str, group_name: &str) -> Vec<Param> {
    vec![
        param("VpcId", vpc_id),
        param("GroupName", group_name),
        param("GroupDescription", "the group an envelope row creates"),
    ]
}

/// One 8 GiB volume in the volume Region's first Availability Zone, as its id.
async fn create_volume(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(
        case,
        router,
        VOLUME_REGION,
        "CreateVolume",
        &[
            param("AvailabilityZone", &zone_name(0)),
            param("Size", "8"),
        ],
    )
    .await;
    element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: no <volumeId> in {body}"))
}

/// Run one row against a freshly built emulator and answer what the service replied.
async fn perform(case: &str, call: Call) -> Answer {
    let (router, engine) = super::ec2_query::emulator();
    match call {
        Call::CreateTagsOnAVpc => {
            let vpc = create_vpc(case, &router, "10.0.0.0/16").await;
            answer(
                &router,
                REGION,
                "CreateTags",
                &[
                    param("ResourceId.1", &vpc),
                    param("Tag.1.Key", "Name"),
                    param("Tag.1.Value", "envelope"),
                ],
            )
            .await
        }
        Call::DeleteAnEmptyVpc => {
            let vpc = create_vpc(case, &router, "10.1.0.0/16").await;
            answer(&router, REGION, "DeleteVpc", &[param("VpcId", &vpc)]).await
        }
        Call::DeleteAnEmptySubnet => {
            let vpc = create_vpc(case, &router, "10.2.0.0/16").await;
            let subnet = create_subnet(case, &router, &vpc, "10.2.1.0/24").await;
            answer(
                &router,
                REGION,
                "DeleteSubnet",
                &[param("SubnetId", &subnet)],
            )
            .await
        }
        Call::ModifyASubnetAttribute => {
            let vpc = create_vpc(case, &router, "10.3.0.0/16").await;
            let subnet = create_subnet(case, &router, &vpc, "10.3.1.0/24").await;
            answer(
                &router,
                REGION,
                "ModifySubnetAttribute",
                &[
                    param("SubnetId", &subnet),
                    param("MapPublicIpOnLaunch.Value", "true"),
                ],
            )
            .await
        }
        Call::ModifyAVpcAttribute => {
            let vpc = create_vpc(case, &router, "10.4.0.0/16").await;
            answer(
                &router,
                REGION,
                "ModifyVpcAttribute",
                &[
                    param("VpcId", &vpc),
                    param("EnableDnsHostnames.Value", "true"),
                ],
            )
            .await
        }
        Call::CreateTagsOnTwoVpcsWithThreeTags => {
            let first = create_vpc(case, &router, "10.9.0.0/16").await;
            let second = create_vpc(case, &router, "10.10.0.0/16").await;
            answer(
                &router,
                REGION,
                "CreateTags",
                &[
                    param("ResourceId.1", &first),
                    param("ResourceId.2", &second),
                    param("Tag.1.Key", "Name"),
                    param("Tag.1.Value", "envelope"),
                    param("Tag.2.Key", "owner"),
                    param("Tag.2.Value", "the envelope suite"),
                    // Documented on `CreateTags`' `Tags`: a tag given with no value is
                    // stored with an empty one.
                    param("Tag.3.Key", "valueless"),
                    param("Tag.3.Value", ""),
                ],
            )
            .await
        }
        Call::ModifyAVpcAttributeTwice => {
            let vpc = create_vpc(case, &router, "10.11.0.0/16").await;
            let params = [
                param("VpcId", &vpc),
                param("EnableNetworkAddressUsageMetrics.Value", "true"),
            ];
            let _ = ok(case, &router, REGION, "ModifyVpcAttribute", &params).await;
            answer(&router, REGION, "ModifyVpcAttribute", &params).await
        }
        Call::CreateASecurityGroup => {
            let vpc = create_vpc(case, &router, "10.5.0.0/16").await;
            answer(
                &router,
                REGION,
                "CreateSecurityGroup",
                &security_group_params(&vpc, "envelope-created-group"),
            )
            .await
        }
        Call::DeleteASecurityGroup => {
            let vpc = create_vpc(case, &router, "10.6.0.0/16").await;
            let group = create_security_group(case, &router, &vpc, "envelope-deleted-group").await;
            answer(
                &router,
                REGION,
                "DeleteSecurityGroup",
                &[param("GroupId", &group)],
            )
            .await
        }
        Call::EnableIoOnAVolume => {
            let volume = create_volume(case, &router).await;
            // The one case the operation documents: Amazon EBS found the volume's data
            // potentially inconsistent and disabled its I/O. Nothing over the wire puts a
            // volume in that state, so the row seeds it the way the volume-I/O suite does.
            {
                let store = engine.state::<Ec2State>();
                let mut guard = store.lock();
                guard
                    .volumes
                    .get_mut(&volume)
                    .unwrap_or_else(|| panic!("case {case:?}: the volume just created"))
                    .io_enabled = false;
            }
            answer(
                &router,
                VOLUME_REGION,
                "EnableVolumeIO",
                &[param("VolumeId", &volume)],
            )
            .await
        }
        Call::CreateAVpc => {
            answer(
                &router,
                REGION,
                "CreateVpc",
                &[param("CidrBlock", "10.7.0.0/16")],
            )
            .await
        }
        Call::DescribeTheVpcs => {
            let _ = create_vpc(case, &router, "10.8.0.0/16").await;
            answer(&router, REGION, "DescribeVpcs", &[]).await
        }
        Call::DescribeAVpcTheAccountDoesNotHold => {
            answer(
                &router,
                REGION,
                "DescribeVpcs",
                &[param("VpcId.1", UNKNOWN_VPC_ID)],
            )
            .await
        }
        Call::DeleteAVpcTheAccountDoesNotHold => {
            answer(
                &router,
                REGION,
                "DeleteVpc",
                &[param("VpcId", UNKNOWN_VPC_ID)],
            )
            .await
        }
    }
}

/// The text of every DIRECT child of the response envelope named `name`, in document
/// order, having first pinned that the envelope's root is `<{action}Response>` in the
/// model's XML namespace.
///
/// Direct children only: a `<return>` nested inside a reported resource would be that
/// resource's member, not the envelope's flag.
fn envelope_children(case: &str, answered: &Answer, name: &str) -> Vec<String> {
    let parsed = document(case, &answered.body);
    let root = parsed.root_element();
    let expected = format!("{}Response", answered.action);
    assert_eq!(
        root.tag_name().name(),
        expected,
        "case {case:?}: expected a <{expected}> root, got {}",
        answered.body
    );
    assert_eq!(
        root.tag_name().namespace(),
        Some(XML_NAMESPACE),
        "case {case:?}: expected the <{expected}> root in the namespace {XML_NAMESPACE:?}, \
         got {:?} in {}",
        root.tag_name().namespace(),
        answered.body
    );
    root.children()
        .filter(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// Every row whose response carries the envelope's success flag, and the modelled elements
/// the flag must not displace.
///
/// The first six are the operations whose recordings are missing the flag. `EnableVolumeIO`
/// and `DeleteSecurityGroup` are two the recordings do not cite: the first another
/// `smithy.api#Unit` output, on a resource kind nothing above touches; the second the
/// branch where the model declares `return` itself.
const FLAG_ROWS: [(&str, Call, &[&str]); 10] = [
    ("CreateTags on a VPC", Call::CreateTagsOnAVpc, &[]),
    ("DeleteVpc on an empty VPC", Call::DeleteAnEmptyVpc, &[]),
    (
        "DeleteSubnet on an empty subnet",
        Call::DeleteAnEmptySubnet,
        &[],
    ),
    (
        "ModifySubnetAttribute MapPublicIpOnLaunch=true",
        Call::ModifyASubnetAttribute,
        &[],
    ),
    (
        "ModifyVpcAttribute EnableDnsHostnames=true",
        Call::ModifyAVpcAttribute,
        &[],
    ),
    (
        "CreateTags naming two VPCs and three tags at once",
        Call::CreateTagsOnTwoVpcsWithThreeTags,
        &[],
    ),
    (
        "ModifyVpcAttribute EnableNetworkAddressUsageMetrics=true, repeated",
        Call::ModifyAVpcAttributeTwice,
        &[],
    ),
    (
        "CreateSecurityGroup, whose documented response carries groupId beside the flag",
        Call::CreateASecurityGroup,
        &["groupId"],
    ),
    (
        "EnableVolumeIO on a volume whose I/O is disabled",
        Call::EnableIoOnAVolume,
        &[],
    ),
    (
        "DeleteSecurityGroup, whose own output models the flag",
        Call::DeleteASecurityGroup,
        &["groupId"],
    ),
];

/// Every row whose response carries NO flag, and the result element it reports instead.
const NO_FLAG_ROWS: [(&str, Call, &str); 2] = [
    (
        "CreateVpc, which reports the VPC it minted",
        Call::CreateAVpc,
        "vpcId",
    ),
    (
        "DescribeVpcs, which only reports",
        Call::DescribeTheVpcs,
        "vpcSet",
    ),
];

/// Every row that is REFUSED, the HTTP status it is refused with and the `<Code>` of the
/// refusal. The second is an operation whose modelled output is `smithy.api#Unit` — the
/// shape rule 1 writes the flag for — so it pins that the flag follows the call's SUCCESS
/// and not the output shape.
const REFUSED_ROWS: [(&str, Call, u16, &str); 2] = [
    (
        "DescribeVpcs for a VPC the account does not hold",
        Call::DescribeAVpcTheAccountDoesNotHold,
        400,
        "InvalidVpcID.NotFound",
    ),
    (
        "DeleteVpc for a VPC the account does not hold",
        Call::DeleteAVpcTheAccountDoesNotHold,
        400,
        "InvalidVpcID.NotFound",
    ),
];

// ---------------------------------------------------------------------------
// RULE 1: the envelope's success flag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_response_with_no_result_of_its_own_carries_exactly_one_return_flag() {
    for (case, call, modelled) in FLAG_ROWS {
        let answered = perform(case, call).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from {}, got {} with body {}",
            answered.action, answered.status, answered.body
        );
        let flags = envelope_children(case, &answered, "return");
        assert_eq!(
            flags,
            vec!["true".to_string()],
            "case {case:?}: expected exactly one <return>true</return> in the {} envelope, \
             got {flags:?} in {}",
            answered.action,
            answered.body
        );
        // The flag is added to the envelope, not in place of it.
        let request_ids = envelope_children(case, &answered, "requestId");
        assert_eq!(
            request_ids.len(),
            1,
            "case {case:?}: the {} envelope must still carry one <requestId>, got \
             {request_ids:?} in {}",
            answered.action,
            answered.body
        );
        for name in modelled {
            assert!(
                element(&answered.body, name).is_some(),
                "case {case:?}: the flag must not displace the modelled <{name}>, got {}",
                answered.body
            );
        }
    }
}

#[tokio::test]
async fn a_response_that_reports_a_result_carries_no_return_flag() {
    for (case, call, reported) in NO_FLAG_ROWS {
        let answered = perform(case, call).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from {}, got {} with body {}",
            answered.action, answered.status, answered.body
        );
        let flags = envelope_children(case, &answered, "return");
        assert!(
            flags.is_empty(),
            "case {case:?}: the {} envelope reports <{reported}> and must carry no \
             <return>, got {flags:?} in {}",
            answered.action,
            answered.body
        );
        assert!(
            element(&answered.body, reported).is_some(),
            "case {case:?}: expected the reported <{reported}> in {}",
            answered.body
        );
    }
}

#[tokio::test]
async fn a_refused_call_carries_no_return_flag() {
    for (case, call, expected_status, expected_code) in REFUSED_ROWS {
        let answered = perform(case, call).await;
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from {}, got {} with body {}",
            answered.action, answered.status, answered.body
        );
        assert_eq!(
            error_code(&answered.body),
            expected_code,
            "case {case:?}: expected the {expected_code} refusal from {}, got {}",
            answered.action,
            answered.body
        );
        // Anywhere in the body, not just under the root: the flag reports a success, and
        // this call had none to report.
        let parsed = document(case, &answered.body);
        let flags: Vec<String> = parsed
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "return")
            .map(|node| node.text().unwrap_or_default().to_string())
            .collect();
        assert!(
            flags.is_empty(),
            "case {case:?}: a refused {} must carry no <return>, got {flags:?} in {}",
            answered.action,
            answered.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the media type every response declares
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_response_declares_the_xml_media_type_with_its_charset() {
    // Every row of all three tables above — each 200 of the first two, and each modelled
    // refusal, which declares the same media type.
    let mut rows: Vec<(&str, Call, u16)> = FLAG_ROWS
        .iter()
        .map(|(case, call, _)| (*case, *call, 200u16))
        .collect();
    rows.extend(
        NO_FLAG_ROWS
            .iter()
            .map(|(case, call, _)| (*case, *call, 200u16)),
    );
    rows.extend(
        REFUSED_ROWS
            .iter()
            .map(|(case, call, status, _)| (*case, *call, *status)),
    );

    for (case, call, expected_status) in rows {
        let answered = perform(case, call).await;
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from {}, got {} with body {}",
            answered.action, answered.status, answered.body
        );
        assert_eq!(
            answered.content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected content-type {RESPONSE_CONTENT_TYPE:?} from {}, got \
             {:?} with body {}",
            answered.action,
            answered.content_type,
            answered.body
        );
    }
}
