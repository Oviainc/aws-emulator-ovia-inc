// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone metadata form types.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response as evidence, so a handler answering
//! from a hardcoded value instead of from the store fails these tests.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateFormType` persists the whole record under its domain and `GetFormType`
//!    reports what was stored, with `status` defaulting to the usable member of
//!    `FormTypeStatus` when the request names none.
//! 2. The model gives form types no `UpdateFormType` and makes `revision` a required
//!    output, so re-creating a name appends the NEXT revision and every earlier
//!    revision stays readable at its own number.
//! 3. A form type belongs to one domain and one owning project: another domain cannot
//!    see it, another project cannot add to it, and it goes away with its domain.
//! 4. A request the service cannot satisfy is refused with the error the model
//!    declares, never with a success.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// The `FormTypeStatus` a request that names none gets. The enum is ENABLED | DISABLED
/// (smithy_query shape="FormTypeStatus") and a created form type is usable at once.
const DEFAULT_STATUS: &str = "ENABLED";

/// `FormTypeName`'s modelled maximum length, 1..128 (smithy_query shape="FormTypeName").
const NAME_MAX: usize = 128;

/// `Description`'s modelled maximum length, ..2048 (smithy_query shape="Description").
const DESCRIPTION_MAX: usize = 2048;

/// The modelled length range of `Model$smithy`, 1..100000 (smithy_query shape="Model").
const MODEL_MIN: usize = 1;
const MODEL_MAX: usize = 100_000;

/// One accepted `CreateFormType` request. The rows differ along what the rule
/// generalises over: the form of the name and its length, whether a description is
/// present and how long, the size of the `model` payload, and which `FormTypeStatus`
/// the request names.
struct FormTypeCase {
    label: &'static str,
    name: String,
    model_smithy: String,
    description: Option<String>,
    status: Option<&'static str>,
    /// What both calls must report for `status`: the request's value, or the default
    /// when the request named none.
    expect_status: &'static str,
}

fn form_type_cases() -> Vec<FormTypeCase> {
    vec![
        FormTypeCase {
            label: "a plain name, a one-structure model, nothing optional",
            name: "OrderForm".to_string(),
            model_smithy: "structure OrderForm {\n  orderId: String\n}".to_string(),
            description: None,
            status: None,
            expect_status: DEFAULT_STATUS,
        },
        FormTypeCase {
            label: "underscore-leading name, description at the modelled maximum",
            name: "_order_form_2".to_string(),
            model_smithy: "structure _order_form_2 {\n  sku: String\n}".to_string(),
            description: Some("d".repeat(DESCRIPTION_MAX)),
            status: Some("ENABLED"),
            expect_status: "ENABLED",
        },
        FormTypeCase {
            label: "digits and mixed case in the name, explicitly DISABLED",
            name: "Fin2Form_v3".to_string(),
            model_smithy: "structure Fin2Form_v3 {\n  amount: Double\n}".to_string(),
            description: Some("the finance form, retired".to_string()),
            status: Some("DISABLED"),
            expect_status: "DISABLED",
        },
        FormTypeCase {
            label: "name at the modelled maximum length, model at its minimum",
            name: format!("F{}", "o".repeat(NAME_MAX - 1)),
            model_smithy: "s".repeat(MODEL_MIN),
            description: Some(String::new()),
            status: None,
            expect_status: DEFAULT_STATUS,
        },
        FormTypeCase {
            label: "model at the modelled maximum length",
            name: "BigModelForm".to_string(),
            model_smithy: "s".repeat(MODEL_MAX),
            description: None,
            status: Some("DISABLED"),
            expect_status: "DISABLED",
        },
        FormTypeCase {
            // The modelled pattern admits an `amazon.datazone.` prefix, which also
            // puts dots in the path segment `GetFormType` addresses it by.
            label: "a qualified name behind the amazon.datazone. prefix",
            name: "amazon.datazone.QualifiedForm".to_string(),
            model_smithy: "structure QualifiedForm {\n  id: String\n}".to_string(),
            description: Some("qualified".to_string()),
            status: None,
            expect_status: DEFAULT_STATUS,
        },
        FormTypeCase {
            label: "a name that is an underscore followed only by digits",
            name: "_123".to_string(),
            model_smithy: "structure _123 {\n  id: String\n}".to_string(),
            description: None,
            status: None,
            expect_status: DEFAULT_STATUS,
        },
    ]
}

/// The request body for one case.
fn create_body(case: &FormTypeCase, project: &str) -> Value {
    let mut body = json!({
        "name": case.name,
        "model": { "smithy": case.model_smithy },
        "owningProjectIdentifier": project,
    });
    if let Some(description) = &case.description {
        body["description"] = json!(description);
    }
    if let Some(status) = case.status {
        body["status"] = json!(status);
    }
    body
}

