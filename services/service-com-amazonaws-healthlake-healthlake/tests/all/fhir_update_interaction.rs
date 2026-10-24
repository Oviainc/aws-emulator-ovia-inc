// SPDX-License-Identifier: CC0-1.0

//! The FHIR `update` interaction of a data store's REST data plane:
//! `PUT [base]/{type}/{id}`.
//!
//! The rule under test, over a SET of inputs rather than one recorded case: a `PUT`
//! stores the submitted resource AT the logical id the URL names — answering `201` when
//! the data store holds nothing there (FHIR's `update-as-create`) and `200` when it
//! replaces the version it does hold — the stored version's `meta.versionId` advances
//! with every write, and a `GET` of that id then answers exactly what was stored.
//!
//! The rows differ along what the rule generalises over: the resource TYPE, the FORM of
//! the caller-chosen id (plain, hyphenated, dotted, digits only, one character, the
//! published 64-character maximum), the submitted content, and whether the resource is
//! being created at that id or replaced there. The case the issue recorded
//! (`PUT [base]/Patient/sim2real-patient-1` with a minimal Patient) is one row.
//!
//! The id alphabet and length asserted here are FHIR R4's own `id` primitive type —
//! "Any combination of upper- or lower-case ASCII letters, numerals, '-' and '.', with
//! a length limit of 64 characters" — which the handler's `is_logical_id` enforces.
//!
//! The harness (`HealthLake`, `outcome_code`) is the sibling suite's: one way of driving
//! this service through the production router, not two.

use serde_json::{json, Value};

use super::fhir_rest_and_validate_source::{outcome_code, HealthLake};

/// FHIR R4's `id` primitive type: 1..64 characters of ASCII letters, digits, `-` and
/// `.`. Asserted below at one character, at a value the issue did not cite, and at the
/// published maximum — and one past the maximum, which must be refused.
const MAX_LOGICAL_ID_LEN: usize = 64;

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    resource_type: &'static str,
    id: String,
    resource: Value,
}

fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the recorded case: a minimal Patient at a hyphenated id",
            resource_type: "Patient",
            id: "sim2real-patient-1".to_string(),
            resource: json!({
                "resourceType": "Patient",
                "name": [{ "family": "Lovelace", "given": ["Ada"] }],
            }),
        },
        UpdateCase {
            case: "a one-character id",
            resource_type: "Patient",
            id: "a".to_string(),
            resource: json!({ "resourceType": "Patient", "active": true }),
        },
        UpdateCase {
            case: "an id of digits only",
            resource_type: "Observation",
            id: "12345".to_string(),
            resource: json!({
                "resourceType": "Observation",
                "status": "final",
                "code": { "coding": [{ "system": "http://loinc.org", "code": "8867-4" }] },
                "valueQuantity": { "value": 72, "unit": "beats/minute" },
            }),
        },
        UpdateCase {
            case: "a dotted id, and nested content",
            resource_type: "MedicationRequest",
            id: "rx.2026.01.01".to_string(),
            resource: json!({
                "resourceType": "MedicationRequest",
                "status": "active",
                "intent": "order",
                "subject": { "reference": "Patient/sim2real-patient-1" },
            }),
        },
        UpdateCase {
            case: "an id at the published 64-character maximum",
            resource_type: "Practitioner",
            id: "p".repeat(MAX_LOGICAL_ID_LEN),
            resource: json!({
                "resourceType": "Practitioner",
                "name": [{ "family": "Hopper", "given": ["Grace"] }],
            }),
        },
        UpdateCase {
            case: "a body that carries the same id as the URL",
            resource_type: "Patient",
            id: "echoed-id".to_string(),
            resource: json!({
                "resourceType": "Patient",
                "id": "echoed-id",
                "meta": { "profile": ["http://example.org/fhir/StructureDefinition/demo"] },
                "name": [{ "family": "Turing" }],
            }),
        },
    ]
}

/// One write this suite makes, and what its answer must state.
struct Write<'a> {
    case: &'a str,
    resource_type: &'a str,
    id: &'a str,
    resource: &'a Value,
    expected_status: u16,
    expected_version: &'a str,
}

