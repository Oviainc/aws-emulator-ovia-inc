// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone domain -> project lifecycle.
//!
//! Every case here drives the service the way a client does: a real `restJson1`
//! request through the router, and only the response body as evidence. Nothing reaches
//! into `crate::state`, so a handler that answered from a hardcoded value instead of
//! from the store would fail these tests.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A create persists the whole record; a later get/list reports what was stored.
//! 2. A listing is scoped, filtered and paginated over the stored map, and a page
//!    token round-trips every stored key exactly once.
//! 3. A delete is observable by the next call.
//! 4. A domain AWS reports in a non-terminal status (`CREATING`, `DELETING`) settles
//!    once the clock passes the span the emulator chose, and not before.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// `DomainId`'s modelled pattern, `^dzd[-_][a-zA-Z0-9_-]{1,36}$`
/// (smithy_query shape="DomainId").
fn is_domain_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("dzd") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('_').or_else(|| rest.strip_prefix('-')) else {
        return false;
    };
    (1..=36).contains(&rest.len())
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `ProjectId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$`
/// (smithy_query shape="ProjectId").
fn is_project_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `DomainUnitId`'s modelled pattern, `^[a-z0-9_\-]+$` with length 1..256
/// (smithy_query shape="DomainUnitId").
pub(crate) fn is_domain_unit_id(id: &str) -> bool {
    (1..=256).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// One `CreateDomain` request: the rows differ along what the rule generalises over —
/// which optional members are present, how many tags, and which `domainVersion`.
struct DomainCase {
    label: &'static str,
    body: Value,
    /// What `GetDomain` must report for `domainVersion` afterwards.
    expect_version: &'static str,
}

fn domain_cases() -> Vec<DomainCase> {
    vec![
        DomainCase {
            label: "name only",
            body: json!({ "name": "minimal-domain" }),
            // `DomainVersion` is V1 | V2 (smithy_query shape="DomainVersion"); a
            // request naming none gets the older version.
            expect_version: "V1",
        },
        DomainCase {
            label: "description, execution role and one tag",
            body: json!({
                "name": "demo-domain",
                "description": "hello",
                "domainExecutionRole": "arn:aws:iam::123456789012:role/DZExecution",
                "tags": { "team": "data" },
            }),
            expect_version: "V1",
        },
        DomainCase {
            label: "V2 with three tags, a KMS key and single sign-on",
            body: json!({
                "name": "v2 domain",
                "description": "a longer description with punctuation: ok!",
                "domainVersion": "V2",
                "serviceRole": "arn:aws:iam::123456789012:role/DZService",
                "kmsKeyIdentifier": "arn:aws:kms:us-west-2:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "singleSignOn": { "type": "IAM_IDC", "userAssignment": "MANUAL" },
                "tags": { "a": "1", "b": "2", "c": "3" },
            }),
            expect_version: "V2",
        },
    ]
}

#[tokio::test]
async fn created_domains_are_persisted_and_reported_by_get_and_list() {
    let dz = Emulator::new();
    let mut created: Vec<(&'static str, String, Value)> = Vec::new();

    for case in domain_cases() {
        let answer = dz
            .call("POST", "/v2/domains", Some(case.body.clone()))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_domain_id(&id),
            "case {:?}: expected an id matching ^dzd[-_][a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        assert!(
            is_domain_unit_id(&answer.s("rootDomainUnitId")),
            "case {:?}: expected a rootDomainUnitId matching ^[a-z0-9_-]+$, got {:?}",
            case.label,
            answer.s("rootDomainUnitId")
        );
        // A domain AWS has just accepted is not ready yet.
        assert_eq!(
            answer.s("status"),
            "CREATING",
            "case {:?}: expected a freshly created domain to report CREATING, got {:?}",
            case.label,
            answer.s("status")
        );
        assert_eq!(
            answer.s("arn"),
            format!("arn:aws:datazone:us-west-2:000000000000:domain/{id}"),
            "case {:?}: unexpected arn {:?}",
            case.label,
            answer.s("arn")
        );
        assert!(
            answer.s("portalUrl").contains(&id),
            "case {:?}: expected the portal URL to address the domain, got {:?}",
            case.label,
            answer.s("portalUrl")
        );
        created.push((case.label, id, case.body));
    }

    // Distinct creates never share an id.
    for (i, (label_a, id_a, _)) in created.iter().enumerate() {
        for (label_b, id_b, _) in created.iter().skip(i + 1) {
            assert_ne!(
                id_a, id_b,
                "cases {label_a:?} and {label_b:?}: two domains were given the same id {id_a:?}"
            );
        }
    }

    // The clock owes each domain exactly one transition, and it is not due yet.
    for (label, id, _) in &created {
        let got = dz.call("GET", &format!("/v2/domains/{id}"), None).await;
        assert_eq!(
            got.s("status"),
            "CREATING",
            "case {label:?}: a domain read straight after creation must still be CREATING, got {:?}",
            got.s("status")
        );
    }

    tokio::time::sleep(SETTLED).await;

    for (label, id, request) in &created {
        let got = dz.call("GET", &format!("/v2/domains/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetDomain, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.s("status"),
            "AVAILABLE",
            "case {label:?}: the creation the clock owed must have settled, got {:?}",
            got.s("status")
        );
        // Every member the request supplied comes back from the store unchanged.
        for member in ["name", "description", "serviceRole", "kmsKeyIdentifier"] {
            if let Some(sent) = request.get(member).and_then(Value::as_str) {
                assert_eq!(
                    got.s(member),
                    sent,
                    "case {label:?}: GetDomain must report the persisted {member}"
                );
            }
        }
        assert_eq!(
            got.body.get("tags"),
            request.get("tags").or(Some(&json!({}))),
            "case {label:?}: GetDomain must report the persisted tags, got {:?}",
            got.body.get("tags")
        );
        assert_eq!(
            got.body.get("singleSignOn").cloned(),
            request.get("singleSignOn").cloned(),
            "case {label:?}: GetDomain must report the persisted singleSignOn, got {:?}",
            got.body.get("singleSignOn")
        );
        assert!(
            got.body.get("createdAt").is_some() && got.body.get("lastUpdatedAt").is_some(),
            "case {label:?}: expected createdAt and lastUpdatedAt, got {:?}",
            got.body
        );
        // A domain execution role is required on output: the request's, or the one
        // the service creates for a request that named none.
        let expected_role = request
            .get("domainExecutionRole")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                "arn:aws:iam::000000000000:role/service-role/AmazonDataZoneDomainExecution"
                    .to_string()
            });
        assert_eq!(
            got.s("domainExecutionRole"),
            expected_role,
            "case {label:?}: unexpected domainExecutionRole"
        );
        assert_eq!(
            got.s("domainVersion"),
            domain_cases()
                .into_iter()
                .find(|c| c.label == *label)
                .expect("the case is one of the table rows")
                .expect_version,
            "case {label:?}: unexpected domainVersion"
        );

        // A read changes nothing a later caller sees.
        let again = dz.call("GET", &format!("/v2/domains/{id}"), None).await;
        assert_eq!(
            again.body, got.body,
            "case {label:?}: reading a domain twice must answer identically"
        );
    }

    // ListDomains reports every stored domain, and only those.
    let listed = dz.call("GET", "/v2/domains", None).await;
    assert_eq!(
        listed.status, 200,
        "ListDomains: expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let mut listed_ids: Vec<String> = listed.body["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected_ids: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "ListDomains must report exactly the stored domains"
    );
    for item in listed.body["items"].as_array().expect("items is a list") {
        assert_eq!(
            item["managedAccountId"].as_str(),
            Some("000000000000"),
            "ListDomains: expected the emulator account id, got {:?}",
            item["managedAccountId"]
        );
        assert_eq!(
            item["status"].as_str(),
            Some("AVAILABLE"),
            "ListDomains: expected a settled domain, got {:?}",
            item["status"]
        );
    }
}

