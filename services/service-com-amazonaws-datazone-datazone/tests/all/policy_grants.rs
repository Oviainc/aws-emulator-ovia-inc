// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone policy-grant trio: `AddPolicyGrant`,
//! `ListPolicyGrants` and `RemovePolicyGrant`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — so a handler answering
//! from a hardcoded value instead of from the store would fail these tests. The harness
//! (`Emulator`) is the one the sibling suite already defines.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. An added grant is persisted whole: a later `ListPolicyGrants` reports back the
//!    `principal` and `detail` the request carried, plus the service-minted `grantId`,
//!    the caller as `createdBy` and a `createdAt`.
//! 2. A grant is reported only for the entity (type + id) and `policyType` it was added
//!    to, and only while its domain is there.
//! 3. `detail` must be the `PolicyGrantDetail` member the request's `policyType` names;
//!    any other pairing is refused and stores nothing.
//! 4. The listing pages over the stored grants: no page exceeds `maxResults` and the
//!    token walks every stored grant exactly once.
//! 5. `RemovePolicyGrant` drops the grants of the named principal — narrowed to one by
//!    `grantIdentifier` — and removing what is not there changes nothing.
//! 6. A repeated `clientToken` answers with the grant the first request created instead
//!    of adding a second one.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// `GrantIdentifier`'s modelled pattern, `^[A-Za-z0-9+/]{10}$`
/// (smithy_query shape="GrantIdentifier").
fn is_grant_id(id: &str) -> bool {
    id.len() == 10
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Drop every `null` member, recursively, so what came back can be compared to what was
/// sent: an optional member the request omitted may be rendered as `null` rather than
/// left out, and that is not a difference in what was stored.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), without_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

/// Every `ManagedPolicyType` value, each paired with the `PolicyGrantDetail` member of
/// the same name — the model declares the two in one-to-one correspondence
/// (smithy_query shape="ManagedPolicyType", shape="PolicyGrantDetail"). Each detail
/// carries every member its own shape declares (smithy_query shape="<Detail>"), so the
/// listing can be compared to the request member for member.
fn policy_catalog() -> Vec<(&'static str, Value)> {
    vec![
        (
            "CREATE_DOMAIN_UNIT",
            json!({ "createDomainUnit": { "includeChildDomainUnits": true } }),
        ),
        (
            "OVERRIDE_DOMAIN_UNIT_OWNERS",
            json!({ "overrideDomainUnitOwners": { "includeChildDomainUnits": false } }),
        ),
        (
            "ADD_TO_PROJECT_MEMBER_POOL",
            json!({ "addToProjectMemberPool": { "includeChildDomainUnits": true } }),
        ),
        (
            "OVERRIDE_PROJECT_OWNERS",
            json!({ "overrideProjectOwners": { "includeChildDomainUnits": false } }),
        ),
        (
            "CREATE_GLOSSARY",
            json!({ "createGlossary": { "includeChildDomainUnits": true } }),
        ),
        (
            "CREATE_FORM_TYPE",
            json!({ "createFormType": { "includeChildDomainUnits": false } }),
        ),
        (
            "CREATE_ASSET_TYPE",
            json!({ "createAssetType": { "includeChildDomainUnits": true } }),
        ),
        (
            "CREATE_PROJECT",
            json!({ "createProject": { "includeChildDomainUnits": false } }),
        ),
        (
            "CREATE_ENVIRONMENT_PROFILE",
            // `domainUnitId` is a `DomainUnitId`: ^[a-z0-9_\-]+$, length 1..256.
            json!({ "createEnvironmentProfile": { "domainUnitId": "du-example-one" } }),
        ),
        (
            "DELEGATE_CREATE_ENVIRONMENT_PROFILE",
            // The model targets `Unit` here: a member with no members of its own.
            json!({ "delegateCreateEnvironmentProfile": {} }),
        ),
        ("CREATE_ENVIRONMENT", json!({ "createEnvironment": {} })),
        (
            "CREATE_ENVIRONMENT_FROM_BLUEPRINT",
            json!({ "createEnvironmentFromBlueprint": {} }),
        ),
        (
            "CREATE_PROJECT_FROM_PROJECT_PROFILE",
            json!({
                "createProjectFromProjectProfile": {
                    "includeChildDomainUnits": true,
                    "projectProfiles": ["profile-a", "profile-b"],
                }
            }),
        ),
        (
            "USE_ASSET_TYPE",
            json!({ "useAssetType": { "domainUnitId": "du-example-two" } }),
        ),
    ]
}

/// Every member of the `PolicyGrantPrincipal` union, and of the unions nested in it
/// (smithy_query shape="PolicyGrantPrincipal").
fn principal_catalog() -> Vec<(&'static str, Value)> {
    vec![
        (
            "user by identifier",
            json!({ "user": { "userIdentifier": "data.engineer@example.com" } }),
        ),
        (
            "all users",
            json!({ "user": { "allUsersGrantFilter": {} } }),
        ),
        (
            "group by identifier",
            json!({ "group": { "groupIdentifier": "data-engineers" } }),
        ),
        (
            "project owners, filtered to a domain unit",
            json!({
                "project": {
                    "projectDesignation": "OWNER",
                    "projectIdentifier": "project-1",
                    "projectGrantFilter": {
                        "domainUnitFilter": {
                            "domainUnit": "du-example-one",
                            "includeChildDomainUnits": true,
                        }
                    },
                }
            }),
        ),
        (
            "domain unit owners, all domain units",
            json!({
                "domainUnit": {
                    "domainUnitDesignation": "OWNER",
                    "domainUnitIdentifier": "du-example-one",
                    "domainUnitGrantFilter": { "allDomainUnitsGrantFilter": {} },
                }
            }),
        ),
    ]
}

/// Every `TargetEntityType` value (smithy_query shape="TargetEntityType").
const ENTITY_TYPES: [&str; 4] = [
    "DOMAIN_UNIT",
    "ENVIRONMENT_BLUEPRINT_CONFIGURATION",
    "ENVIRONMENT_PROFILE",
    "ASSET_TYPE",
];

/// The principal `createdBy` must name: the caller's access key id, as the harness signs
/// every request (`crate::state::caller_principal`).
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

fn add_uri(domain: &str, entity_type: &str, entity_id: &str) -> String {
    format!("/v2/domains/{domain}/policies/managed/{entity_type}/{entity_id}/addGrant")
}

fn remove_uri(domain: &str, entity_type: &str, entity_id: &str) -> String {
    format!("/v2/domains/{domain}/policies/managed/{entity_type}/{entity_id}/removeGrant")
}

fn list_uri(domain: &str, entity_type: &str, entity_id: &str, policy_type: &str) -> String {
    format!(
        "/v2/domains/{domain}/policies/managed/{entity_type}/{entity_id}/grants?policyType={policy_type}"
    )
}

/// The `grantList` of a listing, as a vector of members.
fn grant_list(body: &Value) -> Vec<Value> {
    body.get("grantList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `grantId` of a listed member, for assertions over a whole page.
trait GrantIdOf {
    fn s_grant_id(&self) -> String;
}

impl GrantIdOf for Value {
    fn s_grant_id(&self) -> String {
        self.get("grantId")
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    }
}

/// One `AddPolicyGrant` request: a policy type with its matching detail, on one entity,
/// for one principal.
struct GrantCase {
    label: String,
    entity_type: &'static str,
    entity_id: String,
    policy_type: &'static str,
    detail: Value,
    principal: Value,
}

/// The table: every policy type (with its own detail), spread over every entity type and
/// every principal form, so the rows differ along each axis the rule generalises over.
fn grant_cases(root_domain_unit: &str) -> Vec<GrantCase> {
    let principals = principal_catalog();
    policy_catalog()
        .into_iter()
        .enumerate()
        .map(|(i, (policy_type, detail))| {
            let entity_type = ENTITY_TYPES[i % ENTITY_TYPES.len()];
            // Every DOMAIN_UNIT row targets the domain's own root domain unit, so those
            // rows share one entity and are told apart only by their policy type; the
            // other entity types get an id of their own.
            let entity_id = if entity_type == "DOMAIN_UNIT" {
                root_domain_unit.to_string()
            } else {
                format!("entity-{i}")
            };
            let (principal_label, principal) = principals[i % principals.len()].clone();
            GrantCase {
                label: format!("{policy_type} on {entity_type} for {principal_label}"),
                entity_type,
                entity_id,
                policy_type,
                detail,
                principal,
            }
        })
        .collect()
}

#[tokio::test]
async fn added_grants_are_persisted_and_reported_per_entity_and_policy_type() {
    let dz = Emulator::new();
    let domain = dz.available_domain("grant-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    let cases = grant_cases(&root);
    let mut ids: Vec<(String, String)> = Vec::new();

    for case in &cases {
        let answer = dz
            .call(
                "POST",
                &add_uri(&domain, case.entity_type, &case.entity_id),
                Some(json!({
                    "policyType": case.policy_type,
                    "principal": case.principal.clone(),
                    "detail": case.detail.clone(),
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let grant_id = answer.s("grantId");
        assert!(
            is_grant_id(&grant_id),
            "case {:?}: expected a grantId matching ^[A-Za-z0-9+/]{{10}}$, got {grant_id:?}",
            case.label
        );
        ids.push((case.label.clone(), grant_id));
    }

    // Distinct adds never share a grant id.
    for (i, (label_a, id_a)) in ids.iter().enumerate() {
        for (label_b, id_b) in ids.iter().skip(i + 1) {
            assert_ne!(
                id_a, id_b,
                "cases {label_a:?} and {label_b:?}: two grants were given the same id {id_a:?}"
            );
        }
    }

    // Each grant comes back from the store, under its own entity and policy type, with
    // everything the request carried.
    for (case, (_, grant_id)) in cases.iter().zip(&ids) {
        let listed = dz
            .call(
                "GET",
                &list_uri(&domain, case.entity_type, &case.entity_id, case.policy_type),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListPolicyGrants, got {} {:?}",
            case.label, listed.status, listed.body
        );
        let members = grant_list(&listed.body);
        assert_eq!(
            members.len(),
            1,
            "case {:?}: expected exactly the one stored grant, got {:?}",
            case.label,
            listed.body
        );
        let member = &members[0];
        assert_eq!(
            member.get("grantId").and_then(Value::as_str),
            Some(grant_id.as_str()),
            "case {:?}: the listed grant must be the one that was added, got {member:?}",
            case.label
        );
        assert_eq!(
            without_nulls(member.get("detail").unwrap_or(&Value::Null)),
            without_nulls(&case.detail),
            "case {:?}: the listed detail must be the persisted one, got {:?}",
            case.label,
            member.get("detail")
        );
        assert_eq!(
            without_nulls(member.get("principal").unwrap_or(&Value::Null)),
            without_nulls(&case.principal),
            "case {:?}: the listed principal must be the persisted one, got {:?}",
            case.label,
            member.get("principal")
        );
        assert_eq!(
            member.get("createdBy").and_then(Value::as_str),
            Some(CALLER),
            "case {:?}: createdBy must be the calling principal, got {:?}",
            case.label,
            member.get("createdBy")
        );
        assert!(
            member.get("createdAt").is_some(),
            "case {:?}: expected a createdAt, got {member:?}",
            case.label
        );
        assert!(
            listed.body.get("nextToken").is_none_or(Value::is_null),
            "case {:?}: a single-grant page must not offer a nextToken, got {:?}",
            case.label,
            listed.body
        );
    }

    // Scoping: the same entity under another policy type, and the same policy type under
    // another entity, report nothing of this grant.
    for (case, (_, grant_id)) in cases.iter().zip(&ids) {
        let other_policy = cases
            .iter()
            .map(|c| c.policy_type)
            .find(|p| *p != case.policy_type)
            .expect("the table has more than one policy type");
        let wrong_policy = dz
            .call(
                "GET",
                &list_uri(&domain, case.entity_type, &case.entity_id, other_policy),
                None,
            )
            .await;
        assert!(
            grant_list(&wrong_policy.body)
                .iter()
                .all(|m| &m.s_grant_id() != grant_id),
            "case {:?}: a grant must not be reported under policy type {other_policy}, got {:?}",
            case.label,
            wrong_policy.body
        );
        let wrong_entity = dz
            .call(
                "GET",
                &list_uri(&domain, case.entity_type, "no-such-entity", case.policy_type),
                None,
            )
            .await;
        assert_eq!(
            (wrong_entity.status, grant_list(&wrong_entity.body).len()),
            (200, 0),
            "case {:?}: an entity nothing was granted on must report an empty list, got {} {:?}",
            case.label,
            wrong_entity.status,
            wrong_entity.body
        );
    }

    // A grant lives in its domain: once the domain is being deleted, the listing no
    // longer identifies a domain to list grants for.
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "expected 202 from DeleteDomain, got {} {:?}",
        deleted.status, deleted.body
    );
    let after_delete = dz
        .call(
            "GET",
            &list_uri(&domain, "DOMAIN_UNIT", &root, "CREATE_DOMAIN_UNIT"),
            None,
        )
        .await;
    assert_eq!(
        (after_delete.status, after_delete.error_code.as_str()),
        (400, "ValidationException"),
        "listing grants of a deleted domain must be refused, got {} {:?}",
        after_delete.status,
        after_delete.body
    );
}

#[tokio::test]
async fn detail_must_be_the_member_the_policy_type_names() {
    let dz = Emulator::new();
    let domain = dz.available_domain("mismatch-domain").await;
    let catalog = policy_catalog();

    // Every policy type, paired with the NEXT entry's detail: a detail that describes a
    // different policy from the one the request asks to grant.
    for (i, (policy_type, _)) in catalog.iter().enumerate() {
        let (other_policy, other_detail) = &catalog[(i + 1) % catalog.len()];
        let label = format!("policyType {policy_type} with the {other_policy} detail");
        let answer = dz
            .call(
                "POST",
                &add_uri(&domain, "DOMAIN_UNIT", "entity-mismatch"),
                Some(json!({
                    "policyType": policy_type,
                    "principal": { "user": { "userIdentifier": "someone" } },
                    "detail": other_detail.clone(),
                })),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );

        // The matching pairing is accepted, so the refusal is about the pairing and
        // nothing else.
        let matching = dz
            .call(
                "POST",
                &add_uri(&domain, "DOMAIN_UNIT", "entity-mismatch"),
                Some(json!({
                    "policyType": policy_type,
                    "principal": { "user": { "userIdentifier": "someone" } },
                    "detail": catalog[i].1.clone(),
                })),
            )
            .await;
        assert_eq!(
            matching.status, 201,
            "case {label:?}: the matching detail must be accepted, got {} {:?}",
            matching.status, matching.body
        );
    }

    // Nothing the refused requests described was stored: every entry in the listing is
    // the detail its own policy type names.
    for (policy_type, detail) in &catalog {
        let listed = dz
            .call(
                "GET",
                &list_uri(&domain, "DOMAIN_UNIT", "entity-mismatch", policy_type),
                None,
            )
            .await;
        let members = grant_list(&listed.body);
        assert_eq!(
            members.len(),
            1,
            "policy type {policy_type}: only the accepted grant must be stored, got {:?}",
            listed.body
        );
        assert_eq!(
            without_nulls(members[0].get("detail").unwrap_or(&Value::Null)),
            without_nulls(detail),
            "policy type {policy_type}: the stored detail must be the accepted one, got {:?}",
            members[0].get("detail")
        );
    }
}

#[tokio::test]
async fn an_unknown_or_unsettled_domain_is_refused() {
    let dz = Emulator::new();
    let body = json!({
        "policyType": "CREATE_GLOSSARY",
        "principal": { "user": { "userIdentifier": "someone" } },
        "detail": { "createGlossary": { "includeChildDomainUnits": true } },
    });

    // A well-formed domain id nothing was created under: a permission is held ON a
    // domain, so the caller holds none here and authorization decides the call first.
    let unknown = dz
        .call(
            "POST",
            &add_uri("dzd_unknowndomain", "DOMAIN_UNIT", "entity-1"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code.as_str(), unknown.s("message")),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: AddPolicyGrant".to_string()
        ),
        "a domain nothing created must be denied, got {} {:?}",
        unknown.status,
        unknown.body
    );

    // A domain the clock has not finished creating cannot take a grant yet.
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "unsettled" })))
        .await;
    let domain = created.s("id");
    let creating = dz
        .call(
            "POST",
            &add_uri(&domain, "DOMAIN_UNIT", "entity-1"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        (creating.status, creating.error_code.as_str()),
        (409, "ConflictException"),
        "a CREATING domain must refuse a grant with the modelled conflict, got {} {:?}",
        creating.status,
        creating.body
    );

    // Once the creation the clock owed has settled, the same request is accepted.
    tokio::time::sleep(SETTLED).await;
    let settled = dz
        .call(
            "POST",
            &add_uri(&domain, "DOMAIN_UNIT", "entity-1"),
            Some(body),
        )
        .await;
    assert_eq!(
        settled.status, 201,
        "an AVAILABLE domain must accept the grant, got {} {:?}",
        settled.status, settled.body
    );
}

#[tokio::test]
async fn the_listing_pages_over_every_stored_grant_exactly_once() {
    let dz = Emulator::new();
    let domain = dz.available_domain("paging-domain").await;
    let entity = "entity-paged";
    let policy_type = "CREATE_PROJECT";

    // Seven grants on one entity and policy type, told apart by their principal.
    let mut added: Vec<String> = Vec::new();
    for n in 0..7 {
        let answer = dz
            .call(
                "POST",
                &add_uri(&domain, "DOMAIN_UNIT", entity),
                Some(json!({
                    "policyType": policy_type,
                    "principal": { "user": { "userIdentifier": format!("user-{n}") } },
                    "detail": { "createProject": { "includeChildDomainUnits": false } },
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "grant {n}: expected 201, got {} {:?}",
            answer.status, answer.body
        );
        added.push(answer.s("grantId"));
    }
    added.sort();

    let base = list_uri(&domain, "DOMAIN_UNIT", entity, policy_type);

    // Page sizes spanning the modelled 1..25 range (smithy_query
    // operation="ListPolicyGrants"): both published bounds and a size in between.
    for page_size in [1usize, 3, 25] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let uri = match &token {
                Some(t) => format!("{base}&maxResults={page_size}&nextToken={t}"),
                None => format!("{base}&maxResults={page_size}"),
            };
            let page = dz.call("GET", &uri, None).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let members = grant_list(&page.body);
            assert!(
                members.len() <= page_size,
                "page size {page_size}: a page returned {} grants",
                members.len()
            );
            seen.extend(
                members
                    .iter()
                    .map(|m| m.s_grant_id())
                    .collect::<Vec<String>>(),
            );
            token = page
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen, added,
            "page size {page_size}: paging must walk every stored grant exactly once"
        );
    }

    // A request naming no maxResults gets the whole set, since it is smaller than the
    // top of the range.
    let unpaged = dz.call("GET", &base, None).await;
    let mut all: Vec<String> = grant_list(&unpaged.body)
        .iter()
        .map(|m| m.s_grant_id())
        .collect();
    all.sort();
    assert_eq!(
        all, added,
        "a listing with no maxResults must report every stored grant, got {:?}",
        unpaged.body
    );

    // A token this service never issued is not a resumption point.
    let bad_token = dz
        .call("GET", &format!("{base}&nextToken=not-a-token"), None)
        .await;
    assert_eq!(
        (bad_token.status, bad_token.error_code.as_str()),
        (400, "ValidationException"),
        "a foreign nextToken must be refused, got {} {:?}",
        bad_token.status,
        bad_token.body
    );
}

#[tokio::test]
async fn remove_drops_the_named_principals_grants_and_nothing_else() {
    let dz = Emulator::new();
    let domain = dz.available_domain("remove-domain").await;
    let entity = "entity-removal";
    let policy_type = "CREATE_GLOSSARY";
    let detail = json!({ "createGlossary": { "includeChildDomainUnits": true } });
    let base = list_uri(&domain, "DOMAIN_UNIT", entity, policy_type);

    // One grant per principal form, so a removal has to pick its principal out of a set
    // that spans the union.
    let mut grants: Vec<(String, Value, String)> = Vec::new();
    for (label, principal) in principal_catalog() {
        let answer = dz
            .call(
                "POST",
                &add_uri(&domain, "DOMAIN_UNIT", entity),
                Some(json!({
                    "policyType": policy_type,
                    "principal": principal.clone(),
                    "detail": detail.clone(),
                })),
            )
            .await;
        assert_eq!(
            answer.status, 201,
            "case {label:?}: expected 201, got {} {:?}",
            answer.status, answer.body
        );
        grants.push((label.to_string(), principal, answer.s("grantId")));
    }

    // Removing a principal's grant leaves every other principal's grant in place.
    let mut remaining = grants.clone();
    while let Some((label, principal, grant_id)) = remaining.pop() {
        let removed = dz
            .call(
                "POST",
                &remove_uri(&domain, "DOMAIN_UNIT", entity),
                Some(json!({
                    "policyType": policy_type,
                    "principal": principal.clone(),
                })),
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "case {label:?}: expected 204 from RemovePolicyGrant, got {} {:?}",
            removed.status, removed.body
        );
        let listed = dz.call("GET", &base, None).await;
        let ids: Vec<String> = grant_list(&listed.body)
            .iter()
            .map(|m| m.s_grant_id())
            .collect();
        assert!(
            !ids.contains(&grant_id),
            "case {label:?}: the removed grant must be gone, got {ids:?}"
        );
        let mut expected: Vec<String> = remaining.iter().map(|(_, _, id)| id.clone()).collect();
        expected.sort();
        let mut got = ids;
        got.sort();
        assert_eq!(
            got, expected,
            "case {label:?}: every other principal's grant must survive the removal"
        );

        // Removing it again is a no-op that still succeeds: the operation is
        // `@idempotent` and models no not-found error.
        let again = dz
            .call(
                "POST",
                &remove_uri(&domain, "DOMAIN_UNIT", entity),
                Some(json!({
                    "policyType": policy_type,
                    "principal": principal.clone(),
                })),
            )
            .await;
        assert_eq!(
            again.status, 204,
            "case {label:?}: a repeated removal must still answer 204, got {} {:?}",
            again.status, again.body
        );
        let after = dz.call("GET", &base, None).await;
        let mut after_ids: Vec<String> = grant_list(&after.body)
            .iter()
            .map(|m| m.s_grant_id())
            .collect();
        after_ids.sort();
        assert_eq!(
            after_ids, expected,
            "case {label:?}: a repeated removal must change nothing"
        );
    }

    // `grantIdentifier` narrows a removal to one of a principal's grants.
    let principal = json!({ "user": { "userIdentifier": "narrow-user" } });
    let mut narrow: Vec<String> = Vec::new();
    for n in 0..3 {
        let answer = dz
            .call(
                "POST",
                &add_uri(&domain, "DOMAIN_UNIT", entity),
                Some(json!({
                    "policyType": policy_type,
                    "principal": principal.clone(),
                    "detail": detail.clone(),
                    "clientToken": format!("narrow-{n}"),
                })),
            )
            .await;
        narrow.push(answer.s("grantId"));
    }
    let dropped = narrow.remove(1);
    let narrowed = dz
        .call(
            "POST",
            &remove_uri(&domain, "DOMAIN_UNIT", entity),
            Some(json!({
                "policyType": policy_type,
                "principal": principal.clone(),
                "grantIdentifier": dropped,
            })),
        )
        .await;
    assert_eq!(
        narrowed.status, 204,
        "a narrowed removal must answer 204, got {} {:?}",
        narrowed.status, narrowed.body
    );
    let mut left: Vec<String> = grant_list(&dz.call("GET", &base, None).await.body)
        .iter()
        .map(|m| m.s_grant_id())
        .collect();
    left.sort();
    narrow.sort();
    assert_eq!(
        left, narrow,
        "a removal naming a grantIdentifier must drop exactly that grant"
    );

    // A grantIdentifier naming no stored grant removes nothing.
    let no_such = dz
        .call(
            "POST",
            &remove_uri(&domain, "DOMAIN_UNIT", entity),
            Some(json!({
                "policyType": policy_type,
                "principal": principal.clone(),
                "grantIdentifier": "AAAAAAAAAA",
            })),
        )
        .await;
    assert_eq!(
        no_such.status, 204,
        "a removal naming an unknown grant must answer 204, got {} {:?}",
        no_such.status, no_such.body
    );
    let mut still: Vec<String> = grant_list(&dz.call("GET", &base, None).await.body)
        .iter()
        .map(|m| m.s_grant_id())
        .collect();
    still.sort();
    assert_eq!(
        still, narrow,
        "a removal naming an unknown grant must change nothing"
    );

    // A domain the store serves no record of is denied rather than silently succeeding:
    // a permission is held ON a domain, so the caller holds none there.
    let unknown = dz
        .call(
            "POST",
            &remove_uri("dzd_unknowndomain", "DOMAIN_UNIT", entity),
            Some(json!({
                "policyType": policy_type,
                "principal": principal.clone(),
            })),
        )
        .await;
    assert_eq!(
        (unknown.status, unknown.error_code.as_str(), unknown.s("message")),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: RemovePolicyGrant".to_string()
        ),
        "removing a grant in a domain nothing created must be denied, got {} {:?}",
        unknown.status,
        unknown.body
    );
}

#[tokio::test]
async fn a_repeated_client_token_adds_no_second_grant() {
    let dz = Emulator::new();
    let domain = dz.available_domain("token-domain").await;
    let entity = "entity-token";
    let base = list_uri(&domain, "ASSET_TYPE", entity, "USE_ASSET_TYPE");
    let body = json!({
        "policyType": "USE_ASSET_TYPE",
        "principal": { "user": { "userIdentifier": "token-user" } },
        "detail": { "useAssetType": { "domainUnitId": "du-example-one" } },
        "clientToken": "token-abc",
    });

    let first = dz
        .call("POST", &add_uri(&domain, "ASSET_TYPE", entity), Some(body.clone()))
        .await;
    assert_eq!(
        first.status, 201,
        "the first add must be accepted, got {} {:?}",
        first.status, first.body
    );
    let replay = dz
        .call("POST", &add_uri(&domain, "ASSET_TYPE", entity), Some(body))
        .await;
    assert_eq!(
        replay.s("grantId"),
        first.s("grantId"),
        "a replayed clientToken must answer with the grant the first request created"
    );
    let listed = dz.call("GET", &base, None).await;
    assert_eq!(
        grant_list(&listed.body).len(),
        1,
        "a replayed clientToken must not add a second grant, got {:?}",
        listed.body
    );

    // A different token is a different request, and a different principal is a
    // different grant.
    let other = dz
        .call(
            "POST",
            &add_uri(&domain, "ASSET_TYPE", entity),
            Some(json!({
                "policyType": "USE_ASSET_TYPE",
                "principal": { "group": { "groupIdentifier": "token-group" } },
                "detail": { "useAssetType": { "domainUnitId": "du-example-one" } },
                "clientToken": "token-xyz",
            })),
        )
        .await;
    assert_eq!(
        other.status, 201,
        "a second principal must get its own grant, got {} {:?}",
        other.status, other.body
    );
    assert_ne!(
        other.s("grantId"),
        first.s("grantId"),
        "two different grants must not share an id"
    );
    let both = dz.call("GET", &base, None).await;
    assert_eq!(
        grant_list(&both.body).len(),
        2,
        "both grants must be reported, got {:?}",
        both.body
    );

    // The first token again, but describing a DIFFERENT grant: that is not a replay of
    // the request the token created, so the grant it describes is added.
    let reused = dz
        .call(
            "POST",
            &add_uri(&domain, "ASSET_TYPE", entity),
            Some(json!({
                "policyType": "USE_ASSET_TYPE",
                "principal": { "user": { "userIdentifier": "another-user" } },
                "detail": { "useAssetType": { "domainUnitId": "du-example-one" } },
                "clientToken": "token-abc",
            })),
        )
        .await;
    assert_eq!(
        reused.status, 201,
        "a token reused for a different grant must still add it, got {} {:?}",
        reused.status, reused.body
    );
    assert_ne!(
        reused.s("grantId"),
        first.s("grantId"),
        "a token reused for a different grant must not answer with the first grant's id"
    );
    let all_three = dz.call("GET", &base, None).await;
    assert_eq!(
        grant_list(&all_three.body).len(),
        3,
        "every distinct grant must be reported, got {:?}",
        all_three.body
    );
}
