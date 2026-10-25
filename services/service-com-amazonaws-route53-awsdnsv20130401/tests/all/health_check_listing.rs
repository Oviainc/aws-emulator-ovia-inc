// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListHealthChecks`.
//!
//! The rule under test: the operation REPORTS the health checks the account
//! holds at the moment of the call, read from the service's state in one stable
//! total order over their ids, as a page that starts at the requested `Marker`,
//! carries at most `MaxItems` of them, echoes both of those back and names in
//! `NextMarker` the first health check the next page starts at — so the pages
//! concatenate to the whole population exactly once, and nothing a caller reads
//! changes what a later caller sees.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! renaming a health check: the POPULATION (none, one, every modelled
//! `HealthCheckType`), the PAGE SIZE (every size from one to one past the
//! population, plus the documented maximum and one past it) and the MARKER
//! (every position in the listing). Each listed health check is cross-checked
//! against what `GetHealthCheck` reports for the same id, so the listing is
//! proved against another reader of the same state rather than against itself.
//!
//! Every test drives the service through the real router, using the same
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_checks::{
    child_for, create_health_check, get_health_check, health_check_count, health_check_id, CATALOG,
};
use crate::hosted_zone_records::{call, router, Reply};

/// The most health checks one response carries, from
/// `ListHealthChecksRequest$MaxItems`'s own documentation: "Amazon Route 53
/// returns a maximum of 1000 items. If you set MaxItems to a value greater than
/// 1000, Route 53 returns only the first 1000 health checks."
const DOCUMENTED_MAX_ITEMS: usize = 1_000;

/// Send one `ListHealthChecks` request, with `query` as its query string.
async fn list_health_checks(router: &MultiServiceRouter, query: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/healthcheck{query}"), "").await
}

/// The ids a listing reports, in the order it reports them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    reply
        .all("HealthCheck/Id")
        .iter()
        .map(|id| (*id).to_string())
        .collect()
}

