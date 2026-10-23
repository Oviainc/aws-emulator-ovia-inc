// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyVolumeAttribute` and `DescribeVolumeAttribute`, driven over
//! the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with
//! XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that a `VolumeAttributeName` attribute is a property of the
//! volume the store holds: `ModifyVolumeAttribute` sets the attribute its request names on
//! the volume it names, `DescribeVolumeAttribute` reports that attribute — and ONLY that
//! attribute, "you can specify only one attribute at a time" — back for the volume it
//! names, starting from the value AWS gives a volume at creation, and a volume's
//! `productCodes` are the Marketplace codes propagated from the snapshot it was restored
//! from.
//!
//! So the tables below span that class rather than one recorded request: both members of
//! the model's own `VolumeAttributeName` catalog, both boolean values of the one that is a
//! boolean, every modelled `VolumeType` at the bottom of its own published size range,
//! sequences of modifications in different orders, more than one volume at a time, and
//! every refusal the pair makes with the EC2 error code for it.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for: the volume suite's own, so the zone helpers
/// it shares resolve against the Region these calls are signed for.
const REGION: &str = super::volume_lifecycle::REGION;

/// `autoEnableIO`, the one attribute of a volume a caller sets.
const AUTO_ENABLE_IO: &str = ec2::state::VOLUME_ATTRIBUTE_AUTO_ENABLE_IO;

/// `productCodes`, the attribute a volume inherits from its source snapshot.
const PRODUCT_CODES: &str = ec2::state::VOLUME_ATTRIBUTE_PRODUCT_CODES;

/// Every attribute a volume has. Pinned against the model's own `VolumeAttributeName`
/// catalog by [`the_attribute_catalog_is_the_models_own`], so a member the model gains
/// cannot be left untested here.
const ATTRIBUTES: [&str; 2] = [AUTO_ENABLE_IO, PRODUCT_CODES];

/// A volume id of the right shape that no volume of the account has.
const ABSENT_VOLUME: &str = "vol-0000000000000beef";

/// A volume id that is not shaped like one at all.
const MALFORMED_VOLUME: &str = "vol-not-an-id";

/// The error code the centralized request walk answers for an input that breaks a
/// MODELLED constraint — here an `Attribute` outside the `VolumeAttributeName` enum, which
/// is refused before any handler runs.
const VALIDATION_ERROR: &str = "ValidationError";

/// The error code the request walk answers for a form field it cannot read as the member's
/// type — here an `AutoEnableIO.Value` that carries no boolean. ec2Query has no way to
/// send the `AutoEnableIO` structure with its `Value` omitted (a member with no value is a
/// field with an empty value, which is refused here), so the handler's second
/// `MissingParameter` guard is defensive only; this row pins what the wire really answers.
const MALFORMED_INPUT: &str = "MalformedInput";

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `CreateVolume` of `volume_type` at `size` GiB, as the id it minted.
///
/// The zone and the IOPS a type requires come from the volume suite's own helpers
/// ([`super::volume_lifecycle::zone_name`], [`super::volume_lifecycle::required_iops`]),
/// which are signed for the same Region, so there is one definition of each here.
async fn create_volume(
    router: &MultiServiceRouter,
    case: &str,
    volume_type: &str,
    size: i32,
) -> String {
    let mut params = vec![
        param("AvailabilityZone", &super::volume_lifecycle::zone_name(0)),
        param("VolumeType", volume_type),
        param("Size", &size.to_string()),
    ];
    params.extend(super::volume_lifecycle::required_iops(volume_type));
    let (status, body) = call(router, REGION, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume of a {volume_type} volume, \
         got {status} with body {body}"
    );
    child_text(document(case, &body).root_element(), "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: no <volumeId> in the CreateVolume body {body}"))
}

/// One `DescribeVolumeAttribute` call, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeVolumeAttribute", params).await
}

/// One `ModifyVolumeAttribute` call, as its HTTP status and XML body.
async fn modify(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "ModifyVolumeAttribute", params).await
}

/// The form fields of a `ModifyVolumeAttribute` that sets `autoEnableIO` on `volume_id`.
fn modify_params(volume_id: &str, value: bool) -> Vec<Param> {
    vec![
        param("VolumeId", volume_id),
        param("AutoEnableIO.Value", if value { "true" } else { "false" }),
    ]
}

