// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the precondition `DeleteAppBundle` documents, driven
//! over the wire (restJson1) through the same router the emulator boots.
//!
//! The RULE under test: a delete whose documented precondition names child
//! resources is refused — with the precondition error that operation models, and
//! without writing anything — for as long as the parent still holds one of them.
//! For `DeleteAppBundle` the children are the bundle's app authorizations and the
//! refusal is the modelled `ValidationException` (HTTP 400): the request cannot
//! succeed as written until the caller deletes them itself.
//!
//! The harness (the router, `Answer`, `encode`, `uuid_of`, the app-bundle and
//! ingestion calls) is the one `app_bundle_ingestion_lifecycle` builds, and the
//! app-authorization fixtures are the ones `app_authorization_deletion` builds;
//! only the tables below are new here.

use serde_json::{json, Value};

use crate::app_authorization_deletion::{
    as_form, authorization_arns, bundle_with_authorizations, delete_authorization,
    get_authorization, Form,
};
use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// The sentence the refusal states: AppFabric's own fixed text for an app bundle
/// that is not deletable yet, the state its documentation of the operation
/// describes (`smithy_query(query="doc", shape="DeleteAppBundle")`: "Deletes an
/// app bundle. You must delete all associated app authorizations before you can
/// delete an app bundle."). It has no variable part: it names neither the bundle
/// it refused nor which authorization is in the way.
const PRECONDITION_MESSAGE: &str = "The specified app bundle cannot be deleted.";

/// Assert one refused `DeleteAppBundle`: the modelled validation refusal, its
/// status, its wording and the members the shape requires.
fn assert_refused(case: &str, answer: &Answer, bundle: &str) {
    // The bundle the call named is in the caller's own request, not in the
    // refusal: nothing of it may reach the message.
    assert!(
        !answer.body.to_string().contains(bundle),
        "case {case:?}: the refusal must not carry the bundle it refused, got {}",
        answer.body
    );
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400 while an app authorization remains, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("message").and_then(Value::as_str),
        Some(PRECONDITION_MESSAGE),
        "case {case:?}: expected the service's fixed sentence, got {}",
        answer.body
    );
    // `reason` is required by the shape, and no member of the request is at
    // fault: it is the bundle's STATE that forbids the call.
    assert_eq!(
        answer.body.get("reason").and_then(Value::as_str),
        Some("other"),
        "case {case:?}: expected the reason of a refusal that faults no member, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("fieldList"),
        None,
        "case {case:?}: a refusal that faults no member carries no fieldList, got {}",
        answer.body
    );
}

/// RULE: `DeleteAppBundle` is refused with the modelled `ValidationException`
/// (HTTP 400) for as long as the bundle holds ANY app authorization — the
/// precondition its own documentation states — and the bundle and every
/// authorization under it survive the refusal; the delete succeeds (HTTP 204)
/// exactly once the last authorization has been deleted.
///
/// The table spans how many authorizations the bundle holds (none, through five),
/// whether they completed the connect handshake and which of the two identifier
/// forms the delete names the bundle by, because none of those may change the
/// answer. A bundle holding none is the row where the delete succeeds at once.
#[tokio::test]
async fn deleting_an_app_bundle_that_still_holds_app_authorizations_is_refused() {
    // (case, authorizations created, connected, identifier form)
    let cases: Vec<(&str, usize, bool, Form)> = vec![
        ("none, by arn", 0, false, Form::Arn),
        ("none, by uuid", 0, true, Form::Uuid),
        ("one pending, by arn", 1, false, Form::Arn),
        ("one connected, by uuid", 1, true, Form::Uuid),
        ("two pending, by uuid", 2, false, Form::Uuid),
        ("three connected, by arn", 3, true, Form::Arn),
        ("five pending, by arn", 5, false, Form::Arn),
    ];

    for (case, total, connected, form) in cases {
        let emulator = Emulator::new();
        let (bundle, authorizations) =
            bundle_with_authorizations(&emulator, 0, total, connected, case).await;
        let identifier = as_form(&bundle, form);

        // One refusal per authorization still held: the precondition is about
        // the bundle holding NONE, so removing all but the last is not enough.
        for (index, held) in authorizations.iter().enumerate() {
            let refused = emulator.delete_app_bundle(&identifier).await;
            assert_refused(case, &refused, &bundle);

            // The refusal wrote nothing: the bundle is still there, and so is
            // every authorization it holds.
            let read = emulator.get_app_bundle(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} with {} held: the refused bundle must survive, got {} with body {}",
                authorizations.len() - index,
                read.status,
                read.body
            );
            let listed = authorization_arns(&emulator, &bundle, case).await;
            assert_eq!(
                listed,
                authorizations[index..].to_vec(),
                "case {case:?}: the refusal must change nothing, expected {:?}, got {listed:?}",
                &authorizations[index..]
            );

            // Satisfy the precondition one authorization at a time.
            let deleted = delete_authorization(&emulator, &bundle, held).await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteAppAuthorization, got {} with body {}",
                deleted.status, deleted.body
            );
        }

        let deleted = emulator.delete_app_bundle(&identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once no authorization remains, got {} with body {}",
            deleted.status, deleted.body
        );
        for read_form in [bundle.clone(), uuid_of(&bundle)] {
            let read = emulator.get_app_bundle(&read_form).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: {read_form:?} must be gone, got {} with body {}",
                read.status, read.body
            );
        }
    }
}

