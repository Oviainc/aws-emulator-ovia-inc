// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what decides a DOMAIN-SCOPED call that is NOT a
//! single-resource read: a create, an update, a delete, a listing, and the delete whose
//! own identifier IS the domain.
//!
//! The rule: a DataZone permission is held ON a domain, and the service decides every
//! domain-scoped call by authorization BEFORE it resolves the domain or any resource
//! inside it — so a call scoped to a domain this store serves no record of is refused
//! with `AccessDeniedException` (HTTP 403; every operation below binds it, e.g.
//! `smithy_query(query="operation", operation="DeleteProject")`), never with an
//! existence answer. The recorded wording follows what the request addresses: a call
//! INSIDE a domain carries `User is not permitted to perform operation: <OperationName>`,
//! whose only variable part is the operation name of the request, while the call whose
//! own identifier IS the domain scope (`DeleteDomain`) carries no message at all.
//! Existence is reported only INSIDE a domain the caller may act in: a resource the
//! served domain does not hold is still that operation's own miss (404
//! `ResourceNotFoundException`, or 400 `ValidationException` where the operation models
//! no not-found).
//!
//! `domain_scoped_read_authorization` asserts the same rule over the single-resource
//! reads; this suite is its counterpart for everything else, so the two together cover
//! every domain-scoped operation this emulator serves.
//!
//! Each rule is asserted over a TABLE, not one request:
//!
//! * every domain-scoped create, update, delete and listing this emulator serves,
//!   crossed with domain identifiers differing in separator, length and content, each
//!   satisfying `DomainId`'s pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query
//!   shape="DomainId") and naming no stored domain;
//! * the same calls addressing the SAME stored resources through the domain that holds
//!   them, which are never denied — so the denial is about the scope, not the operation;
//! * the same calls addressing resources the served domain does not hold, which answer
//!   the operation's own miss — so authorization has not swallowed the existence answer;
//! * the same calls once the served domain has been DELETED, which are denied too: the
//!   identifier names no domain any more.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// Domain identifiers no domain in the store carries, each satisfying `DomainId`'s
/// modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so
/// that nothing refuses them before a handler runs. The rows differ along what the
/// pattern leaves open: which separator, the shortest and the longest tail it admits,
/// and a tail mixing the other characters of the class.
fn unserved_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an underscore separator", "dzd_nowritedomain000".to_string()),
        ("a hyphen separator", "dzd-nowritedomain001".to_string()),
        ("the shortest tail the pattern admits", "dzd_w".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd_{}", "w".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_8-absent_1".to_string(),
        ),
    ]
}

/// One domain-scoped call: the operation the model binds, the HTTP method and the path
/// it addresses under `/v2/domains/{domainIdentifier}` (empty for `DeleteDomain`, whose
/// resource IS the domain the request is scoped to), plus the request body if it takes
/// one.
struct Call {
    operation: &'static str,
    method: &'static str,
    suffix: String,
    body: Option<Value>,
}

impl Call {
    fn new(
        operation: &'static str,
        method: &'static str,
        suffix: String,
        body: Option<Value>,
    ) -> Self {
        Self {
            operation,
            method,
            suffix,
            body,
        }
    }

    /// This call, scoped to `domain`.
    fn uri(&self, domain: &str) -> String {
        if self.suffix.is_empty() {
            format!("/v2/domains/{domain}")
        } else {
            format!("/v2/domains/{domain}/{}", self.suffix)
        }
    }

    /// The `message` this call's denial must carry: the recorded operation-scoped
    /// sentence for a call INSIDE a domain, and nothing at all for the one whose own
    /// identifier is the domain scope (recorded behaviour: that denial's body carries no
    /// members).
    fn denial_message(&self) -> String {
        if self.suffix.is_empty() {
            String::new()
        } else {
            format!(
                "User is not permitted to perform operation: {}",
                self.operation
            )
        }
    }
}

/// One domain holding one of every resource the calls below address.
struct Served {
    domain: String,
    root_unit: String,
    unit: String,
    project: String,
    asset: String,
    form_type: String,
    glossary: String,
    glossary_term: String,
}

/// A `UserDesignation` the model declares (smithy_query shape="UserDesignation").
const DESIGNATION: &str = "PROJECT_CONTRIBUTOR";

