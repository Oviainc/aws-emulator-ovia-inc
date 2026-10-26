// SPDX-License-Identifier: CC0-1.0

//! What SHAPE of request the environment-information pair —
//! `RequestEnvironmentInfo` ("Initiates a request to compile the specified type
//! of information of the deployed environment") and `RetrieveEnvironmentInfo` —
//! is served for, and what it answers for one it is not.
//!
//! RULE under test: the request's SHAPE is decided before the store is touched.
//! A value the model refuses — an `InfoType` outside `EnvironmentInfoType`, an
//! `EnvironmentName` outside its modelled length bound — is refused 400
//! `ValidationError` in AWS's `"<N> validation error(s) detected: …"` envelope,
//! and an absent `@required` member 400 `MissingRequiredParameter`, both before
//! any environment is resolved and without starting any compile; a value the
//! model accepts reaches the handler, which compiles the information for an
//! environment that exists — at either published bound of the name's length —
//! and refuses an identifier that names none with `InvalidParameterValue`.
//!
//! The constraints come from the model, not from this file's prose:
//! `smithy_query(query="shape",shape="com.amazonaws.elasticbeanstalk#RequestEnvironmentInfoMessage")`
//! states `InfoType` `@required`, targeting the `EnvironmentInfoType` enum, and
//! `EnvironmentName` with `length {min: 4, max: 40}`. The accepted value
//! catalogue below is read from the generated
//! `ENVIRONMENT_INFO_TYPE_VALUES` rather than written out, so it cannot drift
//! from the model, and the length rows are the two published bounds and one
//! character past each.
//!
//! The harness, and the fixtures that launch an environment and read a
//! retrieve's entries, are the ones the `environment_info` suite owns, and the
//! validation envelope's wording is the `request_refusals` suite's own
//! `envelope`: this suite adds no second copy of any of them.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::environment_info::{application, launch, ok, retrieve, SINGLE_INSTANCE};
use crate::lifecycle::{Emulator, PAST_DUE};
use crate::request_refusals::envelope;

/// The two operations the rule holds for alike: both take the same
/// `EnvironmentId` / `EnvironmentName` / `InfoType` input shape.
const BOTH_OPERATIONS: [&str; 2] = ["RequestEnvironmentInfo", "RetrieveEnvironmentInfo"];

/// One malformed request and what the boundary must answer for it.
///
/// Not `request_refusals::ConstraintCase`: a row here also states WHICH code the
/// boundary answers with, because this input shape can fail both as a violated
/// constraint and as an absent `@required` member, and only the first of those
/// carries the validation envelope. The envelope wording itself is that suite's
/// [`envelope`], reused rather than restated.
struct ShapeCase {
    name: &'static str,
    params: Vec<(String, String)>,
    code: &'static str,
    /// How many MEMBERS the request violates, for the refusals stated in the
    /// validation envelope; `None` for a refusal that carries no envelope.
    violations: Option<usize>,
    /// Fragments the message must carry: the member path the clause names and
    /// the requirement that member failed.
    fragments: Vec<String>,
}

