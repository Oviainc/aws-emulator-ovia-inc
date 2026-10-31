// SPDX-License-Identifier: CC0-1.0

//! The MEDIA TYPE every EC2 `ec2Query` response declares, extended across the RESPONSE
//! LAYERS and the operations `super::ec2_query_envelope` does not reach. Driven over the
//! wire the service actually speaks through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The RULE under test: EVERY RESPONSE THIS SERVICE ANSWERS DECLARES
//! `content-type: text/xml;charset=UTF-8` — the media type the protocol names together with
//! the `charset` the bytes are written in. The model declares the `ec2Query` protocol but
//! not the parameter, so the parameter is recorded behaviour: a client or a proxy that
//! decodes the body by the declared charset is told the encoding instead of guessing it.
//! The media type itself is named from that sibling suite ([`RESPONSE_CONTENT_TYPE`]), so
//! the two cannot drift.
//!
//! The sibling suite owns the rule over the operations whose rows it scripts, through a
//! `Call` enum whose every variant carries the resources that row needs created first. The
//! rows here are plain `(action, params)` pairs instead, which is what lets them span the
//! two dimensions that suite's rows do not:
//!
//! * WHICH LAYER rendered the response — a handler's success, a handler's own refusal, and
//!   the REFUSAL TAIL OF THE REQUEST WALK, which refuses a value outside a constraint the
//!   model declares (or a member the request left out) before any handler runs and renders
//!   that refusal itself. That last one is the response no serializer of this service ever
//!   sees, and one of its refusals carries this service's own `InvalidMaxResults` code — so
//!   a refused page size is as much an EC2 response as a described image is, and declares
//!   the same media type;
//! * WHICH OPERATION answered — nine reporting operations that need no input, two that need
//!   input of their own, three mutations (an account setting, a delete, a terminate) and the
//!   reads that confirm what those mutations did, beside the ten operations the recorded
//!   corpus reports this media type for whose clusters showed nothing else
//!   ([`corpus_rows`]);
//! * WHAT THE DECLARED CHARSET MEANS — the parameter is a statement about the BYTES, so a
//!   response reporting text outside US-ASCII carries it back in the encoding it declares:
//!   a tag key, a tag value and an error message holding non-ASCII text each read back as
//!   the caller wrote them, which is what a client decoding by the declared charset gets
//!   and what a client guessing a default would not ([`NON_ASCII_TEXT`]).
//!
//! Each refusal row also pins that the error envelope names the request it answers exactly
//! ONCE, whichever layer rendered it: the envelope is finished in one place, so a response
//! that passes that place cannot collect a second `<RequestID>`.

use service_com_amazonaws_ec2_amazonec2::state::availability_zones_in_region;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call_with_content_type, element, error_code, error_message};
// The rule's media type and the id that resolves to nothing, from the suite that owns the
// rule over the operations it scripts.
use super::ec2_query_envelope::{RESPONSE_CONTENT_TYPE, UNKNOWN_VPC_ID};
use super::ec2_query_error_envelope::{root_children, ERROR_ENVELOPE_ROOT, ERROR_REQUEST_ID};
// One request parameter, from the one shared definition every suite of this crate uses.
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The AMI the instance rows launch from — which image it is does not matter to a media
/// type, so one is used throughout.
const AMI: &str = "ami-0123456789abcdef0";

/// Three instance types of the modelled `InstanceType` catalog, enough for a spot placement
/// score request to be scored rather than refused.
const INSTANCE_TYPES: [&str; 3] = ["c5.large", "m5.large", "m5.xlarge"];

/// One answered call: the action that answered, its HTTP status, the `content-type` it
/// declared (`None` when it declared none at all) and its body.
struct Answered {
    action: String,
    status: u16,
    content_type: Option<String>,
    body: String,
}