/// The one `DataZoneEntityType` member (smithy_query shape="DataZoneEntityType").
const ENTITY_TYPE: &str = "DOMAIN_UNIT";

impl Served {
    /// Every domain-scoped create, update, delete and listing this emulator serves, each
    /// addressing the resource this fixture really stores.
    ///
    /// Ordered so that a pass over the table against the SERVING domain keeps its own
    /// preconditions: the creates, listings and updates run before the deletes, and the
    /// delete of the domain itself is last.
    fn calls(&self) -> Vec<Call> {
        let project = &self.project;
        let asset = &self.asset;
        let unit = &self.unit;
        let member = json!({ "userIdentifier": "denied.member" });
        let owner = json!({ "owner": { "user": { "userIdentifier": "denied.owner" } } });
        let grant = json!({
            "policyType": "CREATE_GLOSSARY",
            "principal": { "user": { "userIdentifier": "denied.grantee" } },
            "detail": { "createGlossary": { "includeChildDomainUnits": true } },
        });
        vec![
            Call::new(
                "CreateProject",
                "POST",
                "projects".to_string(),
                Some(json!({ "name": "denied project" })),
            ),
            Call::new("ListProjects", "GET", "projects".to_string(), None),
            Call::new(
                "GetProject",
                "GET",
                format!("projects/{project}"),
                None,
            ),
            Call::new(
                "UpdateProject",
                "PATCH",
                format!("projects/{project}"),
                Some(json!({ "description": "patched by a denied caller" })),
            ),
            Call::new(
                "CreateProjectMembership",
                "POST",
                format!("projects/{project}/createMembership"),
                Some(json!({ "member": member, "designation": DESIGNATION })),
            ),
            Call::new(
                "ListProjectMemberships",
                "GET",
                format!("projects/{project}/memberships"),
                None,
            ),
            Call::new(
                "DeleteProjectMembership",
                "POST",
                format!("projects/{project}/deleteMembership"),
                Some(json!({ "member": member })),
            ),
            Call::new(
                "CreateFormType",
                "POST",
                "form-types".to_string(),
                Some(json!({
                    "name": "DeniedForm",
                    "model": { "smithy": "structure DeniedForm {\n  id: String\n}" },
                    "owningProjectIdentifier": project,
                })),
            ),
            Call::new(
                "CreateAssetType",
                "POST",
                "asset-types".to_string(),
                Some(json!({
                    "name": "DeniedType",
                    "owningProjectIdentifier": project,
                    "formsInput": {},
                })),
            ),
            Call::new(
                "CreateAsset",
                "POST",
                "assets".to_string(),
                Some(json!({
                    "name": "denied asset",
                    "typeIdentifier": "amazon.datazone.RelationalTable",
                    "owningProjectIdentifier": project,
                })),
            ),
            Call::new(
                "CreateAssetRevision",
                "POST",
                format!("assets/{asset}/revisions"),
                Some(json!({ "name": "denied revision" })),
            ),
            Call::new(
                "ListAssetRevisions",
                "GET",
                format!("assets/{asset}/revisions"),
                None,
            ),
            Call::new(
                "CreateAssetFilter",
                "POST",
                format!("assets/{asset}/filters"),
                Some(json!({
                    "name": "denied filter",
                    "configuration": {
                        "columnConfiguration": { "includedColumnNames": ["order_id"] },
                    },
                })),
            ),
            Call::new(
                "CreateAccountPool",
                "POST",
                "account-pools".to_string(),
                Some(json!({
                    "name": "denied pool",
                    "resolutionStrategy": "MANUAL",
                    "accountSource": {
                        "accounts": [
                            { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                        ]
                    },
                })),
            ),
            Call::new("ListAccountPools", "GET", "account-pools".to_string(), None),
            Call::new(
                "CreateDomainUnit",
                "POST",
                "domain-units".to_string(),
                Some(json!({
                    "name": "denied unit",
                    "parentDomainUnitIdentifier": self.root_unit,
                })),
            ),
            Call::new(
                "ListDomainUnitsForParent",
                "GET",
                format!(
                    "domain-units?parentDomainUnitIdentifier={}",
                    self.root_unit
                ),
                None,
            ),
            Call::new(
                "AddEntityOwner",
                "POST",
                format!("entities/{ENTITY_TYPE}/{unit}/addOwner"),
                Some(owner.clone()),
            ),
            Call::new(
                "ListEntityOwners",
                "GET",
                format!("entities/{ENTITY_TYPE}/{unit}/owners"),
                None,
            ),
            Call::new(
                "RemoveEntityOwner",
                "POST",
                format!("entities/{ENTITY_TYPE}/{unit}/removeOwner"),
                Some(owner),
            ),
            Call::new(
                "AddPolicyGrant",
                "POST",
                format!("policies/managed/{ENTITY_TYPE}/{unit}/addGrant"),
                Some(grant.clone()),
            ),
            Call::new(
                "ListPolicyGrants",
                "GET",
                format!(
                    "policies/managed/{ENTITY_TYPE}/{unit}/grants?policyType=CREATE_GLOSSARY"
                ),
                None,
            ),
            Call::new(
                "RemovePolicyGrant",
                "POST",
                format!("policies/managed/{ENTITY_TYPE}/{unit}/removeGrant"),
                Some(grant),
            ),
            Call::new(
                "CreateGlossary",
                "POST",
                "glossaries".to_string(),
                Some(json!({ "name": "denied glossary", "owningProjectIdentifier": project })),
            ),
            Call::new(
                "CreateGlossaryTerm",
                "POST",
                "glossary-terms".to_string(),
                Some(json!({ "glossaryIdentifier": self.glossary, "name": "denied term" })),
            ),
            Call::new(
                "UpdateGlossaryTerm",
                "PATCH",
                format!("glossary-terms/{}", self.glossary_term),
                Some(json!({ "status": "DISABLED" })),
            ),
            Call::new(
                "CreateListingChangeSet",
                "POST",
                "listings/change-set".to_string(),
                Some(json!({
                    "entityIdentifier": asset,
                    "entityType": "ASSET",
                    "action": "PUBLISH",
                })),
            ),
            Call::new(
                "CreateUserProfile",
                "POST",
                "user-profiles".to_string(),
                Some(json!({ "userIdentifier": "denied.profile" })),
            ),
            Call::new(
                "DeleteFormType",
                "DELETE",
                format!("form-types/{}", self.form_type),
                None,
            ),
            Call::new("DeleteAsset", "DELETE", format!("assets/{asset}"), None),
            Call::new(
                "UpdateGlossary",
                "PATCH",
                format!("glossaries/{}", self.glossary),
                Some(json!({ "status": "DISABLED" })),
            ),
            Call::new(
                "DeleteGlossary",
                "DELETE",
                format!("glossaries/{}", self.glossary),
                None,
            ),
            Call::new(
                "DeleteDomainUnit",
                "DELETE",
                format!("domain-units/{unit}"),
                None,
            ),
            Call::new(
                "DeleteProject",
                "DELETE",
                format!("projects/{project}"),
                None,
            ),
            // The one whose own identifier IS the domain scope, so it runs last.
            Call::new("DeleteDomain", "DELETE", String::new(), None),
        ]
    }
}

/// The same calls, each addressing an identifier that satisfies its member's modelled
/// pattern but names nothing the served domain holds, and the miss each must answer
/// with. A `ValidationException` row is an operation that models no
/// `ResourceNotFoundException`, so its own refusal for an unresolvable member is bad
/// input.
fn unstored_resource_calls() -> Vec<(Call, u16, &'static str)> {
    let member = json!({ "userIdentifier": "denied.member" });
    let owner = json!({ "owner": { "user": { "userIdentifier": "denied.owner" } } });
    vec![
        (
            Call::new("GetProject", "GET", "projects/nosuchproject".to_string(), None),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "UpdateProject",
                "PATCH",
                "projects/nosuchproject".to_string(),
                Some(json!({ "description": "no such project" })),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "DeleteProject",
                "DELETE",
                "projects/nosuchproject".to_string(),
                None,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "ListProjectMemberships",
                "GET",
                "projects/nosuchproject/memberships".to_string(),
                None,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "DeleteProjectMembership",
                "POST",
                "projects/nosuchproject/deleteMembership".to_string(),
                Some(json!({ "member": member })),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "DeleteFormType",
                "DELETE",
                "form-types/NoSuchFormType".to_string(),
                None,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new("DeleteAsset", "DELETE", "assets/nosuchasset".to_string(), None),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "ListAssetRevisions",
                "GET",
                "assets/nosuchasset/revisions".to_string(),
                None,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "UpdateGlossary",
                "PATCH",
                "glossaries/nosuchglossary".to_string(),
                Some(json!({ "status": "DISABLED" })),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "DeleteGlossary",
                "DELETE",
                "glossaries/nosuchglossary".to_string(),
                None,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "UpdateGlossaryTerm",
                "PATCH",
                "glossary-terms/nosuchterm".to_string(),
                Some(json!({ "status": "DISABLED" })),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "AddEntityOwner",
                "POST",
                format!("entities/{ENTITY_TYPE}/nosuchunit/addOwner"),
                Some(owner.clone()),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "RemoveEntityOwner",
                "POST",
                format!("entities/{ENTITY_TYPE}/nosuchunit/removeOwner"),
                Some(owner),
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            Call::new(
                "ListEntityOwners",
                "GET",
                format!("entities/{ENTITY_TYPE}/nosuchunit/owners"),
                None,
            ),
            400,
            "ValidationException",
        ),
        (
            Call::new(
                "ListDomainUnitsForParent",
                "GET",
                "domain-units?parentDomainUnitIdentifier=nosuchparent".to_string(),
                None,
            ),
            400,
            "ValidationException",
        ),
    ]
}

/// Create a domain with one of every resource the calls address, through the same HTTP
/// surface a client uses.
async fn served(dz: &Emulator, domain_name: &str) -> Served {
    let created = dz
        .call("POST", "/v2/domains", Some(json!({ "name": domain_name })))
        .await;
    assert_eq!(
        created.status, 201,
        "creating domain {domain_name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    let domain = created.s("id");
    let root_unit = created.s("rootDomainUnitId");
    // The domain must have settled to AVAILABLE before it takes resources.
    tokio::time::sleep(SETTLED).await;

    let project = dz.project(&domain, "write scope owner").await;

    let unit = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": "scope unit", "parentDomainUnitIdentifier": root_unit })),
        )
        .await;
    assert_eq!(
        unit.status, 201,
        "CreateDomainUnit: expected 201, got {} {:?}",
        unit.status, unit.body
    );

    let asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "scope asset",
                "typeIdentifier": "amazon.datazone.RelationalTable",
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        asset.status, 201,
        "CreateAsset: expected 201, got {} {:?}",
        asset.status, asset.body
    );

