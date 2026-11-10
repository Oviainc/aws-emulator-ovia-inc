// SPDX-License-Identifier: CC0-1.0

//! Preparing and reading a CloudFormation template of a published version, end to end over
//! HTTP (`CreateCloudFormationTemplate`, `GetCloudFormationTemplate`).
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! 1. preparing a template persists one record of the version it was prepared from — the
//!    version the request names, or the application's latest when it names none — under a
//!    freshly minted UUID `TemplateId`, and reports it: the `TemplateUrl` is the location
//!    of the template body PUBLISHED with that version, and the template expires exactly
//!    [`EXPIRY_WINDOW`] after it was created;
//! 2. `Status` is the state of the "template creation workflow" at the instant of the
//!    read, a function of the stored instants and the clock: `ACTIVE` from the create that
//!    prepared it until the template expires, `EXPIRED` after that — so a read never moves
//!    it and two reads at one instant answer identically;
//! 3. `GetCloudFormationTemplate` reports the stored record member for member, and a
//!    template read through an application that is not the one it was prepared from is as
//!    absent as one that was never prepared;
//! 4. an application, a version or a template the store does not hold is answered with the
//!    modelled `NotFoundException` — HTTP 404 — whose `errorCode` names WHICH of the three
//!    was absent.
//!
//! The harness (`Repo`, `Answer`) is the `applications` suite's, so these cases drive the
//! same wire path: the router resolves this service from the request's SigV4 credential
//! scope, restJson1 routes on method + URI, and the handler answers out of the one shared
//! engine.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::state::{
    self, ComAmazonawsServerlessapplicationrepositoryServerlessapplicationrepositoryState as ServiceState,
};

use super::application_policies::is_uuid_shaped;
use super::applications::{created, path_label, Answer, Repo};
use super::version_templates::SAM_TRANSFORM;

/// The window a prepared template lives for: six hours from its `CreationTime` to its
/// `ExpirationTime`.
///
/// Recorded behavior — the service reports an `ExpirationTime` exactly six hours after
/// the `CreationTime` of an `ACTIVE` prepared template. The model's own doc sentence on
/// the member says one hour ("Templates expire 1 hour after creation",
/// `smithy_query(query="doc", shape="CreateCloudFormationTemplateResponse",
/// member="ExpirationTime")`), which the service contradicts; the window clients observe
/// is the one asserted here, and it is the service's own published duration rather than a
/// span this emulator chose.
const EXPIRY_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// Prepare a template of `application_id`, naming `semantic_version` when there is one.
///
/// `pub(crate)` because the readiness of what a prepare answers — a template that is
/// `ACTIVE` in the create's own response — is a rule of its own, asserted in
/// `prepared_template_readiness.rs` over the class of publishes rather than through a
/// second copy of this call.
pub(crate) async fn prepare(
    repo: &Repo,
    application_id: &str,
    semantic_version: Option<&str>,
) -> Answer {
    let body = match semantic_version {
        Some(semantic_version) => json!({ "semanticVersion": semantic_version }),
        None => json!({}),
    };
    let path = format!("/applications/{}/templates", path_label(application_id));
    repo.call("POST", &path, Some(body)).await
}

/// Read a prepared template back.
///
/// `pub(crate)` because the wording of the 404 this read answers for a template the store
/// does not hold is a rule of its own, asserted in `deploy_refusal_messages.rs` beside the
/// other refusal wordings rather than through a second copy of this call.
pub(crate) async fn read_template(repo: &Repo, application_id: &str, template_id: &str) -> Answer {
    let path = format!(
        "/applications/{}/templates/{}",
        path_label(application_id),
        path_label(template_id)
    );
    repo.call("GET", &path, None).await
}

/// Publish `semantic_version` of `application_id` and answer with the `templateUrl` the
/// publish reports: the location of the body it published, which is what a template
/// prepared from that version must serve.
async fn published(
    repo: &Repo,
    application_id: &str,
    semantic_version: &str,
    request: Value,
) -> String {
    let answer = repo
        .create_version(application_id, semantic_version, request)
        .await;
    assert_eq!(
        answer.status, 201,
        "publishing {semantic_version}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
        .at("templateUrl")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A SAM template body naming `resource`, as a `templateBody` request member.
fn inline_template(resource: &str) -> Value {
    json!({
        "templateBody": json!({
            "Transform": SAM_TRANSFORM,
            "Resources": { resource: { "Type": "AWS::SQS::Queue" } }
        })
        .to_string()
    })
}

/// `member`, parsed as the ISO-8601 instant every time-valued `__string` of this service
/// carries, in epoch seconds.
fn epoch_seconds(case: &str, answer: &Answer, member: &str) -> i64 {
    let text = answer.at(member).as_str().unwrap_or_default().to_string();
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} is not an ISO-8601 instant: {error}")
    });
    parsed.secs()
}

