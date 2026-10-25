// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE three CodeDeploy refusals carry, driven over the wire.
//!
//! RULE under test (the one this service's refusals already follow for its other
//! modelled errors): a modelled client error carries the fixed sentence of the
//! FAILED ACTION, interpolating only the request value that error is scoped to —
//! so a refusal scoped to a resource that has no identifier in the request
//! interpolates nothing, a refusal about a service-side length limit states the
//! LIMIT rather than the size of the value it rejected, and no refusal is worded
//! from the error shape's model documentation, which describes the shape to a
//! reader of the model rather than answering a caller.
//!
//! The three refusals covered here, and what each row varies:
//!
//! * `DescriptionTooLongException` — the sentence names the resource the comment
//!   is ABOUT and ends in the limit, which is the only machine-readable part of
//!   the error beyond its code, so a caller matching it recovers the bound. Rows
//!   span both operations that carry a `description` and model the error, and the
//!   length and character width of the value rejected: the sentence is the same
//!   whichever one was sent, and the figure it states is the figure enforced (the
//!   two boundary rows below).
//! * `IamArnRequiredException` — one sentence for a registration that named no
//!   IAM identity, over every way a request can fail to name one and over
//!   several instance names: nothing from the request appears in it.
//! * `RevisionDoesNotExistException` — one sentence for an application revision
//!   no registry holds, over each `RevisionLocation` form the model carries and
//!   several application names: neither the application nor the location appears
//!   in it, because the error is scoped to the revision.
//!
//! The harness (`Emulator`, `error_code`, `s3_revision`, `seeded_group`) is the
//! lifecycle suite's, so every case goes through the same front door a client
//! uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_DESCRIPTION_CHARS;

use crate::codedeploy_lifecycle::{error_code, s3_revision, seeded_group, Emulator};