/// The stored resource a successful update answered with, after the facts every such
/// answer must state have been asserted.
async fn updated(service: &HealthLake, path: &str, write: Write<'_>) -> Value {
    let Write {
        case,
        resource_type,
        id,
        resource,
        expected_status,
        expected_version,
    } = write;
    let (status, body, headers) = service
        .fhir("PUT", &format!("{path}{resource_type}/{id}"), resource)
        .await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status} from the update, got {status}: {body}"
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/fhir+json"),
        "case {case:?}: a FHIR response carries the FHIR media type, got {headers:?}"
    );
    assert_eq!(
        body["id"].as_str(),
        Some(id),
        "case {case:?}: the update stores the resource at the id the URL names, got {body}"
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some(resource_type),
        "case {case:?}: the stored resource keeps its type, got {body}"
    );
    assert_eq!(
        body["meta"]["versionId"].as_str(),
        Some(expected_version),
        "case {case:?}: expected meta.versionId {expected_version:?}, got {body}"
    );
    assert_eq!(
        headers.get("etag").map(String::as_str),
        Some(format!("W/\"{expected_version}\"").as_str()),
        "case {case:?}: the update reports the stored version as an ETag, got {headers:?}"
    );
    assert!(
        headers
            .get("location")
            .is_some_and(|location| location.contains(id)),
        "case {case:?}: the update reports a Location naming the resource, got {headers:?}"
    );
    body
}