/// Hold the stored template's instants as if it had been prepared `age` ago, so a
/// transition the CLOCK owes can be asserted without sleeping for it. The emulator derives
/// every status from these instants, so this is the same record a later clock would see.
///
/// `pub(crate)`: the expiry sweep in `prepared_template_readiness.rs` holds a record the
/// same way rather than writing a second copy of this.
pub(crate) fn age_template(repo: &Repo, template_id: &str, age: Duration) {
    let handle = repo.engine.state::<ServiceState>();
    let mut store = handle.lock();
    let template = store
        .templates
        .get_mut(template_id)
        .unwrap_or_else(|| panic!("the store holds template {template_id}"));
    let created_at = SystemTime::now() - age;
    template.created_at = created_at;
    template.expires_at = created_at + state::TEMPLATE_LIFETIME;
}

/// The six members a template reports that do NOT depend on the clock, as one comparable
/// value. `status` is deliberately left out: it is the record's place in the expiry window
/// at the instant of the read, so two reads either side of a transition instant are
/// SUPPOSED to differ — that member is asserted in
/// [`the_status_of_a_template_is_its_place_in_the_expiry_window`], where the age is held.
fn reported(answer: &Answer) -> Value {
    json!({
        "applicationId": answer.at("applicationId"),
        "creationTime": answer.at("creationTime"),
        "expirationTime": answer.at("expirationTime"),
        "semanticVersion": answer.at("semanticVersion"),
        "templateId": answer.at("templateId"),
        "templateUrl": answer.at("templateUrl"),
    })
}

