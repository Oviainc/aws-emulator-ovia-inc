// SPDX-License-Identifier: CC0-1.0

//! What EC2 refuses about a REQUEST before it reads or writes any state, driven end to end
//! over the wire the service speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what that rule
//! generalises over. Every row is one request; no row is the specification.
//!
//! 1. A page size and an explicit resource-id selection cannot be given together: a request
//!    that NAMES the resources it wants is not walking a collection, so a paged describe
//!    carrying both is refused `InvalidParameterCombination` — over six operations and six
//!    id members, and with each selector given ALONE to show it is the COMBINATION that is
//!    refused.
//! 2. A Capacity Block search names EITHER the UltraServer dimension or the instance
//!    dimension: one that names members of both, or of neither, is refused
//!    `InvalidParameterValue`, and one that names a single dimension — by its type, by its
//!    count, or by both — is served.
//! 3. An idempotency token is idempotent only for the request it was first given with: the
//!    same `ClientToken` with any different parameter is refused
//!    `IdempotentParameterMismatch`, the same token with the same parameters answers the one
//!    volume it already minted, and the refusals create nothing.
//! 4. An AMI id a launch names is read for its FORM before any image is looked up: a value
//!    EC2 could not have issued is refused `InvalidAMIID.Malformed`, whatever makes it
//!    unreadable, and a well-formed id launches an instance that reports it back.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, emulator, error_code, error_message};

/// The Region every call here is signed for. Nothing in any of the four rules depends on
/// it: each is a fact of the request, read before anything regional is resolved.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// One form field.
fn p(name: &str, value: impl Into<String>) -> (String, String) {
    (name.to_string(), value.into())
}

/// One call signed for [`REGION`].
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// The first Availability Zone of [`REGION`], as a `CreateVolume` names one.
fn zone() -> String {
    ec2::state::availability_zones_in_region(REGION)[0]
        .zone_name
        .clone()
}

// ---------------------------------------------------------------------------
// Rule 1: a page size cannot be combined with an explicit id selection
// ---------------------------------------------------------------------------

/// One row of the combination table: a paged describe, the form field its id selector is
/// sent as, a well-formed id of that kind, and the two parameter names that operation's own
/// refusal names — EC2 spells the pair with the public member name on some operations and
/// with the wire set name on others, and in either order.
struct Combination {
    action: &'static str,
    id_field: &'static str,
    id: &'static str,
    first: &'static str,
    second: &'static str,
}

/// The paged describes this rule is asserted over: six operations and six id kinds, so no
/// one operation's members are the rule.
///
/// The page size every row sends is 5, the smallest value the `@range(min: 5, max: 1000)`
/// the model publishes for `DescribeSecurityGroupsMaxResults` / `DescribeSubnetsMaxResults`
/// admits, so each row reaches the handler rather than the boundary validator in front of
/// it.
const COMBINATIONS: [Combination; 6] = [
    Combination {
        action: "DescribeSecurityGroups",
        id_field: "GroupId.1",
        id: "sg-0123456789abcdef0",
        first: "securityGroupIdSet",
        second: "maxResults",
    },
    Combination {
        action: "DescribeSubnets",
        id_field: "SubnetId.1",
        id: "subnet-0123456789abcdef0",
        first: "SubnetIds",
        second: "MaxResults",
    },
    Combination {
        action: "DescribeInstances",
        id_field: "InstanceId.1",
        id: "i-0123456789abcdef0",
        first: "instancesSet",
        second: "maxResults",
    },
    Combination {
        action: "DescribeNetworkInterfaces",
        id_field: "NetworkInterfaceId.1",
        id: "eni-0123456789abcdef0",
        first: "MaxResults",
        second: "NetworkInterfaceId",
    },
    Combination {
        action: "DescribeHosts",
        id_field: "HostId.1",
        id: "h-0123456789abcdef0",
        first: "maxResults",
        second: "hostId",
    },
    Combination {
        action: "DescribeStoreImageTasks",
        id_field: "ImageId.1",
        id: "ami-0123456789abcdef0",
        first: "MaxResults",
        second: "ImageIds",
    },
];

/// The page size every row of [`COMBINATIONS`] sends.
const PAGE_SIZE: &str = "5";