/// Borrow a built parameter list for [`Emulator::call`].
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn owned(params: &[(&str, &str)]) -> Vec<(String, String)> {
    params
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// RULE: a request whose shape the model refuses is refused at the boundary,
/// before any environment is resolved, and starts no compile.
#[tokio::test]
async fn a_request_the_model_refuses_is_refused_before_the_store_is_touched() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    // An environment that DOES exist, so a row that is refused can only have
    // been refused for its shape.
    let environment = "info-shape";
    launch(&emulator, environment, SINGLE_INSTANCE, "shape").await;

    // Rows span the constraint classes this input shape carries (an enumerated
    // value set, a string length bound at each end), the positions a value can
    // fail in (one member, two members at once), an absent required member, and
    // whether the environment the row names exists at all.
    let under_min = "ebx";
    let at_max = "n".repeat(40);
    let over_max = "n".repeat(41);
    let modelled: Vec<String> = eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    let enum_clause = |value: &str| {
        format!(
            "Value '{value}' at 'infoType' failed to satisfy constraint: Member must satisfy \
             enum value set: ["
        )
    };

    let cases = vec![
        ShapeCase {
            name: "an InfoType outside the modelled value set",
            params: owned(&[("EnvironmentName", environment), ("InfoType", "logs")]),
            code: "ValidationError",
            violations: Some(1),
            fragments: [vec![enum_clause("logs")], modelled.clone()].concat(),
        },
        ShapeCase {
            name: "a modelled InfoType in the wrong case",
            params: owned(&[("EnvironmentName", environment), ("InfoType", "Tail")]),
            code: "ValidationError",
            violations: Some(1),
            fragments: vec![enum_clause("Tail")],
        },
        ShapeCase {
            name: "an InfoType outside the set beside an environment that does not exist",
            params: owned(&[("EnvironmentName", "info-ghost"), ("InfoType", "logs")]),
            code: "ValidationError",
            violations: Some(1),
            fragments: vec![enum_clause("logs")],
        },
        ShapeCase {
            name: "an EnvironmentName one character under the modelled minimum of 4",
            params: owned(&[("EnvironmentName", under_min), ("InfoType", "tail")]),
            code: "ValidationError",
            violations: Some(1),
            fragments: vec![format!(
                "Value '{under_min}' at 'environmentName' failed to satisfy constraint: Member \
                 must have length greater than or equal to 4"
            )],
        },
        ShapeCase {
            name: "an EnvironmentName one character over the modelled maximum of 40",
            params: owned(&[
                ("EnvironmentName", over_max.as_str()),
                ("InfoType", "bundle"),
            ]),
            code: "ValidationError",
            violations: Some(1),
            fragments: vec![
                "at 'environmentName' failed to satisfy constraint: Member must have length less \
                 than or equal to 40"
                    .to_string(),
            ],
        },
        ShapeCase {
            name: "two members outside their constraints at once",
            params: owned(&[
                ("EnvironmentName", over_max.as_str()),
                ("InfoType", "logs"),
            ]),
            code: "ValidationError",
            violations: Some(2),
            fragments: vec![
                "at 'environmentName'".to_string(),
                "at 'infoType'".to_string(),
            ],
        },
        ShapeCase {
            name: "no InfoType at all, the one required member",
            params: owned(&[("EnvironmentName", environment)]),
            code: "MissingRequiredParameter",
            violations: None,
            fragments: vec![
                "Value null at 'infoType' failed to satisfy constraint: Member must not be null"
                    .to_string(),
            ],
        },
    ];

    for action in BOTH_OPERATIONS {
        for case in &cases {
            let name = format!("{action}: {}", case.name);
            let refused = emulator.call(action, &borrowed(&case.params)).await;
            assert_eq!(
                refused.status, 400,
                "case {name:?}: expected HTTP 400, got {} and body {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                case.code,
                "case {name:?}: expected error code {}, got {:?} in body {}",
                case.code,
                refused.error_code(),
                refused.body
            );
            let message = refused.error_message();
            if let Some(violations) = case.violations {
                let expected = envelope(violations);
                assert!(
                    message.starts_with(&expected),
                    "case {name:?}: expected a message starting {expected:?}, got {message:?}"
                );
            }
            for fragment in &case.fragments {
                assert!(
                    message.contains(fragment.as_str()),
                    "case {name:?}: expected the message to carry {fragment:?}, got {message:?}"
                );
            }
        }
    }

    // The other side of the same rule: every value the model DOES declare for
    // `EnvironmentInfoType` passes the boundary, so what the caller then gets is
    // the handler's own answer — here the refusal for an environment that does
    // not exist, which is not a request-shape refusal.
    for info_type in eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES {
        for action in BOTH_OPERATIONS {
            let name = format!("{action}: modelled {info_type} against a ghost environment");
            let answered = emulator
                .call(
                    action,
                    &[("EnvironmentName", "info-ghost"), ("InfoType", info_type)],
                )
                .await;
            assert_eq!(
                answered.error_code(),
                "InvalidParameterValue",
                "case {name:?}: a modelled InfoType must reach the handler, got {} and body {}",
                answered.status,
                answered.body
            );
            assert_eq!(
                answered.error_message(),
                eb::state::missing_environment_message(None, Some("info-ghost")),
                "case {name:?}: unexpected message in {}",
                answered.body
            );
        }
    }

    // Nothing above was served, so no compile was started for the environment
    // that does exist: once every compile the clock could owe is past due, the
    // information is still not retrievable and the store holds no record of
    // one.
    std::thread::sleep(PAST_DUE);
    for info_type in eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES {
        let name = format!("nothing compiled for {info_type}");
        let reported = retrieve(&emulator, ("EnvironmentName", environment), info_type, &name).await;
        assert!(
            reported.is_empty(),
            "case {name:?}: a refused request must start no compile, got {reported:?}"
        );
    }
    let held = emulator.store().lock().environment_info.len();
    assert_eq!(
        held, 0,
        "case \"nothing persisted\": a refused request must persist nothing, {held} record(s) held"
    );

    // The bad shapes are not merely absent from the length bound: the SAME
    // length that was refused one character over is served at the bound itself,
    // which the next test drives end to end. Here it is enough that the
    // in-bound name reaches the handler.
    for action in BOTH_OPERATIONS {
        let name = format!("{action}: a name at the maximum length reaches the handler");
        let answered = emulator
            .call(
                action,
                &[("EnvironmentName", at_max.as_str()), ("InfoType", "tail")],
            )
            .await;
        assert_eq!(
            answered.error_code(),
            "InvalidParameterValue",
            "case {name:?}: a name of {} characters is inside the bound, got {} and body {}",
            at_max.len(),
            answered.status,
            answered.body
        );
    }
}

