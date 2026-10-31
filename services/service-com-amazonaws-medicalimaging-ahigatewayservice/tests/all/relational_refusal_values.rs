// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHAT a handler-raised `ValidationException` of this service may
//! carry.
//!
//! RULE: a refusal whose requirement is about a member's PRESENCE or about its RELATION
//! to another member — rather than about what that member itself held — states the
//! member path and the requirement and nothing the request carried: no
//! `Value '<v>' at '<path>'` prefix, and in particular no identifier naming a resource
//! the request addressed. A refusal about WHAT a member held keeps the echoed value, so
//! the same member path can appear in both forms: which form is answered is decided by
//! the REQUIREMENT, never by which member or which value the request put there.
//!
//! The rows differ along what the rule generalises over: which operation raised the
//! refusal (`CopyImageSet`, `UpdateImageSetMetadata`, `SearchImageSets`), which member it
//! is about (a body member, a nested member of a payload, a query parameter, a member of
//! a list element), which kind of relation the request broke (one member naming another's
//! value, one member present beside another, a list's cardinality fixed by a sibling
//! member), and which identifiers and values the request held while it was refused.
//!
//! The harness is the sibling suites' (`crate::datastore_lifecycle`): real requests
//! through the production front door, so what is asserted is the wire text a
//! `medical-imaging` SDK client sees.

use serde_json::{json, Value};

use crate::datastore_lifecycle::{Answer, Emulator};
use crate::image_set_copy::{copy_image_set, information, seed_image_set};
use crate::image_set_metadata_updates::update_metadata;
use crate::image_set_search::search;
use crate::resource_tagging::VALIDATION_ENVELOPE;

/// The words that join a clause's subject to its requirement, as every refusal of this
/// service spells them.
const SEPARATOR: &str = " failed to satisfy constraint: ";

/// Assert `answer` is a refusal stating `path` and a requirement, carrying no echoed
/// value at all and none of `withheld` — the identifiers and values the refused request
/// held.
fn assert_withholds_values(case: &str, answer: &Answer, path: &str, withheld: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    let expected = format!("{VALIDATION_ENVELOPE}: {path}{SEPARATOR}");
    assert!(
        message.starts_with(&expected),
        "case {case:?}: expected the message to open with {expected:?}, got {message:?}"
    );
    assert!(
        message.len() > expected.len(),
        "case {case:?}: expected a requirement after {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains("Value "),
        "case {case:?}: expected no echoed value, got {message:?}"
    );
    assert!(
        !message.contains(&format!("at '{path}'")),
        "case {case:?}: expected the member to be stated as the subject, not after an \
         echoed value, got {message:?}"
    );
    for value in withheld {
        assert!(
            !message.contains(value),
            "case {case:?}: expected the refusal never to carry {value:?}, got {message:?}"
        );
    }
}

/// Assert `answer` is a refusal about WHAT `member` held, which keeps the echoed value —
/// the contrast that shows the withholding is keyed on the requirement, not on the
/// member.
fn assert_echoes_value(case: &str, answer: &Answer, member: &str, value: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected error code ValidationException, got {answer:?}"
    );
    let message = answer.string(&["message"]);
    assert!(
        message.starts_with(VALIDATION_ENVELOPE),
        "case {case:?}: expected the message to open with {VALIDATION_ENVELOPE:?}, got {message:?}"
    );
    assert!(
        message.contains(&format!("Value '{value}'")),
        "case {case:?}: expected the clause to echo {value:?}, got {message:?}"
    );
    assert!(
        message.contains(member),
        "case {case:?}: expected the clause to name {member:?}, got {message:?}"
    );
}

/// The validation path of the destination image set's identifier — the member a
/// self-naming copy breaks the rule on.
const DESTINATION_IMAGE_SET_ID_PATH: &str = "copyImageSetInformation.destinationImageSet.imageSetId";

/// The validation path of the destination image set member itself — the member a promote
/// request must not carry.
const DESTINATION_IMAGE_SET_PATH: &str = "copyImageSetInformation.destinationImageSet";

/// RULE, on one member NAMING another member's value: a copy whose
/// `destinationImageSet.imageSetId` is the source image set is refused by the relation,
/// so the refusal names the member and withholds the identifier — which is a legal
/// `ImageSetId` naming an image set the data store really holds, and so says nothing
/// about what the member held.
///
/// The rows differ along how many versions the self-named image set holds, which version
/// the request claims as the latest (the current one and a stale one), and — since every
/// row seeds its own image set — which identifier was refused.
#[tokio::test]
async fn a_self_naming_destination_is_refused_without_the_identifier() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("self.naming", "self-naming").await;

    for (versions, claimed) in [(1_u32, "1"), (2, "2"), (3, "3"), (3, "1"), (5, "2")] {
        let image_set_id = seed_image_set(&emulator, &datastore_id, versions);
        let case = format!("versions={versions} claimed={claimed}");
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &image_set_id,
            information(claimed, None, Some((image_set_id.as_str(), claimed))),
            &[],
        )
        .await;
        assert_withholds_values(
            &case,
            &refused,
            DESTINATION_IMAGE_SET_ID_PATH,
            &[image_set_id.as_str(), datastore_id.as_str()],
        );
    }
}