/// RULE (the same one, through the whole resource chain): while the bundle holds
/// an app authorization the refused delete destroys nothing under it — the
/// authorization, the ingestion of its (application, tenant) pair and that
/// ingestion's destination all stay readable — and the bundle becomes deletable
/// only once the caller has walked the chain down to its last authorization.
///
/// The table spans the application and tenant of the pair, whether the ingestion
/// carries a destination, and the identifier form the delete names the bundle by.
#[tokio::test]
async fn a_refused_app_bundle_delete_destroys_nothing_under_the_bundle() {
    // (case, app, tenant, with an ingestion destination, bundle form)
    let cases: Vec<(&str, &str, &str, bool, Form)> = vec![
        ("SLACK, no destination, by arn", "SLACK", "t-1", false, Form::Arn),
        ("ZENDESK, a destination, by uuid", "ZENDESK", "tenant-z", true, Form::Uuid),
        ("OKTA, a destination, by arn", "OKTA", "tenant-o", true, Form::Arn),
        ("M365, no destination, by uuid", "M365", "tenant-m", false, Form::Uuid),
    ];

    for (case, app, tenant, with_destination, form) in cases {
        let emulator = Emulator::new();
        let bundle = emulator.fixture_bundle(0).await;
        let created = emulator
            .create_ingestion(
                &bundle,
                json!({"app": app, "tenantId": tenant, "ingestionType": "auditLog"}),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateIngestion, got {} with body {}",
            created.status, created.body
        );
        let ingestion = arn_of(&created.body, "ingestion");
        let destinations_uri = format!(
            "/appbundles/{}/ingestions/{}/ingestiondestinations",
            encode(&bundle),
            encode(&ingestion)
        );
        let destination = if with_destination {
            let answer = emulator
                .call(
                    "POST",
                    &destinations_uri,
                    Some(json!({
                        "processingConfiguration": {"auditLog": {"schema": "ocsf", "format": "json"}},
                        "destinationConfiguration": {"auditLog": {"destination":
                            {"s3Bucket": {"bucketName": "audit-logs-bucket"}}}},
                    })),
                )
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected HTTP 201 from CreateIngestionDestination, got {} with body {}",
                answer.status, answer.body
            );
            Some(arn_of(&answer.body, "ingestionDestination"))
        } else {
            None
        };

        // The authorization `create_ingestion` had to put in place first — the
        // one the bundle holds, and therefore the one that blocks its delete.
        let authorizations = authorization_arns(&emulator, &bundle, case).await;
        assert_eq!(
            authorizations.len(),
            1,
            "case {case:?}: expected the ingestion's one app authorization, got {authorizations:?}"
        );
        let authorization = authorizations[0].clone();
        let identifier = as_form(&bundle, form);

        let refused = emulator.delete_app_bundle(&identifier).await;
        assert_refused(case, &refused, &bundle);

        // Nothing under the bundle was destroyed.
        let read = emulator.get_app_bundle(&identifier).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the refused bundle must survive, got {} with body {}",
            read.status, read.body
        );
        let read = get_authorization(&emulator, &bundle, &authorization).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the app authorization must survive, got {} with body {}",
            read.status, read.body
        );
        let read = emulator.get_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the ingestion must survive, got {} with body {}",
            read.status, read.body
        );
        if let Some(destination) = &destination {
            let read = emulator
                .call(
                    "GET",
                    &format!("{destinations_uri}/{}", encode(destination)),
                    None,
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the ingestion destination must survive, got {} with body {}",
                read.status, read.body
            );
        }

        // Walk the chain down in the order the three operations document, and
        // check the bundle stays protected until its last authorization is gone.
        if let Some(destination) = &destination {
            let deleted = emulator
                .call(
                    "DELETE",
                    &format!("{destinations_uri}/{}", encode(destination)),
                    None,
                )
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?}: expected HTTP 204 from DeleteIngestionDestination, got {} with body {}",
                deleted.status, deleted.body
            );
            assert_refused(case, &emulator.delete_app_bundle(&identifier).await, &bundle);
        }

        let deleted = emulator.delete_ingestion(&bundle, &ingestion).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from DeleteIngestion, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_refused(case, &emulator.delete_app_bundle(&identifier).await, &bundle);

        let deleted = delete_authorization(&emulator, &bundle, &authorization).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 from DeleteAppAuthorization, got {} with body {}",
            deleted.status, deleted.body
        );

        let deleted = emulator.delete_app_bundle(&identifier).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected HTTP 204 once the chain is gone, got {} with body {}",
            deleted.status, deleted.body
        );
        let read = emulator.get_app_bundle(&identifier).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the deleted bundle must be gone, got {} with body {}",
            read.status, read.body
        );
    }
}