/// RULE: an `EnvironmentName` at either published bound of its modelled length
/// is served end to end — the information a request compiles for it is the
/// information a retrieve for it reports.
#[tokio::test]
async fn an_environment_name_at_either_published_bound_is_served_end_to_end() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;

    // The two bounds the model publishes for `EnvironmentName` (len 4..40): the
    // shortest and the longest name a caller can send.
    let shortest = "eb04";
    let longest = "info-shape-environment-at-max-length-abc";
    assert_eq!(
        shortest.len(),
        4,
        "the shortest row must be the modelled minimum of 4 characters, got {shortest:?}"
    );
    assert_eq!(
        longest.len(),
        40,
        "the longest row must be the modelled maximum of 40 characters, got {longest:?}"
    );

    for environment in [shortest, longest] {
        let case = format!("{} characters", environment.len());
        launch(&emulator, environment, SINGLE_INSTANCE, &case).await;
        for info_type in eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES {
            let case = format!("{case}: {info_type}");
            ok(
                &emulator,
                &case,
                "RequestEnvironmentInfo",
                &[("EnvironmentName", environment), ("InfoType", info_type)],
            )
            .await;
            std::thread::sleep(PAST_DUE);
            let compiled =
                retrieve(&emulator, ("EnvironmentName", environment), info_type, &case).await;
            assert_eq!(
                compiled.len(),
                1,
                "case {case:?}: expected one entry for the environment's single instance, got \
                 {compiled:?}"
            );
            let entry = &compiled[0];
            assert_eq!(
                entry.info_type, *info_type,
                "case {case:?}: the entry must report the InfoType asked for, got {entry:?}"
            );
            assert!(
                !entry.ec2_instance_id.is_empty() && !entry.message.is_empty(),
                "case {case:?}: the entry must name the instance it was compiled from and the \
                 file it downloads at, got {entry:?}"
            );
            let downloaded = emulator.get_url(&entry.message).await;
            assert_eq!(
                downloaded.status, 200,
                "case {case:?}: the compiled file must download, got {} and body {}",
                downloaded.status, downloaded.body
            );
        }
    }
}