/// RULE, over every OTHER handler-raised refusal of this service that is about a member's
/// presence or its relation to another member: each states its member and the requirement
/// and carries nothing the request held. Three operations, four relations, so the rule is
/// asserted where it holds rather than only where the copy raises it.
#[tokio::test]
async fn every_relation_refusal_states_the_member_and_nothing_the_request_held() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("relations", "relations").await;
    let source = seed_image_set(&emulator, &datastore_id, 2);
    let other = seed_image_set(&emulator, &datastore_id, 1);

    // A promote mints its own destination, so a request that also names one holds two
    // members that cannot describe one copy. Rows: the destination naming another image
    // set and naming the source itself, with and without `force`.
    for (destination, forced) in [
        (other.as_str(), false),
        (other.as_str(), true),
        (source.as_str(), false),
    ] {
        let case = format!("promote with destination={destination} force={forced}");
        let mut query = vec![("promoteToPrimary", true)];
        if forced {
            query.push(("force", true));
        }
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &source,
            information("2", None, Some((destination, "1"))),
            &query,
        )
        .await;
        assert_withholds_values(
            &case,
            &refused,
            DESTINATION_IMAGE_SET_PATH,
            &[source.as_str(), other.as_str(), datastore_id.as_str()],
        );
    }

    // A revert names a version of ONE image set's own history, so it cannot describe the
    // other image sets of the study: `includeStudyImageSets` and `revertToVersionId`
    // cannot hold together. Rows: distinctive version identifiers, so a refusal echoing
    // the one the request named would be visible.
    for version_id in ["4711", "90210"] {
        let case = format!("revert to version {version_id} across the study");
        let refused = update_metadata(
            &emulator,
            &datastore_id,
            &source,
            "2",
            json!({ "revertToVersionId": version_id }),
            &[("includeStudyImageSets", true)],
        )
        .await;
        assert_withholds_values(
            &case,
            &refused,
            "includeStudyImageSets",
            &[version_id, source.as_str(), datastore_id.as_str()],
        );
    }

    // A `DICOMUpdates` blob carrying neither document names nothing to change: the
    // requirement is about which of its two members is present.
    let refused = update_metadata(
        &emulator,
        &datastore_id,
        &source,
        "2",
        json!({ "DICOMUpdates": {} }),
        &[],
    )
    .await;
    assert_withholds_values(
        "DICOMUpdates carrying neither document",
        &refused,
        "updateImageSetMetadataUpdates.DICOMUpdates",
        &[source.as_str(), datastore_id.as_str()],
    );

    // How many values a filter's `values` may hold is fixed by its SIBLING member, the
    // operator — a relation between two members of one list element. Rows: each operator,
    // given the cardinality the other one takes.
    let filter_rows: [(&str, &str, Value); 2] = [
        (
            "EQUAL given two values",
            "EQUAL",
            json!([{ "DICOMPatientId": "PID-ALPHA" }, { "DICOMPatientId": "PID-BETA" }]),
        ),
        (
            "BETWEEN given one bound",
            "BETWEEN",
            json!([{ "createdAt": 1_767_225_600.0 }]),
        ),
    ];
    for (case, operator, values) in filter_rows {
        let criteria = json!({ "filters": [{ "operator": operator, "values": values }] });
        let refused = search(&emulator, &datastore_id, Some(criteria), "").await;
        assert_withholds_values(
            case,
            &refused,
            "searchCriteria.filters.1.member.values",
            &["PID-ALPHA", "PID-BETA", datastore_id.as_str()],
        );
    }
}

/// RULE, from the other side: a refusal about WHAT a member held still echoes the value,
/// including on the very member a self-naming destination withholds. So withholding is
/// decided by the requirement, not by the member — and an `imageSetId` outside its
/// `@pattern` (`^[0-9a-z]{32}$`, `smithy_query(query="shape",shape="ImageSetId")`) is
/// still reported with the value the caller sent.
#[tokio::test]
async fn a_refusal_about_what_the_member_held_still_echoes_it() {
    let emulator = Emulator::new();
    let datastore_id = emulator.active_datastore("echoed", "echoed").await;
    let source = seed_image_set(&emulator, &datastore_id, 1);

    // Rows: a value too short for the pattern, one in the wrong case, one a character
    // past its 32, and one that is empty — each a claim about what the member held.
    let illegal_ids = [
        "abc".to_string(),
        "A".repeat(32),
        "a".repeat(33),
        String::new(),
    ];
    for illegal in illegal_ids {
        let case = format!("destination imageSetId {illegal:?}");
        let refused = copy_image_set(
            &emulator,
            &datastore_id,
            &source,
            information("1", None, Some((illegal.as_str(), "1"))),
            &[],
        )
        .await;
        assert_echoes_value(&case, &refused, "imageSetId", &illegal);
    }

    // And the sibling member of the filter whose cardinality rule withholds its values:
    // an `operator` outside the modelled enum is a claim about what THAT member held, so
    // the value is echoed.
    for operator in ["LESS_THAN", "equal"] {
        let criteria = json!({
            "filters": [{ "operator": operator, "values": [{ "DICOMPatientId": "PID-ALPHA" }] }],
        });
        let refused = search(&emulator, &datastore_id, Some(criteria), "").await;
        assert_echoes_value(
            &format!("operator {operator:?}"),
            &refused,
            "searchCriteria.filters.1.member.operator",
            operator,
        );
    }
}
