// SPDX-License-Identifier: CC0-1.0

//! How far ONE `DeleteDistributionTenant` reaches, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. [`crate::distribution_tenant_delete`] asserts what the operation answers;
//! this suite asserts WHAT IT REMOVES: a delete removes exactly the one tenant record its
//! `Id` names, so everything that record held stops being reachable — the tenant itself,
//! the invalidations created under it, the web ACL customization carried on it and its place
//! in the account's tenant listing — while every other tenant, siblings of the same
//! multi-tenant distribution included, keeps the record and the version token it had.
//!
//! Two consequences each row also pins down:
//!
//! * a resource addressed UNDER the removed tenant is answered with the `EntityNotFound` the
//!   operation models for the tenant, not served from a record nothing holds any more;
//! * a later create under the freed name and domains gets a FRESH record, inheriting neither
//!   the invalidations nor the customizations of the one that was deleted.
//!
//! The rows differ along what the rule generalises over: how many invalidations the deleted
//! tenant holds (none, one, two, three), how many domains it claims, and whether its web ACL
//! customization was stated by the create, written by a later
//! `AssociateDistributionTenantWebACL` call, or absent.
//!
//! The wire harness is [`crate::distribution_lifecycle`]'s; the tenant, invalidation, web
//! ACL, listing and delete calls go through the sibling suites' helpers rather than through
//! copies of them.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    CUSTOMIZATION_ACTION_OVERRIDE, MAX_ITEMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{batch_xml, emulator, API_VERSION};
use crate::distribution_tenant_delete::{delete_tenant, get_tenant, set_enabled};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::distribution_tenants_list::{list_body, list_tenants, summary_ids};
use crate::tenant_invalidations::{
    create_invalidation_for_tenant, get_invalidation_for_tenant, list_invalidations_for_tenant,
    summaries,
};
use crate::tenant_web_acl_association::{associate, read_tenant, web_acl_of};
use crate::web_acl_association::WEB_ACLS;

// ---------------------------------------------------------------------------
// The requests
// ---------------------------------------------------------------------------

/// A `Customizations` element stating a web ACL override, as a create may carry one. The
/// action is the model's own `override`
/// (`smithy_query(query="shape",shape="CustomizationActionType")`), read off the service
/// rather than written out here.
fn web_acl_customization(arn: &str) -> String {
    format!(
        "<Customizations><WebAcl><Action>{CUSTOMIZATION_ACTION_OVERRIDE}</Action>\
         <Arn>{}</Arn></WebAcl></Customizations>",
        escape(arn)
    )
}

/// The `CreateDistributionTenantRequest` document for a tenant named `name`, claiming
/// `domains`, carrying `customizations` verbatim (empty for none).
///
/// `tenantName` is the parameter the multi-tenant distribution under test marks required, so
/// every create supplies a value for it, and every row claims names in a customer's zone, so
/// every create asks CloudFront to validate them.
fn create_body(
    distribution_id: &str,
    name: &str,
    domains: &[String],
    customizations: &str,
) -> String {
    let mut out = format!(
        "<CreateDistributionTenantRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionId>{}</DistributionId><Name>{}</Name>",
        escape(distribution_id),
        escape(name)
    );
    out.push_str("<Domains>");
    for domain in domains {
        out.push_str(&format!(
            "<member><Domain>{}</Domain></member>",
            escape(domain)
        ));
    }
    out.push_str("</Domains>");
    out.push_str(&format!(
        "<Parameters><member><Name>tenantName</Name><Value>{}</Value></member></Parameters>",
        escape(name)
    ));
    out.push_str(customizations);
    out.push_str(&managed_certificate_request());
    out.push_str("</CreateDistributionTenantRequest>");
    out
}

// ---------------------------------------------------------------------------
// The tenants under test
// ---------------------------------------------------------------------------