#[tokio::test]
async fn list_domains_filters_by_status_and_pages_through_every_stored_domain() {
    let dz = Emulator::new();
    let mut expected_ids: Vec<String> = Vec::new();
    for case in domain_cases() {
        let answer = dz.call("POST", "/v2/domains", Some(case.body)).await;
        assert_eq!(answer.status, 201, "case {:?}: expected 201", case.label);
        expected_ids.push(answer.s("id"));
    }
    expected_ids.sort();

    // The `status` query filter selects on the stored status, so it answers the
    // in-progress status before the span elapses and the settled one after.
    let creating = dz.call("GET", "/v2/domains?status=CREATING", None).await;
    assert_eq!(
        creating.body["items"].as_array().map(Vec::len),
        Some(expected_ids.len()),
        "status=CREATING must report the domains that have not settled yet, got {:?}",
        creating.body
    );
    let available = dz.call("GET", "/v2/domains?status=AVAILABLE", None).await;
    assert_eq!(
        available.body["items"].as_array().map(Vec::len),
        Some(0),
        "status=AVAILABLE must report nothing before the creations settle, got {:?}",
        available.body
    );

    tokio::time::sleep(SETTLED).await;

    let creating = dz.call("GET", "/v2/domains?status=CREATING", None).await;
    assert_eq!(
        creating.body["items"].as_array().map(Vec::len),
        Some(0),
        "status=CREATING must report nothing once every creation settled, got {:?}",
        creating.body
    );

    // Each page size spans the class: one page per domain, two pages, and a size at
    // the top of the modelled 1..25 range, which fits everything in one page.
    for page_size in [1usize, 2, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match &token {
                Some(t) => format!("/v2/domains?maxResults={page_size}&nextToken={t}"),
                None => format!("/v2/domains?maxResults={page_size}"),
            };
            let page = dz.call("GET", &uri, None).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let items = page.body["items"].as_array().expect("items is a list");
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page returned {} items",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|i| i["id"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= expected_ids.len() + 1,
                "page size {page_size}: pagination did not terminate after {pages} pages"
            );
            match page.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, expected_ids,
            "page size {page_size}: paging must yield every stored domain exactly once, saw {seen:?}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    let bad = dz.call("GET", "/v2/domains?nextToken=zzzz", None).await;
    assert_eq!(
        (bad.status, bad.error_code.as_str()),
        (400, "ValidationException"),
        "an unissued nextToken must be refused, got {} {:?}",
        bad.status,
        bad.body
    );
}

