// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication` over this service's awsQuery surface.
//!
//! The rule under test: an update applies the properties the request supplies to
//! the application the store already holds, leaves every property it does not
//! supply exactly as stored, and clears a property supplied as an empty string —
//! it never creates an application and never replaces one.
//!
//! The harness (the router-mounted emulator, its store accessor and the XML
//! element reader) is the one `tests/all/lifecycle.rs` owns; this suite drives
//! the same service through it rather than standing up a second copy. The
//! behaviour is driven entirely over HTTP; the store is only READ, where the
//! difference between "the property is gone" and "an empty value is stored" —
//! and the full precision of a timestamp — cannot be seen in the XML envelope.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{account, Emulator, TEST_REGION};

/// 200 characters: the modelled maximum length of
/// `UpdateApplicationMessage$Description`, from
/// `smithy_query(query="operation", operation="UpdateApplication")` —
/// `Description` (len 0..200). Spelled as 20 ten-character chunks so the count is
/// checkable by eye; the test also asserts the length.
const MAX_LENGTH_DESCRIPTION: &str = concat!(
    "0123456789", "0123456789", "0123456789", "0123456789", "0123456789",
    "0123456789", "0123456789", "0123456789", "0123456789", "0123456789",
    "0123456789", "0123456789", "0123456789", "0123456789", "0123456789",
    "0123456789", "0123456789", "0123456789", "0123456789", "0123456789",
);

/// One row of the update table: what the application starts as, what the update
/// supplies, and what the store must hold afterwards.
struct DescriptionCase {
    /// Doubles as the application name, so each row is an independent resource.
    case: &'static str,
    /// The `Description` CreateApplication supplies (`None`: the member is omitted).
    initial: Option<&'static str>,
    /// The `Description` UpdateApplication supplies (`None`: the member is omitted).
    update: Option<&'static str>,
    /// The description the application must hold afterwards (`None`: none at all).
    expected: Option<&'static str>,
    /// Version labels created under the application before the update, oldest
    /// first — a property the update does not take and must not disturb.
    versions: &'static [&'static str],
}

impl DescriptionCase {
    /// The description as a reader of this service's XML sees it: the serializer
    /// writes an empty member as `<Tag></Tag>`, which carries no text node, so an
    /// empty expected description reads back as absent.
    fn readable(&self) -> Option<&'static str> {
        self.expected.filter(|text| !text.is_empty())
    }
}