/// The body of a `DescribeVolumeAttribute` for `attribute` of `volume_id`, required to
/// succeed, to name the volume it describes and to carry NO attribute but the one asked
/// for ("you can specify only one attribute at a time").
async fn read_attribute(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    attribute: &str,
) -> String {
    let (status, body) = describe(
        router,
        &[param("VolumeId", volume_id), param("Attribute", attribute)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumeAttribute for {attribute}, \
         got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    assert_eq!(
        child_text(root, "volumeId").as_deref(),
        Some(volume_id),
        "case {case:?}: DescribeVolumeAttribute must name the volume it describes, \
         body {body}"
    );
    for other in ATTRIBUTES.iter().filter(|other| **other != attribute) {
        assert!(
            child(root, other).is_none(),
            "case {case:?}: a DescribeVolumeAttribute for {attribute} must not report \
             {other}, body {body}"
        );
    }
    assert!(
        child(root, attribute).is_some(),
        "case {case:?}: no <{attribute}> in the DescribeVolumeAttribute body {body}"
    );
    body
}

/// The `autoEnableIO` of `volume_id`, as `DescribeVolumeAttribute` reports it.
async fn read_auto_enable_io(router: &MultiServiceRouter, case: &str, volume_id: &str) -> bool {
    let body = read_attribute(router, case, volume_id, AUTO_ENABLE_IO).await;
    let parsed = document(case, &body);
    let attribute = child(parsed.root_element(), AUTO_ENABLE_IO)
        .unwrap_or_else(|| panic!("case {case:?}: no <{AUTO_ENABLE_IO}> in {body}"));
    let value = child_text(attribute, "value")
        .unwrap_or_else(|| panic!("case {case:?}: no <value> inside <{AUTO_ENABLE_IO}> in {body}"));
    match value.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: <{AUTO_ENABLE_IO}> carried {other:?}, not a boolean"),
    }
}

/// The product codes of `volume_id`, as `DescribeVolumeAttribute` reports them: one
/// `(ProductCode, Type)` pair per `<item>` of the `productCodes` list.
async fn read_product_codes(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
) -> Vec<(String, String)> {
    let body = read_attribute(router, case, volume_id, PRODUCT_CODES).await;
    let parsed = document(case, &body);
    let list = child(parsed.root_element(), PRODUCT_CODES)
        .unwrap_or_else(|| panic!("case {case:?}: no <{PRODUCT_CODES}> in {body}"));
    list.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "productCode").unwrap_or_default(),
                child_text(item, "type").unwrap_or_default(),
            )
        })
        .collect()
}

/// Both attributes of `volume_id`, asserted against the `autoEnableIO` the caller expects
/// and against the product codes a volume created empty has: none.
async fn assert_attributes(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    auto_enable_io: bool,
) {
    let actual = read_auto_enable_io(router, case, volume_id).await;
    assert_eq!(
        actual, auto_enable_io,
        "case {case:?}: expected {AUTO_ENABLE_IO} of {volume_id} to be {auto_enable_io}, \
         got {actual:?}"
    );
    // "Any Amazon Web Services Marketplace product codes from the snapshot are propagated
    // to the volume" (`CreateVolume`): these volumes are created empty, so they carry
    // none — and `ModifyVolumeAttribute` cannot add any, so this holds after every
    // modification too. While no path can give a stored volume a source snapshot, the
    // empty list is all the propagation rule can be observed to produce; when a snapshot
    // store lands, this is the assertion to extend with a volume restored from a snapshot
    // that carries codes (`VolumeRecord::product_codes` is the one place that derives
    // them).
    let codes = read_product_codes(router, case, volume_id).await;
    assert!(
        codes.is_empty(),
        "case {case:?}: expected no product codes on the empty volume {volume_id}, \
         got {codes:?}"
    );
}

