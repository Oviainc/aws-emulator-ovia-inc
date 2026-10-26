// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeFpgaImageAttribute`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A DESCRIBE OF ONE ATTRIBUTE REPORTS THAT ATTRIBUTE AND NO OTHER: the result carries
//!    the AFI's own id and the ONE member of `FpgaImageAttribute` the request's `Attribute`
//!    names, read off the stored AFI — so the table below spans the whole published
//!    `FpgaImageAttributeName` catalog and asserts, per row, both the member reported and
//!    the three left out.
//! 2. THE REPORTED VALUE IS THE STORED ONE: a text attribute the AFI holds no value for is
//!    reported as absent rather than invented, a list attribute is reported as the list the
//!    AFI carries (empty or not), and a change another operation makes to that list is what
//!    the next describe reports.
//! 3. A DESCRIBE IS A READ: two describes of the same attribute answer the same value and
//!    the store is as it was.
//! 4. AN AFI ID IS RESOLVED MALFORMED BEFORE NOT FOUND, with `DryRun` answered between
//!    them: something that is not an AFI id at all is `InvalidFpgaImageID.Malformed`, an
//!    authorized `DryRun` is 412 `DryRunOperation` even for an id the account holds nothing
//!    for, and a well-formed id naming no AFI is `InvalidFpgaImageID.NotFound` — the same
//!    refusal, in the same wording, as every other operation that resolves an AFI.
//!
//! Each table row is a distinct input along what the rule generalises over — which
//! attribute of the catalog, whether the AFI holds a value for it, which id shape, which
//! operation resolves the id — so no row is the specification.

use std::collections::BTreeSet;

use ec2::ops::types::{FPGA_IMAGE_ATTRIBUTE_NAME_VALUES, LoadPermission, ProductCode};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};
use super::fpga_image_tasks_and_volume_io::seed_fpga_image;
use super::volume_lifecycle::{REGION, p};
use super::wire::{child, child_text, document, set_items};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a `DryRun` request with: 412, carrying `DryRunOperation`.
const DRY_RUN_STATUS: u16 = 412;

/// A well-formed AFI id, in the long (17 hex digit) form EC2 mints today, that the account
/// holds no AFI for.
const UNKNOWN_FPGA_IMAGE_ID: &str = "afi-0123456789abcdef0";

/// The name of the AFI the attribute table is read off.
const AFI_NAME: &str = "edge-detect-v3";

/// Which member of `FpgaImageAttribute` each `FpgaImageAttributeName` reports, spelled as
/// the ec2Query envelope writes it.
///
/// SOURCE: the model's own shapes — the enum's values and the result structure's members,
///
/// ```text
/// smithy_query(query="shape",shape="FpgaImageAttributeName,FpgaImageAttribute",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
/// ```
///
/// and [`catalog_table_covers_the_published_attribute_names`] pins the left column against
/// the generated catalog, so a model that grows an attribute fails this suite rather than
/// going untested.
const ATTRIBUTE_MEMBERS: [(&str, &str); 4] = [
    ("description", "description"),
    ("name", "name"),
    ("loadPermission", "loadPermissions"),
    ("productCodes", "productCodes"),
];

async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Store one AFI of the calling account, through the one seeding helper this crate's
/// FPGA-image suites share, and answer its id.
///
/// SEEDED rather than created over the wire for the reason that helper records:
/// `CreateFpgaImage` reads the design checkpoint out of Amazon S3 and this emulator holds no
/// Amazon S3 object for any location to name, so it mints no AFI ([`super::create_fpga_image`]
/// owns that rule).
fn seed_afi(engine: &engine::InMemoryEngine, name: &str) -> String {
    seed_fpga_image(engine, name).0
}

/// The stored AFI's `Name` and `Description`, so a row asserts the describe against what the
/// store actually holds rather than against a value written twice.
fn stored_text(engine: &engine::InMemoryEngine, fpga_image_id: &str) -> (Option<String>, Option<String>) {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let record = guard
        .fpga_images
        .get(fpga_image_id)
        .expect("the seeded AFI is stored");
    (record.view.name.clone(), record.view.description.clone())
}

/// Store one AFI the way a mint that was given no `Name` and no `Description` leaves it:
/// those two members hold nothing at all.
fn seed_afi_without_name_or_description(engine: &engine::InMemoryEngine) -> String {
    let fpga_image_id = seed_afi(engine, "unnamed-afi");
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let record = guard
        .fpga_images
        .get_mut(&fpga_image_id)
        .expect("the seeded AFI is stored");
    record.view.name = None;
    record.view.description = None;
    drop(guard);
    fpga_image_id
}

