// SPDX-License-Identifier: CC0-1.0

//! When a prepared CloudFormation template becomes usable, end to end over HTTP
//! (`CreateCloudFormationTemplate`, `GetCloudFormationTemplate`).
//!
//! The rule under test, asserted over a SET of publishes rather than one recorded request:
//! the "template creation workflow" `Status` names (`smithy_query(query="doc",
//! shape="CreateCloudFormationTemplateResponse", member="Status")`) is finished by the time
//! the create answers, so a prepared template reports `ACTIVE` in the 201 of the create
//! itself and in every read of it from that instant on; the only status change left is the
//! one the clock owes at the end of its life, `EXPIRED` from the `ExpirationTime` the record
//! reports onward. A client that reads the create response and deploys, without polling for
//! a transition, therefore sees a usable template — and `PREPARING` is reported by no read
//! at any age.
//!
//! The rows differ along what the rule generalises over: how the version being prepared
//! published its template (inline JSON, inline YAML with the short-form intrinsics, or a
//! link the publisher supplied), whether the prepare names a version or takes the
//! application's latest, which of several published versions that is, and how many
//! templates one version has prepared.
//!
//! The harness (`Repo`, `Answer`) is the `applications` suite's, so these cases drive the
//! same wire path a `serverlessrepo` SDK client does: the router resolves this service from
//! the request's SigV4 credential scope, restJson1 routes on method + URI, and the handler
//! answers out of the one shared engine.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::state;

use super::applications::{created, Answer, Repo};
use super::cloudformation_templates::{age_template, prepare, read_template};
use super::version_templates::{sam_template, SAM_TRANSFORM};

/// The status a prepared template reports for as long as it lives.
const ACTIVE: &str = "ACTIVE";

/// The status it reports from its own `ExpirationTime` on.
///
/// These two are the only values of the `Status` enum
/// (`smithy_query(query="shape", shape="Status")`: PREPARING | ACTIVE | EXPIRED) a read of
/// this service can answer: the create finishes the creation workflow before it answers, so
/// no read reports the workflow's first state.
const EXPIRED: &str = "EXPIRED";

/// The same document written as the YAML a SAM CLI publish sends, short-form intrinsics
/// and all.
fn yaml_body(resource: &str) -> Value {
    json!({
        "templateBody": format!(
            "Transform: {SAM_TRANSFORM}\n\
             Parameters:\n  \
               QueueVisibility:\n    \
                 Type: Number\n    \
                 Default: 30\n\
             Resources:\n  \
               {resource}:\n    \
                 Type: AWS::SQS::Queue\n    \
                 Properties:\n      \
                   VisibilityTimeout: !Ref QueueVisibility\n"
        )
    })
}

/// A version published as a LINK the publisher supplied, so nothing of the body is this
/// service's to prepare.
fn supplied_link(semantic_version: &str) -> Value {
    json!({ "templateUrl": format!("https://example.com/packaged/{semantic_version}/template.yaml") })
}

/// The `status` member of an answer, or `""` when it carries none.
fn status(answer: &Answer) -> &str {
    answer.at("status").as_str().unwrap_or_default()
}

/// Publish `semantic_version` of `application_id` with `request`.
async fn publish(repo: &Repo, application_id: &str, semantic_version: &str, request: Value) {
    let answer = repo
        .create_version(application_id, semantic_version, request)
        .await;
    assert_eq!(
        answer.status, 201,
        "publishing {semantic_version}: expected 201, got {} with {}",
        answer.status, answer.body
    );
}