/// One `action` call carrying `params`, as what it answered.
async fn answer(router: &MultiServiceRouter, action: &str, params: &[Param]) -> Answered {
    let (status, content_type, body) = call_with_content_type(router, REGION, action, params).await;
    Answered {
        action: action.to_string(),
        status,
        content_type,
        body,
    }
}

/// Assert `answered` carries the status the row expects and declares the media type every
/// response of this service declares.
fn assert_media_type(case: &str, answered: &Answered, expected_status: u16) {
    assert_eq!(
        answered.status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from {}, got {} with body {}",
        answered.action, answered.status, answered.body
    );
    assert_eq!(
        answered.content_type.as_deref(),
        Some(RESPONSE_CONTENT_TYPE),
        "case {case:?}: expected content-type {RESPONSE_CONTENT_TYPE:?} from {}, got {:?} \
         with body {}",
        answered.action,
        answered.content_type,
        answered.body
    );
}

/// The reporting operations that report out of an empty store without being given anything:
/// each answers 200 and declares the media type.
///
/// Every one is an operation the recorded corpus reports this media type for, read off the
/// issue's own trace list rather than chosen here.
const REPORTING_ACTIONS: &[&str] = &[
    "GetAllowedImagesSettings",
    "DescribeApplicationStatus",
    "DescribeFastLaunchImages",
    "DescribeVpcEndpointConnections",
    "DescribeImages",
    "DescribeImportImageTasks",
    "DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations",
    "DescribePlacementGroups",
    "DescribeVolumesModifications",
];

/// One VPC of the account, as the id `CreateVpc` reported for it.
async fn created_vpc(router: &MultiServiceRouter) -> String {
    let answered = answer(router, "CreateVpc", &[param("CidrBlock", "10.0.0.0/16")]).await;
    assert_media_type("a VPC for the rows that name one", &answered, 200);
    element(&answered.body, "vpcId")
        .unwrap_or_else(|| panic!("CreateVpc must report the VPC it minted: {}", answered.body))
}

#[tokio::test]
async fn every_reporting_operation_declares_the_media_type_with_its_charset() {
    let (router, _engine) = super::ec2_query::emulator();
    for action in REPORTING_ACTIONS {
        let answered = answer(&router, action, &[]).await;
        assert_media_type(action, &answered, 200);
    }
}

/// One row of the corpus table: the operation that answered and the form fields it carried.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
}

/// One volume of the account, as the id `CreateVolume` reported for it — the source the copy
/// row copies.
///
/// The zone it sits in comes from the service's own zone catalog for the SIGNED Region (the
/// one `DescribeAvailabilityZones` reports out of, and the one
/// [`super::volume_lifecycle`] names its zones from), so the row names a zone this Region
/// actually has rather than a literal written here.
async fn created_volume(router: &MultiServiceRouter) -> String {
    let zone = availability_zones_in_region(REGION)
        .first()
        .unwrap_or_else(|| panic!("{REGION} must have an Availability Zone"))
        .zone_name
        .clone();
    let answered = answer(
        router,
        "CreateVolume",
        &[param("AvailabilityZone", &zone), param("Size", "8")],
    )
    .await;
    assert_media_type("a volume for the row that copies one", &answered, 200);
    element(&answered.body, "volumeId").unwrap_or_else(|| {
        panic!(
            "CreateVolume must report the volume it minted: {}",
            answered.body
        )
    })
}

