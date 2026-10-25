// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CopyFpgaImage`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A SOURCE AFI THE COPY CANNOT RESOLVE IS REFUSED WITH ONE ANSWER: 400
//!    `InvalidFpgaImageID.Malformed` naming the id, whether that id is not an AFI id at all
//!    or is well formed and names no AFI — the copy's subject lives in the Region
//!    `SourceRegion` names rather than in the Region the call is made in, so it validates
//!    the id instead of resolving a resource of its own Region. The operations that DO
//!    resolve an AFI of their own Region keep the malformed-before-not-found pair, which
//!    the same id table is run through to pin the contrast.
//! 2. `SourceRegion` HAS TO NAME A REGION of the account's partition — "the Region that
//!    contains the source AFI" — so a value that names none is refused with
//!    `InvalidParameterValue` naming the parameter.
//! 3. A COPY IS A NEW AFI COMPUTED FROM THE STORED SOURCE: its own ids, the name the
//!    request gave it or the source's name as the default, the bitstream facts the source
//!    reports, no load permission and no tag; it is handed back `pending` and its copy then
//!    ends `failed` on the clock, because no AFI of this store carries an FPGA bitstream,
//!    so there is nothing for a copy to produce.
//! 4. `DryRun` ASKS WHETHER THE CALLER MAY MAKE THE REQUEST, so it is answered 412
//!    `DryRunOperation` once the request's own members have been read — including for a
//!    source the store holds nothing for — and writes nothing.
//! 5. ONE `ClientToken` NAMES ONE AFI of the account: a replay reports the AFI that token
//!    minted and mints nothing more, whichever call minted it.
//!
//! Each table row is a distinct input along what the rule generalises over — which id shape,
//! which Region, which name/description combination, which token length — so no row is the
//! specification.

use std::time::Instant;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FPGA_IMAGE_STATE_FAILED, FPGA_IMAGE_STATE_PENDING,
    MAX_CLIENT_TOKEN_CHARS, regions_in_partition_of,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};
use super::volume_lifecycle::{REGION, p, past_the_transition};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a `DryRun` request with: 412, carrying `DryRunOperation`.
const DRY_RUN_STATUS: u16 = 412;

/// The one answer `CopyFpgaImage` has for a source AFI it cannot resolve.
const UNRESOLVABLE_SOURCE_CODE: &str = "InvalidFpgaImageID.Malformed";

/// A well-formed AFI id, in the long (17 hex digit) form EC2 mints today, that the account
/// holds no AFI for.
const UNKNOWN_FPGA_IMAGE_ID: &str = "afi-0123456789abcdef0";

/// A bucket name inside every one of Amazon S3's general-purpose naming rules, so a
/// `CreateFpgaImage` that uses it is refused for nothing.
const READABLE_BUCKET: &str = "my-design-checkpoints";