#[tokio::test]
async fn an_unknown_or_malformed_domain_identifier_is_refused() {
    let dz = Emulator::new();
    // (identifier, expected status, expected wire error code)
    let cases: Vec<(&str, u16, &str)> = vec![
        // Well-formed but absent: a permission is held ON a domain, so the caller holds
        // none here and authorization decides the read before any lookup.
        ("dzd_doesnotexist", 403, "AccessDeniedException"),
        ("dzd-alsoabsent", 403, "AccessDeniedException"),
        // Outside `DomainId`'s pattern: the request itself is invalid.
        ("not-a-domain", 400, "ValidationException"),
        ("dzd", 400, "ValidationException"),
        ("dzd_", 400, "ValidationException"),
    ];
    for (identifier, status, code) in cases {
        let answer = dz
            .call("GET", &format!("/v2/domains/{identifier}"), None)
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {identifier:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_domain() {
    let dz = Emulator::new();
    let body = json!({ "name": "idempotent-domain", "clientToken": "token-one" });
    let first = dz.call("POST", "/v2/domains", Some(body.clone())).await;
    let second = dz.call("POST", "/v2/domains", Some(body)).await;
    assert_eq!(
        first.s("id"),
        second.s("id"),
        "a repeat carrying the same clientToken must return the first domain, got {:?} then {:?}",
        first.body,
        second.body
    );
    // A different token, with a name no live domain holds, creates a different domain:
    // only the token makes a request a repeat.
    let other = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "idempotent-domain-two", "clientToken": "token-two" })),
        )
        .await;
    assert_ne!(
        first.s("id"),
        other.s("id"),
        "a different clientToken must create a second domain"
    );
    // A fresh token does NOT excuse a duplicate name: domain names are unique within
    // the account, so this is the modelled conflict rather than a third domain.
    let duplicate = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "idempotent-domain", "clientToken": "token-three" })),
        )
        .await;
    assert_eq!(
        (duplicate.status, duplicate.error_code.as_str()),
        (409, "ConflictException"),
        "a name already in use must be refused whatever token the repeat carries, got {} {:?}",
        duplicate.status,
        duplicate.body
    );
    let listed = dz.call("GET", "/v2/domains", None).await;
    assert_eq!(
        listed.body["items"].as_array().map(Vec::len),
        Some(2),
        "the repeat must not have left a third domain behind, got {:?}",
        listed.body
    );

    // A remembered token is released with the domain it created: once that domain is
    // gone the token can no longer replay into it, so the same token creates a new one.
    let doomed = first.s("id");
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{doomed}"), None)
        .await;
    assert_eq!(deleted.status, 202, "expected 202, got {:?}", deleted.body);
    tokio::time::sleep(SETTLED).await;
    let after = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "idempotent-domain", "clientToken": "token-one" })),
        )
        .await;
    assert_eq!(
        after.status, 201,
        "expected 201 from the re-used token, got {} {:?}",
        after.status, after.body
    );
    assert_ne!(
        after.s("id"),
        doomed,
        "a token whose domain was deleted must not replay into the deleted domain"
    );
}

