// SPDX-License-Identifier: CC0-1.0

//! `DeleteDistributionTenant` end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this operation
//! ("Deletes a distribution tenant. If you use this API operation to delete a distribution
//! tenant that is currently enabled, the request will fail. To delete a distribution tenant,
//! you must first disable the distribution tenant by using the UpdateDistributionTenant API
//! operation"):
//!
//! * a tenant that is still ENABLED is refused with `ResourceNotDisabled` (HTTP 409) and
//!   stays readable at the version it had — whatever version the request carries;
//! * the version token the last mutation handed back is the precondition: a value that is no
//!   version token at all is `InvalidIfMatchVersion` (HTTP 400) and a well-formed token that
//!   is not the tenant's is `PreconditionFailed` (HTTP 412), both before anything is removed;
//! * an id no stored tenant holds is `EntityNotFound` (HTTP 404) with the one sentence every
//!   read of an unknown tenant carries;
//! * a delete of a disabled tenant at its current version answers 204 with no body and
//!   REMOVES the record: the tenant is no longer readable by id, no longer resolved by any
//!   domain it claimed, its managed certificate is gone with it, and its name and domains
//!   are free for a later create to claim.
//!
//! The rows differ along what the rules generalise over: how many domains the tenant claims,
//! whether the create asked CloudFront for a managed certificate, and whether the create
//! stated `Enabled` or left it out. The wire harness is
//! [`crate::distribution_lifecycle`]'s and the tenant helpers
//! [`crate::distribution_tenant_lifecycle`]'s.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, encode_label, escape, managed_certificate_request,
    multi_tenant_distribution,
};
use crate::managed_certificate_details::get_managed_certificate_details;
use crate::response_headers_policy_lifecycle::text;

/// The sentence every read of a distribution tenant nothing holds answers with.
const TENANT_NOT_FOUND_MESSAGE: &str = "The specified distribution tenant doesn't exist.";

// ---------------------------------------------------------------------------
// Calling the operations
// ---------------------------------------------------------------------------

/// `DELETE /2020-05-31/distribution-tenant/{Id}` with the `If-Match` header `if_match`
/// (left off entirely when `None`). The operation binds no payload.
async fn delete_tenant(router: &MultiServiceRouter, id: &str, if_match: Option<&str>) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/{API_VERSION}/distribution-tenant/{}", encode_label(id)),
        None,
        if_match,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant/{Identifier}`.
async fn get_tenant(router: &MultiServiceRouter, identifier: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distribution-tenant/{}",
            encode_label(identifier)
        ),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/distribution-tenant?domain={Domain}`.
