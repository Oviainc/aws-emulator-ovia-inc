// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what decides a DOMAIN-SCOPED single-resource read.
//!
//! The rule: a DataZone permission is held ON a domain, and the service decides such a
//! read by authorization BEFORE it resolves the domain or the resource inside it — so a
//! read scoped to a domain this store serves no record of is refused with
//! `AccessDeniedException` (HTTP 403, `smithy_query` operation="GetAccountPool" and the
//! ten sibling reads below all bind it). The recorded wording follows what the request
//! addresses: a read INSIDE a domain carries
//! `User is not permitted to perform operation: <OperationName>`, whose only variable
//! part is the operation name of the request, while the read whose own identifier IS the
//! domain scope (`GetDomain`) carries no message at all. Existence is reported only
//! INSIDE a domain the caller may act in: a resource the served domain does not hold is
//! still a `ResourceNotFoundException` (HTTP 404).
//!
//! Each rule is asserted over a TABLE, not one request:
//!
//! * every single-resource read this emulator serves — `GetDomain`, `GetDomainUnit`,
//!   `GetProject`, `GetAccountPool`, `GetAssetType`, `GetAsset`, `GetAssetFilter`,
//!   `GetFormType`, `GetGlossary`, `GetGlossaryTerm` and `GetListing` — crossed with
//!   domain identifiers differing in separator, length and content, each satisfying
//!   `DomainId`'s pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId")
//!   and naming no stored domain;
//! * the same reads addressing the SAME stored resources through the domain that holds
//!   them, which answer 200 — so the denial is about the scope, not the operation;
//! * the same reads addressing resources the served domain does not hold, which answer
//!   404 — so authorization has not swallowed the existence answer;
//! * the same reads once the served domain has been DELETED, which are denied too: the
//!   identifier names no domain any more.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Emulator, SETTLED};
use serde_json::json;

/// Domain identifiers no domain in the store carries, each satisfying `DomainId`'s
/// modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so
/// that nothing refuses them before a handler runs. The rows differ along what the
/// pattern leaves open: which separator, the shortest and the longest tail it admits,
/// and a tail mixing the other characters of the class.
fn unserved_domain_ids() -> Vec<(&'static str, String)> {
    vec![
        ("an underscore separator", "dzd_nosuchdomain000".to_string()),
        ("a hyphen separator", "dzd-nosuchdomain001".to_string()),
        ("the shortest tail the pattern admits", "dzd_a".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd_{}", "d".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_9-absent_0".to_string(),
        ),
    ]
}

/// One single-resource read: the operation the model binds, and the path it addresses
/// under `/v2/domains/{domainIdentifier}` (empty for `GetDomain`, whose resource IS the
/// domain the request is scoped to).
struct Read {
    operation: &'static str,
    suffix: String,
}

impl Read {
    fn new(operation: &'static str, suffix: String) -> Self {
        Self { operation, suffix }
    }

    /// This read, scoped to `domain`.
    fn uri(&self, domain: &str) -> String {
        if self.suffix.is_empty() {
            format!("/v2/domains/{domain}")
        } else {
            format!("/v2/domains/{domain}/{}", self.suffix)
        }
    }

    /// The `message` member this read's denial must carry: the recorded operation-scoped
    /// sentence for a read INSIDE a domain, and NO member at all for the one whose own
    /// identifier is the domain scope (recorded behaviour: that denial's body carries no
    /// members, and a member the service states nothing in is absent from a `restJson1`
    /// body rather than present and empty).
    fn denial_message(&self) -> Option<String> {
        if self.suffix.is_empty() {
            None
        } else {
            Some(format!(
                "User is not permitted to perform operation: {}",
                self.operation
            ))
        }
    }
}