    let form_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": "ScopeForm",
                "model": { "smithy": "structure ScopeForm {\n  id: String\n}" },
                "owningProjectIdentifier": project,
            })),
        )
        .await;
    assert_eq!(
        form_type.status, 201,
        "CreateFormType: expected 201, got {} {:?}",
        form_type.status, form_type.body
    );

    let glossary = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossaries"),
            Some(json!({ "name": "scope glossary", "owningProjectIdentifier": project })),
        )
        .await;
    assert_eq!(
        glossary.status, 201,
        "CreateGlossary: expected 201, got {} {:?}",
        glossary.status, glossary.body
    );
    let glossary_id = glossary.s("id");

    let term = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossary-terms"),
            Some(json!({ "glossaryIdentifier": glossary_id, "name": "scope term" })),
        )
        .await;
    assert_eq!(
        term.status, 201,
        "CreateGlossaryTerm: expected 201, got {} {:?}",
        term.status, term.body
    );

    Served {
        domain,
        root_unit,
        unit: unit.s("id"),
        project,
        asset: asset.s("id"),
        form_type: form_type.s("name"),
        glossary: glossary_id,
        glossary_term: term.s("id"),
    }
}

#[tokio::test]
async fn every_domain_scoped_call_is_denied_when_the_store_serves_no_such_domain() {
    let dz = Emulator::new();
    let fixture = served(&dz, "scoped-write-domain").await;

    for (domain_label, unserved) in unserved_domain_ids() {
        for call in fixture.calls() {
            // The resource identifiers are the ones the fixture really stores, so the
            // only thing wrong with the request is the domain it is scoped to.
            let case = format!("{} through {domain_label}", call.operation);
            let answer = dz
                .call(call.method, &call.uri(&unserved), call.body.clone())
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected the modelled denial, got {} {:?}",
                answer.status,
                answer.body
            );
            let message = answer.s("message");
            assert_eq!(
                message,
                call.denial_message(),
                "case {case:?}: the denial must carry the recorded message, got {message:?}"
            );
            assert!(
                !message.contains(&unserved),
                "case {case:?}: the denial must not echo the requested domain {unserved:?}, \
                 got {message:?}"
            );
            assert!(
                answer.body.get("id").is_none() && answer.body.get("status").is_none(),
                "case {case:?}: a denied call must report no resource, got {:?}",
                answer.body
            );
        }
    }

    // Nothing above reached the store: every resource the denied calls addressed is
    // still exactly where the fixture put it, and the denied creates stored nothing.
    let project = dz
        .call(
            "GET",
            &format!(
                "/v2/domains/{}/projects/{}",
                fixture.domain, fixture.project
            ),
            None,
        )
        .await;
    assert_eq!(
        (project.status, project.s("name").as_str()),
        (200, "write scope owner"),
        "the project the denied calls addressed must be untouched, got {} {:?}",
        project.status,
        project.body
    );
    let projects = dz
        .call("GET", &format!("/v2/domains/{}/projects", fixture.domain), None)
        .await;
    assert_eq!(
        projects.body["items"].as_array().map(Vec::len),
        Some(1),
        "a denied create must store nothing, got {:?}",
        projects.body
    );
}

