// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a length bound one of its handlers enforces for itself, end to
//! end over the wire.
//!
//! Every assertion goes through the real request path — the shared `Harness` of
//! `tests/all/benefit_application_lifecycle.rs` (`MultiServiceRouter::dispatch` with
//! `awsJson1_0` bodies, `POST /` and `X-Amz-Target:
//! PartnerCentralBenefitsService.<Operation>`) — so what it pins is the emulator's HTTP
//! behaviour, not an internal helper.
//!
//! The rule under test: a length bound a handler enforces for itself — one the model
//! leaves off the member, so the handler owns it — is refused with the requirement this
//! service states a length violation with, at BOTH ends of the bound, so the two ends of
//! one member are stated in one vocabulary and a caller parses either the same way.
//!
//! The rows span what the rule generalises over: which end of the bound the request misses
//! (the minimum, the maximum), what the length counts (the elements of a list, the
//! characters of a string), which handler enforces it (`AmendBenefitApplication`'s
//! amendment count and its `NewValue` bounds, `AssociateBenefitApplicationResource`'s
//! association bound), the distance from the bound (at it, one past it, far past it) and
//! the character width of the value (ASCII and multi-byte, since a length counts
//! characters).

use serde_json::{json, Value};
use service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::state::{
    MAX_AMENDMENTS_PER_REQUEST, MAX_ASSOCIATED_RESOURCES_PER_APPLICATION, MAX_DETAILS_VALUE_LEN,
    MIN_AMENDMENTS_PER_REQUEST, VALIDATION_REASON_FIELD,
};

use crate::amend_benefit_application::{amend_body, amendment};
use crate::benefit_application_lifecycle::{error_code, text, Harness, CATALOG};
use crate::disassociate_benefit_application_resource::{body as association_body, seed as seed_full};
use crate::service_error_rules::{assert_validation_invariants, clause, message_of, one};

/// The longest value `Name` accepts: the modelled `BenefitApplicationName`
/// `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape",shape="BenefitApplicationName")`).
const NAME_MAX_LEN: usize = 255;

/// The longest value `Description` accepts: the modelled
/// `BenefitApplicationDescription` `@length(min: 1, max: 1000)`.
const DESCRIPTION_MAX_LEN: usize = 1000;

/// The shortest value either of those two members accepts, from the same two `@length`
/// traits.
const STRING_MIN_LEN: usize = 1;

/// The requirement a length below a bound fails, written out here rather than read from
/// the service so the words themselves are under test: the sentence AWS states a length
/// minimum with, and the one this service's centralized constraint walk reports for a
/// modelled `@length(min:)` (pinned on modelled members in
/// `tests/all/service_error_rules.rs`).
fn min_length(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a length above a bound fails, the maximum half of that same sentence.
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The prefix both ends share: what makes the two refusals of one member one vocabulary.
const LENGTH_PREFIX: &str = "Member must have length ";

/// The call one row makes, built when the row runs so each amendment carries the
/// application's CURRENT revision (an accepted row moves it).
enum Call {
    /// One `AmendBenefitApplication` carrying these amendments.
    Amend(Value),
    /// One `AssociateBenefitApplicationResource` of this ARN onto the record already
    /// filled to the store's association bound.
    Associate(&'static str),
}

/// One row: the call, the member a refusal must name, and the requirement it must state —
/// `None` where the length is INSIDE the bound and the call must be served.
struct Row {
    case: String,
    call: Call,
    member: &'static str,
    expected: Option<String>,
}

/// `count` amendments, each writing a distinct member of the details document, so a row
/// varies the element count and nothing else.
fn amendments(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| amendment(&format!("BenefitApplicationDetails.f{index}"), "x"))
            .collect(),
    )
}

