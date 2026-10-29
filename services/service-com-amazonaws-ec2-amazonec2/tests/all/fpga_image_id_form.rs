// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ID FORM an Amazon FPGA Image id is read in, driven over the wire
//! the service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test: an operation that takes an AFI id reads that id, BEFORE any lookup,
//! as one EC2 could have ISSUED — the `afi-` prefix followed by 8 or 17 lowercase
//! hexadecimal digits, of which not every digit is `0` — and refuses anything else with HTTP
//! 400 `InvalidFpgaImageID.Malformed` and the FPGA-image kind's own sentence,
//! `Image ID '<value>' is invalid.`, built from the value the REQUEST sent; an id inside that
//! form which the account holds no AFI under is the DIFFERENT answer
//! `InvalidFpgaImageID.NotFound`, so a client can tell a typo from a missing image. The null
//! (all-zero) id is outside the form because the mint counts from one, so no AFI can ever
//! hold it.
//!
//! Each row is a distinct input along what the rule generalises over:
//!
//! * the WAY the value falls outside the form — the prefix alone, the prefix without its
//!   separator, no prefix at all, another resource kind's prefix, a suffix of neither
//!   published length (one digit short and one digit long), a non-hexadecimal digit, an
//!   upper-case suffix, and the null id in EACH of the two published lengths;
//! * the SHAPE of the value — one carrying the characters that are significant in the XML
//!   envelope the refusal is reported back inside, and one far longer than any id EC2
//!   issues, which the message names the leading
//!   [`ec2::state::MAX_ECHOED_RESOURCE_ID_CHARS`] characters of;
//! * the OPERATION that resolves the id — a delete, a reset, an attribute describe and a
//!   copy, so no one operation's wording is the specification;
//! * which SIDE of the form the id is on — the rows that are inside it (both published
//!   lengths, and a leading digit above zero, which is a clause of some OTHER id kinds and
//!   not of this one) are answered as not found rather than as malformed.
//!
//! The round-trip at the end holds the form to the resource: the AFI a delete removes is the
//! one a stored id names, and the id that named it is answered as not found once it is gone.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INVALID_FPGA_IMAGE_ID_MALFORMED,
    INVALID_FPGA_IMAGE_ID_NOT_FOUND, MAX_ECHOED_RESOURCE_ID_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, emulator, error_code, error_message};
use super::fpga_image_tasks_and_volume_io::seed_fpga_image;
use super::volume_lifecycle::{REGION, p};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The Region every call here is signed for. Nothing in the rule depends on it: an AFI id is
/// read for its form before anything regional is resolved.
const SOURCE_REGION: &str = REGION;

/// The parameters each operation that resolves an AFI id is called with, for `fpga_image_id`.
///
/// `CopyFpgaImage` is in the list because it takes an AFI id too — it reads the id of its
/// SOURCE — and it is the one operation here whose subject does not live in the Region the
/// call is made in, so it answers every id it cannot resolve with the malformed answer. The
/// other three resolve an AFI of their own Region and so keep the two answers apart.
fn resolver_params(action: &str, fpga_image_id: &str) -> Params {
    match action {
        "ResetFpgaImageAttribute" | "DescribeFpgaImageAttribute" => vec![
            p("FpgaImageId", fpga_image_id),
            p("Attribute", "loadPermission"),
        ],
        "CopyFpgaImage" => vec![
            p("SourceFpgaImageId", fpga_image_id),
            p("SourceRegion", SOURCE_REGION),
        ],
        _ => vec![p("FpgaImageId", fpga_image_id)],
    }
}

/// The operations that resolve an AFI id of the Region they are called in: each keeps the
/// malformed answer apart from the not-found one.
const REGIONAL_RESOLVERS: [&str; 3] = [
    "DeleteFpgaImage",
    "ResetFpgaImageAttribute",
    "DescribeFpgaImageAttribute",
];

/// One `action` call signed for the Region the suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &Params) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// How many AFIs the account holds right now.
fn stored_afi_count(engine: &engine::InMemoryEngine) -> usize {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.fpga_images.len()
}