async fn get_by_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/distribution-tenant?domain={}",
            encode_label(domain)
        ),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/distribution-tenant/{Id}` stating `Enabled` alone — the call the
/// documentation points a caller at before a delete.
async fn set_enabled(
    router: &MultiServiceRouter,
    id: &str,
    enabled: bool,
    if_match: &str,
) -> Answer {
    let body = format!(
        "<UpdateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Enabled>{enabled}</Enabled></UpdateDistributionTenantRequest>"
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution-tenant/{id}"),
        Some(&body),
        Some(if_match),
    )
    .await
}

// ---------------------------------------------------------------------------
// The tenants under test
// ---------------------------------------------------------------------------

/// One row: a distribution tenant create.
struct Case {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, unique across the table.
    name: String,
    /// The domains the create claims.
    domains: Vec<String>,
    /// Whether the create asks CloudFront for a managed certificate over those domains.
    certificate: bool,
    /// `Enabled`, or `None` to leave the optional member out (the create then enables it).
    enabled: Option<bool>,
}

impl Case {
    /// The `CreateDistributionTenantRequest` XML a client sends for this row.
    fn xml(&self, distribution_id: &str) -> String {
        let mut out = format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{}</DistributionId><Name>{}</Name>",
            escape(distribution_id),
            escape(&self.name)
        );
        out.push_str("<Domains>");
        for domain in &self.domains {
            out.push_str(&format!(
                "<member><Domain>{}</Domain></member>",
                escape(domain)
            ));
        }
        out.push_str("</Domains>");
        // The multi-tenant distribution below declares one REQUIRED parameter with no
        // default, so every create has to carry a value for it.
        out.push_str(&format!(
            "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
            escape(&self.name)
        ));
        if let Some(enabled) = self.enabled {
            out.push_str(&format!("<Enabled>{enabled}</Enabled>"));
        }
        // Every row claims a name in a customer's zone, so the create asks CloudFront to
        // validate it — which is also what mints the managed certificate a delete has to
        // take with the record.
        if self.certificate {
            out.push_str(&managed_certificate_request());
        }
        out.push_str("</CreateDistributionTenantRequest>");
        out
    }
}

/// The creates every rule below is asserted over: one domain and several, a create that asks
/// for a managed certificate and one that does not, and `Enabled` stated true, stated false
/// and left out.
fn case_table() -> Vec<Case> {
    vec![
        Case {
            case: "one domain, a managed certificate, Enabled left out",
            name: "tenant-delete-one".to_string(),
            domains: vec!["delete-one.example.com".to_string()],
            certificate: true,
            enabled: None,
        },
        Case {
            case: "two domains, a managed certificate, Enabled stated true",
            name: "tenant-delete-two".to_string(),
            domains: vec![
                "delete-two-a.example.com".to_string(),
                "delete-two-b.example.com".to_string(),
            ],
            certificate: true,
            enabled: Some(true),
        },
        Case {
            case: "one domain, a managed certificate, created disabled",
            name: "tenant-delete-disabled".to_string(),
            domains: vec!["delete-disabled.example.com".to_string()],
            certificate: true,
            enabled: Some(false),
        },
    ]
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A tenant is deleted only once it is disabled and only at its current version: an enabled
/// tenant is `ResourceNotDisabled`, an unusable or stale `If-Match` is refused on the
/// version, and the accepted delete removes the record — after which the tenant is unknown
/// by id and by domain, its certificate is gone, and its name and domains are free again.
#[tokio::test]
async fn a_disabled_tenant_is_deleted_at_its_current_version() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-delete").await;

    for row in case_table() {
        let case = row.case;
        let created = create_distribution_tenant(&router, &row.xml(&distribution_id)).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201 from the create, got {} with {}",
            created.status, created.raw
        );
        let id = text(created.root(), "Id", case, "the create", &created.raw);
        let mut etag = created.header("etag").to_string();

        // An enabled tenant is refused whatever version the request carries: no version
        // makes the delete legal while the tenant still serves traffic.
        if row.enabled != Some(false) {
            for (description, if_match) in [
                ("the tenant's own version token", etag.clone()),
                ("a well-formed token that is not the tenant's", "E1STALEVERSION".to_string()),
            ] {
                let refused = delete_tenant(&router, &id, Some(&if_match)).await;
                if description == "the tenant's own version token" {
                    assert_eq!(
                        refused.status, 409,
                        "case {case} with {description}: expected HTTP 409 while enabled, got {} \
                         with {}",
                        refused.status, refused.raw
                    );
                    assert_eq!(
                        refused.error_code(),
                        "ResourceNotDisabled",
                        "case {case} with {description}: expected ResourceNotDisabled, got {}",
                        refused.raw
                    );
                    assert_eq!(
                        refused.root().find_text("Message").unwrap_or_default(),
                        DISTRIBUTION_TENANT_NOT_DISABLED_MESSAGE,
                        "case {case} with {description}: expected the modelled not-disabled \
                         sentence, got {}",
                        refused.raw
                    );
                } else {
                    // The precondition is evaluated before the tenant's own state, so a
                    // stale version is answered on the version.
                    assert_eq!(
                        refused.status, 412,
                        "case {case} with {description}: expected HTTP 412, got {} with {}",
                        refused.status, refused.raw
                    );
                    assert_eq!(
                        refused.error_code(),
                        "PreconditionFailed",
                        "case {case} with {description}: expected PreconditionFailed, got {}",
                        refused.raw
                    );
                }
                assert_eq!(
                    get_tenant(&router, &id).await.status,
                    200,
                    "case {case} with {description}: expected the refused delete to leave the \
                     tenant readable"
                );
            }

            // Disabling it is the call the documentation points the caller at.
            let disabled = set_enabled(&router, &id, false, &etag).await;
            assert_eq!(
                disabled.status, 200,
                "case {case}: expected HTTP 200 disabling the tenant, got {} with {}",
                disabled.status, disabled.raw
            );
            etag = disabled.header("etag").to_string();
        }

        // A version the tenant cannot satisfy is still refused once it is disabled.
        for (description, if_match, status, code) in [
            (
                "an If-Match that is no version token",
                "not-a-version".to_string(),
                400,
                "InvalidIfMatchVersion",
            ),
            (
                "a well-formed token that is not the tenant's",
                "E1STALEVERSION".to_string(),
                412,
                "PreconditionFailed",
            ),
        ] {
            let refused = delete_tenant(&router, &id, Some(&if_match)).await;
            assert_eq!(
                refused.status, status,
                "case {case} with {description}: expected HTTP {status}, got {} with {}",
                refused.status, refused.raw
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case} with {description}: expected {code}, got {}",
                refused.raw
            );
            assert_eq!(
                get_tenant(&router, &id).await.status,
                200,
                "case {case} with {description}: expected the refused delete to leave the tenant \
                 readable"
            );
        }

        // The accepted delete: 204, no body, and the record is gone.
        let deleted = delete_tenant(&router, &id, Some(&etag)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case}: expected HTTP 204 from the delete, got {} with {}",
            deleted.status, deleted.raw
        );
        assert!(
            deleted.raw.trim().is_empty(),
            "case {case}: expected no body from the delete, got {}",
            deleted.raw
        );

        for (reader, answer) in [
            ("by id", get_tenant(&router, &id).await),
            ("by name", get_tenant(&router, &row.name).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case} ({reader}): expected the deleted tenant to be gone, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "EntityNotFound",
                "case {case} ({reader}): expected EntityNotFound, got {}",
                answer.raw
            );
            assert_eq!(
                answer.root().find_text("Message").unwrap_or_default(),
                TENANT_NOT_FOUND_MESSAGE,
                "case {case} ({reader}): expected the unknown-tenant sentence, got {}",
                answer.raw
            );
        }

        // No domain resolves to it any more, and the certificate held on its record went
        // with the record.
        for domain in &row.domains {
            let by_domain = get_by_domain(&router, domain).await;
            assert_eq!(
                by_domain.status, 404,
                "case {case} ({domain}): expected the domain to resolve to nothing, got {} with \
                 {}",
                by_domain.status, by_domain.raw
            );
        }
        let certificate = get_managed_certificate_details(&router, &row.name).await;
        assert_eq!(
            certificate.status, 404,
            "case {case}: expected the certificate to go with the tenant, got {} with {}",
            certificate.status, certificate.raw
        );

        // A second delete of the same id is the modelled 404 rather than another 204.
        let again = delete_tenant(&router, &id, Some(&etag)).await;
        assert_eq!(
            again.status, 404,
            "case {case}: expected a second delete to report 404, got {} with {}",
            again.status, again.raw
        );

        // The name and the domains are free: the identical create is accepted again, under a
        // fresh id.
        let recreated = create_distribution_tenant(&router, &row.xml(&distribution_id)).await;
        assert_eq!(
            recreated.status, 201,
            "case {case}: expected the identical create to be accepted after the delete, got {} \
             with {}",
            recreated.status, recreated.raw
        );
        let fresh_id = text(recreated.root(), "Id", case, "the re-create", &recreated.raw);
        assert_ne!(
            fresh_id, id,
            "case {case}: expected the re-create to mint a fresh id, got {fresh_id} again"
        );
    }
}

/// An id no stored tenant holds is `EntityNotFound` (HTTP 404) whatever shape the id takes,
/// and the tenant that does exist is untouched.
#[tokio::test]
async fn an_id_no_tenant_holds_is_refused() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-delete-unknown").await;
    let present = Case {
        case: "the one tenant that exists",
        name: "tenant-delete-present".to_string(),
        domains: vec!["delete-present.example.com".to_string()],
        certificate: true,
        enabled: Some(false),
    };
    let created = create_distribution_tenant(&router, &present.xml(&distribution_id)).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201 from the create, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = text(
        created.root(),
        "Id",
        present.case,
        "the create",
        &created.raw,
    );
    let etag = created.header("etag").to_string();

    let mut flipped: Vec<char> = id.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'A' { 'B' } else { 'A' };
    let rows: [(&str, String); 4] = [
        ("an id never minted", "dt_NEVERMINTED1".to_string()),
        (
            "the existing id with one character changed",
            flipped.into_iter().collect(),
        ),
        // `Id` is "the ID of the distribution tenant to delete", so neither the name nor the
        // ARN the reads also accept resolves one here.
        ("the tenant's name", present.name.clone()),
        ("a value of no id's shape", "not-a-tenant".to_string()),
    ];

    for (case, candidate) in &rows {
        let answer = delete_tenant(&router, candidate, Some(&etag)).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404 for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {candidate:?}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            TENANT_NOT_FOUND_MESSAGE,
            "case {case}: expected the unknown-tenant sentence, got {}",
            answer.raw
        );
    }

    // The tenant that does exist survived every refusal, at the version it was created with.
    let fetched = get_tenant(&router, &id).await;
    assert_eq!(
        fetched.status, 200,
        "case after the refusals: expected the tenant to be readable, got {} with {}",
        fetched.status, fetched.raw
    );
    assert_eq!(
        fetched.header("etag"),
        etag,
        "case after the refusals: expected the stored version token {etag}, got {:?}",
        fetched.header("etag")
    );
}