/// Describe `attribute` of `fpga_image_id`.
async fn describe(
    router: &MultiServiceRouter,
    fpga_image_id: &str,
    attribute: &str,
) -> (u16, String) {
    op(
        router,
        "DescribeFpgaImageAttribute",
        &[p("FpgaImageId", fpga_image_id), p("Attribute", attribute)],
    )
    .await
}

/// The one `<fpgaImageAttribute>` a successful response carries, as a document the members
/// are read out of by TREE — so a member of the attribute is never confused with one of the
/// same name nested inside a list entry.
fn attribute_document<'a>(case: &str, body: &'a str) -> roxmltree::Document<'a> {
    document(case, body)
}

/// The text of the attribute's direct child named `member`, `None` when it carries none.
fn attribute_member(case: &str, body: &str, member: &str) -> Option<String> {
    let parsed = attribute_document(case, body);
    let attribute = child(parsed.root_element(), "fpgaImageAttribute")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <fpgaImageAttribute>, got {body}"));
    child_text(attribute, member)
}

/// Whether the attribute carries a direct child named `member` at all.
fn has_attribute_member(case: &str, body: &str, member: &str) -> bool {
    let parsed = attribute_document(case, body);
    let attribute = child(parsed.root_element(), "fpgaImageAttribute")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <fpgaImageAttribute>, got {body}"));
    child(attribute, member).is_some()
}

/// The `<item>` entries of the attribute's `member` set, each as the (first, second) texts
/// of the two members `named` spells.
fn attribute_set(
    case: &str,
    body: &str,
    member: &str,
    named: (&str, &str),
) -> Vec<(Option<String>, Option<String>)> {
    let parsed = attribute_document(case, body);
    let attribute = child(parsed.root_element(), "fpgaImageAttribute")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <fpgaImageAttribute>, got {body}"));
    set_items(attribute, member)
        .into_iter()
        .map(|item| (child_text(item, named.0), child_text(item, named.1)))
        .collect()
}

/// Grant `permissions` to the stored AFI and give it `product_codes`, the way the operations
/// that write them leave the record — `ModifyFpgaImageAttribute` for a load permission and a
/// Marketplace purchase for a product code, neither of which this suite is about.
fn seed_attributes(
    engine: &engine::InMemoryEngine,
    fpga_image_id: &str,
    permissions: Vec<LoadPermission>,
    product_codes: Vec<ProductCode>,
) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let record = guard
        .fpga_images
        .get_mut(fpga_image_id)
        .expect("the AFI the create reported is stored");
    record.load_permissions = permissions;
    record.view.product_codes = Some(product_codes);
}

/// The load permissions the table is read off: one named account and the group that makes an
/// AFI loadable by everyone.
fn seeded_permissions() -> Vec<LoadPermission> {
    vec![
        LoadPermission {
            user_id: Some("210987654321".to_string()),
            group: None,
        },
        LoadPermission {
            user_id: None,
            group: Some("all".to_string()),
        },
    ]
}

/// The product code the table is read off.
fn seeded_product_codes() -> Vec<ProductCode> {
    vec![ProductCode {
        product_code_id: Some("abcdef0123456789abcdef012".to_string()),
        product_code_type: Some("marketplace".to_string()),
    }]
}

/// The left column of [`ATTRIBUTE_MEMBERS`] is the model's published catalog, no more and no
/// less — so the per-attribute table below cannot silently stop covering the surface.
#[test]
fn catalog_table_covers_the_published_attribute_names() {
    let tabled: BTreeSet<&str> = ATTRIBUTE_MEMBERS.iter().map(|(name, _)| *name).collect();
    let published: BTreeSet<&str> = FPGA_IMAGE_ATTRIBUTE_NAME_VALUES.iter().copied().collect();
    assert_eq!(
        tabled, published,
        "the attribute table must cover every published FpgaImageAttributeName, \
         got {tabled:?} against {published:?}"
    );
}