/// One `CreateProject` request. The rows differ along the name's shape, which optional
/// members are present, and how many glossary terms and resource tags it carries.
struct ProjectCase {
    label: &'static str,
    body: Value,
}

fn project_cases() -> Vec<ProjectCase> {
    vec![
        ProjectCase {
            label: "name only",
            // `ProjectName` is 1..64 of `^[\w -]+$` (smithy_query shape="ProjectName").
            body: json!({ "name": "p" }),
        },
        ProjectCase {
            label: "name with spaces, a description and one glossary term",
            body: json!({
                "name": "demo project one",
                "description": "hello",
                "glossaryTerms": ["term_one"],
            }),
        },
        ProjectCase {
            label: "underscores and digits, a category, a domain unit and three tags",
            body: json!({
                "name": "Project_2 name",
                "description": "a longer description",
                "projectCategory": "ANALYTICS",
                "domainUnitId": "unit-abc_1",
                "glossaryTerms": ["a_1", "b_2", "c_3"],
                "resourceTags": { "x": "1", "y": "2", "z": "3" },
            }),
        },
        ProjectCase {
            label: "a name at the modelled 64-character maximum",
            body: json!({ "name": "a".repeat(64) }),
        },
    ]
}

#[tokio::test]
async fn created_projects_are_persisted_scoped_and_deletable() {
    let dz = Emulator::new();
    let domain = dz.available_domain("projects-domain").await;
    let other_domain = dz.available_domain("other-domain").await;
    let projects_uri = format!("/v2/domains/{domain}/projects");

    let mut created: Vec<(&'static str, String, Value)> = Vec::new();
    for case in project_cases() {
        let answer = dz
            .call("POST", &projects_uri, Some(case.body.clone()))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_project_id(&id),
            "case {:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        assert_eq!(
            answer.s("domainId"),
            domain,
            "case {:?}: the project must report the domain it was created in",
            case.label
        );
        // `ProjectStatus` has no in-progress member for creation, so a project is
        // ACTIVE at once (smithy_query shape="ProjectStatus").
        assert_eq!(
            answer.s("projectStatus"),
            "ACTIVE",
            "case {:?}: expected ACTIVE, got {:?}",
            case.label,
            answer.s("projectStatus")
        );
        // `createdBy` is the caller's principal, derived from the request's SigV4
        // credential scope — never a constant echo of the request body.
        assert_eq!(
            answer.s("createdBy"),
            "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY",
            "case {:?}: expected the signing principal as createdBy, got {:?}",
            case.label,
            answer.s("createdBy")
        );
        created.push((case.label, id, case.body));
    }

    // GetProject reports exactly what CreateProject stored.
    for (label, id, request) in &created {
        let got = dz
            .call("GET", &format!("{projects_uri}/{id}"), None)
            .await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetProject, got {} {:?}",
            got.status, got.body
        );
        for member in ["name", "description", "projectCategory", "domainUnitId"] {
            assert_eq!(
                got.body.get(member).and_then(Value::as_str),
                request.get(member).and_then(Value::as_str),
                "case {label:?}: GetProject must report the persisted {member}, got {:?}",
                got.body.get(member)
            );
        }
        assert_eq!(
            got.body.get("glossaryTerms").cloned(),
            request.get("glossaryTerms").cloned(),
            "case {label:?}: GetProject must report the persisted glossaryTerms, got {:?}",
            got.body.get("glossaryTerms")
        );
        let expected_tags: Vec<Value> = request
            .get("resourceTags")
            .and_then(Value::as_object)
            .map(|map| {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                keys.into_iter()
                    // `ResourceTagSource` is PROJECT | PROJECT_PROFILE (smithy_query
                    // shape="ResourceTagSource"); a tag the request carried is the
                    // project's own.
                    .map(|k| json!({ "key": k, "value": map[k], "source": "PROJECT" }))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            got.body["resourceTags"],
            Value::Array(expected_tags),
            "case {label:?}: GetProject must report the persisted resourceTags, got {:?}",
            got.body["resourceTags"]
        );
        assert_eq!(
            got.s("projectStatus"),
            "ACTIVE",
            "case {label:?}: expected ACTIVE from GetProject"
        );
        assert!(
            got.body.get("createdAt").is_some(),
            "case {label:?}: expected createdAt, got {:?}",
            got.body
        );
    }

    // The listing is scoped to ONE domain: the other domain has none of these.
    let other_listed = dz
        .call("GET", &format!("/v2/domains/{other_domain}/projects"), None)
        .await;
    assert_eq!(
        other_listed.body["items"].as_array().map(Vec::len),
        Some(0),
        "a second domain must not see the first domain's projects, got {:?}",
        other_listed.body
    );

    let listed = dz.call("GET", &projects_uri, None).await;
    let mut listed_ids: Vec<String> = listed.body["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_ids.sort();
    let mut expected_ids: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    expected_ids.sort();
    assert_eq!(
        listed_ids, expected_ids,
        "ListProjects must report exactly the projects stored in this domain"
    );

    // The `name` filter selects the one project with that name; `projectCategory`
    // selects on the stored category.
    for (label, _, request) in &created {
        let name = request["name"].as_str().expect("every case names the project");
        let filtered = dz
            .call(
                "GET",
                &format!("{projects_uri}?name={}", urlencode(name)),
                None,
            )
            .await;
        let names: Vec<&str> = filtered.body["items"]
            .as_array()
            .expect("items is a list")
            .iter()
            .map(|i| i["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec![name],
            "case {label:?}: the name filter must select exactly that project, got {names:?}"
        );
    }
    let by_category = dz
        .call("GET", &format!("{projects_uri}?projectCategory=ANALYTICS"), None)
        .await;
    assert_eq!(
        by_category.body["items"].as_array().map(Vec::len),
        Some(1),
        "the projectCategory filter must select the one stored ANALYTICS project, got {:?}",
        by_category.body
    );
    let absent_category = dz
        .call("GET", &format!("{projects_uri}?projectCategory=NOSUCH"), None)
        .await;
    assert_eq!(
        absent_category.body["items"].as_array().map(Vec::len),
        Some(0),
        "a category nothing was stored under must select nothing, got {:?}",
        absent_category.body
    );

    // Paging over the projects of one domain yields each exactly once.
    for page_size in [1usize, 3, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let uri = match &token {
                Some(t) => format!("{projects_uri}?maxResults={page_size}&nextToken={t}"),
                None => format!("{projects_uri}?maxResults={page_size}"),
            };
            let page = dz.call("GET", &uri, None).await;
            let items = page.body["items"].as_array().expect("items is a list");
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page returned {} items",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|i| i["id"].as_str().unwrap_or_default().to_string()),
            );
            match page.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen, expected_ids,
            "page size {page_size}: paging must yield every stored project exactly once"
        );
    }

    // A delete is observable by the next call, and is not repeatable.
    for (label, id, _) in &created {
        let deleted = dz
            .call("DELETE", &format!("{projects_uri}/{id}"), None)
            .await;
        // 204 with no members: `DeleteProjectOutput` models none, and the framework
        // renders that empty shape as an empty JSON object.
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected 204, got {} {:?}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body == Value::Null || deleted.body == json!({}),
            "case {label:?}: expected a body with no members, got {:?}",
            deleted.body
        );
        let got = dz
            .call("GET", &format!("{projects_uri}/{id}"), None)
            .await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a deleted project must be gone, got {} {:?}",
            got.status,
            got.body
        );
        let again = dz
            .call("DELETE", &format!("{projects_uri}/{id}"), None)
            .await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a second delete must answer 404, got {} {:?}",
            again.status,
            again.body
        );
    }
    let listed = dz.call("GET", &projects_uri, None).await;
    assert_eq!(
        listed.body["items"].as_array().map(Vec::len),
        Some(0),
        "ListProjects must be empty once every project was deleted, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_project_needs_a_domain_that_exists_and_is_available() {
    let dz = Emulator::new();

    // No such domain at all: `CreateProject` RESOLVES the domain a project is created in,
    // so the create answers that domain's own miss naming the identifier, while the
    // listing — which a permission is held ON the domain for — is denied before any
    // lookup.
    let orphan = dz
        .call(
            "POST",
            "/v2/domains/dzd_doesnotexist/projects",
            Some(json!({ "name": "orphan" })),
        )
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_str(), orphan.s("message")),
        (
            404,
            "ResourceNotFoundException",
            "Domain dzd_doesnotexist not found".to_string()
        ),
        "a domain nothing created must be the create's own miss, got {} {:?}",
        orphan.status,
        orphan.body
    );
    let listed = dz
        .call("GET", "/v2/domains/dzd_doesnotexist/projects", None)
        .await;
    assert_eq!(
        (listed.status, listed.error_code.as_str(), listed.s("message")),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: ListProjects".to_string()
        ),
        "a listing scoped to a domain nothing created must be denied, got {} {:?}",
        listed.status,
        listed.body
    );

    // A domain that has not finished being created cannot take a project yet.
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "not-ready" })))
        .await;
    let domain = created.s("id");
    let early = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": "too early" })),
        )
        .await;
    assert_eq!(
        (early.status, early.error_code.as_str()),
        (409, "ConflictException"),
        "a CREATING domain must refuse a project, got {} {:?}",
        early.status,
        early.body
    );

    // Once the creation the clock owed has settled, the same request succeeds.
    tokio::time::sleep(SETTLED).await;
    let accepted = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": "now fine" })),
        )
        .await;
    assert_eq!(
        accepted.status, 201,
        "an AVAILABLE domain must accept the project, got {} {:?}",
        accepted.status, accepted.body
    );
}