/// The `$.message` of a failure body.
fn message(body: &Value, case: &str) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation`, require HTTP 400 with `code`, and answer the message.
async fn refused(
    emulator: &Emulator,
    operation: &str,
    request: Value,
    code: &str,
    case: &str,
) -> String {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    message(&body, case)
}

/// The `RevisionLocation` forms the model carries — s3Location, gitHubLocation,
/// string, appSpecContent
/// (`smithy_query(query="shape",shape="RevisionLocation")`) — each naming a
/// location nothing registers in this suite, with the values that appear in it so
/// a row can check the sentence quotes none of them.
fn unregistered_revisions() -> Vec<(&'static str, Value, Vec<String>)> {
    vec![
        (
            "an S3 object",
            s3_revision("never-registered-bucket", "never/registered.zip"),
            vec![
                "never-registered-bucket".to_string(),
                "never/registered.zip".to_string(),
            ],
        ),
        (
            "a GitHub commit",
            json!({
                "revisionType": "GitHub",
                "gitHubLocation": {"repository": "octo/ghost", "commitId": "0f1e2d3c4b5a"},
            }),
            vec!["octo/ghost".to_string(), "0f1e2d3c4b5a".to_string()],
        ),
        (
            "an inline string",
            json!({
                "revisionType": "String",
                "string": {"content": "version: 0.0\nos: linux\n", "sha256": "abc123"},
            }),
            vec!["abc123".to_string(), "os: linux".to_string()],
        ),
        (
            "inline AppSpec content",
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": "version: 0.0\nresources: []\n", "sha256": "def456"},
            }),
            vec!["def456".to_string(), "resources: []".to_string()],
        ),
    ]
}

/// RULE: `DescriptionTooLongException` says
/// `<described> description has a maximum length of <limit>` — the resource the
/// comment is about and the bound — and quotes nothing back from the request.
#[tokio::test]
async fn a_comment_past_the_bound_is_refused_with_the_limit_and_nothing_echoed() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "sentence-comments", "Server").await;

    // Rows span the LENGTH and the character WIDTH of the rejected comment: one
    // character past the bound, far past it, and multi-byte characters past it
    // (so a sentence built from the value's size, in characters or in bytes,
    // would differ row to row where the template does not).
    let rejected: Vec<(&str, String)> = vec![
        ("one character past the bound", "a".repeat(MAX_DESCRIPTION_CHARS + 1)),
        ("far past the bound", "a".repeat(MAX_DESCRIPTION_CHARS * 20)),
        ("accented past the bound", "é".repeat(MAX_DESCRIPTION_CHARS + 7)),
        ("emoji past the bound", "🚀".repeat(MAX_DESCRIPTION_CHARS + 3)),
    ];

    // Both operations that carry a `description` and model the error, with the
    // resource that operation's comment is ABOUT.
    let operations: Vec<(&str, &str, Value)> = vec![
        (
            "RegisterApplicationRevision",
            "Revision",
            json!({
                "applicationName": "sentence-comments",
                "revision": s3_revision("artifacts", "commented.zip"),
            }),
        ),
        (
            "CreateDeployment",
            "Deployment",
            json!({
                "applicationName": "sentence-comments",
                "deploymentGroupName": group,
                "revision": s3_revision("artifacts", "commented.zip"),
            }),
        ),
    ];

    for (operation, described, base) in &operations {
        // The sentence states the LIMIT, which is the bound this service
        // enforces: built from the service's own constant so a sentence that
        // stated a figure other than the enforced one fails here.
        let expected =
            format!("{described} description has a maximum length of {MAX_DESCRIPTION_CHARS}");
        for (row, description) in &rejected {
            let case = format!("{operation}/{row}");
            let mut request = base.clone();
            request["description"] = json!(description);
            let got = refused(
                &emulator,
                operation,
                request,
                "DescriptionTooLongException",
                &case,
            )
            .await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
            // Nothing about the value sent is reported: not its character count,
            // not its byte count.
            for echoed in [description.chars().count(), description.len()] {
                assert!(
                    !got.contains(&echoed.to_string()),
                    "case {case:?}: the sentence must not report the size of the comment sent \
                     ({echoed}), got {got:?}"
                );
            }
        }
    }

    // The figure the sentence states is the figure enforced: a comment OF that
    // many characters is accepted, one character more is refused. Both bounds,
    // on the operation whose own sentence names the revision.
    let at_bound = emulator
        .call(
            "RegisterApplicationRevision",
            json!({
                "applicationName": "sentence-comments",
                "revision": s3_revision("artifacts", "at-the-bound.zip"),
                "description": "a".repeat(MAX_DESCRIPTION_CHARS),
            }),
        )
        .await;
    assert_eq!(
        at_bound.0, 200,
        "case \"at the bound\": a comment of exactly {MAX_DESCRIPTION_CHARS} characters must be \
         accepted, got {} {:?}",
        at_bound.0, at_bound.1
    );
    let past_bound = refused(
        &emulator,
        "RegisterApplicationRevision",
        json!({
            "applicationName": "sentence-comments",
            "revision": s3_revision("artifacts", "past-the-bound.zip"),
            "description": "a".repeat(MAX_DESCRIPTION_CHARS + 1),
        }),
        "DescriptionTooLongException",
        "one past the bound",
    )
    .await;
    assert_eq!(
        past_bound,
        format!("Revision description has a maximum length of {MAX_DESCRIPTION_CHARS}"),
        "case \"one past the bound\": expected the limit sentence, got {past_bound:?}"
    );
    // The same sentence with the figure written out, so the bound this service
    // enforces is pinned to the published one and not merely to its own
    // constant: a constant that drifted would fail here as well as at the two
    // boundary rows above.
    assert_eq!(
        past_bound, "Revision description has a maximum length of 512",
        "case \"one past the bound\": expected the published limit, got {past_bound:?}"
    );
}

/// RULE: `IamArnRequiredException` says the registration could not be made
/// because the request named no IAM identity, and what to include instead — one
/// fixed sentence, interpolating nothing, whichever way the request failed to
/// name one.
#[tokio::test]
async fn a_registration_without_an_iam_arn_states_the_failed_action_and_the_remedy() {
    let emulator = Emulator::new();
    let expected = "The on-premises instance could not be registered because no IAM ARN was \
                    included in the request. Include either an IAM session ARN or IAM user ARN \
                    in the request, and then try again.";

    // Rows differ along HOW the request fails to name an IAM identity (both
    // members absent, either one present but empty, both present but empty) and
    // along the instance name's form, which the sentence must not quote.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("neither member", "ghost-plain", json!({})),
        ("an empty user ARN", "ghost+a=b,c.d@e_f-g", json!({"iamUserArn": ""})),
        ("an empty session ARN", "g", json!({"iamSessionArn": ""})),
        (
            "both members empty",
            "ghost-both-empty",
            json!({"iamUserArn": "", "iamSessionArn": ""}),
        ),
        (
            "a blank user ARN",
            "ghost-blank",
            json!({"iamUserArn": "   "}),
        ),
    ];

    for (case, instance, members) in &cases {
        let mut request = json!({"instanceName": instance});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member.as_str()] = value.clone();
        }
        let got = refused(
            &emulator,
            "RegisterOnPremisesInstance",
            request,
            "IamArnRequiredException",
            case,
        )
        .await;
        assert_eq!(got, expected, "case {case:?}: expected {expected:?}, got {got:?}");
        // Checked only for a name long enough to be distinctive: a
        // one-character name is a letter the fixed prose itself contains, so
        // finding it would say nothing about an echo.
        if instance.chars().count() > 3 {
            assert!(
                !got.contains(instance),
                "case {case:?}: the sentence must not quote the instance name {instance:?}, got \
                 {got:?}"
            );
        }
        // The refusal stored nothing: the name is still unregistered.
        let (status, body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": instance}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InstanceNotRegisteredException"),
            "case {case:?}: a refused registration must store nothing, got {status} {body:?}"
        );
    }
}

/// RULE: `RevisionDoesNotExistException` says that no application revision was
/// found for the revision — the error is scoped to the revision, which the
/// request identifies by a LOCATION rather than an id, so the sentence
/// interpolates nothing and names neither the application nor the location.
#[tokio::test]
async fn an_unregistered_revision_is_refused_with_a_sentence_scoped_to_the_revision() {
    let emulator = Emulator::new();
    let expected = "No application revision found for revision.";

    // Rows differ along the application the revision was looked up in (name
    // forms the modelled `@length(1..100)`/`^[A-Za-z0-9+=,.@_-]*$` admits) and
    // along the `RevisionLocation` form the request carried.
    let applications = [
        ("plain", "rev-sentence-plain".to_string()),
        ("every punctuation character the pattern admits", "rev+a=b,c.d@e_f-g".to_string()),
        ("one character", "r".to_string()),
        ("the modelled maximum length (100)", format!("rev{}", "n".repeat(97))),
    ];

    for (name_form, application) in &applications {
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": application, "computePlatform": "Server"}),
            )
            .await;
        for (location_form, revision, values) in unregistered_revisions() {
            let case = format!("{name_form}/{location_form}");
            let got = refused(
                &emulator,
                "GetApplicationRevision",
                json!({"applicationName": application, "revision": revision}),
                "RevisionDoesNotExistException",
                &case,
            )
            .await;
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?}, got {got:?}"
            );
            // As above, only a name long enough to be distinctive: the fixed
            // prose contains single letters of its own.
            if application.chars().count() > 3 {
                assert!(
                    !got.contains(application.as_str()),
                    "case {case:?}: the sentence must not name the application {application:?}, \
                     got {got:?}"
                );
            }
            for value in &values {
                assert!(
                    !got.contains(value.as_str()),
                    "case {case:?}: the sentence must not name the location value {value:?}, got \
                     {got:?}"
                );
            }
        }

        // The same sentence after a DIFFERENT revision of the same application
        // was registered: what is missing is still only the revision asked for.
        let registered = s3_revision("artifacts", "registered.zip");
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": application, "revision": registered}),
            )
            .await;
        let case = format!("{name_form}/after another revision is registered");
        let got = refused(
            &emulator,
            "GetApplicationRevision",
            json!({
                "applicationName": application,
                "revision": s3_revision("artifacts", "still-never-registered.zip"),
            }),
            "RevisionDoesNotExistException",
            &case,
        )
        .await;
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}