/// Where a tenant's web ACL customization comes from, as an index into [`WEB_ACLS`].
#[derive(Debug, Clone, Copy)]
enum WebAcl {
    /// The record carries no web ACL customization at all.
    Absent,
    /// The create states the customization.
    StatedByTheCreate(usize),
    /// A later `AssociateDistributionTenantWebACL` call writes it.
    WrittenByACall(usize),
}

/// One row: a tenant staged with invalidations and a web ACL, then deleted.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The tenant's `Name`, which also seeds its domains; unique across the table.
    slug: &'static str,
    /// How many domains the create claims.
    domains: usize,
    /// The invalidation batches created under the tenant before the delete.
    batches: &'static [&'static [&'static str]],
    /// Where the tenant's web ACL customization comes from.
    web_acl: WebAcl,
}

impl Row {
    /// The domains this row's create claims, one per `domains`.
    fn domains(&self) -> Vec<String> {
        (0..self.domains)
            .map(|index| format!("{}-{index}.example.com", self.slug))
            .collect()
    }
}

/// The tenants every delete below names: none to a few invalidations, one domain and
/// several, and a web ACL customization stated by the create, written by a call, or absent.
const ROWS: &[Row] = &[
    Row {
        case: "no invalidations, one domain, a web ACL written by a call",
        slug: "scope-none",
        domains: 1,
        batches: &[],
        web_acl: WebAcl::WrittenByACall(0),
    },
    Row {
        case: "one invalidation, one domain, a web ACL stated by the create",
        slug: "scope-one",
        domains: 1,
        batches: &[&["/index.html"]],
        web_acl: WebAcl::StatedByTheCreate(1),
    },
    Row {
        case: "three invalidations, two domains, a web ACL written by a call",
        slug: "scope-three",
        domains: 2,
        batches: &[&["/*"], &["/a.css", "/b.css"], &["/images/*"]],
        web_acl: WebAcl::WrittenByACall(3),
    },
    Row {
        case: "two invalidations, two domains, no web ACL at all",
        slug: "scope-plain",
        domains: 2,
        batches: &[&["/one"], &["/two", "/three"]],
        web_acl: WebAcl::Absent,
    },
];

/// The tenant no delete below names: a sibling of the same distribution, holding
/// invalidations and a web ACL customization of its own.
const KEEPER: Row = Row {
    case: "the untouched tenant",
    slug: "scope-keeper",
    domains: 1,
    batches: &[&["/keep/*"], &["/keep/one.html", "/keep/two.html"]],
    web_acl: WebAcl::WrittenByACall(2),
};

/// A tenant as it stands once its invalidations and its web ACL are in place.
#[derive(Debug)]
struct Staged {
    /// The id the create minted.
    id: String,
    /// The version token the last mutation on the record handed back.
    etag: String,
    /// The invalidations held under the tenant, in creation order.
    invalidations: Vec<String>,
    /// The web ACL ARN the record's customization names, when it carries one.
    web_acl: Option<&'static str>,
}