/// The operations the recorded corpus reports this media type for whose own clusters showed
/// NOTHING ELSE — so each is an operation that is otherwise wire-correct and whose whole
/// divergence was the media type. Read off that record rather than chosen here, and carrying
/// whatever each needs to be ANSWERED rather than refused.
///
/// They span what an EC2 operation can be: one that mints a resource and reports it
/// (`CreateVpc`), one that mints one from another (`CopyVolumes`), one that hands out an
/// address, reads narrowed to a resource the account holds, reads over an empty collection,
/// and a read of a per-account catalog that is answered out of no stored resource at all
/// (`DescribeInstanceTypeOfferings`). Beside them sit the reads whose own suites already
/// cover the rule for the operations [`REPORTING_ACTIONS`] names.
fn corpus_rows(vpc_id: &str, volume_id: &str) -> Vec<Row> {
    vec![
        Row {
            case: "an address allocated for use in a VPC",
            action: "AllocateAddress",
            params: vec![param("Domain", "vpc")],
        },
        Row {
            case: "a VPC created",
            action: "CreateVpc",
            params: vec![param("CidrBlock", "10.1.0.0/16")],
        },
        Row {
            case: "every VPC of the account",
            action: "DescribeVpcs",
            params: Vec::new(),
        },
        Row {
            case: "one attribute of one VPC",
            action: "DescribeVpcAttribute",
            params: vec![
                param("VpcId", vpc_id),
                param("Attribute", "enableDnsSupport"),
            ],
        },
        Row {
            case: "every address of the account",
            action: "DescribeAddresses",
            params: Vec::new(),
        },
        Row {
            case: "the capacity blocks of the account",
            action: "DescribeCapacityBlocks",
            params: Vec::new(),
        },
        Row {
            case: "the instance types this Region offers",
            action: "DescribeInstanceTypeOfferings",
            params: Vec::new(),
        },
        Row {
            case: "the security groups of the account",
            action: "DescribeSecurityGroups",
            params: Vec::new(),
        },
        Row {
            case: "the customer-owned IP pools of the account",
            action: "DescribeCoipPools",
            params: Vec::new(),
        },
        Row {
            case: "a volume copied from another",
            action: "CopyVolumes",
            params: vec![param("SourceVolumeId", volume_id)],
        },
    ]
}

#[tokio::test]
async fn every_recorded_operation_declares_the_media_type_with_its_charset() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = created_vpc(&router).await;
    let volume_id = created_volume(&router).await;
    for row in corpus_rows(&vpc_id, &volume_id) {
        let answered = answer(&router, row.action, &row.params).await;
        assert_media_type(row.case, &answered, 200);
    }
}

/// Text outside US-ASCII, as a caller writes it into a tag and into a value the service
/// reports back inside a refusal.
///
/// The rows span the UTF-8 sequence LENGTHS a code point is written in — two bytes (Latin-1
/// Supplement and Greek), three (CJK), four (an astral-plane symbol) — and combine them, so
/// a response that reached the client in any other encoding reads back differently from
/// every row rather than from one.
const NON_ASCII_TEXT: [(&str, &str); 4] = [
    ("équipe", "café"),
    ("Ωμέγα", "μ-service"),
    ("環境", "本番"),
    ("rocket", "🚀 launch"),
];

/// The code point a UTF-8 decoder substitutes for bytes it could not read: a response whose
/// declared charset and actual encoding disagree is read back carrying one.
const REPLACEMENT: char = '\u{FFFD}';

/// A refusal's `<Message>` reports the value the request carried, so a non-ASCII value
/// reaches the client through the error envelope too. `vpc-` plus text that is no id makes
/// the id unreadable, which is what gets the value echoed.
fn unreadable_id(text: &str) -> String {
    format!("vpc-{text}")
}

