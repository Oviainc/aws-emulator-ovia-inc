// SPDX-License-Identifier: CC0-1.0

//! How far ONE `DisassociateDistributionTenantWebACL` call reaches, end to end over HTTP.
//!
//! The rule under test: a disassociation is a configuration change to the ONE record the
//! request names — `Customizations$WebAcl` of the distribution tenant `Id` resolves — so it
//! takes that member off that tenant and versions it, and every OTHER record the account
//! holds is left exactly as it was: a sibling tenant of the same multi-tenant distribution
//! keeps its own web ACL customization at its own version, and the parent distribution keeps
//! its own `WebACLId` at its own version.
//!
//! That scope is what makes the per-tenant customization meaningful: the tenant member
//! "override[s] or disable[s]" the distribution's web ACL, so removing the override has to
//! leave the distribution's own web ACL in place for the tenant to fall back to, and has to
//! say nothing about any other tenant of that distribution.
//!
//! The rows differ along what the rule generalises over: the FORM of the web ACL the parent
//! distribution itself names (EVERY entry of [`crate::web_acl_association::WEB_ACLS`]),
//! WHICH tenant of the set the call names (each position of
//! [`crate::tenant_web_acl_association::STARTS`] in turn), and what each tenant carries while
//! it is a bystander (each entry of that same table — no customizations at all,
//! customizations that are not about a web ACL, a web ACL overridden, a web ACL disabled).
//!
//! The wire harness, the tenant creation helpers and the readers are the ones
//! [`crate::distribution_lifecycle`], [`crate::distribution_tenant_lifecycle`],
//! [`crate::tenant_web_acl_association`] and [`crate::web_acl_association`] already build,
//! so these assertions travel the same path a client takes.

use crate::distribution_lifecycle::emulator;
use crate::distribution_tenant_lifecycle::multi_tenant_distribution;
use crate::tenant_web_acl_association::{
    create_tenant, disassociate, read_tenant, web_acl_of, STARTS,
};
use crate::web_acl_association::{associate_web_acl, read_back, WEB_ACLS};

/// One bystander tenant as a read reports it before the call under test: its id, the version
/// it stands at, and the `(Action, Arn)` of its web ACL customization (`None` when it
/// carries none).
#[derive(Debug, Clone)]
struct Snapshot {
    case: &'static str,
    id: String,
    etag: String,
    web_acl: Option<(String, String)>,
}

/// Disassociating the web ACL of ONE tenant reaches that tenant only: it loses the
/// customization and stands at a version the answer and a later read agree on, while every
/// sibling tenant and the parent distribution are untouched — same web ACL, same version.
#[tokio::test]
async fn a_disassociation_reaches_only_the_named_tenant() {
    // One row per web ACL form the parent distribution can name, each naming a different
    // position of the tenant set, so both the forms and the positions are spanned.
    for (index, (acl_case, distribution_web_acl)) in WEB_ACLS.iter().enumerate() {
        let target = index % STARTS.len();
        let case = format!("{acl_case}/{}", STARTS[target].case);
        let case = case.as_str();
        // A fresh emulator per row, so what the row observes is what the row caused.
        let router = emulator();
        let distribution_id =
            multi_tenant_distribution(&router, &format!("ref-disassoc-scope-{index}")).await;

        // The parent distribution names a web ACL of its own, which is what a tenant's
        // customization overrides — and what a disassociation of that customization must
        // leave in place.
        let distribution_web_acl = *distribution_web_acl;
        let associated =
            associate_web_acl(&router, &distribution_id, distribution_web_acl, None).await;
        assert_eq!(
            associated.status, 200,
            "case {case}: expected HTTP 200 from AssociateDistributionWebACL, got {} with {}",
            associated.status, associated.raw
        );
        let (distribution_acl_before, distribution_etag_before, _) =
            read_back(&router, &distribution_id, case).await;
        assert_eq!(
            distribution_acl_before, distribution_web_acl,
            "case {case}: expected the distribution to name web ACL {distribution_web_acl:?} \
             before the disassociation, got {distribution_acl_before:?}"
        );

        // One tenant per starting state, so whichever position the call names, the bystanders
        // span the whole set of states a tenant can be in.
        let mut tenants: Vec<Snapshot> = Vec::with_capacity(STARTS.len());
        for (position, row) in STARTS.iter().enumerate() {
            let slug = format!("sc{index}{position}");
            // The label names the tenant being created, so a failure inside the create or the
            // read names the starting state it was for.
            let creating = format!("{case}/creating-{}", row.case);
            let (id, _) = create_tenant(&router, &distribution_id, &slug, &creating, *row).await;
            let read = read_tenant(&router, &id, &creating).await;
            tenants.push(Snapshot {
                case: row.case,
                id,
                etag: read.header("etag").to_string(),
                web_acl: web_acl_of(read.root()),
            });
        }

        // The call under test: the tenant at `target`, at the version it stands at.
        let named = tenants[target].clone();
        let answer = disassociate(&router, &named.id, Some(&named.etag)).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from DisassociateDistributionTenantWebACL, got {} \
             with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.root().text_at("Id"),
            Some(named.id.as_str()),
            "case {case}: expected the answer to name tenant {:?}, got {:?}",
            named.id,
            answer.root().text_at("Id")
        );
        let answered_etag = answer.header("etag").to_string();

        // The named tenant answers from the record the call wrote, at the version the answer
        // reported — what that record holds beyond the web ACL member is
        // [`crate::tenant_web_acl_association`]'s subject, not this suite's.
        let read = read_tenant(&router, &named.id, case).await;
        assert_eq!(
            web_acl_of(read.root()),
            None,
            "case {case}: expected the named tenant to report no web ACL customization, got \
             {:?} in {}",
            web_acl_of(read.root()),
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            answered_etag,
            "case {case}: expected the named tenant to read at version {answered_etag:?}, got \
             {:?}",
            read.header("etag")
        );

        // Every bystander tenant is exactly as it was: same customization, same version.
        for (position, sibling) in tenants.iter().enumerate() {
            if position == target {
                continue;
            }
            let read = read_tenant(&router, &sibling.id, case).await;
            assert_eq!(
                web_acl_of(read.root()),
                sibling.web_acl,
                "case {case}: expected sibling tenant {} ({}) to keep web ACL customization \
                 {:?}, got {:?}",
                position,
                sibling.case,
                sibling.web_acl,
                web_acl_of(read.root())
            );
            assert_eq!(
                read.header("etag"),
                sibling.etag,
                "case {case}: expected sibling tenant {} ({}) to stay at version {:?}, got {:?}",
                position,
                sibling.case,
                sibling.etag,
                read.header("etag")
            );
        }

        // The parent distribution's own web ACL is another record's member, so the tenant's
        // disassociation says nothing about it.
        let (distribution_acl_after, distribution_etag_after, _) =
            read_back(&router, &distribution_id, case).await;
        assert_eq!(
            distribution_acl_after, distribution_web_acl,
            "case {case}: expected the distribution to keep web ACL {distribution_web_acl:?}, \
             got {distribution_acl_after:?}"
        );
        assert_eq!(
            distribution_etag_after, distribution_etag_before,
            "case {case}: expected the distribution to stay at version \
             {distribution_etag_before:?}, got {distribution_etag_after:?}"
        );
    }
}