/// RULE 1 and 3: a prepared template records the version it was prepared from, serves the
/// template body published with that version, expires [`EXPIRY_WINDOW`] after it was
/// created, and is read back member for member.
///
/// The rows differ along what the rule generalises over: whether the request names a
/// version or takes the latest, how the named version published its template (inline, or
/// as a link the publisher supplied), and which of several stored versions is the latest
/// (a prerelease has LOWER precedence than the release of the same core version, so the
/// latest is not the one published last).
#[tokio::test]
async fn a_prepared_template_reports_the_version_it_was_prepared_from() {
    let repo = Repo::new();
    let application_id = created(&repo, "template-app").await;

    // (semantic version, the publish request, the templateUrl the publish reported)
    let mut versions: Vec<(&str, String)> = Vec::new();
    for (semantic_version, request) in [
        ("1.0.0", inline_template("First")),
        (
            "2.0.0",
            json!({ "templateUrl": "https://example.com/packaged/2.0.0/template.yaml" }),
        ),
        ("3.0.0-beta.1", inline_template("Prerelease")),
        ("2.1.0", inline_template("Latest")),
    ] {
        let url = published(&repo, &application_id, semantic_version, request).await;
        versions.push((semantic_version, url));
    }
    // The latest version by semantic-version precedence: 2.1.0 beats 2.0.0 and 1.0.0, and
    // the 3.0.0-beta.1 prerelease has lower precedence than any release of 3.0.0 — but
    // higher than 2.1.0, so it IS the latest of what is stored (semver 2.0.0 \u00a711).
    let latest = ("3.0.0-beta.1", versions[2].1.clone());

    // (case, the version the request names, the version expected, the url expected)
    let mut cases: Vec<(String, Option<&str>, &str, String)> = versions
        .iter()
        .map(|(semantic_version, url)| {
            (
                format!("the request names {semantic_version}"),
                Some(*semantic_version),
                *semantic_version,
                url.clone(),
            )
        })
        .collect();
    cases.push((
        "the request names no version, so the latest is prepared".to_string(),
        None,
        latest.0,
        latest.1.clone(),
    ));

    for (case, named, expected_version, expected_url) in &cases {
        let prepared = prepare(&repo, &application_id, *named).await;
        assert_eq!(
            prepared.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            prepared.status, prepared.body
        );
        assert_eq!(
            prepared.at("applicationId").as_str(),
            Some(application_id.as_str()),
            "case {case:?}: the prepared template names its application, got {}",
            prepared.at("applicationId")
        );
        assert_eq!(
            prepared.at("semanticVersion").as_str(),
            Some(*expected_version),
            "case {case:?}: expected the version {expected_version:?}, got {}",
            prepared.at("semanticVersion")
        );
        assert_eq!(
            prepared.at("templateUrl").as_str(),
            Some(expected_url.as_str()),
            "case {case:?}: the prepared template serves the body published with that \
             version, at {expected_url:?}, got {}",
            prepared.at("templateUrl")
        );
        let template_id = prepared.at("templateId").as_str().unwrap_or_default().to_string();
        // The member documents "Pattern:
        // [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
        // (`smithy_query(query="doc", shape="CreateCloudFormationTemplateResponse",
        // member="TemplateId")`), and what this service mints is the lowercase form the
        // `statementId` check in `application_policies` already pins.
        assert!(
            is_uuid_shaped(&template_id),
            "case {case:?}: the templateId is a UUID, got {template_id:?}"
        );
        // The creation workflow finishes within the create, so the template is deployable
        // from this very response.
        assert_eq!(
            prepared.at("status").as_str(),
            Some("ACTIVE"),
            "case {case:?}: a freshly prepared template is ACTIVE, got {}",
            prepared.at("status")
        );
        let created_at = epoch_seconds(case, &prepared, "creationTime");
        let expires_at = epoch_seconds(case, &prepared, "expirationTime");
        assert_eq!(
            expires_at - created_at,
            EXPIRY_WINDOW.as_secs() as i64,
            "case {case:?}: a template expires {EXPIRY_WINDOW:?} after creation, got {} then {}",
            prepared.at("creationTime"),
            prepared.at("expirationTime")
        );

        // RULE 3: the read answers out of the stored record, member for member — only the
        // clock-owed status may differ, and no read moves anything.
        let read = read_template(&repo, &application_id, &template_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the prepared template: expected 200, got {} with {}",
            read.status, read.body
        );
        let again = read_template(&repo, &application_id, &template_id).await;
        assert_eq!(
            reported(&read),
            reported(&again),
            "case {case:?}: two reads of one template report the same stored record, got {} \
             then {}",
            read.body,
            again.body
        );
        for member in [
            "applicationId",
            "creationTime",
            "expirationTime",
            "semanticVersion",
            "templateId",
            "templateUrl",
        ] {
            assert_eq!(
                read.at(member),
                prepared.at(member),
                "case {case:?}: the read reports the stored {member}, got {} not {}",
                read.at(member),
                prepared.at(member)
            );
        }
    }

    // Every prepare mints its own template, so no two of them share an id and each is
    // readable on its own.
    let first = prepare(&repo, &application_id, Some("1.0.0")).await;
    let second = prepare(&repo, &application_id, Some("1.0.0")).await;
    assert_ne!(
        first.at("templateId"),
        second.at("templateId"),
        "two prepares of one version mint two templates, got {} twice",
        first.at("templateId")
    );
    for answer in [&first, &second] {
        let template_id = answer.at("templateId").as_str().unwrap_or_default();
        let read = read_template(&repo, &application_id, template_id).await;
        assert_eq!(
            read.status, 200,
            "case {template_id:?}: each minted template is readable, got {} with {}",
            read.status, read.body
        );
    }
}