/// RULE 1: a page size beside an explicit id selection is refused
/// `InvalidParameterCombination`, with the sentence that names both parameters; either one
/// alone is not.
#[tokio::test]
async fn a_page_size_cannot_be_combined_with_an_id_selection() {
    let (router, _engine) = emulator();

    for row in COMBINATIONS {
        let Combination {
            action,
            id_field,
            id,
            first,
            second,
        } = row;

        let both = [p(id_field, id), p("MaxResults", PAGE_SIZE)];
        let case = format!("{action} with {id_field} and MaxResults");
        let (status, body) = op(&router, action, &both).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_COMBINATION,
            "case {case:?}: expected the code {:?}, got {code:?} in {body}",
            ec2::state::INVALID_PARAMETER_COMBINATION
        );
        let expected = format!("The parameter {first} cannot be used with the parameter {second}");
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The ids ALONE: whatever the operation answers for an id nothing holds, it is not
        // the combination refusal — so the row pins the pair rather than the selector.
        let case = format!("{action} with {id_field} alone");
        let (_, body) = op(&router, action, &[p(id_field, id)]).await;
        let code = error_code(&body);
        assert_ne!(
            code,
            ec2::state::INVALID_PARAMETER_COMBINATION,
            "case {case:?}: an id selection alone is no combination failure, got {body}"
        );

        // The page size ALONE: a walk of the account's resources, which is what the page
        // size is for.
        let case = format!("{action} with MaxResults alone");
        let (status, body) = op(&router, action, &[p("MaxResults", PAGE_SIZE)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a Capacity Block search names exactly one dimension
// ---------------------------------------------------------------------------

/// The sentence EC2 refuses a Capacity Block search that names both dimensions, or neither,
/// with. FIXED text: it states the rule and names nothing of the request.
const DIMENSION_MESSAGE: &str = "Invalid inputs, either ultraserver-type and ultraserver-count, \
     or instance-type and instance-count needs to be set, not both";

/// A duration inside the published increments ("1-day increments up 14 days"), so every row
/// below is decided by its dimensions rather than by its duration.
const DURATION_HOURS: &str = "24";

/// RULE 2: a search that names members of BOTH dimensions, or of NEITHER, is refused; one
/// that names a single dimension is served.
#[tokio::test]
async fn a_capacity_block_search_names_one_dimension() {
    let (router, _engine) = emulator();

    // The refused rows differ along WHICH members of the two dimensions are named: both
    // types, both counts, each type beside the other dimension's count, and neither
    // dimension at all (once bare, once beside members that select nothing of either
    // dimension).
    let refused: [(&str, Vec<(String, String)>); 6] = [
        (
            "both types",
            vec![
                p("InstanceType", "p5.48xlarge"),
                p("UltraserverType", "u-p6e-gb200x72"),
            ],
        ),
        (
            "both counts",
            vec![p("InstanceCount", "2"), p("UltraserverCount", "2")],
        ),
        (
            "an instance dimension beside an UltraServer count",
            vec![
                p("InstanceType", "trn2.48xlarge"),
                p("InstanceCount", "4"),
                p("UltraserverCount", "1"),
            ],
        ),
        (
            "an UltraServer dimension beside an instance count",
            vec![
                p("UltraserverType", "u-trn2x16"),
                p("UltraserverCount", "2"),
                p("InstanceCount", "8"),
            ],
        ),
        ("neither dimension", Vec::new()),
        (
            "neither dimension, beside a page size",
            vec![p("MaxResults", "5")],
        ),
    ];
    for (case, extra) in refused {
        let mut params = vec![p("CapacityDurationHours", DURATION_HOURS)];
        params.extend(extra);
        let (status, body) = op(&router, "DescribeCapacityBlockOfferings", &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PARAMETER_VALUE,
            "case {case:?}: expected the code {:?}, got {code:?} in {body}",
            ec2::state::INVALID_PARAMETER_VALUE
        );
        let message = error_message(&body);
        assert_eq!(
            message, DIMENSION_MESSAGE,
            "case {case:?}: expected the message {DIMENSION_MESSAGE:?}, got {message:?}"
        );
        assert!(
            elements(&body, "capacityBlockOfferingId").is_empty(),
            "case {case:?}: a refused search reports no offering, got {body}"
        );
    }

    // One dimension named — by both its members, by its type alone, or by its count alone —
    // is a search this operation answers.
    let served: [(&str, Vec<(String, String)>); 5] = [
        (
            "the instance dimension",
            vec![p("InstanceType", "p4d.24xlarge"), p("InstanceCount", "8")],
        ),
        ("an instance type alone", vec![p("InstanceType", "m6g.medium")]),
        ("an instance count alone", vec![p("InstanceCount", "3")]),
        (
            "the UltraServer dimension",
            vec![
                p("UltraserverType", "u-p6e-gb200x72"),
                p("UltraserverCount", "2"),
            ],
        ),
        (
            "an UltraServer type alone",
            vec![p("UltraserverType", "u-trn2x16")],
        ),
    ];
    for (case, extra) in served {
        let mut params = vec![p("CapacityDurationHours", DURATION_HOURS)];
        params.extend(extra);
        let (status, body) = op(&router, "DescribeCapacityBlockOfferings", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            !elements(&body, "capacityBlockOfferingId").is_empty(),
            "case {case:?}: a search of one dimension is answered with offerings, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one idempotency token names one volume
// ---------------------------------------------------------------------------

/// The parameters of the volume every row of the idempotency table replays the token of:
/// the size, the type and the zone of the FIRST request.
fn first_volume_params(token: &str) -> Vec<(String, String)> {
    vec![
        p("AvailabilityZone", zone()),
        p("Size", "8"),
        p("VolumeType", "gp2"),
        p("ClientToken", token),
    ]
}

/// The volume ids the account holds, in the order `DescribeVolumes` reports them.
async fn stored_volume_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = op(router, "DescribeVolumes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe of the account's volumes must succeed, got {status} with \
         body {body}"
    );
    elements(&body, "volumeId")
}

/// RULE 3: a `ClientToken` replayed with a different parameter is refused
/// `IdempotentParameterMismatch`; replayed with the same parameters it answers the volume it
/// already minted; and a refusal creates nothing.
#[tokio::test]
async fn one_client_token_names_one_volume() {
    let (router, _engine) = emulator();
    let token = "a-token-this-suite-chose";

    let (status, body) = op(&router, "CreateVolume", &first_volume_params(token)).await;
    assert_eq!(
        status, 200,
        "the first create: expected HTTP 200, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("the first create must report its volume id: {body}"));

    // The same token with the SAME parameters is the same request: it answers that volume
    // and mints no second one.
    let (status, body) = op(&router, "CreateVolume", &first_volume_params(token)).await;
    assert_eq!(
        status, 200,
        "a replay of the same request: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "volumeId").as_deref(),
        Some(volume_id.as_str()),
        "a replay of the same request: expected the volume {volume_id:?}, got {body}"
    );

    // The rows differ along WHICH member of the request differs: the size, the type, the
    // zone, the encryption state and the tags.
    let other_zone = ec2::state::availability_zones_in_region(REGION)[1]
        .zone_name
        .clone();
    let mismatched: [(&str, Vec<(String, String)>); 5] = [
        (
            "another size",
            vec![
                p("AvailabilityZone", zone()),
                p("Size", "16"),
                p("VolumeType", "gp2"),
                p("ClientToken", token),
            ],
        ),
        (
            "another volume type",
            vec![
                p("AvailabilityZone", zone()),
                p("Size", "8"),
                p("VolumeType", "gp3"),
                p("ClientToken", token),
            ],
        ),
        (
            "another Availability Zone",
            vec![
                p("AvailabilityZone", other_zone),
                p("Size", "8"),
                p("VolumeType", "gp2"),
                p("ClientToken", token),
            ],
        ),
        (
            "an encrypted volume",
            vec![
                p("AvailabilityZone", zone()),
                p("Size", "8"),
                p("VolumeType", "gp2"),
                p("Encrypted", "true"),
                p("ClientToken", token),
            ],
        ),
        (
            "a tag the first request did not carry",
            vec![
                p("AvailabilityZone", zone()),
                p("Size", "8"),
                p("VolumeType", "gp2"),
                p("TagSpecification.1.ResourceType", "volume"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "second"),
                p("ClientToken", token),
            ],
        ),
    ];
    for (case, params) in mismatched {
        let (status, body) = op(&router, "CreateVolume", &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::IDEMPOTENT_PARAMETER_MISMATCH,
            "case {case:?}: expected the code {:?}, got {code:?} in {body}",
            ec2::state::IDEMPOTENT_PARAMETER_MISMATCH
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            ec2::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
            "case {case:?}: expected the message {:?}, got {message:?}",
            ec2::state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE
        );
        assert!(
            element(&body, "volumeId").is_none(),
            "case {case:?}: a refused create reports no volume, got {body}"
        );
    }

    // Nothing the refusals asked for was stored, and the volume the token does name still
    // holds what the FIRST request asked for — read back through the describe.
    let held = stored_volume_ids(&router, "after the mismatches").await;
    assert_eq!(
        held,
        vec![volume_id.clone()],
        "after the mismatches: expected only the volume the token minted, got {held:?}"
    );
    let (status, body) = op(
        &router,
        "DescribeVolumes",
        &[p("VolumeId.1", volume_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the volume back: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "size").as_deref(),
        Some("8"),
        "reading the volume back: expected the size the first request asked for, got {body}"
    );

    // The SAME zone spelled the other way the model admits — by its zone id rather than its
    // name — is not a different request: what the token is compared on is the zone the
    // request RESOLVES to, so this replay answers the volume the token already minted.
    let zone_id = ec2::state::availability_zones_in_region(REGION)[0]
        .zone_id
        .clone();
    let (status, body) = op(
        &router,
        "CreateVolume",
        &[
            p("AvailabilityZoneId", zone_id.clone()),
            p("Size", "8"),
            p("VolumeType", "gp2"),
            p("ClientToken", token),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the same zone named by its id: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "volumeId").as_deref(),
        Some(volume_id.as_str()),
        "the same zone named by its id: expected the volume {volume_id:?}, got {body}"
    );

    // A DIFFERENT token is a different request, whatever it asks for: it mints its own
    // volume rather than being refused as a mismatch.
    let (status, body) = op(
        &router,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone()),
            p("Size", "16"),
            p("ClientToken", "another-token"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "another token: expected HTTP 200, got {status} with body {body}"
    );
    let second = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("another token must mint its own volume: {body}"));
    assert_ne!(
        second, volume_id,
        "another token: expected a volume of its own, got {volume_id:?} again"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a launch reads its AMI id's form before any lookup
// ---------------------------------------------------------------------------

/// An AMI id of the form EC2 issues: `ami-` plus 17 lowercase hexadecimal digits under a
/// leading `0`, not all of them `0`.
const LAUNCHABLE_AMI: &str = "ami-0123456789abcdef0";

/// RULE 4: a value outside the AMI id form EC2 issues is refused `InvalidAMIID.Malformed`
/// before any image is looked up, and a well-formed id launches an instance that reports it.
#[tokio::test]
async fn a_launch_reads_the_ami_id_form_before_any_lookup() {
    let (router, _engine) = emulator();

    // The rows differ along WHAT puts the value outside the form: the long form's leading
    // digit, the null id every client reaches for as a placeholder, the retired 8-digit
    // short form, a non-hexadecimal digit, an upper-case suffix, a suffix one digit too
    // long, another resource kind's id, the prefix alone, and a value with no resource-id
    // prefix at all.
    let malformed = [
        ("a leading digit above zero", "ami-1234567890abcdef0"),
        ("every digit above zero", "ami-fedcba9876543210f"),
        ("the null id", "ami-00000000000000000"),
        ("the retired short form", "ami-0abc1234"),
        ("a non-hexadecimal digit", "ami-0123456789abcdefg"),
        ("an upper-case suffix", "ami-0123456789ABCDEF0"),
        ("a suffix one digit too long", "ami-0123456789abcdef01"),
        ("another resource kind's id", "i-0123456789abcdef0"),
        ("the prefix alone", "ami-"),
        ("no resource-id prefix at all", "not-an-ami"),
    ];
    for (case, image_id) in malformed {
        let params = [
            p("ImageId", image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ];
        let (status, body) = op(&router, "RunInstances", &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} for {image_id:?}, got {status} with \
             body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_AMI_ID_MALFORMED,
            "case {case:?}: expected the code {:?} for {image_id:?}, got {code:?} in {body}",
            ec2::state::INVALID_AMI_ID_MALFORMED
        );
        let expected = format!("Invalid id: \"{image_id}\" (expecting \"ami-...\")");
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        assert!(
            element(&body, "instanceId").is_none(),
            "case {case:?}: a refused launch mints no instance, got {body}"
        );
    }

    // No refusal launched anything.
    let (status, body) = op(&router, "DescribeInstances", &[]).await;
    assert_eq!(
        status, 200,
        "after the refusals: expected HTTP 200, got {status} with body {body}"
    );
    assert!(
        elements(&body, "instanceId").is_empty(),
        "after the refusals: expected no instance to have been launched, got {body}"
    );

    // A well-formed id is read as one and the launch goes on, and the instance it minted
    // reports that id back through the describe.
    let params = [
        p("ImageId", LAUNCHABLE_AMI),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    let (status, body) = op(&router, "RunInstances", &params).await;
    assert_eq!(
        status, 200,
        "a well-formed id: expected HTTP 200, got {status} with body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("a well-formed id must launch an instance: {body}"));

    let (status, body) = op(
        &router,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id.clone())],
    )
    .await;
    assert_eq!(
        status, 200,
        "reading the instance back: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "imageId").as_deref(),
        Some(LAUNCHABLE_AMI),
        "reading the instance back: expected the launch's imageId, got {body}"
    );
}