/// RULE 1 and RULE 2: each attribute of the catalog is reported out of the stored AFI, and
/// the three the request did not ask for are left out of the result.
#[tokio::test]
async fn each_attribute_reports_its_own_member_and_no_other() {
    let (router, engine) = emulator();

    let fpga_image_id = seed_afi(&engine, AFI_NAME);
    seed_attributes(
        &engine,
        &fpga_image_id,
        seeded_permissions(),
        seeded_product_codes(),
    );
    let (stored_name, stored_description) = stored_text(&engine, &fpga_image_id);
    assert!(
        stored_name.as_deref().is_some_and(|name| !name.is_empty())
            && stored_description
                .as_deref()
                .is_some_and(|description| !description.is_empty()),
        "the AFI the table is read off holds a name and a description, got \
         {stored_name:?} and {stored_description:?}"
    );

    for (attribute, member) in ATTRIBUTE_MEMBERS {
        let case = format!("the {attribute} attribute");
        let (status, body) = describe(&router, &fpga_image_id, attribute).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            attribute_member(&case, &body, "fpgaImageId"),
            Some(fpga_image_id.clone()),
            "case {case:?}: expected the AFI's own id, got {body}"
        );
        assert!(
            has_attribute_member(&case, &body, member),
            "case {case:?}: expected the <{member}> member to be reported, got {body}"
        );
        for (_, other) in ATTRIBUTE_MEMBERS {
            if other == member {
                continue;
            }
            assert!(
                !has_attribute_member(&case, &body, other),
                "case {case:?}: expected no <{other}> member, got {body}"
            );
        }

        // The value, per attribute, is the one the store holds for it.
        match attribute {
            "description" => assert_eq!(
                attribute_member(&case, &body, member),
                stored_description,
                "case {case:?}: expected the AFI's stored description, got {body}"
            ),
            "name" => assert_eq!(
                attribute_member(&case, &body, member),
                stored_name,
                "case {case:?}: expected the AFI's stored name, got {body}"
            ),
            "loadPermission" => {
                let reported = attribute_set(&case, &body, member, ("userId", "group"));
                let expected: Vec<(Option<String>, Option<String>)> = seeded_permissions()
                    .into_iter()
                    .map(|permission| (permission.user_id, permission.group))
                    .collect();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the stored load permissions, got {reported:?}"
                );
            }
            "productCodes" => {
                let reported = attribute_set(&case, &body, member, ("productCode", "type"));
                let expected: Vec<(Option<String>, Option<String>)> = seeded_product_codes()
                    .into_iter()
                    .map(|code| (code.product_code_id, code.product_code_type))
                    .collect();
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the stored product codes, got {reported:?}"
                );
            }
            other => panic!("case {case:?}: the table carries an untested attribute {other:?}"),
        }
    }
}

/// RULE 2: an AFI stored with no name and no description holds no value for those
/// attributes, and the describe says so — the member is ABSENT rather than carrying a value
/// this emulator invented — while the list attributes of the same AFI are reported as the
/// empty lists it holds.
#[tokio::test]
async fn an_attribute_the_afi_holds_no_value_for_is_reported_as_absent() {
    let (router, engine) = emulator();

    let fpga_image_id = seed_afi_without_name_or_description(&engine);

    // `(case, the attribute, whether the member is reported at all, how many entries)`.
    let rows: [(&str, &str, bool, usize); 4] = [
        ("a description the AFI holds none of", "description", false, 0),
        ("a name the AFI holds none of", "name", false, 0),
        ("the load permissions of a private AFI", "loadPermission", true, 0),
        ("the product codes of a free AFI", "productCodes", true, 0),
    ];

    for (case, attribute, reported, entries) in rows {
        let member = ATTRIBUTE_MEMBERS
            .iter()
            .find(|(name, _)| *name == attribute)
            .map(|(_, member)| *member)
            .expect("every attribute of the rows is in the table");
        let (status, body) = describe(&router, &fpga_image_id, attribute).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            has_attribute_member(case, &body, member),
            reported,
            "case {case:?}: expected the <{member}> member reported={reported}, got {body}"
        );
        let item_count = attribute_set(case, &body, member, ("userId", "productCode")).len();
        assert_eq!(
            item_count, entries,
            "case {case:?}: expected {entries} entries, got {item_count} in {body}"
        );
        // The AFI's own id is reported whichever attribute was asked for.
        assert_eq!(
            attribute_member(case, &body, "fpgaImageId"),
            Some(fpga_image_id.clone()),
            "case {case:?}: expected the AFI's own id, got {body}"
        );
    }
}