/// Create one tenant for `row` under `distribution_id`, with its invalidations and its web
/// ACL customization, and report the record as it now stands.
async fn stage(router: &MultiServiceRouter, distribution_id: &str, row: &Row) -> Staged {
    let case = row.case;
    let domains = row.domains();
    let customizations = match row.web_acl {
        WebAcl::StatedByTheCreate(index) => web_acl_customization(WEB_ACLS[index].1),
        _ => String::new(),
    };
    let created = create_distribution_tenant(
        router,
        &create_body(distribution_id, row.slug, &domains, &customizations),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case}: expected HTTP 201 from the create, got {} with {}",
        created.status, created.raw
    );
    let id = created
        .root()
        .text_at("Id")
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case}: expected the create to report an Id, got {}",
        created.raw
    );
    let mut etag = created.header("etag").to_string();

    let mut invalidations: Vec<String> = Vec::new();
    for (index, paths) in row.batches.iter().enumerate() {
        let answer = create_invalidation_for_tenant(
            router,
            &id,
            &batch_xml(&format!("{}-batch-{index}", row.slug), paths),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201 creating invalidation {index}, got {} with {}",
            answer.status, answer.raw
        );
        let invalidation = answer
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert!(
            !invalidation.is_empty(),
            "case {case}: expected invalidation {index} to report an Id, got {}",
            answer.raw
        );
        invalidations.push(invalidation);
    }

    let web_acl = match row.web_acl {
        WebAcl::Absent => None,
        WebAcl::StatedByTheCreate(index) => Some(WEB_ACLS[index].1),
        WebAcl::WrittenByACall(index) => {
            let arn = WEB_ACLS[index].1;
            let associated = associate(router, &id, arn, Some(&etag)).await;
            assert_eq!(
                associated.status, 200,
                "case {case}: expected HTTP 200 associating {arn}, got {} with {}",
                associated.status, associated.raw
            );
            etag = associated.header("etag").to_string();
            Some(arn)
        }
    };

    Staged {
        id,
        etag,
        invalidations,
        web_acl,
    }
}

/// The ids the tenant's invalidation listing reports, asserted served.
async fn listed_invalidations(
    router: &MultiServiceRouter,
    tenant_id: &str,
    case: &str,
) -> BTreeSet<String> {
    let answer = list_invalidations_for_tenant(router, tenant_id, "").await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200 listing the invalidations of {tenant_id}, got {} with {}",
        answer.status, answer.raw
    );
    summaries(answer.root())
        .iter()
        .map(|summary| summary.text_at("Id").unwrap_or_default().to_string())
        .collect()
}