#[tokio::test]
async fn a_put_stores_the_resource_at_the_caller_chosen_id_and_a_read_answers_it() {
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-update").await;

    for UpdateCase {
        case,
        resource_type,
        id,
        resource,
    } in update_cases()
    {
        // update-as-create: the data store holds nothing at this id yet, so the write
        // creates the resource there and answers 201 at version 1.
        let created = updated(
            &service,
            &path,
            Write {
                case,
                resource_type,
                id: &id,
                resource: &resource,
                expected_status: 201,
                expected_version: "1",
            },
        )
        .await;
        // Everything the caller submitted except the values the server owns is kept.
        for (member, value) in resource.as_object().expect("the row is an object") {
            if member == "id" || member == "meta" {
                continue;
            }
            assert_eq!(
                &created[member], value,
                "case {case:?}: the stored resource must keep {member}, got {created}"
            );
        }
        if let Some(profile) = resource["meta"].get("profile") {
            assert_eq!(
                &created["meta"]["profile"], profile,
                "case {case:?}: meta members the server does not own are kept, got {created}"
            );
        }

        // ROUND TRIP: the read answers exactly what the update stored.
        let read_uri = format!("{path}{resource_type}/{id}");
        let (status, read, headers) = service.fhir("GET", &read_uri, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read of the created id answers 200, got {status}: {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: a read must answer the stored resource, got {read}"
        );
        assert_eq!(
            headers.get("etag").map(String::as_str),
            Some("W/\"1\""),
            "case {case:?}: the read reports the stored version, got {headers:?}"
        );

        // The same id again REPLACES what is held there: 200, the next version, and the
        // new content — and still exactly one resource at that id.
        let replacement = json!({
            "resourceType": resource_type,
            "id": id,
            "text": { "status": "generated", "div": "<div>replaced</div>" },
        });
        let replaced = updated(
            &service,
            &path,
            Write {
                case,
                resource_type,
                id: &id,
                resource: &replacement,
                expected_status: 200,
                expected_version: "2",
            },
        )
        .await;
        assert_eq!(
            replaced["text"], replacement["text"],
            "case {case:?}: the replacement's content is what is stored, got {replaced}"
        );
        assert!(
            replaced.get("name").is_none() && replaced.get("status").is_none(),
            "case {case:?}: an update replaces the resource rather than merging into it, \
             got {replaced}"
        );
        let (status, read, _headers) = service.fhir("GET", &read_uri, &Value::Null).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read after the replacement answers 200, got {status}: {read}"
        );
        assert_eq!(
            read, replaced,
            "case {case:?}: the read answers the replacing version, got {read}"
        );

        // A third write advances the version again, so the version is the count of
        // writes rather than a constant.
        let third = updated(
            &service,
            &path,
            Write {
                case,
                resource_type,
                id: &id,
                resource: &json!({ "resourceType": resource_type }),
                expected_status: 200,
                expected_version: "3",
            },
        )
        .await;
        assert_eq!(
            third["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the id is stable across versions, got {third}"
        );
    }
}

#[tokio::test]
async fn a_put_is_scoped_to_its_type_its_id_and_its_data_store() {
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-update-scope").await;
    let (_other_id, other_path) = service.active_datastore("fhir-update-other").await;

    let id = "scoped-1";
    let (status, _body, _headers) = service
        .fhir(
            "PUT",
            &format!("{path}Patient/{id}"),
            &json!({ "resourceType": "Patient", "name": [{ "family": "Noether" }] }),
        )
        .await;
    assert_eq!(status, 201, "the update-as-create must answer 201");

    // (case, the uri a read is sent to, the status it must answer)
    let reads: Vec<(&str, String, u16)> = vec![
        ("the id it was stored at", format!("{path}Patient/{id}"), 200),
        (
            "another id of the same type",
            format!("{path}Patient/scoped-2"),
            404,
        ),
        (
            "the same id under another resource type",
            format!("{path}Observation/{id}"),
            404,
        ),
        (
            "the same id in another data store",
            format!("{other_path}Patient/{id}"),
            404,
        ),
    ];
    for (case, uri, expected) in reads {
        let (status, body, _headers) = service.fhir("GET", &uri, &Value::Null).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({uri}): expected HTTP {expected}, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn a_put_outside_the_rules_of_the_interaction_is_refused_and_stores_nothing() {
    let service = HealthLake::new();
    let (_id, path) = service.active_datastore("fhir-update-refusals").await;

    // (case, the id in the URL, the submitted body, expected status, expected
    // OperationOutcome code). Each row breaks exactly one rule of the interaction.
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "an id one character past the published maximum",
            "p".repeat(MAX_LOGICAL_ID_LEN + 1),
            json!({ "resourceType": "Patient" }),
            400,
            "invalid",
        ),
        (
            "an id carrying a character outside the FHIR id alphabet",
            "patient_1".to_string(),
            json!({ "resourceType": "Patient" }),
            400,
            "invalid",
        ),
        (
            "a body whose id disagrees with the URL",
            "refused-1".to_string(),
            json!({ "resourceType": "Patient", "id": "refused-2" }),
            400,
            "invalid",
        ),
        (
            "a body whose resourceType disagrees with the URL",
            "refused-3".to_string(),
            json!({ "resourceType": "Observation", "status": "final" }),
            400,
            "invalid",
        ),
        (
            "a body carrying no resourceType",
            "refused-4".to_string(),
            json!({ "name": [{ "family": "Curie" }] }),
            400,
            "structure",
        ),
        (
            "a body that is not a JSON object",
            "refused-5".to_string(),
            json!(["Patient"]),
            400,
            "structure",
        ),
        (
            "no body at all",
            "refused-6".to_string(),
            Value::Null,
            400,
            "structure",
        ),
    ];

    for (case, id, body, expected_status, expected_code) in rows {
        let uri = format!("{path}Patient/{id}");
        let (status, answered, _headers) = service.fhir("PUT", &uri, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status}: {answered}"
        );
        let code = outcome_code(&answered);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the OperationOutcome code {expected_code:?}, got \
             {code:?} in {answered}"
        );
        // A refused update stored nothing: the id it named is still empty.
        let (status, read, _headers) = service.fhir("GET", &uri, &Value::Null).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must store nothing, but a read of \
             {uri} answered {status}: {read}"
        );
    }

    // An update on a data store that does not exist is the same answer as a read of
    // one: no endpoint serves it.
    let (status, answered, _headers) = service
        .fhir(
            "PUT",
            "/datastore/ffffffffffffffffffffffffffffffff/r4/Patient/absent-store",
            &json!({ "resourceType": "Patient" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "an update on an absent data store must answer 404, got {status}: {answered}"
    );
    assert_eq!(
        outcome_code(&answered),
        "not-found",
        "an update on an absent data store is a not-found OperationOutcome, got {answered}"
    );
}
