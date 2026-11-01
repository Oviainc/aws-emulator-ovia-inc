// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the FIXED TEXT of a refusal's `<Message>`, driven over the wire the
//! service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test: a refusal's message is the sentence the real service words that
//! error CLASS with — that sentence's fixed text exactly, and only the variable parts the
//! service interpolates, taken from the request and from the store. Two halves follow from
//! it, and every row below pins one of them:
//!
//! * nothing the service does not send is ADDED — no restatement of the rule the value
//!   missed (the block size a CIDR has to be inside, the range an ASN has to be inside, the
//!   digits a pool id is made of) and no identifier read out of the account's own state that
//!   the class does not name (the subnet that already holds an overlapping block, the
//!   Availability Zone each of two resources is in);
//! * nothing the sentence DOES carry is DROPPED — the holders a delete's `VolumeInUse`
//!   refusal names keep the brace rendering of the set they are, and a class that enumerates
//!   the catalog it supports enumerates it in the order the service publishes rather than
//!   sorted.
//!
//! WHICH sentence a class carries is a fact of the operation that answers it, not of the
//! code: the two operations that refuse `VolumeInUse` word it differently, and each keeps its
//! own.
//!
//! The rows differ along what the rule generalises over: the refusal CLASS (the eight the
//! issue behind this suite names, plus the same CIDR class on a ninth operation), the
//! OPERATION that answers it where several share a wording site (`CreateSubnet` and
//! `AssociateSubnetCidrBlock` for a subnet block, `CreateVpc` and `CreateSecondaryNetwork`
//! for a resource's own, `DeleteVolume` and `AttachVolume` for a held volume), the FORM of the
//! value the request sent (both published CIDR bounds missed and a block whose host bits are
//! set; an ASN below and above each published range; seven ways a value can fail to be a pool
//! id, one of them longer than the service repeats back; eight ways an instance id can fail to
//! resolve; seven filter names outside the catalog), and the STATE the refusal is computed
//! from (which zone a volume is in, and whether one holder or two hold a volume).
//!
//! Every value a row names is derived from the service's own published constants where it has
//! one, so a bound and the row that probes it cannot drift. The addresses are from the blocks
//! RFC 5737 and RFC 3849 set aside for documentation.

use std::sync::Arc;

use ec2::ops::types::SECONDARY_NETWORK_TYPE_VALUES;
use ec2::state::{
    DEFAULT_INSTANCE_TYPE, INVALID_FILTER, INVALID_PARAMETER_VALUE, INVALID_POOL_ID_MALFORMED,
    INVALID_SUBNET_CONFLICT, INVALID_SUBNET_RANGE, INVALID_VOLUME_ZONE_MISMATCH, INVALID_VPC_RANGE,
    MAX_ECHOED_COIP_POOL_ID_CHARS, MAX_SUBNET_IPV4_PREFIX, MAX_VPC_IPV4_PREFIX,
    MIN_SUBNET_IPV4_PREFIX, MIN_VPC_IPV4_PREFIX, SUBNET_IPV6_PREFIX_LENGTH, VOLUME_IN_USE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message, filters};