/// RULE 2: the status a template reports is the state of the creation workflow AT THE
/// INSTANT OF THE READ, derived from the stored instants.
///
/// The rows span the whole window by the age of the record: fresh, a second old, mid-life,
/// the last second before expiry, exactly at expiry, and long past it. Each row is prepared
/// through the wire and then its stored instants are held at that age, because the one
/// published bound of the window — the life of a prepared template — is the clock's, not a
/// caller's.
#[tokio::test]
async fn the_status_of_a_template_is_its_place_in_the_expiry_window() {
    let repo = Repo::new();
    let application_id = created(&repo, "expiry-app").await;
    published(&repo, &application_id, "1.0.0", inline_template("Queue")).await;

    assert_eq!(
        state::TEMPLATE_LIFETIME, EXPIRY_WINDOW,
        "a prepared template lives {EXPIRY_WINDOW:?}, got {:?}",
        state::TEMPLATE_LIFETIME
    );

    let lifetime = state::TEMPLATE_LIFETIME;
    // (case, the age the record is held at, the status expected, whether the next
    // transition is far enough away for a second read to be guaranteed the same status)
    let cases: Vec<(&str, Duration, &str, bool)> = vec![
        ("just prepared", Duration::ZERO, "ACTIVE", true),
        ("a second old", Duration::from_secs(1), "ACTIVE", true),
        ("halfway through its life", lifetime / 2, "ACTIVE", true),
        (
            "the last second of its life",
            lifetime - Duration::from_secs(1),
            "ACTIVE",
            false,
        ),
        ("exactly at its expiry", lifetime, "EXPIRED", true),
        ("long past its expiry", lifetime * 25, "EXPIRED", true),
    ];

    for (case, age, expected, stable) in &cases {
        let prepared = prepare(&repo, &application_id, Some("1.0.0")).await;
        assert_eq!(
            prepared.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            prepared.status, prepared.body
        );
        let template_id = prepared.at("templateId").as_str().unwrap_or_default().to_string();
        age_template(&repo, &template_id, *age);

        let read = read_template(&repo, &application_id, &template_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200, got {} with {}",
            read.status, read.body
        );
        assert_eq!(
            read.at("status").as_str(),
            Some(*expected),
            "case {case:?}: a template {age:?} old is {expected}, got {}",
            read.at("status")
        );
        // A read reports the window; it never shortens or extends it.
        let created_at = epoch_seconds(case, &read, "creationTime");
        let expires_at = epoch_seconds(case, &read, "expirationTime");
        assert_eq!(
            expires_at - created_at,
            EXPIRY_WINDOW.as_secs() as i64,
            "case {case:?}: the expiry window stays {EXPIRY_WINDOW:?}, got {} then {}",
            read.at("creationTime"),
            read.at("expirationTime")
        );
        let again = read_template(&repo, &application_id, &template_id).await;
        if *stable {
            // Only where the next transition is far away: a row held AT a transition
            // instant is supposed to differ across it, so asserting equality there would
            // be a race, not a rule.
            assert_eq!(
                again.at("status"),
                read.at("status"),
                "case {case:?}: reading a template does not move its status, got {} then {}",
                read.at("status"),
                again.at("status")
            );
        }
        assert_eq!(
            reported(&again),
            reported(&read),
            "case {case:?}: reading a template changes nothing stored about it, got {} then {}",
            read.body,
            again.body
        );
    }
}

/// RULE 4: an application, a version or a template the store does not hold is answered
/// with the modelled 404, whose `errorCode` names which of the three was absent.
///
/// The rows span every absence either operation can meet, including the two that are
/// absences only in context: a template read through an application that is not the one it
/// was prepared from, and a template whose application has since been deleted.
#[tokio::test]
async fn an_absent_application_version_or_template_is_a_modelled_404() {
    let repo = Repo::new();
    let application_id = created(&repo, "present-app").await;
    published(&repo, &application_id, "1.0.0", inline_template("Queue")).await;
    let other_id = created(&repo, "other-app").await;
    published(&repo, &other_id, "1.0.0", inline_template("Queue")).await;

    let mine = prepare(&repo, &application_id, Some("1.0.0")).await;
    let template_id = mine.at("templateId").as_str().unwrap_or_default().to_string();

    let versionless = created(&repo, "versionless-app").await;
    let absent_application =
        "arn:aws:serverlessrepo:us-west-2:000000000000:applications/never-created";

    // (case, the answer, the condition the body must name)
    let cases: Vec<(&str, Answer, &str)> = vec![
        (
            "preparing a template of an application that was never created",
            prepare(&repo, absent_application, Some("1.0.0")).await,
            "ApplicationNotFoundException",
        ),
        (
            "preparing a version the application does not carry",
            prepare(&repo, &application_id, Some("9.9.9")).await,
            "VersionNotFoundException",
        ),
        (
            "preparing a version whose name is not a semantic version at all",
            prepare(&repo, &application_id, Some("not-a-version")).await,
            "VersionNotFoundException",
        ),
        (
            "reading a template of an application that was never created",
            read_template(&repo, absent_application, &template_id).await,
            "ApplicationNotFoundException",
        ),
        (
            "reading a template id that was never minted",
            read_template(
                &repo,
                &application_id,
                "00000000-0000-0000-0000-000000000000",
            )
            .await,
            "TemplateNotFoundException",
        ),
        (
            "reading one application's template through another application",
            read_template(&repo, &other_id, &template_id).await,
            "TemplateNotFoundException",
        ),
    ];

    for (case, answer, error_code) in &cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            answer.at("errorCode"),
            &json!(error_code),
            "case {case:?}: the body's errorCode names the condition, got {}",
            answer.at("errorCode")
        );
    }

    // An application carrying NO version is not an absent resource: nothing the caller
    // named is missing, the application simply has nothing publishable to prepare a
    // template from, so the request is refused as one the service cannot act on.
    let no_versions = prepare(&repo, &versionless, None).await;
    assert_eq!(
        no_versions.status, 400,
        "preparing a template of a versionless application: expected 400, got {} with {}",
        no_versions.status, no_versions.body
    );
    assert_eq!(
        no_versions.error_code(),
        "BadRequestException",
        "preparing a template of a versionless application: expected the modelled \
         BadRequestException, got {}",
        no_versions.body
    );
    assert_eq!(
        no_versions.at("errorCode"),
        &json!("InvalidRequestException"),
        "preparing a template of a versionless application: the body's errorCode names the \
         condition, got {}",
        no_versions.at("errorCode")
    );
    assert_eq!(
        no_versions.at("message").as_str().unwrap_or_default(),
        "CloudFormation template can't be created from application with no version.",
        "preparing a template of a versionless application: the refusal carries this \
         path's own fixed sentence, got {}",
        no_versions.at("message")
    );

    // The template is a sub-resource of its application, so deleting the application makes
    // it absent too — and nothing of it is left behind in the store.
    let deleted = repo.delete_application(&application_id).await;
    assert_eq!(
        deleted.status, 204,
        "deleting the application: expected 204, got {} with {}",
        deleted.status, deleted.body
    );
    let orphan = read_template(&repo, &application_id, &template_id).await;
    assert_eq!(
        orphan.status, 404,
        "a template of a deleted application is absent, got {} with {}",
        orphan.status, orphan.body
    );
    let held = repo
        .engine
        .state::<ServiceState>()
        .lock()
        .templates
        .contains_key(&template_id);
    assert!(
        !held,
        "case {template_id:?}: a deleted application releases its prepared templates"
    );
}