#[tokio::test]
async fn deleting_a_domain_takes_its_projects_with_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("doomed-domain").await;
    let survivor = dz.available_domain("surviving-domain").await;
    let project = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": "doomed project" })),
        )
        .await;
    assert_eq!(project.status, 201, "expected 201, got {:?}", project.body);
    let project_id = project.s("id");

    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.s("status"),
        "DELETING",
        "DeleteDomain must report the status the deletion starts in, got {:?}",
        deleted.s("status")
    );

    // While the deletion is in flight the domain still reports DELETING, and a repeat
    // of the call answers the same without restarting anything.
    let during = dz.call("GET", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        (during.status, during.s("status").as_str()),
        (200, "DELETING"),
        "a domain being deleted must report DELETING, got {} {:?}",
        during.status,
        during.body
    );
    let repeat = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        (repeat.status, repeat.s("status").as_str()),
        (202, "DELETING"),
        "a repeated DeleteDomain must answer the same, got {} {:?}",
        repeat.status,
        repeat.body
    );

    tokio::time::sleep(SETTLED).await;

    let gone = dz.call("GET", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (403, "AccessDeniedException"),
        "a deleted domain must no longer be served to its caller, got {} {:?}",
        gone.status,
        gone.body
    );
    let orphan = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/projects/{project_id}"),
            None,
        )
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_str()),
        (403, "AccessDeniedException"),
        "a read scoped to the deleted domain must be denied, not answered, got {} {:?}",
        orphan.status,
        orphan.body
    );
    // The domain is gone, so what remains is a permissions answer that names the
    // operation and nothing the caller sent.
    assert_eq!(
        orphan.s("message"),
        "User is not permitted to perform operation: GetProject",
        "the denial must name the operation only, got {:?}",
        orphan.body
    );
    let listed = dz.call("GET", "/v2/domains", None).await;
    let ids: Vec<&str> = listed.body["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec![survivor.as_str()],
        "ListDomains must report only the domain that was not deleted, got {ids:?}"
    );
}