/// One row of the readiness table: the case name, the versions to publish (each with the
/// request that publishes it), the `SemanticVersion` the prepare names (`None` for the
/// application's latest), and the version therefore prepared.
type ReadinessCase<'a> = (&'a str, Vec<(&'a str, Value)>, Option<&'a str>, &'a str);

/// RULE: the create's own response reports a template that is already `ACTIVE`, and so does
/// every read of it that follows, whatever the version it was prepared from published and
/// whether the request named that version or took the latest.
#[tokio::test]
async fn a_prepared_template_is_active_in_the_create_response_and_in_every_read() {
    let repo = Repo::new();

    // (case, the versions published, the version the prepare names, the version expected)
    let cases: Vec<ReadinessCase<'_>> = vec![
        (
            "a version published as an inline JSON SAM template",
            vec![("1.0.0", sam_template("Queue"))],
            Some("1.0.0"),
            "1.0.0",
        ),
        (
            "a version published as an inline YAML SAM template",
            vec![("1.0.0", yaml_body("Q"))],
            Some("1.0.0"),
            "1.0.0",
        ),
        (
            "a version published as a link the publisher supplied",
            vec![("1.0.0", supplied_link("1.0.0"))],
            Some("1.0.0"),
            "1.0.0",
        ),
        (
            "the application's latest version, which the request does not name",
            vec![
                ("1.0.0", sam_template("First")),
                ("2.1.0", yaml_body("Latest")),
            ],
            None,
            "2.1.0",
        ),
        (
            "an older version of several, which the request does name",
            vec![
                ("1.0.0", sam_template("First")),
                ("2.1.0", yaml_body("Latest")),
            ],
            Some("1.0.0"),
            "1.0.0",
        ),
    ];

    for (index, (case, versions, named, expected_version)) in cases.iter().enumerate() {
        let application_id = created(&repo, &format!("readiness-app-{index}")).await;
        for (semantic_version, request) in versions {
            publish(&repo, &application_id, semantic_version, request.clone()).await;
        }

        // Two prepares of one version, so the rule holds for every template a version has
        // prepared rather than only for the first.
        for attempt in 0..2 {
            let prepared = prepare(&repo, &application_id, *named).await;
            assert_eq!(
                prepared.status, 201,
                "case {case:?} (attempt {attempt}): expected 201, got {} with {}",
                prepared.status, prepared.body
            );
            assert_eq!(
                prepared.at("semanticVersion").as_str(),
                Some(*expected_version),
                "case {case:?} (attempt {attempt}): expected the version \
                 {expected_version:?}, got {}",
                prepared.at("semanticVersion")
            );
            assert_eq!(
                status(&prepared),
                ACTIVE,
                "case {case:?} (attempt {attempt}): the create's own response reports a \
                 template that is already {ACTIVE}, got {} in {}",
                prepared.at("status"),
                prepared.body
            );

            // The read that a client polling for a transition would make answers the same
            // status, so there is no window in which the prepared template is unusable.
            let template_id = prepared
                .at("templateId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            for read_index in 0..3 {
                let read = read_template(&repo, &application_id, &template_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?} (attempt {attempt}, read {read_index}): expected 200, \
                     got {} with {}",
                    read.status, read.body
                );
                assert_eq!(
                    status(&read),
                    ACTIVE,
                    "case {case:?} (attempt {attempt}, read {read_index}): a read of a \
                     freshly prepared template reports {ACTIVE}, got {} in {}",
                    read.at("status"),
                    read.body
                );
            }
        }
    }
}

/// RULE, at the one instant that moves a prepared template's status: the status is `EXPIRED`
/// exactly from the `ExpirationTime` the record itself reports, and `ACTIVE` at every
/// earlier age — including the instant of the create, so the workflow's first state is
/// reported at no age at all.
///
/// The ages sweep the whole window from the instant of the create to long past the expiry,
/// including both sides of the published bound and the bound itself. Each row is prepared
/// through the wire and its stored instants are then held at that age, because the window is
/// the clock's and no caller can shorten it.
#[tokio::test]
async fn the_only_status_a_prepared_template_moves_to_is_expired_at_its_expiration_time() {
    let repo = Repo::new();
    let application_id = created(&repo, "expiry-sweep-app").await;
    publish(&repo, &application_id, "1.0.0", sam_template("Queue")).await;

    let lifetime = state::TEMPLATE_LIFETIME;
    // (case, the age the record is held at, the status expected)
    let ages: Vec<(&str, Duration, &str)> = vec![
        ("the instant it was created", Duration::ZERO, ACTIVE),
        ("a second old", Duration::from_secs(1), ACTIVE),
        ("a quarter through its life", lifetime / 4, ACTIVE),
        ("halfway through its life", lifetime / 2, ACTIVE),
        (
            "a minute before it expires",
            lifetime - Duration::from_secs(60),
            ACTIVE,
        ),
        (
            "the last second of its life",
            lifetime - Duration::from_secs(1),
            ACTIVE,
        ),
        ("exactly at its expiry", lifetime, EXPIRED),
        (
            "a second past its expiry",
            lifetime + Duration::from_secs(1),
            EXPIRED,
        ),
        ("long past its expiry", lifetime * 30, EXPIRED),
    ];

    for (case, age, expected) in &ages {
        let prepared = prepare(&repo, &application_id, Some("1.0.0")).await;
        assert_eq!(
            prepared.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            prepared.status, prepared.body
        );
        let template_id = prepared
            .at("templateId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        age_template(&repo, &template_id, *age);

        let read = read_template(&repo, &application_id, &template_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            status(&read),
            *expected,
            "case {case:?}: a template {age:?} old reports {expected}, got {} in {}",
            read.at("status"),
            read.body
        );
    }
}