/// Every length bound a handler of this service enforces itself is refused with this
/// service's one length requirement, at either end of the bound and on every member that
/// carries one — while a length inside the bound is served.
#[tokio::test]
async fn a_handler_enforced_length_states_one_requirement_at_both_ends() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let subject = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-length-wording-seed",
                "BenefitIdentifier": benefit_id,
                "Name": "length-wording",
                "Description": "the application the length wording is asserted on",
            }),
        )
        .await;
    let id = text("seed", &subject, "Id");

    // One record whose association list is already at the store's bound, so the next
    // association is the one past it.
    let filled: Vec<String> = (0..MAX_ASSOCIATED_RESOURCES_PER_APPLICATION)
        .map(|slot| format!("arn:aws:s3:::length-wording-{slot}"))
        .collect();
    let stored: Vec<&str> = filled.iter().map(String::as_str).collect();
    let full_id = seed_full(
        &harness,
        "a record at the association bound",
        CATALOG,
        "tok-length-wording-full",
        &stored,
    )
    .await;

    let rows = vec![
        Row {
            case: "an amendment list naming nothing to amend".to_string(),
            call: Call::Amend(amendments(0)),
            member: "amendments",
            expected: Some(min_length(MIN_AMENDMENTS_PER_REQUEST)),
        },
        Row {
            case: "an amendment list at its minimum".to_string(),
            call: Call::Amend(amendments(MIN_AMENDMENTS_PER_REQUEST)),
            member: "amendments",
            expected: None,
        },
        Row {
            case: "an amendment list at its maximum".to_string(),
            call: Call::Amend(amendments(MAX_AMENDMENTS_PER_REQUEST)),
            member: "amendments",
            expected: None,
        },
        Row {
            case: "one amendment past the maximum".to_string(),
            call: Call::Amend(amendments(MAX_AMENDMENTS_PER_REQUEST + 1)),
            member: "amendments",
            expected: Some(max_length(MAX_AMENDMENTS_PER_REQUEST)),
        },
        Row {
            case: "an amendment list far past the maximum".to_string(),
            call: Call::Amend(amendments(5 * MAX_AMENDMENTS_PER_REQUEST)),
            member: "amendments",
            expected: Some(max_length(MAX_AMENDMENTS_PER_REQUEST)),
        },
        Row {
            case: "an empty value for Name".to_string(),
            call: Call::Amend(json!([amendment("Name", "")])),
            member: "amendments.1.member.newValue",
            expected: Some(min_length(STRING_MIN_LEN)),
        },
        Row {
            case: "a Name at its maximum".to_string(),
            call: Call::Amend(json!([amendment("Name", &"n".repeat(NAME_MAX_LEN))])),
            member: "amendments.1.member.newValue",
            expected: None,
        },
        Row {
            case: "a Name one character past its maximum".to_string(),
            call: Call::Amend(json!([amendment("Name", &"n".repeat(NAME_MAX_LEN + 1))])),
            member: "amendments.1.member.newValue",
            expected: Some(max_length(NAME_MAX_LEN)),
        },
        Row {
            case: "an empty value for Description".to_string(),
            call: Call::Amend(json!([amendment("Description", "")])),
            member: "amendments.1.member.newValue",
            expected: Some(min_length(STRING_MIN_LEN)),
        },
        Row {
            case: "a multi-byte Description one character past its maximum".to_string(),
            call: Call::Amend(json!([amendment(
                "Description",
                &"é".repeat(DESCRIPTION_MAX_LEN + 1)
            )])),
            member: "amendments.1.member.newValue",
            expected: Some(max_length(DESCRIPTION_MAX_LEN)),
        },
        Row {
            case: "a details value at this service's own bound".to_string(),
            call: Call::Amend(json!([amendment(
                "BenefitApplicationDetails.Campaign",
                &"v".repeat(MAX_DETAILS_VALUE_LEN)
            )])),
            member: "amendments.1.member.newValue",
            expected: None,
        },
        Row {
            case: "a details value one character past it".to_string(),
            call: Call::Amend(json!([amendment(
                "BenefitApplicationDetails.Campaign",
                &"v".repeat(MAX_DETAILS_VALUE_LEN + 1)
            )])),
            member: "amendments.1.member.newValue",
            expected: Some(max_length(MAX_DETAILS_VALUE_LEN)),
        },
        Row {
            case: "one association past the record's bound".to_string(),
            call: Call::Associate("arn:aws:s3:::length-wording-past"),
            member: "associatedResources",
            expected: Some(max_length(MAX_ASSOCIATED_RESOURCES_PER_APPLICATION)),
        },
    ];

    // The table covers both ends of a bound, so it cannot shrink to the maximum half the
    // central wording already reached. Every requirement it expects is a LENGTH
    // requirement, which is what makes the two ends one vocabulary.
    for end in [
        format!("{LENGTH_PREFIX}greater than or equal to"),
        format!("{LENGTH_PREFIX}less than or equal to"),
    ] {
        assert!(
            rows.iter().any(|row| {
                row.expected
                    .as_ref()
                    .is_some_and(|expected| expected.starts_with(&end))
            }),
            "case \"table\": expected a row refused with {end:?}, got {:?}",
            rows.iter().map(|row| row.expected.clone()).collect::<Vec<_>>()
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let case = &row.case;
        let (operation, body) = match &row.call {
            Call::Amend(list) => {
                let revision = text(case, &harness.get(case, &id).await, "Revision");
                (
                    "AmendBenefitApplication",
                    amend_body(
                        &id,
                        &revision,
                        &format!("tok-length-wording-{index}"),
                        list.clone(),
                    ),
                )
            }
            Call::Associate(arn) => (
                "AssociateBenefitApplicationResource",
                association_body(CATALOG, &full_id, arn),
            ),
        };

        let (status, out) = harness.call(operation, &body).await;
        let Some(expected) = &row.expected else {
            assert_eq!(
                status, 200,
                "case {case:?}: a length inside the bound must be served, got {status} {out:?}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 outside the bound, got {status} {out:?}"
        );
        assert_eq!(
            error_code(&out),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {out:?}"
        );
        let message = message_of(&out)
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no Message: {out:?}"));
        assert_validation_invariants(case, &message);
        let want = one(&clause(row.member, expected));
        assert_eq!(
            message, want,
            "case {case:?}: expected the refusal to state {want:?}, got {message:?}"
        );
        assert_eq!(
            out["Reason"],
            json!(VALIDATION_REASON_FIELD),
            "case {case:?}: expected the field-validation reason, got {out:?}"
        );
    }
}

/// Over the whole domain of the one member whose BOTH length bounds a handler owns — the
/// amendment list, which the model marks `@required` and gives no `@length`
/// (`smithy_query(query="shape",shape="AmendmentList")`) — every refused element count is
/// stated with this service's length requirement, and the two ends differ only in the
/// comparator and the bound they cite.
#[tokio::test]
async fn both_ends_of_the_amendment_count_bound_share_one_wording() {
    let harness = Harness::new();
    let benefit_id = text("seed", &harness.active_benefit().await, "Id");
    let created = harness
        .create(
            "seed",
            &json!({
                "Catalog": CATALOG,
                "ClientToken": "tok-length-sweep-seed",
                "BenefitIdentifier": benefit_id,
                "Name": "length-sweep",
            }),
        )
        .await;
    let id = text("seed", &created, "Id");

    // Every count around each end of the bound: below it, at it, inside, at the other end
    // and past it.
    let counts = [
        0,
        MIN_AMENDMENTS_PER_REQUEST,
        MIN_AMENDMENTS_PER_REQUEST + 1,
        MAX_AMENDMENTS_PER_REQUEST - 1,
        MAX_AMENDMENTS_PER_REQUEST,
        MAX_AMENDMENTS_PER_REQUEST + 1,
        MAX_AMENDMENTS_PER_REQUEST + 2,
        2 * MAX_AMENDMENTS_PER_REQUEST,
    ];

    for (index, count) in counts.iter().enumerate() {
        let case = format!("{count} amendments");
        let revision = text(&case, &harness.get(&case, &id).await, "Revision");
        let (status, out) = harness
            .call(
                "AmendBenefitApplication",
                &amend_body(
                    &id,
                    &revision,
                    &format!("tok-length-sweep-{index}"),
                    amendments(*count),
                ),
            )
            .await;

        let expected = if *count < MIN_AMENDMENTS_PER_REQUEST {
            Some(min_length(MIN_AMENDMENTS_PER_REQUEST))
        } else if *count > MAX_AMENDMENTS_PER_REQUEST {
            Some(max_length(MAX_AMENDMENTS_PER_REQUEST))
        } else {
            None
        };

        match expected {
            None => assert_eq!(
                status, 200,
                "case {case:?}: a count inside the bound must be applied, got {status} {out:?}"
            ),
            Some(requirement) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 outside the bound, got {status} {out:?}"
                );
                let message = message_of(&out).unwrap_or_else(|| {
                    panic!("case {case:?}: the refusal carries no Message: {out:?}")
                });
                assert_eq!(
                    message,
                    one(&clause("amendments", &requirement)),
                    "case {case:?}: expected the refusal to state {requirement:?}, got {message:?}"
                );
                // The two ends are one vocabulary: both state a LENGTH requirement about
                // the member, and differ only in the comparator and the bound.
                assert!(
                    requirement.starts_with(LENGTH_PREFIX),
                    "case {case:?}: both ends must share the length wording, \
                     got {requirement:?}"
                );
            }
        }
    }
}
