// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two things a FAILING DataZone answer is made of: which
//! failure `CreateProject` reports for the domain it is scoped to, and what any failure
//! carries in its body.
//!
//! Rule one — `CreateProject` RESOLVES its domain. The domain a project is created in is
//! a resource this call looks up (`POST /v2/domains/{domainIdentifier}/projects`,
//! `smithy_query(query="operation",operation="CreateProject")`, which binds
//! `ResourceNotFoundException` at HTTP 404 and `AccessDeniedException` at HTTP 403), so an
//! identifier that satisfies `DomainId`'s pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$`
//! (smithy_query shape="DomainId") but names no stored domain is answered with the
//! not-found, whose message is fixed text around the identifier the request's path
//! carried. The denial is reserved for a decision about the caller, which an absent domain
//! is not. The rows below differ along everything the rule generalises over — which
//! separator, the shortest and longest tail the pattern admits, a tail mixing the other
//! characters of the class, and an identifier that named a domain until it was deleted.
//!
//! Rule two — a failure's BODY carries the error shape's own members and nothing else.
//! This service speaks `restJson1` (`smithy_query(query="service_info")`), which names the
//! error shape in the `x-amzn-errortype` header, so the body carries no shape
//! discriminator; and every error shape of this model declares the single member `message`
//! (`smithy_query(query="shape",shape="AccessDeniedException,ResourceNotFoundException")`),
//! which is absent rather than empty when the service states nothing. The rows span the
//! error classes this emulator raises from a handler — a miss, a denial inside a domain,
//! and the denial whose own identifier IS the domain — plus a success, which is not an
//! error envelope at all.
//!
//! A refusal the framework renders BEFORE a handler runs (an absent required member, a
//! value outside a modelled bound) is written by the shared pre-dispatch renderer, which
//! no service-owned seam reaches, so it is not part of rule two's table.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// The member a JSON error envelope of the AWS JSON protocol family names its error shape
/// in — the one `restJson1` names in a header instead, so no body here may carry it.
const SHAPE_DISCRIMINATOR: &str = "__type";

/// Domain identifiers no domain in the store carries, each satisfying `DomainId`'s
/// modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so that
/// nothing refuses them before a handler runs.
fn unserved_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an underscore separator", "dzd_noprojectdomain0".to_string()),
        ("a hyphen separator", "dzd-noprojectdomain1".to_string()),
        ("the shortest tail the pattern admits", "dzd_p".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd_{}", "p".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_7-absent_2".to_string(),
        ),
    ]
}

/// The body members of an answer, sorted, so an assertion names exactly what went on the
/// wire.
fn members(body: &Value) -> Vec<String> {
    match body.as_object() {
        Some(object) => {
            let mut names: Vec<String> = object.keys().cloned().collect();
            names.sort();
            names
        }
        None => vec![format!("<not an object: {body}>")],
    }
}

/// One failure this service answers: the call that provokes it, the status and error class
/// it must carry, and the body members it must carry on the wire.
struct Failure {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    code: &'static str,
    members: Vec<&'static str>,
}