/// The store's own ceiling: a prepared template is released only by the clock, so at the
/// ceiling the oldest EXPIRED template makes room for the next prepare, and a ceiling with
/// nothing expired refuses it as the server-side condition it is (HTTP 500) rather than by
/// evicting a template another caller can still deploy.
#[tokio::test]
async fn the_template_store_releases_an_expired_template_before_it_refuses_a_prepare() {
    let repo = Repo::new();
    let application_id = created(&repo, "ceiling-app").await;
    published(&repo, &application_id, "1.0.0", inline_template("Queue")).await;

    // Fill the store to the ceiling with live templates of this application, written
    // through the state the handler reads so the records are the ones it would have made.
    let now = SystemTime::now();
    let live: Vec<String> = (0..state::MAX_STORED_TEMPLATES)
        .map(|index| format!("{index:08x}-0000-4000-8000-000000000000"))
        .collect();
    {
        let store = repo.engine.state::<ServiceState>();
        let mut store = store.lock();
        for template_id in &live {
            store.templates.insert(
                template_id.clone(),
                state::CloudFormationTemplate::prepared(
                    template_id.clone(),
                    application_id.clone(),
                    "1.0.0".to_string(),
                    "https://example.com/packaged/1.0.0/template.yaml".to_string(),
                    now,
                ),
            );
        }
    }

    let refused = prepare(&repo, &application_id, Some("1.0.0")).await;
    assert_eq!(
        refused.status, 500,
        "a store at its ceiling with nothing expired refuses the prepare: expected 500, \
         got {} with {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InternalServerErrorException",
        "the ceiling is a server-side condition, got {}",
        refused.body
    );
    let still_held = repo
        .engine
        .state::<ServiceState>()
        .lock()
        .templates
        .len();
    assert_eq!(
        still_held,
        state::MAX_STORED_TEMPLATES,
        "a refused prepare evicts no live template, got {still_held} stored"
    );

    // One template expires: the clock already ended its life, so it is what makes room.
    let expired = live[7].clone();
    age_template(&repo, &expired, state::TEMPLATE_LIFETIME * 2);
    let accepted = prepare(&repo, &application_id, Some("1.0.0")).await;
    assert_eq!(
        accepted.status, 201,
        "the expired template makes room for the next prepare: expected 201, got {} with {}",
        accepted.status, accepted.body
    );
    let minted = accepted.at("templateId").as_str().unwrap_or_default().to_string();
    let store = repo.engine.state::<ServiceState>();
    let store = store.lock();
    assert!(
        !store.templates.contains_key(&expired),
        "case {expired:?}: the expired template was released"
    );
    assert!(
        store.templates.contains_key(&minted),
        "case {minted:?}: the newly prepared template is stored"
    );
    assert_eq!(
        store.templates.len(),
        state::MAX_STORED_TEMPLATES,
        "the store stays at its ceiling, got {} stored",
        store.templates.len()
    );
}