#[tokio::test]
async fn the_same_calls_are_not_denied_under_the_domain_that_serves_them() {
    let dz = Emulator::new();
    let fixture = served(&dz, "served-write-domain").await;

    // The round trip first: a create under the served domain is stored and reported by
    // the reader, so the pass below runs against a domain whose calls really work.
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/projects", fixture.domain),
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
            &format!(
                "/v2/domains/{}/projects/{}",
                fixture.domain,
                created.s("id")
            ),
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

    // The control for the rule above: authorization refuses an UNSERVED scope and
    // nothing more, so not one of these calls is denied under the domain that holds the
    // resources they address. What each then answers is its own business — a delete may
    // be refused by a documented precondition — but never 403.
    for call in fixture.calls() {
        let answer = dz
            .call(call.method, &call.uri(&fixture.domain), call.body.clone())
            .await;
        assert_ne!(
            (answer.status, answer.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: a call under its own domain must not be denied, got {:?}",
            call.operation,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_resource_a_served_domain_does_not_hold_is_still_an_existence_answer() {
    let dz = Emulator::new();
    let fixture = served(&dz, "existence-write-domain").await;

    // Inside a domain the caller may act in, existence decides the answer again: the
    // denial above has not swallowed the miss each operation models.
    for (call, status, code) in unstored_resource_calls() {
        let answer = dz
            .call(call.method, &call.uri(&fixture.domain), call.body.clone())
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {:?}: expected the operation's own miss, got {} {:?}",
            call.operation,
            answer.status,
            answer.body
        );
        let message = answer.s("message");
        assert!(
            !message.contains("not permitted"),
            "case {:?}: a miss inside a served domain is not a denial, got {message:?}",
            call.operation
        );
    }
}

#[tokio::test]
async fn every_call_is_denied_once_the_domain_it_is_scoped_to_is_deleted() {
    let dz = Emulator::new();
    let fixture = served(&dz, "deleted-write-domain").await;

    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{}", fixture.domain), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected the modelled success status, got {} {:?}",
        deleted.status, deleted.body
    );
    // Once the deletion the clock owed has completed, the identifier names no domain,
    // so nothing is held on it any more — including a repeat of the delete itself.
    tokio::time::sleep(SETTLED).await;

    for call in fixture.calls() {
        let answer = dz
            .call(call.method, &call.uri(&fixture.domain), call.body.clone())
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: a call scoped to a deleted domain must be denied, got {} {:?}",
            call.operation,
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.s("message"),
            call.denial_message(),
            "case {:?}: the denial must carry the recorded message, got {:?}",
            call.operation,
            answer.body
        );
    }
}