/// RULE 2 and RULE 3: the load permissions a describe reports are the ones the store holds
/// at that moment — a reset clears them and the next describe reports none — and describing
/// the same attribute twice answers the same value and leaves the store as it was.
#[tokio::test]
async fn the_reported_attribute_follows_the_store_and_a_describe_changes_nothing() {
    let (router, engine) = emulator();

    let fpga_image_id = seed_afi(&engine, AFI_NAME);
    seed_attributes(
        &engine,
        &fpga_image_id,
        seeded_permissions(),
        seeded_product_codes(),
    );

    let (status, first) = describe(&router, &fpga_image_id, "loadPermission").await;
    assert_eq!(
        status, 200,
        "describing the load permissions: expected HTTP 200, got {status} with body {first}"
    );
    let (_, second) = describe(&router, &fpga_image_id, "loadPermission").await;
    assert_eq!(
        element(&first, "fpgaImageAttribute"),
        element(&second, "fpgaImageAttribute"),
        "two describes of one attribute answer the same value, got {first} then {second}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .fpga_images
            .get(&fpga_image_id)
            .expect("a describe leaves the AFI in the store");
        assert_eq!(
            record.load_permissions.len(),
            seeded_permissions().len(),
            "a describe changes no load permission, got {:?}",
            record.load_permissions
        );
    }

    // What another operation writes is what the next describe reports.
    let (status, body) = op(
        &router,
        "ResetFpgaImageAttribute",
        &[
            p("FpgaImageId", &fpga_image_id),
            p("Attribute", "loadPermission"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "resetting the load permissions: expected HTTP 200, got {status} with body {body}"
    );
    let (status, body) = describe(&router, &fpga_image_id, "loadPermission").await;
    assert_eq!(
        status, 200,
        "describing the reset load permissions: expected HTTP 200, got {status} with {body}"
    );
    let reported = attribute_set("after the reset", &body, "loadPermissions", ("userId", "group"));
    assert!(
        reported.is_empty(),
        "after the reset: expected no load permission, got {reported:?}"
    );
}

/// RULE 4: the id this describe resolves is answered MALFORMED BEFORE NOT FOUND, an
/// authorized `DryRun` is answered between the two, and an attribute outside the published
/// catalog is refused by the modelled constraint.
#[tokio::test]
async fn an_afi_id_and_an_attribute_are_resolved_in_one_order() {
    let (router, _engine) = emulator();

    // `(case, the id, the code it is answered with)`: the ids differ along prefix, suffix
    // length and alphabet, and the two accepted id FORMS (8 and 17 hex digits) are the rows
    // answered as not found.
    let id_rows: [(&str, &str, &str); 7] = [
        (
            "the long id form, which nothing is stored under",
            UNKNOWN_FPGA_IMAGE_ID,
            "InvalidFpgaImageID.NotFound",
        ),
        (
            "the short id form, which nothing is stored under",
            "afi-01234567",
            "InvalidFpgaImageID.NotFound",
        ),
        ("no prefix at all", "0123456789abcdef0", "InvalidFpgaImageID.Malformed"),
        (
            "another resource's prefix",
            "ami-0123456789abcdef0",
            "InvalidFpgaImageID.Malformed",
        ),
        ("the prefix alone", "afi-", "InvalidFpgaImageID.Malformed"),
        (
            "a suffix outside the hexadecimal alphabet",
            "afi-0123456789abcdefg",
            "InvalidFpgaImageID.Malformed",
        ),
        (
            "an uppercase suffix",
            "afi-0123456789ABCDEF0",
            "InvalidFpgaImageID.Malformed",
        ),
    ];

    for (case, fpga_image_id, expected_code) in id_rows {
        // Every attribute of the catalog resolves the id the same way, so the id rows are
        // run through each of them.
        for (attribute, _) in ATTRIBUTE_MEMBERS {
            let (status, body) = describe(&router, fpga_image_id, attribute).await;
            assert_eq!(
                status, 400,
                "case {case:?} for {attribute}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} for {attribute}: expected {expected_code}, got {body}"
            );
        }
    }

    // The wording of the not-found refusal is the ONE this service answers for an AFI,
    // whichever operation resolved the id, and it names the id the REQUEST carried.
    let expected_message = format!("The fpga-image ID '{UNKNOWN_FPGA_IMAGE_ID}' does not exist");
    let resolver_rows: [(&str, Params); 3] = [
        (
            "DescribeFpgaImageAttribute",
            vec![
                p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID),
                p("Attribute", "loadPermission"),
            ],
        ),
        (
            "ResetFpgaImageAttribute",
            vec![
                p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID),
                p("Attribute", "loadPermission"),
            ],
        ),
        (
            "DeleteFpgaImage",
            vec![p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID)],
        ),
    ];
    for (action, params) in resolver_rows {
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {action:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {action:?}: expected {expected_message:?}, got {body}"
        );
    }

    // `DryRun` asks whether the CALLER MAY make the request, so it is answered before the
    // AFI is resolved — including for an id the account holds nothing for.
    let (status, body) = op(
        &router,
        "DescribeFpgaImageAttribute",
        &[
            p("FpgaImageId", UNKNOWN_FPGA_IMAGE_ID),
            p("Attribute", "loadPermission"),
            p("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "a DryRun describe: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a DryRun describe: expected DryRunOperation, got {body}"
    );

    // An attribute outside the published catalog — including the plural misspelling of the
    // load-permission one — is refused by the modelled constraint, with the boundary's own
    // code for a value outside a declared one.
    for attribute in ["loadPermissions", "productCode", "tags"] {
        let (status, body) = describe(&router, UNKNOWN_FPGA_IMAGE_ID, attribute).await;
        assert_eq!(
            status, 400,
            "case {attribute:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {attribute:?}: expected ValidationError, got {body}"
        );
    }
}