#[tokio::test]
async fn created_form_types_are_persisted_and_reported_by_get() {
    let dz = Emulator::new();
    let domain = dz.available_domain("form-type-domain").await;
    let project = dz.project(&domain, "forms owner").await;

    for case in form_type_cases() {
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/form-types"),
                Some(create_body(&case, &project)),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, created.status, created.body
        );
        assert_eq!(
            created.s("revision"),
            "1",
            "case {:?}: a form type's first revision must be 1, got {:?}",
            case.label,
            created.s("revision")
        );
        for (member, expected) in [
            ("domainId", domain.clone()),
            ("name", case.name.clone()),
            ("owningProjectId", project.clone()),
            // A form type created in this domain originates here.
            ("originDomainId", domain.clone()),
            ("originProjectId", project.clone()),
        ] {
            assert_eq!(
                created.s(member),
                expected,
                "case {:?}: CreateFormType must report {member} as {expected:?}, got {:?}",
                case.label,
                created.s(member)
            );
        }
        assert_eq!(
            created.body.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: CreateFormType must echo the stored description, got {:?}",
            case.label,
            created.body.get("description")
        );

        // The round trip: everything the create stored comes back from the reader.
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/form-types/{}", case.name),
                None,
            )
            .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: expected 200 from GetFormType, got {} {:?}",
            case.label, got.status, got.body
        );
        assert_eq!(
            got.body.get("model"),
            Some(&json!({ "smithy": case.model_smithy })),
            "case {:?}: GetFormType must report the persisted model (length {}), got a \
             model of length {:?}",
            case.label,
            case.model_smithy.len(),
            got.body
                .get("model")
                .and_then(|m| m.get("smithy"))
                .and_then(Value::as_str)
                .map(str::len)
        );
        assert_eq!(
            got.s("status"),
            case.expect_status,
            "case {:?}: GetFormType must report status {:?}, got {:?}",
            case.label,
            case.expect_status,
            got.s("status")
        );
        for (member, expected) in [
            ("domainId", domain.clone()),
            ("name", case.name.clone()),
            ("revision", "1".to_string()),
            ("owningProjectId", project.clone()),
            ("originDomainId", domain.clone()),
            ("originProjectId", project.clone()),
        ] {
            assert_eq!(
                got.s(member),
                expected,
                "case {:?}: GetFormType must report {member} as {expected:?}, got {:?}",
                case.label,
                got.s(member)
            );
        }
        assert_eq!(
            got.body.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: GetFormType must report the persisted description, got {:?}",
            case.label,
            got.body.get("description")
        );
        // `createdBy` comes from the CALLER's identity, not from the request body.
        assert!(
            got.s("createdBy").contains("AKIAEXAMPLETESTKEY"),
            "case {:?}: expected createdBy to name the calling principal, got {:?}",
            case.label,
            got.s("createdBy")
        );
        assert!(
            got.body.get("createdAt").is_some(),
            "case {:?}: expected GetFormType to report createdAt, got {:?}",
            case.label,
            got.body
        );
        // Nothing in this emulator composes a form type out of others, and the
        // modelled list is min-1, so the member is absent rather than empty.
        assert!(
            got.body.get("imports").is_none(),
            "case {:?}: expected no imports, got {:?}",
            case.label,
            got.body.get("imports")
        );
    }
}