#[tokio::test]
async fn an_update_applies_the_supplied_properties_in_place() {
    assert_eq!(
        MAX_LENGTH_DESCRIPTION.chars().count(),
        200,
        "the maximum-length row must be exactly the modelled maximum, got {}",
        MAX_LENGTH_DESCRIPTION.chars().count()
    );

    // Rows differ along what the rule generalises over: whether the property was
    // already set, whether the update supplies it, whether it supplies an empty
    // string (the documented way to clear it), the value's length and its
    // encoding, and how many other resources hang off the application.
    let cases = [
        DescriptionCase {
            case: "sets-a-description-where-there-was-none",
            initial: None,
            update: Some("a new description"),
            expected: Some("a new description"),
            versions: &[],
        },
        DescriptionCase {
            case: "replaces-the-stored-description",
            initial: Some("the first description"),
            update: Some("the second description"),
            expected: Some("the second description"),
            versions: &["v1"],
        },
        DescriptionCase {
            // "To clear these properties, specify an empty string."
            case: "an-empty-string-clears-the-description",
            initial: Some("to be cleared"),
            update: Some(""),
            expected: None,
            versions: &[],
        },
        DescriptionCase {
            // "If a property (for example, description) is not provided, the
            // value remains unchanged."
            case: "an-omitted-description-keeps-the-stored-one",
            initial: Some("kept as it was"),
            update: None,
            expected: Some("kept as it was"),
            versions: &["1.0.0", "1.0.1", "release-2"],
        },
        DescriptionCase {
            case: "an-omitted-description-keeps-none",
            initial: None,
            update: None,
            expected: None,
            versions: &[],
        },
        DescriptionCase {
            case: "a-maximum-length-description",
            initial: Some("short"),
            update: Some(MAX_LENGTH_DESCRIPTION),
            expected: Some(MAX_LENGTH_DESCRIPTION),
            versions: &[],
        },
        DescriptionCase {
            case: "a-multi-byte-description",
            initial: Some("ascii"),
            update: Some("описание — fünf ✅"),
            expected: Some("описание — fünf ✅"),
            versions: &[],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let mut params: Vec<(&str, &str)> = vec![("ApplicationName", case.case)];
        if let Some(description) = case.initial {
            params.push(("Description", description));
        }
        let created = emulator.call("CreateApplication", &params).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateApplication, got {} and body {}",
            case.case, created.status, created.body
        );
        let created_at = created.expect("Application/DateCreated", case.case).to_string();
        let updated_before = created.expect("Application/DateUpdated", case.case).to_string();

        for label in case.versions {
            let version = emulator
                .call(
                    "CreateApplicationVersion",
                    &[("ApplicationName", case.case), ("VersionLabel", label)],
                )
                .await;
            assert_eq!(
                version.status, 200,
                "case {:?}: expected 200 from CreateApplicationVersion for {label:?}, got {} and body {}",
                case.case, version.status, version.body
            );
        }

        let mut update_params: Vec<(&str, &str)> = vec![("ApplicationName", case.case)];
        if let Some(description) = case.update {
            update_params.push(("Description", description));
        }
        let updated = emulator.call("UpdateApplication", &update_params).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateApplication, got {} and body {}",
            case.case, updated.status, updated.body
        );
        assert_eq!(
            updated.value("Application/Description"),
            case.readable(),
            "case {:?}: unexpected Description in the UpdateApplication answer {}",
            case.case,
            updated.body
        );
        // Clearing a property is the property GOING AWAY, not an empty value
        // stored in its place: the serializer writes a present-but-empty member
        // as `<Description></Description>` and omits an absent one, so a cleared
        // description must leave no element in the document at all.
        if case.expected.is_none() {
            assert!(
                !updated.body.contains("<Description>"),
                "case {:?}: a cleared description must leave no Description element, got {}",
                case.case,
                updated.body
            );
        }
        assert_eq!(
            updated.expect("Application/ApplicationName", case.case),
            case.case,
            "case {:?}: UpdateApplication answered for another application: {}",
            case.case,
            updated.body
        );
        // The identity of the application is untouched by an update: the ARN and
        // the creation time are the ones CreateApplication minted.
        assert_eq!(
            updated.expect("Application/ApplicationArn", case.case),
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:application/{}",
                account(),
                case.case
            ),
            "case {:?}: unexpected ApplicationArn in {}",
            case.case,
            updated.body
        );
        assert_eq!(
            updated.expect("Application/DateCreated", case.case),
            created_at,
            "case {:?}: an update must not move DateCreated, got {}",
            case.case,
            updated.body
        );
        // An update that supplies no property changes nothing, so the record's
        // `DateUpdated` is still the one creation set. One that does supply a
        // property is covered, at the clock's own precision, by
        // [`date_updated_moves_only_when_a_property_is_applied`].
        if case.update.is_none() {
            assert_eq!(
                updated.expect("Application/DateUpdated", case.case),
                updated_before,
                "case {:?}: an update that supplies no property must leave DateUpdated at {updated_before:?}, got {}",
                case.case,
                updated.body
            );
        }
        // The application's versions are a property this operation does not
        // take, so they are reported unchanged, newest first.
        let expected_versions: Vec<&str> = case.versions.iter().rev().copied().collect();
        assert_eq!(
            updated.values_at("Versions/member"),
            expected_versions,
            "case {:?}: an update must not disturb the application's versions, got {}",
            case.case,
            updated.body
        );
    }

    // Read every row back through DescribeApplications: the update mutated the
    // stored application, so the store answers with the new value and with one
    // application per row — no copy, no fresh resource.
    for case in &cases {
        let described = emulator
            .call(
                "DescribeApplications",
                &[("ApplicationNames.member.1", case.case)],
            )
            .await;
        assert_eq!(
            described.values_at("Applications/member/ApplicationName"),
            vec![case.case],
            "case {:?}: expected exactly that one application, got {}",
            case.case,
            described.body
        );
        assert_eq!(
            described.value("Applications/member/Description"),
            case.readable(),
            "case {:?}: the stored Description must be the updated one, got {}",
            case.case,
            described.body
        );

        // And in the store itself, where a cleared property is distinguishable
        // from an empty one: a clear leaves NO description, not `Some("")`.
        let stored = stored_application(&emulator, case.case);
        assert_eq!(
            stored.description.as_deref(),
            case.expected.filter(|text| !text.is_empty()),
            "case {:?}: unexpected stored description {:?}",
            case.case,
            stored.description
        );
    }

    let all = emulator.call("DescribeApplications", &[]).await;
    assert_eq!(
        all.values_at("Applications/member/ApplicationName").len(),
        cases.len(),
        "an update must not create an application: expected {} in the store, got {}",
        cases.len(),
        all.body
    );
}