#[tokio::test]
async fn create_project_reports_a_domain_it_cannot_resolve_as_the_domains_own_miss() {
    let dz = Emulator::new();
    let served = dz.available_domain("create-project-resolution").await;

    for (case, unserved) in unserved_domain_ids() {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{unserved}/projects"),
                Some(json!({ "name": "project in no domain" })),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled not-found, got {} {:?}",
            answer.status,
            answer.body
        );
        let message = answer.s("message");
        assert_eq!(
            message,
            format!("Domain {unserved} not found"),
            "case {case:?}: the miss must name the requested domain, got {message:?}"
        );
        assert!(
            answer.body.get("id").is_none(),
            "case {case:?}: a refused create must report no project, got {:?}",
            answer.body
        );
    }

    // Nothing above reached the store: the domain that IS served holds only the project
    // the round trip below puts in it.
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{served}/projects"),
            Some(json!({ "name": "round trip project" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "CreateProject under its own domain: expected 201, got {} {:?}",
        created.status, created.body
    );
    let read_back = dz
        .call(
            "GET",
            &format!("/v2/domains/{served}/projects/{}", created.s("id")),
            None,
        )
        .await;
    assert_eq!(
        (read_back.status, read_back.s("name").as_str()),
        (200, "round trip project"),
        "the created project must be readable back, got {} {:?}",
        read_back.status,
        read_back.body
    );
    let projects = dz
        .call("GET", &format!("/v2/domains/{served}/projects"), None)
        .await;
    assert_eq!(
        projects.body["items"].as_array().map(Vec::len),
        Some(1),
        "a refused create must store nothing, got {:?}",
        projects.body
    );
}

#[tokio::test]
async fn create_project_reports_a_deleted_domain_as_a_miss_naming_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("deleted-project-domain").await;

    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected the modelled success status, got {} {:?}",
        deleted.status, deleted.body
    );
    // A domain still on its way out is a domain this store HOLDS, so the create conflicts
    // with the state it is in rather than missing it.
    let during = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": "project during deletion" })),
        )
        .await;
    assert_eq!(
        (during.status, during.error_code.as_str()),
        (409, "ConflictException"),
        "a domain being deleted still exists, so expected the conflict, got {} {:?}",
        during.status,
        during.body
    );

    // Once the deletion the clock owed has completed, the identifier names no domain.
    tokio::time::sleep(SETTLED).await;
    let after = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": "project after deletion" })),
        )
        .await;
    assert_eq!(
        (after.status, after.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a deleted domain is an absent domain, so expected the miss, got {} {:?}",
        after.status,
        after.body
    );
    let message = after.s("message");
    assert_eq!(
        message,
        format!("Domain {domain} not found"),
        "the miss must name the requested domain, got {message:?}"
    );
}

#[tokio::test]
async fn every_failure_body_carries_the_error_shapes_own_members_and_nothing_else() {
    let dz = Emulator::new();
    let served = dz.available_domain("error-body-domain").await;
    let project = dz.project(&served, "error body owner").await;
    let unserved = "dzd_noerrorbodydomain";

    // Every error class this emulator raises from a handler, each addressed through the
    // HTTP surface a client uses.
    let cases: Vec<Failure> = vec![
        Failure {
            case: "a miss naming the domain it could not resolve",
            method: "POST",
            uri: format!("/v2/domains/{unserved}/projects"),
            body: Some(json!({ "name": "project in no domain" })),
            status: 404,
            code: "ResourceNotFoundException",
            members: vec!["message"],
        },
        Failure {
            case: "a denial of a call inside a domain",
            method: "GET",
            uri: format!("/v2/domains/{unserved}/projects/{project}"),
            body: None,
            status: 403,
            code: "AccessDeniedException",
            members: vec!["message"],
        },
        Failure {
            case: "a denial whose own identifier IS the domain",
            method: "DELETE",
            uri: format!("/v2/domains/{unserved}"),
            body: None,
            status: 403,
            code: "AccessDeniedException",
            members: vec![],
        },
        Failure {
            case: "a miss of a resource inside a served domain",
            method: "GET",
            uri: format!("/v2/domains/{served}/projects/nosuchproject"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
            members: vec!["message"],
        },
    ];

    for Failure {
        case,
        method,
        uri,
        body,
        status,
        code,
        members: expected,
    } in cases
    {
        let answer = dz.call(method, &uri, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {case:?}: expected the modelled failure, got {} {:?}",
            answer.status,
            answer.body
        );
        let on_the_wire = members(&answer.body);
        assert_eq!(
            on_the_wire,
            expected
                .iter()
                .map(|member| (*member).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly these members, got {:?}",
            answer.body
        );
        assert!(
            answer.body.get(SHAPE_DISCRIMINATOR).is_none(),
            "case {case:?}: the error shape travels in the header, got {:?}",
            answer.body
        );
        for member in &on_the_wire {
            let value = answer.body.get(member).and_then(Value::as_str);
            assert!(
                value.is_none_or(|text| !text.is_empty()),
                "case {case:?}: {member:?} must be absent rather than empty, got {:?}",
                answer.body
            );
        }
    }

    // A SUCCESS is not an error envelope: it keeps every member the model gives it.
    let success = dz
        .call("GET", &format!("/v2/domains/{served}/projects"), None)
        .await;
    assert_eq!(
        success.status, 200,
        "ListProjects under its own domain: expected 200, got {} {:?}",
        success.status, success.body
    );
    assert!(
        success.body.get("items").is_some(),
        "a success must carry its own members, got {:?}",
        success.body
    );
}