/// The charset the header declares is the encoding the BYTES are written in: text a caller
/// stored, and text the service echoes inside a refusal, read back exactly as it was sent.
///
/// This is what the parameter is FOR — a client that decodes by the declared charset gets
/// the caller's own text back, where one guessing a default does not — so the rows are
/// asserted on the TEXT as well as on the header: every tag key and value round-trips
/// through a stored resource, every refusal names the value it could not read, and no
/// response carries the replacement code point a mis-decoded byte becomes.
#[tokio::test]
async fn the_declared_charset_is_the_encoding_the_reported_text_is_written_in() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = created_vpc(&router).await;

    let mut tags = vec![param("ResourceId.1", &vpc_id)];
    for (index, (key, value)) in NON_ASCII_TEXT.iter().enumerate() {
        tags.push(param(&format!("Tag.{}.Key", index + 1), key));
        tags.push(param(&format!("Tag.{}.Value", index + 1), value));
    }
    let answered = answer(&router, "CreateTags", &tags).await;
    assert_media_type("non-ASCII tags written", &answered, 200);

    let case = "non-ASCII tags read back";
    let answered = answer(&router, "DescribeVpcs", &[param("VpcId.1", &vpc_id)]).await;
    assert_media_type(case, &answered, 200);
    let reported = super::wire::vpc_tags(case, &answered.body);
    for (key, value) in NON_ASCII_TEXT {
        assert!(
            reported
                .iter()
                .any(|(stored, held)| stored == key && held == value),
            "case {case:?}: expected the tag {key:?}={value:?} back as it was written, got \
             {reported:?}"
        );
    }
    assert!(
        !answered.body.contains(REPLACEMENT),
        "case {case:?}: a body written in the declared charset carries no {REPLACEMENT:?}, \
         got {}",
        answered.body
    );

    for (key, value) in NON_ASCII_TEXT {
        for text in [key, value] {
            let id = unreadable_id(text);
            let case = format!("a refusal naming the unreadable id {id:?}");
            let answered = answer(&router, "DescribeVpcs", &[param("VpcId.1", &id)]).await;
            assert_media_type(&case, &answered, 400);
            let message = error_message(&answered.body);
            assert!(
                message.contains(text),
                "case {case:?}: expected the refusal to name {text:?}, got {message:?}"
            );
            assert!(
                !answered.body.contains(REPLACEMENT),
                "case {case:?}: a body written in the declared charset carries no \
                 {REPLACEMENT:?}, got {}",
                answered.body
            );
        }
    }
}

#[tokio::test]
async fn a_reporting_operation_given_input_declares_the_same_media_type() {
    let (router, _engine) = super::ec2_query::emulator();

    let mut scoring = vec![param("TargetCapacity", "10")];
    for (index, instance_type) in INSTANCE_TYPES.iter().enumerate() {
        scoring.push(param(&format!("InstanceType.{}", index + 1), instance_type));
    }
    let answered = answer(&router, "GetSpotPlacementScores", &scoring).await;
    assert_media_type("a scored spot placement request", &answered, 200);

    let vpc_id = created_vpc(&router).await;
    let answered = answer(
        &router,
        "DescribeStaleSecurityGroups",
        &[param("VpcId", &vpc_id)],
    )
    .await;
    assert_media_type("the stale groups of one VPC", &answered, 200);
}