use super::network_insights_analyses::SUPPORTED_FILTERS;
use super::volume_lifecycle::{REGION, p, past_the_transition, required_iops, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The code an instance id outside the form EC2 reads is refused with. Named here rather
/// than imported because the instance id refusals live on the error channel
/// (`crate::scaffolded`) instead of in `ec2::state`, where every other code is.
const INVALID_INSTANCE_ID_MALFORMED: &str = "InvalidInstanceID.Malformed";

/// The `Type` a customer gateway create has to carry: the one value `GatewayType` has.
const GATEWAY_TYPE: &str = "ipsec.1";

/// One row: a request, the code it must be refused with, the sentence it must be refused
/// with, and the strings this class does NOT send that must therefore be absent from it.
struct Row {
    /// What this row is a case of, named in every assertion about it.
    case: String,
    action: &'static str,
    params: Params,
    code: &'static str,
    message: String,
    /// Text the request or the store carries that this class does not put in its message.
    withheld: Vec<String>,
}

fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `action` call signed for the Region the suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Assert one row end to end: the status, the code, the sentence, and the absence of
/// everything the class withholds.
async fn assert_row(router: &MultiServiceRouter, row: &Row) {
    let Row {
        case,
        action,
        params,
        code,
        message,
        withheld,
    } = row;
    let (status, body) = op(router, action, params).await;
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS} from {action}, got {status} with body \
         {body}"
    );
    let answered_code = error_code(&body);
    assert_eq!(
        &answered_code, code,
        "case {case:?}: expected the code {code:?} from {action}, got {answered_code:?} in \
         {body}"
    );
    let answered = error_message(&body);
    assert_eq!(
        &answered, message,
        "case {case:?}: expected the message {message:?} from {action}, got {answered:?}"
    );
    for absent in withheld {
        assert!(
            !answered.contains(absent.as_str()),
            "case {case:?}: this class does not send {absent:?}, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Class 1: a CIDR block of a size the resource does not permit
// ---------------------------------------------------------------------------

/// The text a refusal of a block size must NOT carry: the service states the verdict, not the
/// published block size the value missed, which is what the error CODE stands for.
fn block_size_withheld() -> Vec<String> {
    vec![
        "netmask".to_string(),
        "block size".to_string(),
        format!("/{MIN_VPC_IPV4_PREFIX} "),
    ]
}

/// Class 1 once more, on the resource whose own block size the model states in prose rather
/// than as a code of its own: `CreateSecondaryNetwork` refuses a block outside
/// `Ipv4CidrBlock`'s published size with the same sentence, under the parameter-value code it
/// answers for that member.
fn secondary_network_block_size_rows() -> Vec<Row> {
    // The modelled `SecondaryNetworkType` enum's first value: the member is `@required`, so
    // every row carries one, and the block is what each row varies.
    let network_type = SECONDARY_NETWORK_TYPE_VALUES
        .first()
        .expect("the modelled SecondaryNetworkType enum has at least one value");
    ["10.0.0.0/8", "10.0.0.0/29", "192.0.2.0/32"]
        .into_iter()
        .map(|block| Row {
            case: format!("CreateSecondaryNetwork asking for the block {block}"),
            action: "CreateSecondaryNetwork",
            params: vec![p("Ipv4CidrBlock", block), p("NetworkType", *network_type)],
            code: INVALID_PARAMETER_VALUE,
            message: format!("The CIDR '{block}' is invalid."),
            withheld: block_size_withheld(),
        })
        .collect()
}

/// Class 1 for a VPC's own block: `CreateVpc` over one past each published bound, far past the
/// lower one, a single address, and a block whose host bits are set — the last of which pins
/// that the sentence names the value the REQUEST sent.
fn vpc_block_size_rows() -> Vec<Row> {
    let blocks = [
        format!("10.0.0.0/{}", MIN_VPC_IPV4_PREFIX - 1),
        format!("10.0.0.0/{}", MAX_VPC_IPV4_PREFIX + 1),
        "10.0.0.0/8".to_string(),
        "192.0.2.0/32".to_string(),
        "198.51.100.73/8".to_string(),
    ];
    blocks
        .into_iter()
        .map(|block| Row {
            case: format!("CreateVpc asking for the block {block}"),
            action: "CreateVpc",
            params: vec![p("CidrBlock", block.clone())],
            code: INVALID_VPC_RANGE,
            message: format!("The CIDR '{block}' is invalid."),
            withheld: block_size_withheld(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Class 2: a value that is not a customer-owned address pool id
// ---------------------------------------------------------------------------

/// Class 2: `DescribeCoipPools` over every way a value can fail to be a `CoipPoolId` — the
/// prefix of another address pool, a prefix EC2 mints no id with, a suffix of neither
/// published length, a non-hexadecimal digit, an upper-case digit, no prefix at all, and a
/// value far longer than the service repeats back.
fn pool_id_rows() -> Vec<Row> {
    let long = "ipv4pool-coip-".to_string() + &"a".repeat(MAX_ECHOED_COIP_POOL_ID_CHARS * 2);
    let values = [
        "ipv4pool-ec2-00000000000000000".to_string(),
        "coip-pool-00000000000000000".to_string(),
        "ipv4pool-coip-00000".to_string(),
        "ipv4pool-coip-0000000000000000z".to_string(),
        "ipv4pool-coip-0000000000000000A".to_string(),
        "not-a-pool".to_string(),
        long,
    ];
    values
        .into_iter()
        .map(|value| {
            // Only the leading characters of a caller-chosen value are named, the way every
            // echo of this surface's id is bounded.
            let echoed: String = value.chars().take(MAX_ECHOED_COIP_POOL_ID_CHARS).collect();
            Row {
                case: format!("DescribeCoipPools selecting on the unreadable id {echoed}"),
                action: "DescribeCoipPools",
                params: vec![p("PoolId.1", value.clone())],
                code: INVALID_POOL_ID_MALFORMED,
                message: format!("The pool ID '{echoed}' is invalid."),
                // The form a pool id has is the reader's rule, not part of the answer.
                withheld: vec![
                    "hexadecimal".to_string(),
                    "followed by".to_string(),
                    "Invalid id".to_string(),
                ],
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Class 3: an ASN a customer gateway member cannot carry
// ---------------------------------------------------------------------------

/// Class 3: `CreateCustomerGateway` over an ASN below and above what each of the two members
/// publishes — a value a member cannot carry is a FORMAT verdict about that member, and the
/// range it publishes is not restated.
fn customer_gateway_asn_rows() -> Vec<Row> {
    // `(the member, the ASN it is given)`: below the bottom of `BgpAsn`'s range twice over,
    // below and above `BgpAsnExtended`'s, and the value just under where the extended member
    // takes over.
    let values: [(&str, &str); 6] = [
        ("BgpAsn", "0"),
        ("BgpAsn", "-1"),
        ("BgpAsn", "-65000"),
        ("BgpAsnExtended", "1"),
        ("BgpAsnExtended", "2147483647"),
        ("BgpAsnExtended", "4294967296"),
    ];
    values
        .into_iter()
        .map(|(member, asn)| {
            // The message names the member as the wire names it: the request's member with a
            // lower-case initial.
            let parameter = format!("{}{}", member[..1].to_lowercase(), &member[1..]);
            Row {
                case: format!("CreateCustomerGateway given {member}={asn}"),
                action: "CreateCustomerGateway",
                params: vec![p("Type", GATEWAY_TYPE), p(member, asn)],
                code: INVALID_PARAMETER_VALUE,
                message: format!(
                    "Value ({asn}) for parameter {parameter} is invalid. Invalid Format."
                ),
                withheld: vec!["Valid values".to_string()],
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Class 4: a filter name outside the catalog an operation supports
// ---------------------------------------------------------------------------

/// Class 4: `DescribeNetworkInsightsAnalyses` over names outside its catalog — another
/// describe's filter, the two `tag` families, the supported name in another case, a name with
/// the wrong separator and the empty name. Every row gets the SAME sentence, which enumerates
/// the catalog in the order the service publishes it ([`SUPPORTED_FILTERS`]).
fn unsupported_filter_rows() -> Vec<Row> {
    let names = [
        "network-insights-path-id",
        "network-insights-analysis-id",
        "tag-key",
        "tag:Name",
        "Status",
        "path_found",
        "",
    ];
    let message = format!("Filter name must be one of [{}]", SUPPORTED_FILTERS.join(", "));
    names
        .into_iter()
        .map(|name| Row {
            case: format!("DescribeNetworkInsightsAnalyses filtering on {name:?}"),
            action: "DescribeNetworkInsightsAnalyses",
            params: filters(&[(name, &["any"])]),
            code: INVALID_FILTER,
            message: message.clone(),
            // The name at fault is not what this class reports: it reports the catalog.
            withheld: vec![format!("'{name}'")],
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Class 5: an instance id this operation's id reader cannot read
// ---------------------------------------------------------------------------

/// Class 5: `CreateSnapshots` over every id it cannot resolve to an instance of the account —
/// the all-zero placeholder, text that is no id, the prefix alone, an upper-case suffix, a
/// leading digit above zero, another resource kind's id, and a WELL-FORMED id the account
/// holds no instance for. One sentence for all of them, and it interpolates NOTHING, so no
/// row may find its own value in it.
fn unreadable_instance_id_rows() -> Vec<Row> {
    let values = [
        "i-00000000000000000",
        "not-an-instance",
        "i-",
        "i-0A1B2C3D4E5F60718",
        "i-1234567890abcdef0",
        "vol-0a1b2c3d4e5f60718",
        "i-0fedcba987654321f",
        "i-0a1b2c3d",
    ];
    values
        .into_iter()
        .map(|value| Row {
            case: format!("CreateSnapshots naming the unresolvable instance id {value}"),
            action: "CreateSnapshots",
            params: vec![p("InstanceSpecification.InstanceId", value)],
            code: INVALID_INSTANCE_ID_MALFORMED,
            message: "Unable to unobfuscate ID.".to_string(),
            withheld: vec![value.to_string()],
        })
        .collect()
}

/// RULE, for every class whose refusal needs no stored resource: the message is that class's
/// own sentence, built from the request alone, and carries nothing the class does not send.
#[tokio::test]
async fn a_refusal_computed_from_the_request_carries_its_classs_sentence_only() {
    let (router, _engine) = emulator();

    let mut rows = Vec::new();
    rows.extend(vpc_block_size_rows());
    rows.extend(secondary_network_block_size_rows());
    rows.extend(pool_id_rows());
    rows.extend(customer_gateway_asn_rows());
    rows.extend(unsupported_filter_rows());
    rows.extend(unreadable_instance_id_rows());
    assert!(
        rows.len() > 20,
        "the table must span the classes it is written for, got {} rows",
        rows.len()
    );
    for row in &rows {
        assert_row(&router, row).await;
    }
}

// ---------------------------------------------------------------------------
// Classes 6 and 7: a subnet block the VPC's state refuses
// ---------------------------------------------------------------------------

/// The VPC every subnet row below sits in.
const VPC_BLOCK: &str = "10.83.0.0/16";

/// The block the subnet that is already there holds, which the conflict rows overlap.
const HELD_SUBNET_BLOCK: &str = "10.83.1.0/24";

/// RULE for the two subnet classes: a block that overlaps one the VPC already has names the
/// REQUESTED block and no stored subnet, and a block of a size a subnet may not have gets the
/// same fixed sentence a VPC's own does.
#[tokio::test]
async fn a_subnet_block_refusal_names_the_requested_block_and_no_stored_subnet() {
    let (router, _engine) = emulator();

    let (status, body) = op(&router, "CreateVpc", &[p("CidrBlock", VPC_BLOCK)]).await;
    assert_eq!(
        status, 200,
        "seeding the VPC must succeed, got {status} with body {body}"
    );
    let vpc_id = element(&body, "vpcId").expect("CreateVpc reports the VPC it minted");

    let (status, body) = op(
        &router,
        "CreateSubnet",
        &[p("VpcId", &vpc_id), p("CidrBlock", HELD_SUBNET_BLOCK)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the subnet must succeed, got {status} with body {body}"
    );
    let held_subnet_id = element(&body, "subnetId").expect("CreateSubnet reports the subnet");

    // Class 6: blocks that overlap the one the stored subnet holds — one inside it, the
    // identical block, one that contains it, and a /28 inside it. The last row's block is
    // given with its host bits set: "we modify the specified CIDR block to its canonical
    // form" (`CreateSubnetRequest$CidrBlock`) is what the store keeps, while the refusal
    // names the block the REQUEST asked for.
    let overlapping: [&str; 5] = [
        "10.83.1.128/25",
        HELD_SUBNET_BLOCK,
        VPC_BLOCK,
        "10.83.1.64/28",
        "10.83.1.200/25",
    ];
    let mut rows: Vec<Row> = overlapping
        .into_iter()
        .map(|requested| Row {
            case: format!("CreateSubnet asking for the overlapping block {requested}"),
            action: "CreateSubnet",
            params: vec![p("VpcId", &vpc_id), p("CidrBlock", requested)],
            code: INVALID_SUBNET_CONFLICT,
            message: format!("The CIDR '{requested}' conflicts with another subnet"),
            // The subnet that holds the overlapping block is the account's state, which this
            // class does not report.
            withheld: vec![held_subnet_id.clone()],
        })
        .collect();

    // Class 7: a block the subnet range rules do not admit, whichever rule it fails — one
    // past each published bound, far past the lower one, a single address, and two blocks of
    // a permitted size that lie OUTSIDE the VPC's own block — plus the IPv6 counterpart
    // through the operation that associates one, whose only published size is
    // `SUBNET_IPV6_PREFIX_LENGTH`. One sentence for all of them.
    let sizes = [
        format!("10.83.0.0/{}", MAX_SUBNET_IPV4_PREFIX + 1),
        format!("10.0.0.0/{}", MIN_SUBNET_IPV4_PREFIX - 1),
        "10.0.0.0/8".to_string(),
        "10.83.1.1/32".to_string(),
        "192.168.4.0/24".to_string(),
        "10.84.0.0/28".to_string(),
    ];
    rows.extend(sizes.into_iter().map(|block| Row {
        case: format!("CreateSubnet asking for the block {block}"),
        action: "CreateSubnet",
        params: vec![p("VpcId", &vpc_id), p("CidrBlock", block.clone())],
        code: INVALID_SUBNET_RANGE,
        message: format!("The CIDR '{block}' is invalid."),
        // Neither the published block size nor the VPC whose block it is outside of.
        withheld: [block_size_withheld(), vec![vpc_id.clone()]].concat(),
    }));
    for prefix in [SUBNET_IPV6_PREFIX_LENGTH - 8, SUBNET_IPV6_PREFIX_LENGTH + 8] {
        let block = format!("2001:db8::/{prefix}");
        rows.push(Row {
            case: format!("AssociateSubnetCidrBlock asking for the IPv6 block {block}"),
            action: "AssociateSubnetCidrBlock",
            params: vec![
                p("SubnetId", &held_subnet_id),
                p("Ipv6CidrBlock", block.clone()),
            ],
            code: INVALID_SUBNET_RANGE,
            message: format!("The CIDR '{block}' is invalid."),
            withheld: block_size_withheld(),
        });
    }

    for row in &rows {
        assert_row(&router, row).await;
    }

    // Nothing the refused creates asked for was stored: the subnet the seed made is still the
    // only one the VPC has.
    let (status, body) = op(
        &router,
        "DescribeSubnets",
        &[p("Filter.1.Name", "vpc-id"), p("Filter.1.Value.1", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeSubnets must succeed, got {status} with body {body}"
    );
    let stored = super::ec2_query::elements(&body, "subnetId");
    assert_eq!(
        stored,
        vec![held_subnet_id.clone()],
        "a refused create stores nothing, so the VPC must still hold only {held_subnet_id}, \
         got {stored:?} in {body}"
    );
}

// ---------------------------------------------------------------------------
// Class 8: the volume refusals, computed from what the store holds
// ---------------------------------------------------------------------------

/// Launch one instance of the default type and answer its id. A launch that names no
/// placement puts the instance in the Region's first zone.
async fn launch(router: &MultiServiceRouter) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            p("ImageId", "ami-0123456789abcdef0"),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("InstanceType", DEFAULT_INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    element(&body, "instanceId").expect("RunInstances reports the instance it minted")
}

/// Create one volume of `size` GiB in `zone`, with whatever extra parameters the case needs,
/// and answer its id.
async fn volume_in(router: &MultiServiceRouter, zone: &str, size: u32, extra: Params) -> String {
    let mut params = vec![p("AvailabilityZone", zone), p("Size", size.to_string())];
    params.extend(extra);
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "CreateVolume must succeed, got {status} with body {body}"
    );
    element(&body, "volumeId").expect("CreateVolume reports the volume it minted")
}

/// Attach `volume_id` to `instance_id` at `device`, requiring the attach to be served.
async fn attach(router: &MultiServiceRouter, volume_id: &str, instance_id: &str, device: &str) {
    let (status, body) = op(
        router,
        "AttachVolume",
        &[
            p("VolumeId", volume_id),
            p("InstanceId", instance_id),
            p("Device", device),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "attaching {volume_id} to {instance_id} must succeed, got {status} with body {body}"
    );
}

/// The holders a `VolumeInUse` message names, read out of its brace-wrapped set rendering,
/// sorted so the comparison is about the SET and not about the order the store happens to
/// hold the attachments in.
fn holder_set(case: &str, volume_id: &str, message: &str) -> Vec<String> {
    let prefix = format!("Volume {volume_id} is currently attached to {{");
    let inner = message
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected {prefix:?} then a brace-closed set, got {message:?}")
        });
    let mut holders: Vec<String> = inner.split(", ").map(str::to_string).collect();
    holders.sort();
    holders
}

/// RULE for the attach refusal: a volume and an instance in different zones are named as the
/// REQUEST named them, and the zones they are in are not reported.
#[tokio::test]
async fn an_attach_across_zones_names_the_two_resources_and_not_their_zones() {
    let (router, _engine) = emulator();
    let instance_id = launch(&router).await;

    // One row per other zone of the Region, and one of them a volume of another type and
    // size, so the sentence is pinned across what the class generalises over rather than on
    // one pair.
    let mut elsewhere = Vec::new();
    for zone in 1..3 {
        let (size, extra) = if zone == 1 {
            (8, Vec::new())
        } else {
            // "A sc1 volume holds from 125 to 16384 GiB": the bottom of that type's range.
            (125, vec![p("VolumeType", "sc1")])
        };
        elsewhere.push((zone, volume_in(&router, &zone_name(zone), size, extra).await));
    }
    past_the_transition().await;

    for (zone, volume_id) in &elsewhere {
        let row = Row {
            case: format!("AttachVolume for {volume_id} in {}", zone_name(*zone)),
            action: "AttachVolume",
            params: vec![
                p("VolumeId", volume_id.as_str()),
                p("InstanceId", instance_id.as_str()),
                p("Device", "/dev/sdf"),
            ],
            code: INVALID_VOLUME_ZONE_MISMATCH,
            message: format!(
                "The volume '{volume_id}' is not in the same availability zone as instance \
                 '{instance_id}'"
            ),
            // Which zone each resource is in is state the caller reads off the resources.
            withheld: vec![zone_name(0), zone_name(*zone)],
        };
        assert_row(&router, &row).await;
    }
}

/// RULE for the two refusals a volume's attachments decide, each in its own operation's
/// wording: the DELETE names every holder that has to let go of the volume first, as the SET
/// it is — brace-wrapped — whether one resource holds it or two, while the ATTACH reports
/// that an instance already holds the volume and names no holder at all.
#[tokio::test]
async fn the_refusals_a_held_volume_decides_carry_each_operations_wording() {
    let (router, _engine) = emulator();
    let first = launch(&router).await;
    let second = launch(&router).await;

    let single = volume_in(&router, &zone_name(0), 8, Vec::new()).await;
    // "This parameter is supported with io1 and io2 volumes only"
    // (`CreateVolumeRequest$MultiAttachEnabled`), and io1 requires its IOPS — the volume kind
    // more than one instance can hold at once, which is what makes the set rendering
    // observable with two members.
    let mut extra = vec![p("VolumeType", "io1"), p("MultiAttachEnabled", "true")];
    extra.extend(required_iops("io1"));
    let shared = volume_in(&router, &zone_name(0), 8, extra).await;
    past_the_transition().await;

    attach(&router, &single, &first, "/dev/sdf").await;
    attach(&router, &shared, &first, "/dev/sdg").await;
    attach(&router, &shared, &second, "/dev/sdg").await;

    // The delete side: `(case, the volume, the holders the store has for it)` — one holder
    // and two, so the set rendering is pinned at both sizes.
    let deletes: [(&str, &String, Vec<String>); 2] = [
        ("a volume one instance holds", &single, vec![first.clone()]),
        (
            "a volume two instances hold",
            &shared,
            vec![first.clone(), second.clone()],
        ),
    ];
    for (case, volume_id, holders) in deletes {
        let (status, body) = op(&router, "DeleteVolume", &[p("VolumeId", volume_id.as_str())]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} from DeleteVolume, got {status} \
             with body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, VOLUME_IN_USE,
            "case {case:?}: expected {VOLUME_IN_USE} from DeleteVolume, got {body}"
        );
        let answered = error_message(&body);
        let mut expected = holders.clone();
        expected.sort();
        assert_eq!(
            holder_set(case, volume_id, &answered),
            expected,
            "case {case:?}: expected the refusal to name the holder set {expected:?}, got \
             {answered:?}"
        );
    }

    // The attach side: the volume another instance holds, and the volume the named instance
    // already holds — one sentence, naming the volume from the request and no holder.
    let attaches: [(&str, &String, &String); 2] = [
        ("a volume another instance holds", &single, &second),
        ("a volume the named instance already holds", &shared, &first),
    ];
    for (case, volume_id, instance_id) in attaches {
        let row = Row {
            case: format!("AttachVolume given {case}"),
            action: "AttachVolume",
            params: vec![
                p("VolumeId", volume_id.as_str()),
                p("InstanceId", instance_id.as_str()),
                p("Device", "/dev/sdh"),
            ],
            code: VOLUME_IN_USE,
            message: format!("{volume_id} is already attached to an instance"),
            // Which instance holds it is state this class does not report.
            withheld: vec![first.clone(), second.clone()],
        };
        assert_row(&router, &row).await;
    }
}