/// `(case, the value)`: every row is outside the form EC2 issues an AFI id in, each for a
/// different reason.
fn malformed_rows() -> Vec<(&'static str, String)> {
    vec![
        ("the prefix alone", "afi-".to_string()),
        (
            "the prefix without its separator",
            "afi0a1b2c3d".to_string(),
        ),
        ("no prefix at all", "0123456789abcdef0".to_string()),
        (
            "another resource kind's prefix",
            "ami-0123456789abcdef0".to_string(),
        ),
        (
            "a suffix one digit short of the long form",
            "afi-0123456789abcdef".to_string(),
        ),
        (
            "a suffix one digit longer than the long form",
            "afi-0123456789abcdef01".to_string(),
        ),
        (
            "a suffix outside the hexadecimal alphabet",
            "afi-0123456789abcdefg".to_string(),
        ),
        (
            "an upper-case suffix",
            "afi-0123456789ABCDEF0".to_string(),
        ),
        (
            // The placeholder every client reaches for: inside the long form's length and
            // alphabet, outside the ids the mint can produce.
            "the null id in the long form",
            format!("afi-{}", "0".repeat(17)),
        ),
        (
            "the null id in the short form",
            format!("afi-{}", "0".repeat(8)),
        ),
        (
            // The value is reported back inside an XML envelope, so one carrying the
            // characters that are significant there is a shape the rule has to hold for.
            "a value carrying XML metacharacters",
            "afi-<&'\"0a1b2c3d".to_string(),
        ),
        (
            // Far longer than any id EC2 issues: the refusal names the leading
            // `MAX_ECHOED_RESOURCE_ID_CHARS` characters, so it cannot be made to carry an
            // unbounded value the caller chose.
            "a value longer than the message repeats back",
            format!("afi-{}", "a".repeat(MAX_ECHOED_RESOURCE_ID_CHARS * 2)),
        ),
    ]
}

/// `(case, the value)`: every row is INSIDE the form EC2 issues an AFI id in, and the account
/// holds no AFI under any of them.
fn unknown_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "the long id form, which nothing is stored under",
            "afi-0123456789abcdef0".to_string(),
        ),
        (
            "the short id form, which nothing is stored under",
            "afi-01234567".to_string(),
        ),
        (
            // A leading digit above zero puts an INSTANCE id outside the form EC2 issues;
            // that clause is the instance id's own, so an AFI id carrying one is read as an
            // id of this kind and answered as missing rather than as malformed.
            "a long-form id whose leading digit is above zero",
            "afi-1234567890abcdef0".to_string(),
        ),
    ]
}

/// RULE: a value outside the form EC2 issues an AFI id in is refused 400
/// `InvalidFpgaImageID.Malformed` with `Image ID '<value>' is invalid.`, from every operation
/// that resolves an AFI id, and nothing is written by the refusal.
#[tokio::test]
async fn an_afi_id_outside_the_issued_form_is_refused_as_malformed() {
    let (router, engine) = emulator();

    // A stored AFI, so a refusal has something it could have removed or changed had the id
    // been read as a lookup instead of as a form.
    let (stored_id, _) = seed_fpga_image(&engine, "id-form");
    let stored_before = stored_afi_count(&engine);

    for (case, fpga_image_id) in malformed_rows() {
        let echoed: String = fpga_image_id
            .chars()
            .take(MAX_ECHOED_RESOURCE_ID_CHARS)
            .collect();
        let expected_message = format!("Image ID '{echoed}' is invalid.");
        for action in REGIONAL_RESOLVERS.iter().chain(["CopyFpgaImage"].iter()) {
            let params = resolver_params(action, &fpga_image_id);
            let (status, body) = op(&router, action, &params).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?} on {action}: expected HTTP {REFUSAL_STATUS}, got {status} with \
                 body {body}"
            );
            let answered_code = error_code(&body);
            assert_eq!(
                answered_code, INVALID_FPGA_IMAGE_ID_MALFORMED,
                "case {case:?} on {action}: expected the code \
                 {INVALID_FPGA_IMAGE_ID_MALFORMED:?}, got {answered_code:?} in {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?} on {action}: expected the message {expected_message:?}, got \
                 {message:?}"
            );
        }
    }

    assert_eq!(
        stored_afi_count(&engine),
        stored_before,
        "case {stored_before:?}: a refused call writes nothing, got {} stored",
        stored_afi_count(&engine)
    );
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    assert!(
        guard.fpga_images.contains_key(&stored_id),
        "the stored AFI {stored_id:?} survives every refusal, got {:?}",
        guard.fpga_images.keys().collect::<Vec<_>>()
    );
}