#[tokio::test]
async fn date_updated_moves_only_when_a_property_is_applied() {
    let emulator = Emulator::new();
    let case = "date-updated";
    let created = emulator
        .call(
            "CreateApplication",
            &[("ApplicationName", case), ("Description", "before")],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    // The STORED timestamps rather than the rendered ones: the envelope renders a
    // timestamp more coarsely than the clock keeps it, so only the store can say
    // whether the handler moved `DateUpdated` at all. Nothing sleeps.
    let at_creation = stored_timestamps(&emulator, case);
    assert_eq!(
        at_creation.0, at_creation.1,
        "case {case:?}: a just-created application was last updated when it was created, got {at_creation:?}"
    );

    // An update that applies a property moves `DateUpdated` STRICTLY forward of
    // the instant the record last changed — and leaves `DateCreated` alone.
    let applied = emulator
        .call(
            "UpdateApplication",
            &[("ApplicationName", case), ("Description", "after")],
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "case {case:?}: expected 200 from UpdateApplication, got {} and body {}",
        applied.status, applied.body
    );
    let after_update = stored_timestamps(&emulator, case);
    assert!(
        after_update.1 > at_creation.1,
        "case {case:?}: expected DateUpdated to move past {:?}, got {:?}",
        at_creation.1,
        after_update.1
    );
    assert_eq!(
        after_update.0, at_creation.0,
        "case {case:?}: an update must not move DateCreated from {:?}, got {:?}",
        at_creation.0, after_update.0
    );

    // An update that applies no property changes nothing, so both timestamps
    // stay where the last real change left them.
    let untouched = emulator
        .call("UpdateApplication", &[("ApplicationName", case)])
        .await;
    assert_eq!(
        untouched.status, 200,
        "case {case:?}: expected 200 from UpdateApplication, got {} and body {}",
        untouched.status, untouched.body
    );
    let after_no_op = stored_timestamps(&emulator, case);
    assert_eq!(
        after_no_op, after_update,
        "case {case:?}: an update that applies nothing must leave both timestamps at {after_update:?}, got {after_no_op:?}"
    );
    assert_eq!(
        untouched.value("Application/Description"),
        Some("after"),
        "case {case:?}: the stored description must survive an update that supplies none, got {}",
        untouched.body
    );
}

/// The application the store holds under `application_name`, failing the test
/// when the store holds none.
fn stored_application(emulator: &Emulator, application_name: &str) -> eb::state::Application {
    emulator
        .store()
        .lock()
        .applications
        .get(application_name)
        .cloned()
        .unwrap_or_else(|| panic!("case {application_name:?}: the application must be stored"))
}

/// The application's stored `(DateCreated, DateUpdated)` in seconds since the
/// epoch, at the clock's own precision rather than the envelope's.
fn stored_timestamps(emulator: &Emulator, application_name: &str) -> (f64, f64) {
    let stored = stored_application(emulator, application_name);
    (
        stored.date_created.as_secs_f64(),
        stored.date_updated.as_secs_f64(),
    )
}

#[tokio::test]
async fn updating_an_application_that_is_not_stored_is_refused() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateApplication",
            &[("ApplicationName", "present"), ("Description", "untouched")],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"present\": expected 200 from CreateApplication, got {} and body {}",
        created.status, created.body
    );

    // "If no such application is found, UpdateApplication returns an
    // InvalidParameterValue error" — a name that is merely close to a stored one
    // is still no such application.
    for name in ["absent", "present-but-not-quite", "PRESENT", "presen"] {
        let refused = emulator
            .call(
                "UpdateApplication",
                &[("ApplicationName", name), ("Description", "a description")],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {name:?}: unexpected error code in {}",
            refused.body
        );
        let described = emulator
            .call("DescribeApplications", &[("ApplicationNames.member.1", name)])
            .await;
        assert!(
            described
                .values_at("Applications/member/ApplicationName")
                .is_empty(),
            "case {name:?}: a refused update must create nothing, got {}",
            described.body
        );
    }

    // The application that does exist was not touched by any of those refusals.
    let described = emulator
        .call(
            "DescribeApplications",
            &[("ApplicationNames.member.1", "present")],
        )
        .await;
    assert_eq!(
        described.value("Applications/member/Description"),
        Some("untouched"),
        "case \"present\": a refused update must not change another application, got {}",
        described.body
    );
}