#[tokio::test]
async fn each_store_is_bounded_and_refuses_with_the_modelled_quota_error() {
    use service_com_amazonaws_datazone_datazone::state::{MAX_DOMAINS, MAX_PROJECTS_PER_DOMAIN};

    let dz = Emulator::new();
    // Fill the domain store to its ceiling, then one more.
    for n in 0..MAX_DOMAINS {
        let answer = dz
            .call("POST", "/v2/domains", Some(json!({ "name": format!("d{n}") })))
            .await;
        assert_eq!(
            answer.status, 201,
            "domain {n}: expected 201 below the ceiling, got {} {:?}",
            answer.status, answer.body
        );
    }
    let over = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "one too many" })))
        .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "a full domain store must refuse with the modelled quota error, got {} {:?}",
        over.status,
        over.body
    );

    // The same bound holds per domain for projects.
    tokio::time::sleep(SETTLED).await;
    let listed = dz.call("GET", "/v2/domains?maxResults=1", None).await;
    let domain = listed.body["items"][0]["id"]
        .as_str()
        .expect("a stored domain")
        .to_string();
    let projects_uri = format!("/v2/domains/{domain}/projects");
    for n in 0..MAX_PROJECTS_PER_DOMAIN {
        let answer = dz
            .call(
                "POST",
                &projects_uri,
                Some(json!({ "name": format!("p{n}") })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "project {n}: expected 201 below the ceiling, got {} {:?}",
            answer.status, answer.body
        );
    }
    let over = dz
        .call(
            "POST",
            &projects_uri,
            Some(json!({ "name": "one too many" })),
        )
        .await;
    assert_eq!(
        (over.status, over.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "a full project store must refuse with the modelled quota error, got {} {:?}",
        over.status,
        over.body
    );
}

/// Percent-encode a value for use as one path segment or a query-parameter value:
/// every byte outside RFC 3986's unreserved set is escaped, so the `:` and `/` of an
/// ARN stay inside a single segment and a `+` in a tag key is never read as a space.
///
/// `pub(crate)` so the sibling suites in this test target share this one encoder.
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