/// List every health check in one call, asserting the call succeeded.
async fn listing(router: &MultiServiceRouter, case: &str) -> Reply {
    let reply = list_health_checks(router, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListHealthChecks, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// Create one health check per catalog row, plus the children the rows that
/// name child health checks need, and return every id created.
async fn populate(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for row in CATALOG {
        let child = child_for(router, row).await;
        if !child.is_empty() {
            ids.push(child.clone());
        }
        let created =
            create_health_check(router, &format!("{prefix}-{}", row.kind), &row.config(&child))
                .await;
        ids.push(health_check_id(&created, row.kind));
    }
    ids
}

#[tokio::test]
async fn the_listing_reports_every_health_check_the_account_holds() {
    // The population is the varying axis: none, then one more after every
    // catalog row, so the listing is asserted against a set that grows by a
    // health check of a different TYPE each time.
    let router = router();
    let mut created: Vec<String> = Vec::new();

    for row in CATALOG {
        let case = format!(
            "{} health check(s), before creating a {}",
            created.len(),
            row.kind
        );
        let reply = listing(&router, &case).await;
        let listed = listed_ids(&reply);

        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must report every health check the account holds, \
             in id order; body {}",
            reply.body
        );
        assert!(
            reply.first("IsTruncated") != Some("true"),
            "case {case:?}: a listing that fits one page must not be truncated; body {}",
            reply.body
        );
        assert!(
            reply.first("NextMarker").is_none(),
            "case {case:?}: an untruncated listing must carry no NextMarker; body {}",
            reply.body
        );

        // The same population, counted through another operation over the same
        // state.
        let counted = health_check_count(&router, &case).await;
        assert_eq!(
            counted,
            i64::try_from(listed.len()).expect("the population fits an i64"),
            "case {case:?}: the listing must hold as many health checks as the count reports"
        );

        // Every listed health check is cross-checked against what
        // `GetHealthCheck` reports for the same id.
        let references = reply.all("HealthCheck/CallerReference");
        let types = reply.all("HealthCheckConfig/Type");
        let versions = reply.all("HealthCheck/HealthCheckVersion");
        assert_eq!(
            (references.len(), types.len(), versions.len()),
            (listed.len(), listed.len(), listed.len()),
            "case {case:?}: every listed health check must be reported whole; body {}",
            reply.body
        );
        for (index, id) in listed.iter().enumerate() {
            let read = get_health_check(&router, id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the listed health check {id} must be readable, got {} with body {}",
                read.status, read.body
            );
            let actual = (references[index], types[index], versions[index]);
            let expected = (
                read.first("HealthCheck/CallerReference").unwrap_or(""),
                read.first("HealthCheckConfig/Type").unwrap_or(""),
                read.first("HealthCheck/HealthCheckVersion").unwrap_or(""),
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: the listing must report {id} as GetHealthCheck does, \
                 got {actual:?}; body {}",
                reply.body
            );
        }

        let child = child_for(&router, row).await;
        if !child.is_empty() {
            created.push(child.clone());
        }
        let reply = create_health_check(
            &router,
            &format!("listing-{}", row.kind),
            &row.config(&child),
        )
        .await;
        created.push(health_check_id(&reply, &case));
    }
}

#[tokio::test]
async fn health_check_paging_covers_the_listing_exactly_once() {
    // The rule as an invariant: a page holds at most MaxItems health checks and
    // NextMarker names where the next page starts, so the pages concatenate to
    // the full listing for EVERY page size. The page size is the varying axis.
    let router = router();
    let created = populate(&router, "paged").await;

    let unpaged = listing(&router, "the whole listing").await;
    let all = listed_ids(&unpaged);
    assert_eq!(
        all.len(),
        created.len(),
        "every created health check must be listed; body {}",
        unpaged.body
    );

    for page_size in 1..=created.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &marker {
                Some(resume) => format!("?maxitems={page_size}&marker={resume}"),
                None => format!("?maxitems={page_size}"),
            };
            let page = list_health_checks(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case maxitems={page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            assert_eq!(
                page.first("Marker"),
                marker.as_deref(),
                "case maxitems={page_size}: the response must echo the request's marker; body {}",
                page.body
            );
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string()).as_deref(),
                "case maxitems={page_size}: the response must echo the request's maxitems; body {}",
                page.body
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case maxitems={page_size}: a page held {} health checks; body {}",
                ids.len(),
                page.body
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case maxitems={page_size}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert!(
                    page.first("NextMarker").is_none(),
                    "case maxitems={page_size}: a final page must carry no NextMarker; body {}",
                    page.body
                );
                break;
            }
            marker = Some(
                page.first("NextMarker")
                    .expect("a truncated page must carry a NextMarker")
                    .to_string(),
            );
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn a_marker_resumes_the_listing_at_the_health_check_it_names() {
    // `Marker` is "the ID of the first health check that Amazon Route 53 will
    // return if you submit another request", so a listing resumed at any
    // position in it reports exactly the tail from that position. Every
    // position is a row, plus an id past the end of the listing.
    let router = router();
    populate(&router, "resumed").await;
    let all = listed_ids(&listing(&router, "the whole listing").await);
    assert!(
        all.len() > 1,
        "the suite needs several health checks to resume at, got {}",
        all.len()
    );

    for (index, id) in all.iter().enumerate() {
        let page = list_health_checks(&router, &format!("?marker={id}")).await;
        assert_eq!(
            page.status, 200,
            "case marker={id}: expected 200, got {} with body {}",
            page.status, page.body
        );
        let tail = listed_ids(&page);
        assert_eq!(
            tail,
            all[index..].to_vec(),
            "case marker={id}: the listing must resume at the health check the marker names; \
             body {}",
            page.body
        );
    }

    // A marker past every id reports an empty, untruncated page rather than
    // wrapping around to the start.
    let past_the_end = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    let page = list_health_checks(&router, &format!("?marker={past_the_end}")).await;
    let tail = listed_ids(&page);
    assert!(
        tail.is_empty() && page.first("IsTruncated") != Some("true"),
        "case marker={past_the_end}: expected an empty final page, got {tail:?}; body {}",
        page.body
    );
}

#[tokio::test]
async fn the_listing_echoes_the_page_size_it_was_asked_for() {
    // `MaxItems` in the response is "the value that you specified for the
    // maxitems parameter", so it is echoed whatever the caller sent — while the
    // page itself never exceeds the documented maximum. The rows are the
    // documented bound, one past it, and a size below the population.
    let router = router();
    let created = populate(&router, "sized").await;

    for requested in [1, 2, DOCUMENTED_MAX_ITEMS, DOCUMENTED_MAX_ITEMS + 1] {
        let page = list_health_checks(&router, &format!("?maxitems={requested}")).await;
        assert_eq!(
            page.status, 200,
            "case maxitems={requested}: expected 200, got {} with body {}",
            page.status, page.body
        );
        let echoed = page.first("MaxItems").unwrap_or("");
        assert_eq!(
            echoed,
            requested.to_string(),
            "case maxitems={requested}: expected the requested maxitems echoed, got {echoed:?}; \
             body {}",
            page.body
        );
        let ids = listed_ids(&page);
        let expected = requested.min(DOCUMENTED_MAX_ITEMS).min(created.len());
        assert_eq!(
            ids.len(),
            expected,
            "case maxitems={requested}: expected {expected} health check(s), got {}; body {}",
            ids.len(),
            page.body
        );
        let truncated = page.first("IsTruncated") == Some("true");
        assert_eq!(
            truncated,
            ids.len() < created.len(),
            "case maxitems={requested}: IsTruncated must say whether more remain; body {}",
            page.body
        );
    }

    // With no maxitems at all the whole population comes back in one page.
    let whole = listing(&router, "no maxitems").await;
    assert_eq!(
        listed_ids(&whole).len(),
        created.len(),
        "case {:?}: a request without maxitems must list every health check; body {}",
        "no maxitems",
        whole.body
    );
}

#[tokio::test]
async fn listing_the_health_checks_leaves_the_account_exactly_as_it_was() {
    // The rule is a report: repeating it answers the same listing, creates and
    // consumes nothing, and the population it reports still moves with a later
    // create.
    let router = router();
    let created = populate(&router, "reported").await;
    let first = listed_ids(&listing(&router, "the first listing").await);
    assert_eq!(
        first.len(),
        created.len(),
        "every created health check must be listed"
    );

    for attempt in 1..=3 {
        let case = format!("listing {attempt}");
        let again = listed_ids(&listing(&router, &case).await);
        assert_eq!(
            again, first,
            "case {case:?}: a read must report what the first read did"
        );
        let counted = health_check_count(&router, &case).await;
        assert_eq!(
            counted,
            i64::try_from(first.len()).expect("the population fits an i64"),
            "case {case:?}: listing must not change the population"
        );
    }

    for id in &first {
        let read = get_health_check(&router, id).await;
        assert_eq!(
            read.status, 200,
            "the listed health check {id} must still be readable, got {} with body {}",
            read.status, read.body
        );
    }

    let added = create_health_check(
        &router,
        "reported-after-the-listing",
        "<Type>HTTP</Type><IPAddress>192.0.2.99</IPAddress><Port>80</Port>",
    )
    .await;
    let added_id = health_check_id(&added, "a health check created after the listing");
    let after = listed_ids(&listing(&router, "after a later create").await);
    assert!(
        after.contains(&added_id) && after.len() == first.len() + 1,
        "case {:?}: the listing must follow the state, got {after:?}",
        "after a later create"
    );
}