/// Assert the staged tenant still answers from the record it was staged with: the read, the
/// version token, the web ACL customization and every invalidation held under it.
async fn assert_intact(router: &MultiServiceRouter, staged: &Staged, case: &str) {
    let read = read_tenant(router, &staged.id, case).await;
    assert_eq!(
        read.header("etag"),
        staged.etag,
        "case {case}: expected the untouched tenant to stay at version {}, got {:?}",
        staged.etag,
        read.header("etag")
    );
    let reported = web_acl_of(read.root());
    let expected = staged
        .web_acl
        .map(|arn| (CUSTOMIZATION_ACTION_OVERRIDE.to_string(), arn.to_string()));
    assert_eq!(
        reported, expected,
        "case {case}: expected the untouched tenant to report web ACL customization \
         {expected:?}, got {reported:?}"
    );
    let listed = listed_invalidations(router, &staged.id, case).await;
    let held: BTreeSet<String> = staged.invalidations.iter().cloned().collect();
    assert_eq!(
        listed, held,
        "case {case}: expected the untouched tenant to still hold {held:?}, got {listed:?}"
    );
    for invalidation in &staged.invalidations {
        let answer = get_invalidation_for_tenant(router, &staged.id, invalidation).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected {invalidation} to stay readable, got {} with {}",
            answer.status, answer.raw
        );
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A delete removes exactly the record its id names: everything addressed under that tenant
/// is afterwards `EntityNotFound`, a create under the freed name gets a record holding
/// nothing of the removed one, and the sibling tenant is readable at the version, the web ACL
/// customization and the invalidations it had before every one of these deletes.
#[tokio::test]
async fn a_delete_removes_one_record_and_leaves_every_other_tenant_whole() {
    let router = emulator();
    let distribution_id = multi_tenant_distribution(&router, "ref-tenant-delete-scope").await;
    let keeper = stage(&router, &distribution_id, &KEEPER).await;
    assert_intact(&router, &keeper, "the untouched tenant before any delete").await;

    for row in ROWS {
        let case = row.case;
        let staged = stage(&router, &distribution_id, row).await;
        let domains = row.domains();

        // Disabling the tenant is the call the documentation points the caller at before a
        // delete, and it hands back the version the delete then runs under.
        let disabled = set_enabled(&router, &staged.id, false, &staged.etag).await;
        assert_eq!(
            disabled.status, 200,
            "case {case}: expected HTTP 200 disabling the tenant, got {} with {}",
            disabled.status, disabled.raw
        );
        let etag = disabled.header("etag").to_string();

        let deleted = delete_tenant(&router, &staged.id, Some(&etag)).await;
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

        // The record the reads answered from is gone, so is every resource addressed under
        // it, and so is the web ACL surface that wrote onto it.
        let gone = get_tenant(&router, &staged.id).await;
        assert_eq!(
            gone.status, 404,
            "case {case}: expected the deleted tenant to be unknown, got {} with {}",
            gone.status, gone.raw
        );
        assert_eq!(
            gone.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound reading the deleted tenant, got {}",
            gone.raw
        );
        let listing = list_invalidations_for_tenant(&router, &staged.id, "").await;
        assert_eq!(
            listing.status, 404,
            "case {case}: expected the invalidation listing of a tenant nothing holds to be \
             refused, got {} with {}",
            listing.status, listing.raw
        );
        assert_eq!(
            listing.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound from the invalidation listing, got {}",
            listing.raw
        );
        for invalidation in &staged.invalidations {
            let answer = get_invalidation_for_tenant(&router, &staged.id, invalidation).await;
            assert_eq!(
                answer.status, 404,
                "case {case}: expected {invalidation} to go with the tenant, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                "EntityNotFound",
                "case {case}: expected EntityNotFound reading {invalidation}, got {}",
                answer.raw
            );
        }
        let reassociated = associate(&router, &staged.id, WEB_ACLS[0].1, Some(&etag)).await;
        assert_eq!(
            reassociated.status, 404,
            "case {case}: expected the association to find no tenant, got {} with {}",
            reassociated.status, reassociated.raw
        );
        assert_eq!(
            reassociated.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound from the association, got {}",
            reassociated.raw
        );

        // The account's tenant listing reports the records that are there: not this one any
        // more, and the sibling still. The request names the page ceiling, so one page
        // carries every tenant staged here whatever the service's default page size is.
        let listed = summary_ids(
            &list_tenants(
                &router,
                &list_body(Some((Some(&distribution_id), None)), None, Some(MAX_ITEMS)),
            )
            .await,
        );
        assert!(
            !listed.contains(&staged.id),
            "case {case}: expected the deleted tenant {} to leave the listing, got {listed:?}",
            staged.id
        );
        assert!(
            listed.contains(&keeper.id),
            "case {case}: expected the untouched tenant {} to stay in the listing, got {listed:?}",
            keeper.id
        );

        // The name and the domains are free again, and the record the create mints holds
        // nothing the deleted one held.
        let recreated = create_distribution_tenant(
            &router,
            &create_body(&distribution_id, row.slug, &domains, ""),
        )
        .await;
        assert_eq!(
            recreated.status, 201,
            "case {case}: expected the create under the freed name to be accepted, got {} with {}",
            recreated.status, recreated.raw
        );
        let fresh_id = recreated
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            fresh_id, staged.id,
            "case {case}: expected the create to mint a fresh id, got {fresh_id} again"
        );
        let fresh = read_tenant(&router, &fresh_id, case).await;
        assert_eq!(
            web_acl_of(fresh.root()),
            None,
            "case {case}: expected the fresh record to carry no web ACL customization, got {:?}",
            web_acl_of(fresh.root())
        );
        let inherited = listed_invalidations(&router, &fresh_id, case).await;
        assert!(
            inherited.is_empty(),
            "case {case}: expected the fresh record to hold no invalidations, got {inherited:?}"
        );

        // Every delete above named one record, so the sibling is still the record it was.
        assert_intact(
            &router,
            &keeper,
            &format!("the untouched tenant after the delete of {case}"),
        )
        .await;
    }
}