#[tokio::test]
async fn recreating_a_name_appends_a_revision_and_earlier_revisions_stay_readable() {
    let dz = Emulator::new();
    let domain = dz.available_domain("revision-domain").await;
    let project = dz.project(&domain, "revision owner").await;

    // The rows differ along HOW MANY times the name is created.
    for creates in [1_usize, 2, 3, 5] {
        let name = format!("RevisionForm{creates}");
        let mut models = Vec::new();
        for revision in 1..=creates {
            let model = format!("structure {name} {{\n  v{revision}: String\n}}");
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/form-types"),
                    Some(json!({
                        "name": name,
                        "model": { "smithy": model },
                        "owningProjectIdentifier": project,
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {creates} creates, create {revision}: expected 201, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.s("revision"),
                revision.to_string(),
                "case {creates} creates, create {revision}: expected revision {revision}, got {:?}",
                created.s("revision")
            );
            models.push(model);
        }

        // A read naming no revision reports the latest.
        let latest = dz
            .call("GET", &format!("/v2/domains/{domain}/form-types/{name}"), None)
            .await;
        assert_eq!(
            (latest.status, latest.s("revision").as_str()),
            (200, creates.to_string().as_str()),
            "case {creates} creates: a read naming no revision must report the latest, got {} {:?}",
            latest.status,
            latest.body
        );
        assert_eq!(
            latest.body.get("model"),
            Some(&json!({ "smithy": &models[creates - 1] })),
            "case {creates} creates: the latest revision must carry the last model stored, got {:?}",
            latest.body.get("model")
        );

        // Every earlier revision is still readable at its own number.
        for (index, model) in models.iter().enumerate() {
            let revision = index + 1;
            let got = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/form-types/{name}?revision={revision}"),
                    None,
                )
                .await;
            assert_eq!(
                (got.status, got.s("revision").as_str()),
                (200, revision.to_string().as_str()),
                "case {creates} creates, revision {revision}: expected 200 and that revision, \
                 got {} {:?}",
                got.status,
                got.body
            );
            assert_eq!(
                got.body.get("model"),
                Some(&json!({ "smithy": model })),
                "case {creates} creates, revision {revision}: expected the model that revision \
                 was created with, got {:?}",
                got.body.get("model")
            );
        }

        // A revision that was never minted does not exist.
        for absent in [0, creates + 1, creates + 7] {
            let got = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/form-types/{name}?revision={absent}"),
                    None,
                )
                .await;
            assert_eq!(
                (got.status, got.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {creates} creates, revision {absent}: expected 404 \
                 ResourceNotFoundException, got {} {:?}",
                got.status,
                got.body
            );
        }

        // A revision is addressed by the exact string the create returned, so a
        // non-canonical spelling of a stored number names no revision.
        for spelling in ["01", "%2B1", "1.0", "one"] {
            let got = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/form-types/{name}?revision={spelling}"),
                    None,
                )
                .await;
            assert_eq!(
                (got.status, got.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {creates} creates, revision {spelling:?}: expected 404 \
                 ResourceNotFoundException, got {} {:?}",
                got.status,
                got.body
            );
        }
    }
}

#[tokio::test]
async fn a_form_type_is_scoped_to_its_domain_and_owning_project() {
    let dz = Emulator::new();
    let domain = dz.available_domain("owning-domain").await;
    let other_domain = dz.available_domain("other-domain").await;
    let project = dz.project(&domain, "first owner").await;
    let other_project = dz.project(&domain, "second owner").await;
    let project_elsewhere = dz.project(&other_domain, "owner elsewhere").await;

    let name = "ScopedForm";
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": name,
                "model": { "smithy": "structure ScopedForm { a: String }" },
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201, got {} {:?}",
        created.status, created.body
    );

    // The other domain never saw it, even though the name is free there.
    let elsewhere = dz
        .call(
            "GET",
            &format!("/v2/domains/{other_domain}/form-types/{name}"),
            None,
        )
        .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a form type must not be visible in another domain, got {} {:?}",
        elsewhere.status,
        elsewhere.body
    );
    // ...so the SAME name is creatable there, as its own revision 1.
    let twin = dz
        .call(
            "POST",
            &format!("/v2/domains/{other_domain}/form-types"),
            Some(json!({
                "name": name,
                "model": { "smithy": "structure ScopedForm { b: String }" },
                "owningProjectIdentifier": project_elsewhere,
            })),
        )
        .await;
    assert_eq!(
        (twin.status, twin.s("revision").as_str()),
        (201, "1"),
        "the same name in another domain is a separate form type, got {} {:?}",
        twin.status,
        twin.body
    );

    // A different project in the SAME domain cannot add to a name it does not own.
    let poached = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": name,
                "model": { "smithy": "structure ScopedForm { c: String }" },
                "owningProjectIdentifier": other_project,
            })),
        )
        .await;
    assert_eq!(
        (poached.status, poached.error_code.as_str()),
        (409, "ConflictException"),
        "a name owned by another project must be refused, got {} {:?}",
        poached.status,
        poached.body
    );
    // ...and the refusal stored nothing: the name still has exactly one revision.
    let unchanged = dz
        .call("GET", &format!("/v2/domains/{domain}/form-types/{name}"), None)
        .await;
    assert_eq!(
        (unchanged.status, unchanged.s("revision").as_str()),
        (200, "1"),
        "a refused create must leave the stored revisions alone, got {} {:?}",
        unchanged.status,
        unchanged.body
    );

    // A form type goes away with the domain that holds it.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(SETTLED).await;
    let orphan = dz
        .call("GET", &format!("/v2/domains/{domain}/form-types/{name}"), None)
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_str()),
        (403, "AccessDeniedException"),
        "a read scoped to the deleted domain must be denied, got {} {:?}",
        orphan.status,
        orphan.body
    );
    // The other domain's twin survived.
    let survivor = dz
        .call(
            "GET",
            &format!("/v2/domains/{other_domain}/form-types/{name}"),
            None,
        )
        .await;
    assert_eq!(
        survivor.status, 200,
        "another domain's form type must survive, got {} {:?}",
        survivor.status, survivor.body
    );
}