/// `ModifyVolumeAttribute` setting `autoEnableIO`, required to succeed.
async fn set_auto_enable_io(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    value: bool,
) {
    let (status, body) = modify(router, &modify_params(volume_id, value)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ModifyVolumeAttribute setting \
         {AUTO_ENABLE_IO} to {value}, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// The catalog of attributes
// ---------------------------------------------------------------------------

/// The attribute names this suite and the handlers use are the model's own
/// `VolumeAttributeName` members — no more and no fewer.
#[tokio::test]
async fn the_attribute_catalog_is_the_models_own() {
    let mut used = ATTRIBUTES.to_vec();
    used.sort_unstable();
    let mut modelled = ec2::ops::types::VOLUME_ATTRIBUTE_NAME_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        used, modelled,
        "case \"VolumeAttributeName catalog\": the attributes under test must be the \
         model's own members, got {used:?} against {modelled:?}"
    );
}

// ---------------------------------------------------------------------------
// The creation defaults
// ---------------------------------------------------------------------------

/// The attributes a volume nobody has modified reports: I/O is NOT auto-enabled ("By
/// default, all I/O operations for the volume are suspended when the data on the volume is
/// determined to be potentially inconsistent […] You can change the default behavior",
/// `ModifyVolumeAttribute`), and a volume created empty carries no product codes.
///
/// The rows are every modelled `VolumeType`, at the bottom of that type's own published
/// size range — the model's `CreateVolumeRequest$Size` catalog, as the service holds it —
/// because the attributes are a property of a volume, not of one kind of volume.
///
/// The expected `autoEnableIO` here is the LITERAL `false` the documentation states, not
/// the constant the handlers initialise from, so flipping that constant fails this test
/// instead of moving the expectation with it.
#[tokio::test]
async fn an_unmodified_volume_reports_the_attributes_it_was_created_with() {
    let router = emulator();
    for (volume_type, min_size, _) in ec2::state::VOLUME_SIZE_RANGES {
        let case = format!("a fresh {volume_type} volume of {min_size} GiB");
        let volume_id = create_volume(&router, &case, volume_type, min_size).await;
        assert_attributes(&router, &case, &volume_id, false).await;
    }
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// The rule: `ModifyVolumeAttribute` sets `autoEnableIO` to the value its request carries
/// and `DescribeVolumeAttribute` reports that value back for the volume it names.
///
/// The rows cross both boolean values with volumes of different types and sizes — the
/// whole domain of the attribute, against several kinds of volume — so no row is the
/// specification.
#[tokio::test]
async fn modify_volume_attribute_sets_auto_enable_io_and_describe_reads_it_back() {
    let router = emulator();
    // (case, the volume type, its size in GiB, the value to set)
    let cases = [
        ("auto-enable on, gp3", "gp3", 1, true),
        ("auto-enable off, gp3", "gp3", 1, false),
        ("auto-enable on, gp2 at 100 GiB", "gp2", 100, true),
        ("auto-enable on, provisioned IOPS", "io2", 4, true),
        ("auto-enable off, provisioned IOPS", "io1", 4, false),
        ("auto-enable on, cold HDD", "sc1", 125, true),
        ("auto-enable on, magnetic", "standard", 1_024, true),
    ];

    for (case, volume_type, size, value) in cases {
        let volume_id = create_volume(&router, case, volume_type, size).await;
        assert_attributes(
            &router,
            case,
            &volume_id,
            ec2::state::AUTO_ENABLE_IO_AT_CREATION,
        )
        .await;
        set_auto_enable_io(&router, case, &volume_id, value).await;
        assert_attributes(&router, case, &volume_id, value).await;
    }
}

/// The attribute PERSISTS and belongs to ONE volume: a sequence of modifications leaves
/// each volume at the value its own last modification set, repeating a modification is
/// idempotent, and modifying one volume moves no other.
///
/// The rows are the sequence's steps, each naming which of three volumes it modifies; the
/// expectation after every step is computed from the steps so far rather than written out,
/// so the sequence can be reordered or extended without restating it.
#[tokio::test]
async fn each_modification_moves_only_the_volume_it_names() {
    let router = emulator();
    let volumes = [
        create_volume(&router, "the first volume", "gp3", 1).await,
        create_volume(&router, "the second volume", "gp2", 8).await,
        create_volume(&router, "the third volume", "st1", 125).await,
    ];
    // (case, which volume the step modifies, the value it sets)
    let steps = [
        ("the second volume on", 1, true),
        ("the second volume on again", 1, true),
        ("the first volume on", 0, true),
        ("the second volume back off", 1, false),
        ("the third volume on", 2, true),
        ("the first volume back off", 0, false),
    ];

    let mut expected = [ec2::state::AUTO_ENABLE_IO_AT_CREATION; 3];
    for (case, index, value) in steps {
        set_auto_enable_io(&router, case, &volumes[index], value).await;
        expected[index] = value;
        for (position, volume_id) in volumes.iter().enumerate() {
            let case = format!("{case}: volume {position}");
            assert_attributes(&router, &case, volume_id, expected[position]).await;
        }
    }
}

/// A describe is a READ: two describes in a row report the same attribute, and a describe
/// of one attribute does not move the other.
#[tokio::test]
async fn describing_an_attribute_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let case = "a volume read twice";
    let volume_id = create_volume(&router, case, "gp3", 2).await;
    set_auto_enable_io(&router, case, &volume_id, true).await;
    for round in 0..3 {
        let case = format!("{case}: round {round}");
        assert_attributes(&router, &case, &volume_id, true).await;
    }
}

/// Every path that MINTS a volume starts it at the creation default, and the attribute of
/// the new volume is its own: a copy of a volume whose `autoEnableIO` is on is handed back
/// with the default, and modifying the copy does not move the source.
///
/// The rows are the two values the source can hold when the copy is taken, so the copy's
/// default cannot pass by agreeing with the source.
#[tokio::test]
async fn a_volume_copy_starts_at_the_creation_default_and_is_modified_on_its_own() {
    let router = emulator();
    // (case, the value the source is put at before the copy is taken)
    let cases = [("copied from a source on", true), ("copied from a source off", false)];

    for (case, source_value) in cases {
        let source = create_volume(&router, case, "gp3", 1).await;
        set_auto_enable_io(&router, case, &source, source_value).await;

        let (status, body) = call(
            &router,
            REGION,
            "CopyVolumes",
            &[param("SourceVolumeId", &source)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CopyVolumes, got {status} with body {body}"
        );
        let copy = super::ec2_query::element(&body, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: no <volumeId> in the CopyVolumes body {body}"));
        assert_ne!(
            copy, source,
            "case {case:?}: the copy must be a volume of its own, got the source id {copy}"
        );

        assert_attributes(
            &router,
            &format!("{case}: the copy"),
            &copy,
            ec2::state::AUTO_ENABLE_IO_AT_CREATION,
        )
        .await;
        set_auto_enable_io(&router, case, &copy, true).await;
        assert_attributes(&router, &format!("{case}: the copy, modified"), &copy, true).await;
        assert_attributes(
            &router,
            &format!("{case}: the source, after the copy was modified"),
            &source,
            source_value,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The refusals
// ---------------------------------------------------------------------------

/// Every refusal these two operations make, each with the EC2 error code and HTTP status
/// it answers.
///
/// The rows differ along what makes the request bad: an attribute that is not a
/// `VolumeAttributeName` member, a volume id that is not shaped like one, a well-formed id
/// no volume of the account has — for the read and for the write — a modification that
/// names no attribute at all, and a dry run of each operation.
#[tokio::test]
async fn a_bad_request_is_refused_with_the_ec2_error_code_for_it() {
    let router = emulator();
    let case = "the refusal table's volume";
    let volume_id = create_volume(&router, case, "gp3", 4).await;
    set_auto_enable_io(&router, case, &volume_id, true).await;

    // (case, the action, its form fields, the HTTP status, the EC2 error code)
    //
    // The two rows that name an attribute outside the model's `VolumeAttributeName` enum
    // expect `ValidationError`: a modelled constraint is refused by the centralized
    // request walk (deserialize -> validate -> handle), before the handler runs.
    let cases: [(&str, &str, Vec<Param>, u16, &str); 13] = [
        (
            "an attribute the model does not declare",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", &volume_id),
                param("Attribute", "autoEnableIo"),
            ],
            400,
            VALIDATION_ERROR,
        ),
        (
            "an attribute of another resource",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", &volume_id),
                param("Attribute", ec2::state::VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT),
            ],
            400,
            VALIDATION_ERROR,
        ),
        (
            "a describe of a volume id that is not shaped like one",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", MALFORMED_VOLUME),
                param("Attribute", AUTO_ENABLE_IO),
            ],
            400,
            ec2::state::INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a describe of a volume that does not exist",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", ABSENT_VOLUME),
                param("Attribute", PRODUCT_CODES),
            ],
            400,
            ec2::state::INVALID_VOLUME_ID_NOT_FOUND,
        ),
        (
            "a modification of a volume id that is not shaped like one",
            "ModifyVolumeAttribute",
            modify_params(MALFORMED_VOLUME, true),
            400,
            ec2::state::INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a modification of a volume that does not exist",
            "ModifyVolumeAttribute",
            modify_params(ABSENT_VOLUME, true),
            400,
            ec2::state::INVALID_VOLUME_ID_NOT_FOUND,
        ),
        (
            "a modification that names no attribute at all",
            "ModifyVolumeAttribute",
            vec![param("VolumeId", &volume_id)],
            400,
            ec2::state::MISSING_PARAMETER,
        ),
        (
            "a modification whose attribute carries an empty value",
            "ModifyVolumeAttribute",
            vec![param("VolumeId", &volume_id), param("AutoEnableIO.Value", "")],
            400,
            MALFORMED_INPUT,
        ),
        (
            "a dry run of a describe that would have succeeded",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", &volume_id),
                param("Attribute", AUTO_ENABLE_IO),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        // `DryRun` is the PERMISSION question, so it is answered between the two halves
        // of the volume-id rule, the ordering `EnableVolumeIO` keeps: a request this
        // service cannot read at all is still malformed, while a well-formed id no volume
        // of the account has is answered `DryRunOperation` — the permission answer, before
        // the account's volumes are looked at. These four rows pin that choice for both
        // operations.
        (
            "a dry run of a describe of a volume id that is not shaped like one",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", MALFORMED_VOLUME),
                param("Attribute", AUTO_ENABLE_IO),
                param("DryRun", "true"),
            ],
            400,
            ec2::state::INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a dry run of a describe of a volume that does not exist",
            "DescribeVolumeAttribute",
            vec![
                param("VolumeId", ABSENT_VOLUME),
                param("Attribute", PRODUCT_CODES),
                param("DryRun", "true"),
            ],
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run of a modification of a volume id that is not shaped like one",
            "ModifyVolumeAttribute",
            {
                let mut params = modify_params(MALFORMED_VOLUME, true);
                params.push(param("DryRun", "true"));
                params
            },
            400,
            ec2::state::INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a dry run of a modification of a volume that does not exist",
            "ModifyVolumeAttribute",
            {
                let mut params = modify_params(ABSENT_VOLUME, true);
                params.push(param("DryRun", "true"));
                params
            },
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
    ];

    for (case, action, params, status_expected, code_expected) in cases {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected HTTP {status_expected} from {action}, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, code_expected,
            "case {case:?}: expected error code {code_expected:?} from {action}, got \
             {code:?} with body {body}"
        );
    }

    // None of those refusals wrote anything: the volume they named still reports the
    // attribute its one successful modification set.
    assert_attributes(&router, "after every refusal", &volume_id, true).await;
}

/// A dry-run modification that WOULD have succeeded is refused and writes nothing: the
/// volume keeps the attribute it had.
///
/// The rows are both values a dry run could have set, against both values the volume could
/// have been at — so the row that "sets" what is already there cannot pass by accident.
#[tokio::test]
async fn a_dry_run_modification_refuses_and_writes_nothing() {
    let router = emulator();
    // (case, the value the volume is put at first, the value the dry run would set)
    let cases = [
        ("off, dry-run on", false, true),
        ("on, dry-run off", true, false),
        ("on, dry-run on", true, true),
        ("off, dry-run off", false, false),
    ];

    for (case, held, attempted) in cases {
        let volume_id = create_volume(&router, case, "gp3", 1).await;
        set_auto_enable_io(&router, case, &volume_id, held).await;

        let mut params = modify_params(&volume_id, attempted);
        params.push(param("DryRun", "true"));
        let (status, body) = modify(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a dry-run ModifyVolumeAttribute, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected error code {:?} from a dry-run \
             ModifyVolumeAttribute, got {code:?} with body {body}",
            ec2::state::DRY_RUN_OPERATION
        );
        assert_attributes(&router, case, &volume_id, held).await;
    }
}
