// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteApplication` (`DELETE /applications/{Arn}`), read back
//! through `GetApplication`, `CreateApplication` and `ListApplicationAssociations`.
//!
//! The rule under test: a delete REMOVES the application from the store, so every later
//! read of it reports it as not found, its (`Namespace`, `Name`) pair and the
//! `ClientToken` of the create that minted it are free again, and the application's own
//! associations go with it. An application that is still associated is removed only when
//! the request carries `Force`.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts (see `harness.rs`), so a delete that answers 200 without
//! changing the store cannot pass. The one exception is noted where it is made: once an
//! application is gone, no wire read reaches the associations it held.
//!
//! The suites are tables: rows differ in which identifier form addressed the delete, in
//! how populated the application was, in how many associations it held and in whether
//! the request forced the delete. The recorded case — delete an application, then poll
//! `GetApplication` until it answers 404 — is one row.

use serde_json::{json, Value};

use super::harness::{encode_label, Emulator, Reply, REGION};

/// The canonical account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// `POST /applications` with `body`.
async fn create(emulator: &Emulator, body: Value) -> Reply {
    emulator
        .send("POST", "/applications", REGION, Some(body))
        .await
}

/// `GET /applications/{Arn}`: the identifier travels in a `@httpLabel`, so its `:` and
/// `/` are percent-encoded.
async fn get(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/applications/{}", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

/// `GET /applications/{ApplicationId}/associations`.
async fn list_associations(emulator: &Emulator, identifier: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/applications/{}/associations", encode_label(identifier)),
            REGION,
            None,
        )
        .await
}

/// `DELETE /applications/{Arn}`, with `Force` as the `force` query member the model
/// binds it to when `force` is set.
async fn delete(emulator: &Emulator, identifier: &str, force: Option<bool>) -> Reply {
    let path = format!("/applications/{}", encode_label(identifier));
    let uri = match force {
        None => path,
        Some(force) => format!("{path}?force={force}"),
    };
    emulator.send("DELETE", &uri, REGION, None).await
}

/// A `CreateApplication` body for `(name, namespace)`, with every member a row needs
/// beyond the required ones.
fn application_body(name: &str, namespace: &str, extra: Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Namespace": namespace,
        "ApplicationSourceConfig": {
            "ExternalUrlConfig": { "AccessUrl": "https://example.com/app" }
        },
    });
    let object = body.as_object_mut().expect("the create body is an object");
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// How many associations the store holds for `application_id`. Reached through the store
/// only because no wire read can: once the application is gone,
/// `ListApplicationAssociations` has no application to report them for.
fn stored_association_count(emulator: &Emulator, application_id: &str) -> usize {
    let state = emulator.appintegrations_state();
    let guard = state.lock();
    guard.application_associations_of(application_id).count()
}

/// Every assertion about a read of an application that is not there: the status, the
/// error code and AppIntegrations' not-found sentence, which carries the identifier the
/// request sent.
fn assert_not_found(label: &str, reply: &Reply, identifier: &str) {
    assert_eq!(
        reply.status, 404,
        "case {label:?}: expected HTTP 404, got {} / {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ResourceNotFoundException",
        "case {label:?}: expected ResourceNotFoundException, got {:?}",
        reply.error_type
    );
    let expected = format!("Could not find Application with identifier: {identifier}.");
    assert_eq!(
        reply.message().as_deref(),
        Some(expected.as_str()),
        "case {label:?}: expected {expected:?}, got {:?}",
        reply.message()
    );
}