async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Create one AFI named `name`, and answer its id.
async fn create_fpga_image(router: &MultiServiceRouter, name: &str) -> String {
    let params = vec![
        p("InputStorageLocation.Bucket", READABLE_BUCKET),
        p("InputStorageLocation.Key", "dcp/checkpoint.tar"),
        p("Name", name),
        p("Description", format!("the source AFI {name}")),
    ];
    let (status, body) = op(router, "CreateFpgaImage", &params).await;
    assert_eq!(
        status, 200,
        "creating the source AFI: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "fpgaImageId").unwrap_or_default()
}

/// The parameters of a copy of `source_fpga_image_id` made from the Region the suite calls
/// in, with nothing else set.
fn copy_params(source_fpga_image_id: &str) -> Params {
    vec![
        p("SourceFpgaImageId", source_fpga_image_id),
        p("SourceRegion", REGION),
    ]
}

/// How many AFIs the store holds.
fn stored_afi_count(engine: &engine::InMemoryEngine) -> usize {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.fpga_images.len()
}

// ---------------------------------------------------------------------------
// RULE 1: a source the copy cannot resolve
// ---------------------------------------------------------------------------

/// `(case, the source id, the code the operations that resolve an AFI of their own Region
/// answer it with)`. The ids differ along prefix, suffix length and alphabet, and the two
/// accepted id FORMS (8 and 17 hex digits) are the rows those operations answer as not
/// found — `CopyFpgaImage` answers every row alike.
fn unresolvable_source_rows() -> [(&'static str, &'static str, &'static str); 9] {
    [
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
        ("no prefix at all", "0123456789abcdef0", UNRESOLVABLE_SOURCE_CODE),
        (
            "another resource's prefix",
            "ami-0123456789abcdef0",
            UNRESOLVABLE_SOURCE_CODE,
        ),
        ("the prefix alone", "afi-", UNRESOLVABLE_SOURCE_CODE),
        ("a suffix that is too short", "afi-0123456", UNRESOLVABLE_SOURCE_CODE),
        (
            "a suffix that is too long",
            "afi-0123456789abcdef01",
            UNRESOLVABLE_SOURCE_CODE,
        ),
        (
            "a suffix outside the hexadecimal alphabet",
            "afi-0123456789abcdefg",
            UNRESOLVABLE_SOURCE_CODE,
        ),
        (
            "an uppercase suffix",
            "afi-0123456789ABCDEF0",
            UNRESOLVABLE_SOURCE_CODE,
        ),
    ]
}

/// Every source id `CopyFpgaImage` cannot resolve is refused with the ONE answer the copy
/// path has for one — 400 `InvalidFpgaImageID.Malformed` naming that id — and nothing is
/// stored by a refusal.
#[tokio::test]
async fn a_source_the_copy_cannot_resolve_is_refused_with_one_answer() {
    let (router, engine) = emulator();

    for (case, source_fpga_image_id, _) in unresolvable_source_rows() {
        let (status, body) = op(&router, "CopyFpgaImage", &copy_params(source_fpga_image_id)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            UNRESOLVABLE_SOURCE_CODE,
            "case {case:?}: expected {UNRESOLVABLE_SOURCE_CODE}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!("Image ID '{source_fpga_image_id}' is invalid."),
            "case {case:?}: the refusal names the id it could not resolve, got {message:?}"
        );
    }

    assert_eq!(
        stored_afi_count(&engine),
        0,
        "a refused copy stores no AFI, got {} stored",
        stored_afi_count(&engine)
    );
}

/// The copy's one answer is the copy's own: the operations that resolve an AFI of the Region
/// they are called in still answer the same id table malformed BEFORE not found.
#[tokio::test]
async fn resolving_an_afi_of_this_region_still_answers_malformed_before_not_found() {
    let (router, _engine) = emulator();

    for (case, fpga_image_id, expected_code) in unresolvable_source_rows() {
        for action in ["DeleteFpgaImage", "ResetFpgaImageAttribute"] {
            let mut params = vec![p("FpgaImageId", fpga_image_id)];
            if action == "ResetFpgaImageAttribute" {
                params.push(p("Attribute", "loadPermission"));
            }
            let (status, body) = op(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on {action}: expected {expected_code}, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 2: SourceRegion has to name a Region
// ---------------------------------------------------------------------------

/// A `SourceRegion` that names a Region of the account's partition is copied from, and one
/// that names no Region is refused — whichever way it fails to be a Region code.
#[tokio::test]
async fn a_copy_source_region_has_to_name_a_region() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "source-afi").await;

    // The accepted rows are Regions of the account's partition, which the catalog
    // `ec2::state::regions_in_partition_of` derives, plus the Region the call is signed for;
    // the asserts below hold each row to that catalog rather than to a hand-written list.
    let accepted: [(&str, &str); 4] = [
        ("the Region the call is signed for", REGION),
        ("another Region of the partition", "us-east-1"),
        ("a Region in another geography", "eu-central-1"),
        ("a Region whose code has three words", "ap-northeast-3"),
    ];
    let catalog = regions_in_partition_of(REGION);
    for (case, source_region) in accepted {
        assert!(
            catalog.contains(&source_region),
            "case {case:?}: {source_region} has to be in the partition's Region catalog, \
             got {catalog:?}"
        );
        let params = vec![
            p("SourceFpgaImageId", &source),
            p("SourceRegion", source_region),
        ];
        let (status, body) = op(&router, "CopyFpgaImage", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            element(&body, "fpgaImageId").is_some_and(|id| id != source),
            "case {case:?}: the copy is a new AFI, got {body}"
        );
    }

    let refused: [(&str, &str); 6] = [
        ("no Region at all", ""),
        ("a Region code with no number", "us-east"),
        ("words that are not a Region code", "not-a-region"),
        ("an uppercase Region code", "US-EAST-1"),
        ("an Availability Zone rather than a Region", "us-west-2a"),
        ("a Region code of no Region AWS has", "xx-central-1"),
    ];
    for (case, source_region) in refused {
        assert!(
            !catalog.contains(&source_region),
            "case {case:?}: {source_region} must not be in the Region catalog"
        );
        let params = vec![
            p("SourceFpgaImageId", &source),
            p("SourceRegion", source_region),
        ];
        let (status, body) = op(&router, "CopyFpgaImage", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        // The whole sentence, not just the parameter name: the refusal is EC2's
        // parameter-validation form, carrying the value the request sent and the member's
        // own name, in the wording this crate gives every parameter that has to name a
        // Region (`create_ipam`'s `operatingRegion` refusal is the same sentence).
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!(
                "Value ({source_region}) for parameter sourceRegion is invalid. \
                 This is not a valid Region name."
            ),
            "case {case:?}: the refusal names the parameter and the value, got {message:?}"
        );
    }

    // Four copies were accepted, one per accepted Region, and no refused row copied
    // anything — so the store holds those four beside the source.
    assert_eq!(
        stored_afi_count(&engine),
        5,
        "only the accepted rows copy anything, got {} AFIs stored",
        stored_afi_count(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 3: the copy is a new AFI computed from the stored source
// ---------------------------------------------------------------------------

/// One row of the naming table: `(case, the `Name` the request carries, the `Description` it
/// carries, the name the copy must end up with)`.
type NamingRow = (
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

/// The copy is its own AFI: its own ids, the name the request gave it or the SOURCE's name
/// as the documented default, the description the request gave it, no load permission and no
/// tag — and the source is left exactly as it was.
#[tokio::test]
async fn a_copy_is_a_new_afi_named_after_the_request_or_the_source() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "golden-bitstream").await;

    // A load permission, a tag and the bitstream facts of a REAL AFI on the SOURCE, all of
    // them different from what a freshly created AFI carries — so "the copy inherits the
    // image's facts and none of the source's permissions or tags" is asserted against
    // values the handler cannot have defaulted to.
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let record = guard
            .fpga_images
            .get_mut(&source)
            .expect("the source AFI is stored");
        record.load_permissions = vec![ec2::ops::types::LoadPermission {
            user_id: Some("210987654321".to_string()),
            group: None,
        }];
        record.view.tags = Some(vec![ec2::ops::types::Tag {
            key: Some("project".to_string()),
            value: Some("fpga".to_string()),
        }]);
        record.view.shell_version = Some("0x04261818".to_string());
        record.view.pci_id = Some(ec2::ops::types::PciId {
            device_id: Some("0x1042".to_string()),
            vendor_id: Some("0x1d0f".to_string()),
            subsystem_id: Some("0x1042".to_string()),
            subsystem_vendor_id: Some("0xfedd".to_string()),
        });
        record.view.instance_types = Some(vec!["f1.2xlarge".to_string(), "f1.4xlarge".to_string()]);
        record.view.data_retention_support = Some(true);
        record.view.product_codes = Some(vec![ec2::ops::types::ProductCode {
            product_code_id: Some("a1b2c3d4e5f6g7h8i9j0k1l2".to_string()),
            product_code_type: Some("marketplace".to_string()),
        }]);
    }

    // `(case, the Name the request carries, the Description it carries, the name the copy
    // must end up with)`: the default is the source's own name.
    let rows: [NamingRow; 4] = [
        (
            "a name and a description of its own",
            Some("copy-one"),
            Some("the first copy"),
            Some("copy-one"),
        ),
        (
            "a name but no description",
            Some("copy-two"),
            None,
            Some("copy-two"),
        ),
        (
            "no name, so the source's name is the default",
            None,
            Some("the third copy"),
            Some("golden-bitstream"),
        ),
        ("neither", None, None, Some("golden-bitstream")),
    ];

    for (case, name, description, expected_name) in rows {
        let mut params = copy_params(&source);
        if let Some(name) = name {
            params.push(p("Name", name));
        }
        if let Some(description) = description {
            params.push(p("Description", description));
        }
        let (status, body) = op(&router, "CopyFpgaImage", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let copy = element(&body, "fpgaImageId").unwrap_or_default();
        assert_ne!(
            copy, source,
            "case {case:?}: the copy is a new AFI, not the source again"
        );

        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard
            .fpga_images
            .get(&copy)
            .unwrap_or_else(|| panic!("case {case:?}: the copy the call reported is stored"));
        assert_eq!(
            record.view.name.as_deref(),
            expected_name,
            "case {case:?}: expected the copy to be named {expected_name:?}, got {:?}",
            record.view.name
        );
        assert_eq!(
            record.view.description.as_deref(),
            description,
            "case {case:?}: the copy carries the description the request gave it, got {:?}",
            record.view.description
        );
        assert_eq!(
            record.view.fpga_image_id.as_deref(),
            Some(copy.as_str()),
            "case {case:?}: the stored id is the one the call reported, got {record:?}"
        );
        let global_id = record.view.fpga_image_global_id.clone().unwrap_or_default();
        assert!(
            global_id.starts_with("agfi-") && global_id != copy,
            "case {case:?}: the copy has a global id of its own, got {global_id:?}"
        );
        assert_eq!(
            record.view.owner_id.as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the copy belongs to the account that asked for it, got {record:?}"
        );
        assert_eq!(
            record.view.public,
            Some(false),
            "case {case:?}: a copy is private, got {record:?}"
        );
        assert!(
            record.load_permissions.is_empty(),
            "case {case:?}: the copy inherits no load permission of the source, got {:?}",
            record.load_permissions
        );
        assert!(
            record.view.tags.as_deref().is_some_and(<[_]>::is_empty),
            "case {case:?}: the copy carries no tag of the source, got {:?}",
            record.view.tags
        );
        // The facts of the IMAGE being copied are the source's, read back out of the stored
        // source — which carries values no freshly created AFI has, so an implementation
        // that reported defaults instead of inheriting them would fail here.
        let stored_source = guard
            .fpga_images
            .get(&source)
            .expect("the source AFI is still stored");
        assert_eq!(
            record.view.shell_version.as_deref(),
            stored_source.view.shell_version.as_deref(),
            "case {case:?}: the copy reports the source's shell version, got {:?}",
            record.view.shell_version
        );
        assert_eq!(
            record.view.instance_types.as_deref(),
            stored_source.view.instance_types.as_deref(),
            "case {case:?}: the copy reports the source's instance types, got {:?}",
            record.view.instance_types
        );
        assert_eq!(
            record.view.data_retention_support,
            stored_source.view.data_retention_support,
            "case {case:?}: the copy reports the source's data retention support, got {:?}",
            record.view.data_retention_support
        );
        assert_eq!(
            record
                .view
                .pci_id
                .as_ref()
                .and_then(|pci_id| pci_id.device_id.clone()),
            stored_source
                .view
                .pci_id
                .as_ref()
                .and_then(|pci_id| pci_id.device_id.clone()),
            "case {case:?}: the copy reports the source's PCI ids, got {:?}",
            record.view.pci_id
        );
        assert_eq!(
            record
                .view
                .product_codes
                .as_deref()
                .map(|codes| codes
                    .iter()
                    .map(|code| code.product_code_id.clone())
                    .collect::<Vec<_>>()),
            stored_source
                .view
                .product_codes
                .as_deref()
                .map(|codes| codes
                    .iter()
                    .map(|code| code.product_code_id.clone())
                    .collect::<Vec<_>>()),
            "case {case:?}: the copy reports the source's product codes, got {:?}",
            record.view.product_codes
        );
        assert_eq!(
            stored_source.view.name.as_deref(),
            Some("golden-bitstream"),
            "case {case:?}: the source is left as it was, got {stored_source:?}"
        );
        assert_eq!(
            stored_source.load_permissions.len(),
            1,
            "case {case:?}: the copy does not touch the source's load permissions, got {:?}",
            stored_source.load_permissions
        );
    }
}

/// A copy is handed back `pending` and its copy then ends `failed` on the clock, with a
/// message naming the source it could not copy a bitstream from — no AFI of this store
/// carries one.
#[tokio::test]
async fn a_copy_is_pending_and_then_fails_on_the_clock() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "source-afi").await;

    let (status, body) = op(&router, "CopyFpgaImage", &copy_params(&source)).await;
    assert_eq!(
        status, 200,
        "copying an AFI: expected HTTP 200, got {status} with body {body}"
    );
    let copy = element(&body, "fpgaImageId").unwrap_or_default();

    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let state = guard
            .fpga_images
            .get(&copy)
            .expect("the copy is stored")
            .state_at(Instant::now());
        assert_eq!(
            state.code.as_deref(),
            Some(FPGA_IMAGE_STATE_PENDING),
            "straight after the call the copy is {FPGA_IMAGE_STATE_PENDING}, got {state:?}"
        );
        assert_eq!(
            state.message, None,
            "a {FPGA_IMAGE_STATE_PENDING} copy carries no outcome message yet, got {state:?}"
        );
    }

    past_the_transition().await;

    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        let record = guard.fpga_images.get(&copy).expect("the copy is still stored");
        let state = record.state_at(Instant::now());
        assert_eq!(
            state.code.as_deref(),
            Some(FPGA_IMAGE_STATE_FAILED),
            "once the copy falls due it is {FPGA_IMAGE_STATE_FAILED}, got {state:?}"
        );
        let message = state.message.unwrap_or_default();
        assert!(
            message.contains(&source) && message.contains(REGION),
            "the failed copy says which source in which Region it could not copy, \
             got {message:?}"
        );
    }

    // The copy round-trips through the operations that change an AFI: it is the AFI a delete
    // removes, the delete leaves the source alone, and a second delete of it is refused.
    let (status, body) = op(&router, "DeleteFpgaImage", &[p("FpgaImageId", &copy)]).await;
    assert_eq!(
        status, 200,
        "deleting the copy: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "deleting the copy reports true, got {body}"
    );
    {
        let store = engine.state::<Ec2State>();
        let guard = store.lock();
        assert!(
            !guard.fpga_images.contains_key(&copy) && guard.fpga_images.contains_key(&source),
            "the delete takes the copy out of the store and leaves the source, got {:?}",
            guard.fpga_images.keys().collect::<Vec<_>>()
        );
    }
    let (status, body) = op(&router, "DeleteFpgaImage", &[p("FpgaImageId", &copy)]).await;
    assert_eq!(
        status, 400,
        "deleting the copy twice: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFpgaImageID.NotFound",
        "a copy that is gone is no longer resolved, got {body}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: DryRun
// ---------------------------------------------------------------------------

/// `DryRun` is answered once the request's own members have been read: a source the store
/// holds nothing for is still 412, because the flag asks whether the CALLER MAY make the
/// request — while a source id that is not an AFI id at all is a fact of the request itself
/// and so is refused ahead of the flag. Either way nothing is written.
#[tokio::test]
async fn a_dry_run_copy_is_answered_before_the_source_is_resolved_and_writes_nothing() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "source-afi").await;

    // `(case, the source id, the status, the code)`.
    let rows: [(&str, &str, u16, &str); 3] = [
        (
            "a source the store holds",
            &source,
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
        (
            "a well-formed source the store holds nothing for",
            UNKNOWN_FPGA_IMAGE_ID,
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
        (
            "a source that is not an AFI id at all",
            "not-an-afi-id",
            400,
            UNRESOLVABLE_SOURCE_CODE,
        ),
    ];

    for (case, source_fpga_image_id, expected_status, expected_code) in rows {
        let mut params = copy_params(source_fpga_image_id);
        params.push(p("DryRun", "true"));
        let (status, body) = op(&router, "CopyFpgaImage", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    assert_eq!(
        stored_afi_count(&engine),
        1,
        "a DryRun copies nothing, so only the source is stored, got {} stored",
        stored_afi_count(&engine)
    );
}

// ---------------------------------------------------------------------------
// RULE 5: one ClientToken names one copy
// ---------------------------------------------------------------------------

/// A replayed `ClientToken` reports the AFI it minted and copies nothing more, a fresh token
/// copies again, and a token longer than the accepted maximum is refused.
#[tokio::test]
async fn one_client_token_names_one_copy() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "source-afi").await;

    // The token lengths differ along the one bound there is: one character, the maximum
    // `ec2::state::MAX_CLIENT_TOKEN_CHARS` accepts, and one character past it.
    let accepted = ["t", &"c".repeat(MAX_CLIENT_TOKEN_CHARS)];
    let mut minted = Vec::new();
    for token in accepted {
        let mut first_ids = Vec::new();
        for attempt in 0..2 {
            let mut params = copy_params(&source);
            params.push(p("ClientToken", token));
            let (status, body) = op(&router, "CopyFpgaImage", &params).await;
            assert_eq!(
                status, 200,
                "token of {} chars, attempt {attempt}: expected HTTP 200, got {status} \
                 with body {body}",
                token.chars().count()
            );
            first_ids.push(element(&body, "fpgaImageId").unwrap_or_default());
        }
        assert_eq!(
            first_ids[0], first_ids[1],
            "a replayed ClientToken names the AFI it minted, got {first_ids:?}"
        );
        minted.push(first_ids[0].clone());
    }
    assert_ne!(
        minted[0], minted[1],
        "two different tokens name two different copies, got {minted:?}"
    );
    assert_eq!(
        stored_afi_count(&engine),
        3,
        "two tokens copied twice leave the source and two copies, got {} stored",
        stored_afi_count(&engine)
    );

    let mut params = copy_params(&source);
    params.push(p("ClientToken", "c".repeat(MAX_CLIENT_TOKEN_CHARS + 1)));
    let (status, body) = op(&router, "CopyFpgaImage", &params).await;
    assert_eq!(
        status, 400,
        "a token one character past the maximum: expected HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a token past the maximum is refused, got {body}"
    );
}

/// One `ClientToken` names ONE AFI of the account, whichever call minted it: a token a
/// `CreateFpgaImage` already used names the AFI that create minted, so a `CopyFpgaImage`
/// carrying it reports that AFI and copies nothing — the two calls share the one token map
/// (`ec2::state::ComAmazonawsEc2Amazonec2State::fpga_image_client_tokens`) because they mint
/// into the one AFI space.
#[tokio::test]
async fn a_token_names_one_afi_whichever_call_minted_it() {
    let (router, engine) = emulator();
    let source = create_fpga_image(&router, "source-afi").await;

    let token = "shared-token";
    let created = {
        let params = vec![
            p("InputStorageLocation.Bucket", READABLE_BUCKET),
            p("InputStorageLocation.Key", "dcp/checkpoint.tar"),
            p("ClientToken", token),
        ];
        let (status, body) = op(&router, "CreateFpgaImage", &params).await;
        assert_eq!(
            status, 200,
            "creating with a token: expected HTTP 200, got {status} with body {body}"
        );
        element(&body, "fpgaImageId").unwrap_or_default()
    };
    let stored_before = stored_afi_count(&engine);

    let mut params = copy_params(&source);
    params.push(p("ClientToken", token));
    let (status, body) = op(&router, "CopyFpgaImage", &params).await;
    assert_eq!(
        status, 200,
        "copying with a token the create used: expected HTTP 200, got {status} \
         with body {body}"
    );
    assert_eq!(
        element(&body, "fpgaImageId").as_deref(),
        Some(created.as_str()),
        "the token names the AFI it minted, got {body}"
    );
    assert_eq!(
        stored_afi_count(&engine),
        stored_before,
        "a replayed token mints nothing, got {} AFIs stored where there were {stored_before}",
        stored_afi_count(&engine)
    );
}