/// RULE: an id INSIDE the issued form that names no stored AFI is the other answer —
/// `InvalidFpgaImageID.NotFound` from the operations that resolve an AFI of their own Region,
/// and the copy's one answer from the copy.
#[tokio::test]
async fn an_afi_id_inside_the_issued_form_is_answered_as_not_found() {
    let (router, _engine) = emulator();

    for (case, fpga_image_id) in unknown_rows() {
        for action in REGIONAL_RESOLVERS {
            let params = resolver_params(action, &fpga_image_id);
            let (status, body) = op(&router, action, &params).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?} on {action}: expected HTTP {REFUSAL_STATUS}, got {status} with \
                 body {body}"
            );
            let answered_code = error_code(&body);
            assert_eq!(
                answered_code, INVALID_FPGA_IMAGE_ID_NOT_FOUND,
                "case {case:?} on {action}: expected the code \
                 {INVALID_FPGA_IMAGE_ID_NOT_FOUND:?}, got {answered_code:?} in {body}"
            );
            // The missing-resource wording is the one this service answers for an AFI, and it
            // names the id the REQUEST carried rather than the malformed sentence.
            let expected_message = format!("The fpga-image ID '{fpga_image_id}' does not exist");
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case:?} on {action}: expected the message {expected_message:?}, got \
                 {message:?}"
            );
        }

        // `CopyFpgaImage` has ONE answer for a source it cannot resolve, so a well-formed id
        // naming no AFI is the malformed answer there — the same sentence, naming the same id.
        let params = resolver_params("CopyFpgaImage", &fpga_image_id);
        let (status, body) = op(&router, "CopyFpgaImage", &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?} on CopyFpgaImage: expected HTTP {REFUSAL_STATUS}, got {status} with \
             body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, INVALID_FPGA_IMAGE_ID_MALFORMED,
            "case {case:?} on CopyFpgaImage: expected the code \
             {INVALID_FPGA_IMAGE_ID_MALFORMED:?}, got {answered_code:?} in {body}"
        );
        let expected_message = format!("Image ID '{fpga_image_id}' is invalid.");
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case:?} on CopyFpgaImage: expected the message {expected_message:?}, got \
             {message:?}"
        );
    }
}

/// RULE: the id a stored AFI was minted with is inside the form and resolves to that AFI — a
/// delete removes it and answers `true`, and the same id is then answered as not found by the
/// delete and by the attribute describe alike.
#[tokio::test]
async fn a_stored_afi_id_resolves_until_the_delete_removes_it() {
    let (router, engine) = emulator();

    let (fpga_image_id, _) = seed_fpga_image(&engine, "delete-round-trip");

    // The id a mint produces is one the readers accept, so the AFI is describable before the
    // delete.
    let params = resolver_params("DescribeFpgaImageAttribute", &fpga_image_id);
    let (status, body) = op(&router, "DescribeFpgaImageAttribute", &params).await;
    assert_eq!(
        status, 200,
        "case {fpga_image_id:?}: expected HTTP 200 from DescribeFpgaImageAttribute, got \
         {status} with body {body}"
    );

    let params = resolver_params("DeleteFpgaImage", &fpga_image_id);
    let (status, body) = op(&router, "DeleteFpgaImage", &params).await;
    assert_eq!(
        status, 200,
        "case {fpga_image_id:?}: expected HTTP 200 from DeleteFpgaImage, got {status} with \
         body {body}"
    );
    let returned = element(&body, "return");
    assert_eq!(
        returned.as_deref(),
        Some("true"),
        "case {fpga_image_id:?}: a delete that succeeded reports true, got {returned:?} in \
         {body}"
    );
    assert_eq!(
        stored_afi_count(&engine),
        0,
        "case {fpga_image_id:?}: the delete takes the AFI out of the store, got {} stored",
        stored_afi_count(&engine)
    );

    // The id is still inside the form, so what changed is the resource: both readers answer
    // the missing AFI, not a malformed id.
    let expected_message = format!("The fpga-image ID '{fpga_image_id}' does not exist");
    for action in ["DeleteFpgaImage", "DescribeFpgaImageAttribute"] {
        let params = resolver_params(action, &fpga_image_id);
        let (status, body) = op(&router, action, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {fpga_image_id:?} on {action} after the delete: expected HTTP \
             {REFUSAL_STATUS}, got {status} with body {body}"
        );
        let answered_code = error_code(&body);
        assert_eq!(
            answered_code, INVALID_FPGA_IMAGE_ID_NOT_FOUND,
            "case {fpga_image_id:?} on {action} after the delete: expected the code \
             {INVALID_FPGA_IMAGE_ID_NOT_FOUND:?}, got {answered_code:?} in {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {fpga_image_id:?} on {action} after the delete: expected the message \
             {expected_message:?}, got {message:?}"
        );
    }
}