#[tokio::test]
async fn a_deleted_application_is_reported_as_not_found_by_every_later_read() {
    // Rows differ in which identifier form the DELETE addressed the application by and
    // in how populated the application was: the delete removes the resource, not the
    // identifier form that named it.
    let forms = [
        "bare UUID",
        "full ARN",
        "UUID with a qualifier",
        "ARN with a qualifier",
    ];
    let populations: Vec<(&str, Value)> = vec![
        ("required members only", json!({})),
        (
            "every optional member set",
            json!({
                "Description": "an application about to go",
                "Permissions": ["User.Details.View", "Contact.Details.View"],
                "Tags": { "owner": "team-a" },
                "IsService": true,
                "InitializationTimeout": 30_000,
                "ApplicationConfig": { "ContactHandling": { "Scope": "CROSS_CONTACTS" } },
                "IframeConfig": { "Allow": ["microphone"] },
                "ApplicationType": "STANDARD",
                "AuthConfig": { "AuthType": "CONNECT_JWT" },
            }),
        ),
    ];

    for form in forms {
        for (population, extra) in &populations {
            let label = format!("{population} / deleted {form}");
            let emulator = Emulator::new();
            // A neighbour, so what the delete removes can be told from what it leaves.
            let neighbour = create(
                &emulator,
                application_body("neighbour-app", "shared-namespace", json!({})),
            )
            .await;
            let neighbour_id = neighbour.string("Id").expect("the create reported an Id");

            let created = create(
                &emulator,
                application_body("doomed-app", "shared-namespace", extra.clone()),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {label:?}: expected the create to answer HTTP 200, got {} / {}",
                created.status, created.body
            );
            let id = created.string("Id").expect("the create reported an Id");
            let arn = created.string("Arn").expect("the create reported an ARN");
            let identifier = match form {
                "bare UUID" => id.clone(),
                "full ARN" => arn.clone(),
                "UUID with a qualifier" => format!("{id}:1"),
                _ => format!("{arn}:1"),
            };

            let before = get(&emulator, &id).await;
            assert_eq!(
                before.status, 200,
                "case {label:?}: expected the application to be readable before the \
                 delete, got {} / {}",
                before.status, before.body
            );

            let deleted = delete(&emulator, &identifier, None).await;
            assert_eq!(
                deleted.status, 200,
                "case {label:?}: expected HTTP 200 from DeleteApplication, got {} / {}",
                deleted.status, deleted.body
            );

            // Every identifier form of the gone application now reads as not found, and
            // the sentence carries the identifier the request sent.
            for (read_form, read_identifier) in [
                ("by id", id.clone()),
                ("by arn", arn.clone()),
                ("by id with a qualifier", format!("{id}:1")),
            ] {
                let read = get(&emulator, &read_identifier).await;
                assert_not_found(&format!("{label} / read {read_form}"), &read, &read_identifier);
            }

            // A second delete has nothing to remove.
            let again = delete(&emulator, &identifier, None).await;
            assert_not_found(&format!("{label} / second delete"), &again, &identifier);
            let neighbour_read = get(&emulator, &neighbour_id).await;
            assert_eq!(
                neighbour_read.status, 200,
                "case {label:?}: expected the neighbour to still be readable, got {} / {}",
                neighbour_read.status, neighbour_read.body
            );
        }
    }
}

#[tokio::test]
async fn a_delete_frees_the_name_namespace_pair_and_the_client_token() {
    // Rows differ in the token the create carried (`ClientToken` is `len 1..2048`) and
    // in the name and namespace form: what a delete frees is the resource's identity,
    // whatever shape it had.
    for (label, name, namespace, token) in [
        (
            "a short token",
            "recreated-app",
            "recreated-namespace",
            "t".to_string(),
        ),
        (
            "a uuid-shaped token, name with the space its pattern allows",
            "Recreated App/v2",
            "Recreated.namespace-2",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "a token at the modelled maximum length",
            "a",
            "n",
            "z".repeat(2048),
        ),
    ] {
        let emulator = Emulator::new();
        let body = application_body(name, namespace, json!({ "ClientToken": token }));

        let first = create(&emulator, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected the first create to succeed, got {} / {}",
            first.status, first.body
        );
        let first_id = first.string("Id").expect("the create reported an Id");

        // While it is stored, the pair is taken and the token is remembered.
        let duplicate = create(
            &emulator,
            application_body(name, namespace, json!({ "Description": "another" })),
        )
        .await;
        assert_eq!(
            duplicate.status, 400,
            "case {label:?}: expected the pair to be taken while the application is \
             stored, got {} / {}",
            duplicate.status, duplicate.body
        );
        let replayed = create(&emulator, body.clone()).await;
        assert_eq!(
            replayed.string("Id"),
            Some(first_id.clone()),
            "case {label:?}: expected the token to report the stored application, got {:?}",
            replayed.string("Id")
        );

        let deleted = delete(&emulator, &first_id, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: expected the delete to succeed, got {} / {}",
            deleted.status, deleted.body
        );

        // The same pair is free again, and the token no longer resolves to a resource
        // that is gone: the create carrying it mints a NEW application.
        let recreated = create(&emulator, body).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: expected the freed (Namespace, Name) pair to be creatable \
             again, got {} / {}",
            recreated.status, recreated.body
        );
        let recreated_id = recreated.string("Id").expect("the create reported an Id");
        assert_ne!(
            recreated_id, first_id,
            "case {label:?}: expected a newly minted Id, got the deleted one"
        );
        let read = get(&emulator, &recreated_id).await;
        assert_eq!(
            read.string("Name").as_deref(),
            Some(name),
            "case {label:?}: expected the recreated application to be readable, got {}",
            read.body
        );
        assert_not_found(label, &get(&emulator, &first_id).await, &first_id);
    }
}