#[tokio::test]
async fn a_mutation_and_the_read_that_confirms_it_declare_the_same_media_type() {
    let (router, _engine) = super::ec2_query::emulator();

    // An account setting: a mutation whose whole answer is the state it left behind.
    let answered = answer(&router, "DisableSnapshotBlockPublicAccess", &[]).await;
    assert_media_type("snapshot public access turned off", &answered, 200);
    let answered = answer(&router, "GetSnapshotBlockPublicAccessState", &[]).await;
    assert_media_type("the snapshot public access state read back", &answered, 200);

    // A delete, and the read that confirms the group is gone.
    let created = answer(
        &router,
        "CreateSecurityGroup",
        &[
            param("GroupName", "media-type-group"),
            param("GroupDescription", "a group the media type suite created"),
        ],
    )
    .await;
    assert_media_type("a security group created", &created, 200);
    let group_id = element(&created.body, "groupId")
        .unwrap_or_else(|| panic!("CreateSecurityGroup must report an id: {}", created.body));
    let answered = answer(
        &router,
        "DeleteSecurityGroup",
        &[param("GroupId", &group_id)],
    )
    .await;
    assert_media_type("that security group deleted", &answered, 200);
    let answered = answer(
        &router,
        "DescribeSecurityGroups",
        &[param("GroupId.1", &group_id)],
    )
    .await;
    assert_media_type("the deleted group read back", &answered, 400);

    // A terminate, and the read that reports the instance's state after it.
    let launched = answer(
        &router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_media_type("an instance launched", &launched, 200);
    let instance_id = element(&launched.body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report an id: {}", launched.body));
    let answered = answer(
        &router,
        "TerminateInstances",
        &[param("InstanceId.1", &instance_id)],
    )
    .await;
    assert_media_type("that instance terminated", &answered, 200);
    let answered = answer(
        &router,
        "DescribeInstances",
        &[param("InstanceId.1", &instance_id)],
    )
    .await;
    assert_media_type("the terminated instance read back", &answered, 200);
}

/// One refused row: what it calls, with which fields, and the status and `<Code>` the
/// service refuses it with.
struct Refusal {
    case: &'static str,
    action: &'static str,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// Every refusal row, each a DIFFERENT layer or status — and, for the page-size rows, a
/// bound read off the model rather than written here: `DescribeStaleSecurityGroups`'s
/// `MaxResults` publishes the range 5..255
/// (`smithy_query(query="operation",operation="DescribeStaleSecurityGroups")`), so one
/// below the smallest page and one above the largest are the two sides of that range.
///
/// The `<Code>` each row expects is the code this service already answers for that layer,
/// asserted so a row cannot pass by being refused for another reason: `InvalidMaxResults` is
/// EC2's own for a page below an operation's published minimum, while a value outside a
/// modelled constraint the service has no recorded EC2 wording for keeps the request walk's
/// own code. Which code EC2 words those with is a different issue
/// (`issues/.../20261023T102725Z-ec2-answers-parameter-rejections-with-generic-validationerror-and.md`);
/// this suite is about the envelope every one of them is announced in.
fn refusals(vpc_id: &str) -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a VPC the account does not hold, refused by the handler",
            action: "DescribeStaleSecurityGroups",
            params: vec![param("VpcId", UNKNOWN_VPC_ID)],
            status: 400,
            code: "InvalidVpcID.NotFound",
        },
        Refusal {
            case: "a page size below the smallest the operation publishes",
            action: "DescribeStaleSecurityGroups",
            params: vec![param("VpcId", vpc_id), param("MaxResults", "4")],
            status: 400,
            code: "InvalidMaxResults",
        },
        Refusal {
            case: "a page size above the largest the operation publishes",
            action: "DescribeStaleSecurityGroups",
            params: vec![param("VpcId", vpc_id), param("MaxResults", "256")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "a request that leaves out a member the model requires",
            action: "DeleteVpc",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
        Refusal {
            case: "a value outside a modelled enum catalog",
            action: "EnableAllowedImagesSettings",
            params: vec![param("AllowedImagesSettingsState", "audit")],
            status: 400,
            code: "ValidationError",
        },
        Refusal {
            case: "a dry run that would otherwise have succeeded",
            action: "DescribeFastLaunchImages",
            params: vec![param("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
        },
    ]
}

#[tokio::test]
async fn every_refusal_declares_the_same_media_type_and_names_the_request_once() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = created_vpc(&router).await;
    for row in refusals(&vpc_id) {
        let answered = answer(&router, row.action, &row.params).await;
        assert_media_type(row.case, &answered, row.status);
        assert_eq!(
            error_code(&answered.body),
            row.code,
            "case {:?}: expected the code {}, got {}",
            row.case,
            row.code,
            answered.body
        );
        let reported = root_children(
            row.case,
            &answered.body,
            ERROR_ENVELOPE_ROOT,
            ERROR_REQUEST_ID,
        );
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: expected exactly one <{ERROR_REQUEST_ID}> under <{ERROR_ENVELOPE_ROOT}>, \
             got {reported:?} in {}",
            row.case,
            answered.body
        );
    }
}