/// The `message` member an answer carries, absent when the body carries none.
fn denial_message_member(answer: &crate::harness::Answer) -> Option<String> {
    answer
        .body
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// One domain with one of every resource the reads below address.
struct Served {
    domain: String,
    root_unit: String,
    project: String,
    account_pool: String,
    asset_type: String,
    asset: String,
    asset_filter: String,
    form_type: String,
    glossary: String,
    glossary_term: String,
    listing: String,
}

impl Served {
    /// The eleven reads, each addressing the resource this fixture really stores.
    fn reads(&self) -> Vec<Read> {
        vec![
            Read::new("GetDomain", String::new()),
            Read::new("GetDomainUnit", format!("domain-units/{}", self.root_unit)),
            Read::new("GetProject", format!("projects/{}", self.project)),
            Read::new(
                "GetAccountPool",
                format!("account-pools/{}", self.account_pool),
            ),
            Read::new("GetAssetType", format!("asset-types/{}", self.asset_type)),
            Read::new("GetAsset", format!("assets/{}", self.asset)),
            Read::new(
                "GetAssetFilter",
                format!("assets/{}/filters/{}", self.asset, self.asset_filter),
            ),
            Read::new("GetFormType", format!("form-types/{}", self.form_type)),
            Read::new("GetGlossary", format!("glossaries/{}", self.glossary)),
            Read::new(
                "GetGlossaryTerm",
                format!("glossary-terms/{}", self.glossary_term),
            ),
            Read::new("GetListing", format!("listings/{}", self.listing)),
        ]
    }
}

/// The same reads, each addressing an identifier that satisfies its member's modelled
/// pattern but names nothing the store holds. `GetDomain` has no such row: the only
/// identifier it takes is the domain scope itself, which the denial rule decides.
fn unstored_resource_reads() -> Vec<Read> {
    vec![
        // `DomainUnitId` is 1..256 of `^[a-z0-9_\-]+$`.
        Read::new("GetDomainUnit", "domain-units/nosuchunit".to_string()),
        // `ProjectId`, `AccountPoolId`, `AssetIdentifier`, `FilterId`, `GlossaryId`,
        // `GlossaryTermId` and `ListingId` are all `^[a-zA-Z0-9_-]{1,36}$`.
        Read::new("GetProject", "projects/nosuchproject".to_string()),
        Read::new("GetAccountPool", "account-pools/nosuchpool".to_string()),
        // `AssetTypeIdentifier` and `FormTypeIdentifier` are `^(?!\.)[\w\.]*\w$`.
        Read::new("GetAssetType", "asset-types/NoSuchAssetType".to_string()),
        Read::new("GetAsset", "assets/nosuchasset".to_string()),
        Read::new(
            "GetAssetFilter",
            "assets/nosuchasset/filters/nosuchfilter".to_string(),
        ),
        Read::new("GetFormType", "form-types/NoSuchFormType".to_string()),
        Read::new("GetGlossary", "glossaries/nosuchglossary".to_string()),
        Read::new("GetGlossaryTerm", "glossary-terms/nosuchterm".to_string()),
        Read::new("GetListing", "listings/nosuchlisting".to_string()),
    ]
}

/// Create a domain with one of every resource the reads address, through the same HTTP
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

    let project = dz.project(&domain, "scope owner").await;

    let pool = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/account-pools"),
            Some(json!({
                "name": "scope pool",
                "resolutionStrategy": "MANUAL",
                "accountSource": {
                    "accounts": [
                        { "awsAccountId": "111122223333", "supportedRegions": ["us-east-1"] }
                    ]
                },
            })),
        )
        .await;
    assert_eq!(
        pool.status, 201,
        "CreateAccountPool: expected 201, got {} {:?}",
        pool.status, pool.body
    );

    let asset_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(json!({
                "name": "ScopedType",
                "owningProjectIdentifier": project,
                "formsInput": {},
            })),
        )
        .await;
    assert_eq!(
        asset_type.status, 201,
        "CreateAssetType: expected 201, got {} {:?}",
        asset_type.status, asset_type.body
    );

    let schema = serde_json::to_string(&json!({
        "columns": [{ "columnName": "order_id", "dataType": "string" }],
    }))
    .expect("the form content serializes");
    let asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "scoped asset",
                "typeIdentifier": "ScopedType",
                "owningProjectIdentifier": project,
                "formsInput": [{
                    "formName": "RelationalTableForm",
                    "typeIdentifier": "amazon.datazone.RelationalTableFormType",
                    "content": schema,
                }],
            })),
        )
        .await;
    assert_eq!(
        asset.status, 201,
        "CreateAsset: expected 201, got {} {:?}",
        asset.status, asset.body
    );
    let asset_id = asset.s("id");

    let filter = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets/{asset_id}/filters"),
            Some(json!({
                "name": "scoped filter",
                "configuration": {
                    "columnConfiguration": { "includedColumnNames": ["order_id"] },
                },
            })),
        )
        .await;
    assert_eq!(
        filter.status, 201,
        "CreateAssetFilter: expected 201, got {} {:?}",
        filter.status, filter.body
    );

    let form_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/form-types"),
            Some(json!({
                "name": "ScopedForm",
                "model": { "smithy": "structure ScopedForm {\n  id: String\n}" },
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
            Some(json!({ "name": "scoped glossary", "owningProjectIdentifier": project })),
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
            Some(json!({ "glossaryIdentifier": glossary_id, "name": "scoped term" })),
        )
        .await;
    assert_eq!(
        term.status, 201,
        "CreateGlossaryTerm: expected 201, got {} {:?}",
        term.status, term.body
    );

    let published = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/listings/change-set"),
            Some(json!({
                "entityIdentifier": asset_id,
                "entityType": "ASSET",
                "action": "PUBLISH",
            })),
        )
        .await;
    assert_eq!(
        published.status, 200,
        "CreateListingChangeSet: expected 200, got {} {:?}",
        published.status, published.body
    );

    Served {
        domain,
        root_unit,
        project,
        account_pool: pool.s("id"),
        asset_type: asset_type.s("name"),
        asset: asset_id,
        asset_filter: filter.s("id"),
        form_type: form_type.s("name"),
        glossary: glossary_id,
        glossary_term: term.s("id"),
        listing: published.s("listingId"),
    }
}