#[tokio::test]
async fn a_create_the_service_cannot_satisfy_is_refused_with_a_modelled_error() {
    let dz = Emulator::new();
    let domain = dz.available_domain("refusal-domain").await;
    let project = dz.project(&domain, "refusal owner").await;
    let model = json!({ "smithy": "structure RefusedForm { a: String }" });

    // (label, domain to post to, request body, expected status, expected wire code)
    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // create before any lookup.
            "a well-formed domain id that names no domain",
            "dzd_doesnotexist".to_string(),
            json!({ "name": "RefusedForm", "model": model, "owningProjectIdentifier": project }),
            403,
            "AccessDeniedException",
        ),
        (
            "an owning project that is not in the domain",
            domain.clone(),
            json!({ "name": "RefusedForm", "model": model, "owningProjectIdentifier": "nosuchproject" }),
            400,
            "ValidationException",
        ),
        (
            "a domain id outside the modelled pattern",
            "not-a-domain".to_string(),
            json!({ "name": "RefusedForm", "model": model, "owningProjectIdentifier": project }),
            400,
            "ValidationException",
        ),
        (
            "a name one character past the modelled maximum",
            domain.clone(),
            json!({
                "name": format!("F{}", "o".repeat(NAME_MAX)),
                "model": model,
                "owningProjectIdentifier": project,
            }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern: a leading digit",
            domain.clone(),
            json!({ "name": "0RefusedForm", "model": model, "owningProjectIdentifier": project }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern: an underscore alone",
            domain.clone(),
            json!({ "name": "_", "model": model, "owningProjectIdentifier": project }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern: a hyphen",
            domain.clone(),
            json!({ "name": "Refused-Form", "model": model, "owningProjectIdentifier": project }),
            400,
            "ValidationException",
        ),
        (
            // A Smithy `@pattern` is an ECMA-262 expression, where `\w` is
            // `[A-Za-z0-9_]`: a letter outside ASCII is not a word character.
            "a name outside the modelled pattern: a non-ASCII letter",
            domain.clone(),
            json!({ "name": "Ünicode", "model": model, "owningProjectIdentifier": project }),
            400,
            "ValidationException",
        ),
        (
            "a name outside the modelled pattern: a leading underscore in a qualified name",
            domain.clone(),
            json!({
                "name": "amazon.datazone._0",
                "model": model,
                "owningProjectIdentifier": project,
            }),
            400,
            "ValidationException",
        ),
        (
            "a description one character past the modelled maximum",
            domain.clone(),
            json!({
                "name": "RefusedForm",
                "model": model,
                "owningProjectIdentifier": project,
                "description": "d".repeat(DESCRIPTION_MAX + 1),
            }),
            400,
            "ValidationException",
        ),
        (
            "a model one character short of the modelled minimum",
            domain.clone(),
            json!({
                "name": "RefusedForm",
                "model": { "smithy": "" },
                "owningProjectIdentifier": project,
            }),
            400,
            "ValidationException",
        ),
        (
            "a model one character past the modelled maximum",
            domain.clone(),
            json!({
                "name": "RefusedForm",
                "model": { "smithy": "s".repeat(MODEL_MAX + 1) },
                "owningProjectIdentifier": project,
            }),
            400,
            "ValidationException",
        ),
    ];

    for (label, target_domain, body, status, code) in cases {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{target_domain}/form-types"),
                Some(body),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // Nothing above was stored.
    let absent = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/form-types/RefusedForm"),
            None,
        )
        .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a refused create must store nothing, got {} {:?}",
        absent.status,
        absent.body
    );
}

#[tokio::test]
async fn a_domain_that_is_not_available_cannot_take_a_form_type() {
    let dz = Emulator::new();
    // Read back straight away, so the domain is still in the status the clock owes a
    // transition out of.
    let creating = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "pending-domain" })))
        .await;
    assert_eq!(
        creating.status, 201,
        "CreateDomain: expected 201, got {} {:?}",
        creating.status, creating.body
    );
    let domain = creating.s("id");
    assert_eq!(
        creating.s("status"),
        "CREATING",
        "a freshly created domain must report CREATING, got {:?}",
        creating.s("status")
    );

    let refused = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": "TooEarlyForm",
                "model": { "smithy": "structure TooEarlyForm { a: String }" },
                "owningProjectIdentifier": "anyproject",
            })),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "a domain that is still CREATING must refuse a form type, got {} {:?}",
        refused.status,
        refused.body
    );

    // Once the clock has settled the domain, the same call is accepted.
    tokio::time::sleep(SETTLED).await;
    let project = dz.project(&domain, "late owner").await;
    let accepted = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": "TooEarlyForm",
                "model": { "smithy": "structure TooEarlyForm { a: String }" },
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        (accepted.status, accepted.s("revision").as_str()),
        (201, "1"),
        "an available domain must accept the form type, got {} {:?}",
        accepted.status,
        accepted.body
    );
}