#[tokio::test]
async fn a_delete_of_an_identifier_no_application_carries_is_not_found() {
    let emulator = Emulator::new();
    let created = create(
        &emulator,
        application_body("live-app", "live-namespace", json!({})),
    )
    .await;
    let id = created.string("Id").expect("the create reported an Id");

    // Rows differ in why the identifier names no application here: a UUID never minted,
    // and ARNs that carry a live id under another account, Region or resource type.
    let unknown = "00000000-0000-0000-0000-000000000000";
    for (label, identifier) in [
        ("a UUID this service never minted", unknown.to_string()),
        (
            "the ARN of that UUID",
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:application/{unknown}"),
        ),
        (
            "a live id under another account",
            format!("arn:aws:app-integrations:{REGION}:111122223333:application/{id}"),
        ),
        (
            "a live id under another Region",
            format!("arn:aws:app-integrations:eu-west-1:{ACCOUNT}:application/{id}"),
        ),
        (
            "a live id under another resource type",
            format!("arn:aws:app-integrations:{REGION}:{ACCOUNT}:data-integration/{id}"),
        ),
    ] {
        let reply = delete(&emulator, &identifier, None).await;
        assert_not_found(label, &reply, &identifier);
    }

    // Every refused delete left the live application exactly where it was.
    let read = get(&emulator, &id).await;
    assert_eq!(
        read.status, 200,
        "a refused delete must change nothing, got {} / {}",
        read.status, read.body
    );
}

#[tokio::test]
async fn an_associated_application_is_deleted_only_when_the_request_forces_it() {
    // Rows differ in how many associations the application holds and in what the request
    // says about `Force`: the member defaults to false, so an absent one and an explicit
    // false are the same request.
    let rows: Vec<(&str, Vec<&str>, Option<bool>, bool)> = vec![
        ("no association, Force absent", vec![], None, true),
        ("no association, Force false", vec![], Some(false), true),
        ("no association, Force true", vec![], Some(true), true),
        (
            "one association, Force absent",
            vec!["connect-instance-1"],
            None,
            false,
        ),
        (
            "one association, Force false",
            vec!["connect-instance-1"],
            Some(false),
            false,
        ),
        (
            "one association, Force true",
            vec!["connect-instance-1"],
            Some(true),
            true,
        ),
        (
            "three associations, Force false",
            vec!["c-1", "c-2", "c-3"],
            Some(false),
            false,
        ),
        (
            "three associations, Force true",
            vec!["c-1", "c-2", "c-3"],
            Some(true),
            true,
        ),
    ];

    for (label, client_ids, force, expected_delete) in rows {
        let emulator = Emulator::new();
        let created = create(
            &emulator,
            application_body("associated-app", "associated-namespace", json!({})),
        )
        .await;
        let id = created.string("Id").expect("the create reported an Id");
        let arn = created.string("Arn").expect("the create reported an ARN");
        let neighbour = create(
            &emulator,
            application_body("neighbour-app", "associated-namespace", json!({})),
        )
        .await;
        let neighbour_id = neighbour.string("Id").expect("the create reported an Id");
        let neighbour_arn = neighbour.string("Arn").expect("the create reported an ARN");
        emulator.seed_application_associations(&id, &arn, &client_ids);
        // A neighbour's association is not this application's, forced or not.
        emulator.seed_application_associations(&neighbour_id, &neighbour_arn, &["other-client"]);

        let reply = delete(&emulator, &id, force).await;
        if expected_delete {
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from DeleteApplication, got {} / {}",
                reply.status, reply.body
            );
            assert_not_found(label, &get(&emulator, &id).await, &id);
            // The associations the application held went with it: no wire read can reach
            // them once the application is gone, so the store is where this is checked.
            assert_eq!(
                stored_association_count(&emulator, &id),
                0,
                "case {label:?}: expected the application's associations to be dropped \
                 with it, got {}",
                stored_association_count(&emulator, &id)
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {label:?}: expected HTTP 400 for an associated application, got \
                 {} / {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "InvalidRequestException",
                "case {label:?}: expected InvalidRequestException, got {:?}",
                reply.error_type
            );
            assert_eq!(
                reply.message(),
                Some(format!(
                    "The application {id} still has associations and cannot be deleted."
                )),
                "case {label:?}: expected the still-associated message, got {:?}",
                reply.message()
            );
            // A refused delete dropped nothing: the application and every association it
            // held are still reported.
            let read = get(&emulator, &id).await;
            assert_eq!(
                read.status, 200,
                "case {label:?}: expected the application to survive a refused delete, \
                 got {} / {}",
                read.status, read.body
            );
            let listed = list_associations(&emulator, &id).await;
            let page = listed
                .field("ApplicationAssociations")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                page.len(),
                client_ids.len(),
                "case {label:?}: expected the {} seeded associations to survive, got {}",
                client_ids.len(),
                listed.body
            );
        }

        // The neighbour and its own association are untouched either way.
        let neighbour_read = get(&emulator, &neighbour_id).await;
        assert_eq!(
            neighbour_read.status, 200,
            "case {label:?}: expected the neighbour to survive, got {} / {}",
            neighbour_read.status, neighbour_read.body
        );
        assert_eq!(
            stored_association_count(&emulator, &neighbour_id),
            1,
            "case {label:?}: expected the neighbour to keep its association, got {}",
            stored_association_count(&emulator, &neighbour_id)
        );
    }
}