#[tokio::test]
async fn a_read_scoped_to_a_domain_the_store_does_not_serve_is_denied() {
    let dz = Emulator::new();
    let fixture = served(&dz, "scoped-read-domain").await;

    for (domain_label, unserved) in unserved_domain_ids() {
        for read in fixture.reads() {
            // The resource identifiers are the ones the fixture really stores, so the
            // only thing wrong with the request is the domain it is scoped to.
            let case = format!("{} through {domain_label}", read.operation);
            let answer = dz.call("GET", &read.uri(&unserved), None).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected the modelled denial, got {} {:?}",
                answer.status,
                answer.body
            );
            let message = denial_message_member(&answer);
            assert_eq!(
                message,
                read.denial_message(),
                "case {case:?}: the denial must carry the recorded message, got {message:?}"
            );
            assert!(
                message.as_ref().is_none_or(|text| !text.contains(&unserved)),
                "case {case:?}: the denial must not echo the requested domain {unserved:?}, \
                 got {message:?}"
            );
            assert!(
                answer.body.get("id").is_none() && answer.body.get("domainId").is_none(),
                "case {case:?}: a denied read must report no resource, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_same_reads_answer_from_state_under_the_domain_that_holds_them() {
    let dz = Emulator::new();
    let fixture = served(&dz, "served-read-domain").await;

    // The control for the rule above: authorization refuses an UNSERVED scope and
    // nothing more, so every one of these reads still answers from the store.
    for read in fixture.reads() {
        let answer = dz.call("GET", &read.uri(&fixture.domain), None).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a read under its own domain must answer from state, got {} {:?}",
            read.operation, answer.status, answer.body
        );
        // Every one of these outputs reports the domain it came out of (`domainId`;
        // `GetDomain`'s own `id` IS that domain), so the answer is evidence that the
        // addressed domain served it.
        let reported = if read.suffix.is_empty() {
            answer.s("id")
        } else {
            answer.s("domainId")
        };
        assert_eq!(
            reported, fixture.domain,
            "case {:?}: the answer must come from the addressed domain, got {:?}",
            read.operation, answer.body
        );
    }
}

#[tokio::test]
async fn a_resource_a_served_domain_does_not_hold_is_still_a_not_found() {
    let dz = Emulator::new();
    let fixture = served(&dz, "existence-read-domain").await;

    // Inside a domain the caller may act in, existence decides the answer again: the
    // denial above has not swallowed the not-found the operations model.
    for read in unstored_resource_reads() {
        let answer = dz.call("GET", &read.uri(&fixture.domain), None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected the modelled not-found, got {} {:?}",
            read.operation,
            answer.status,
            answer.body
        );
        let message = answer.s("message");
        assert!(
            !message.contains("not permitted"),
            "case {:?}: a miss inside a served domain is not a denial, got {message:?}",
            read.operation
        );
    }
}

#[tokio::test]
async fn every_read_is_denied_once_the_domain_it_is_scoped_to_is_deleted() {
    let dz = Emulator::new();
    let fixture = served(&dz, "deleted-scope-domain").await;

    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{}", fixture.domain), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "DeleteDomain: expected the modelled success status, got {} {:?}",
        deleted.status, deleted.body
    );
    // Once the deletion the clock owed has completed, the identifier names no domain,
    // so nothing is held on it any more.
    tokio::time::sleep(SETTLED).await;

    for read in fixture.reads() {
        let answer = dz.call("GET", &read.uri(&fixture.domain), None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {:?}: a read scoped to a deleted domain must be denied, got {} {:?}",
            read.operation,
            answer.status,
            answer.body
        );
        assert_eq!(
            denial_message_member(&answer),
            read.denial_message(),
            "case {:?}: the denial must carry the recorded message, got {:?}",
            read.operation,
            answer.body
        );
    }
}
