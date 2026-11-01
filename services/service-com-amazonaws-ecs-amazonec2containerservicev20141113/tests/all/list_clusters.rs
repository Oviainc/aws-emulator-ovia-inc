// SPDX-License-Identifier: CC0-1.0

//! `ListClusters`, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rule under test: `ListClusters` answers with the ARNs of the clusters the account
//! has STORED and not deleted, in one total order, cut into pages of `maxResults` ARNs
//! whose `nextToken` resumes the walk where the page stopped — and it changes nothing a
//! later caller sees.
//!
//! The docs the rows are read off:
//!
//! * `ListClusters`: "Returns a list of existing clusters."
//! * `Cluster$status`: `INACTIVE` is "The cluster has been deleted. Clusters with an
//!   INACTIVE status may remain discoverable in your account for a period of time" — so a
//!   deleted cluster is still describable but is no longer an EXISTING cluster.
//! * `ListClusters$maxResults`: "This value can be between 1 and 100. If this parameter
//!   isn't used, then ListClusters returns up to 100 results and a nextToken value if
//!   applicable."
//! * `ListClusters$nextToken` (input): "This token should be treated as an opaque
//!   identifier that is only used to retrieve the next items in a list and not for other
//!   programmatic purposes."
//! * `ListClusters$nextToken` (output): "This value is null when there are no more results
//!   to return."
//!
//! Every case goes through the same front door a client uses, so nothing reaches into the
//! state struct and a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refusal, refused, service, string, ACCOUNT, SIGNED_REGION};

/// How many pages a paging walk may take before the test calls it a loop. Generous: the
/// smallest page size under test is one ARN.
const MAX_PAGES: usize = 64;

/// ONE `ListClusters` page: the ARNs it carries and the `nextToken` it hands back.
async fn page(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, "ListClusters", request, case).await;
    let arns = list(&body, "clusterArns", case)
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string ARN, got {value}"))
                .to_string()
        })
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (arns, next_token)
}

/// Every cluster ARN `request` reaches, in page order, walking `nextToken` to the end.
async fn all_pages(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut this_page = request.clone();
        if let Some(token) = &token {
            this_page["nextToken"] = json!(token);
        }
        let (arns, next) = page(router, this_page, case).await;
        seen.extend(arns);
        match next {
            None => return seen,
            Some(next) => token = Some(next),
        }
    }
    panic!("case {case:?}: ListClusters never ran out of pages within {MAX_PAGES} pages");
}

/// Create one cluster from a whole `CreateCluster` body and return its ARN.
async fn create(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let response = ok(router, "CreateCluster", body, case).await;
    string(&response["cluster"], "clusterArn", case)
}

/// Create a plainly-named cluster and return its ARN.
async fn create_named(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    create(router, json!({ "clusterName": name }), case).await
}

/// Delete one cluster, asserting the tombstone the delete leaves behind.
async fn delete(router: &MultiServiceRouter, reference: &str, case: &str) {
    let body = ok(
        router,
        "DeleteCluster",
        json!({ "cluster": reference }),
        case,
    )
    .await;
    assert_eq!(
        string(&body["cluster"], "status", case),
        "INACTIVE",
        "case {case:?}: a deleted cluster is tombstoned INACTIVE"
    );
}

/// Describe one cluster and return its reported `status`, asserting it is reported at all.
async fn described_status(router: &MultiServiceRouter, reference: &str, case: &str) -> String {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [reference] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: expected the cluster to be described, not missing, got {body}"
    );
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one described cluster, got {body}"
    );
    assert_eq!(
        string(&clusters[0], "clusterArn", case),
        reference,
        "case {case:?}: DescribeClusters resolved a listed ARN to another cluster"
    );
    string(&clusters[0], "status", case)
}

#[tokio::test]
async fn list_clusters_reports_the_stored_clusters_and_drops_a_deleted_one() {
    let router = service();

    // An account that has created nothing holds no clusters, so the listing is empty and
    // there is no page after it.
    let case = "an account with no clusters";
    let (empty, next) = page(&router, json!({}), case).await;
    assert!(
        empty.is_empty(),
        "case {case:?}: expected no cluster ARNs, got {empty:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken, got {next:?}"
    );

    // The rows differ along what the rule generalises over: the NAME's shape (plain,
    // hyphenated, underscored and upper-cased, the documented `default` name a request
    // that names no cluster gets) and what the create configured on the cluster (nothing,
    // tags, settings) — none of which changes whether the cluster is listed.
    let rows: [(&str, &str, Value); 6] = [
        ("a plain cluster", "zeta", json!({ "clusterName": "zeta" })),
        (
            "a tagged cluster",
            "alpha",
            json!({
                "clusterName": "alpha",
                "tags": [{ "key": "env", "value": "test" }],
            }),
        ),
        (
            "a cluster carrying a setting",
            "beta-hyphenated",
            json!({
                "clusterName": "beta-hyphenated",
                "settings": [{ "name": "containerInsights", "value": "enabled" }],
            }),
        ),
        (
            "an upper-cased, underscored name",
            "Gamma_2",
            json!({ "clusterName": "Gamma_2" }),
        ),
        (
            "the documented default cluster, named by omission",
            "default",
            json!({}),
        ),
        (
            "a cluster whose name extends another's",
            "zetaextra",
            json!({ "clusterName": "zetaextra" }),
        ),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, name, body) in &rows {
        let arn = create(&router, body.clone(), case).await;
        assert_eq!(
            arn,
            format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{name}"),
            "case {case:?}: the created cluster's ARN names the cluster"
        );
        expected.push(arn);
    }
    // The listing's order is the ARNs' own total order, not the order they were created
    // in: the rows above were created out of it.
    expected.sort();

    let case = "every stored cluster";
    let listed = all_pages(&router, json!({}), case).await;
    assert_eq!(
        listed, expected,
        "case {case:?}: expected {expected:?}, got {listed:?}"
    );
    // A reporting operation: reading changes nothing a later caller sees.
    let again = all_pages(&router, json!({}), case).await;
    assert_eq!(
        again, expected,
        "case {case:?}: a second identical listing must report {expected:?}, got {again:?}"
    );

    // ROUND TRIP: every ARN the listing reports describes back as an ACTIVE cluster.
    for arn in &expected {
        let case = arn.as_str();
        assert_eq!(
            described_status(&router, arn, case).await.as_str(),
            "ACTIVE",
            "case {case:?}: a listed cluster is one that exists"
        );
    }

    // A DELETED cluster is no longer an existing cluster, so it leaves the listing — and
    // stays discoverable through the reader that owns the tombstone.
    let deleted_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/alpha");
    let case = "after deleting one cluster";
    delete(&router, "alpha", case).await;
    let after_delete = all_pages(&router, json!({}), case).await;
    let without_deleted: Vec<String> = expected
        .iter()
        .filter(|arn| **arn != deleted_arn)
        .cloned()
        .collect();
    assert_eq!(
        after_delete, without_deleted,
        "case {case:?}: expected {without_deleted:?}, got {after_delete:?}"
    );
    assert_eq!(
        described_status(&router, &deleted_arn, case).await.as_str(),
        "INACTIVE",
        "case {case:?}: the tombstone stays describable"
    );

    // Created again under the same name, the cluster exists again and the listing says so.
    let case = "after re-creating the deleted cluster";
    let recreated = create_named(&router, "alpha", case).await;
    assert_eq!(
        recreated, deleted_arn,
        "case {case:?}: the same name carries the same ARN"
    );
    let after_recreate = all_pages(&router, json!({}), case).await;
    assert_eq!(
        after_recreate, expected,
        "case {case:?}: expected {expected:?}, got {after_recreate:?}"
    );
}

#[tokio::test]
async fn list_clusters_pages_in_a_stable_order_at_max_results() {
    let router = service();
    let case = "setup";
    // Clusters created OUT of their ARNs' order, so the listing's order cannot come from
    // insertion order.
    let created = ["pf", "pa", "pe", "pb", "pd", "pc"];
    let mut ordered: Vec<String> = Vec::new();
    for name in created {
        ordered.push(create_named(&router, name, case).await);
    }
    ordered.sort();

    // The rows differ along the page size: the published lower bound (1), sizes that
    // divide the set and sizes that do not, and the published upper bound (100), which
    // holds the whole set in one page.
    for size in [1_i64, 2, 3, 4, 5, 6, 7, 100] {
        let case = format!("maxResults {size}");
        let case = case.as_str();
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "maxResults": size });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (arns, next) = page(&router, request, case).await;
            pages += 1;
            assert!(
                arns.len() as i64 <= size,
                "case {case:?}: a page may not exceed maxResults, got {} entries",
                arns.len()
            );
            assert_eq!(
                next.is_some(),
                seen.len() + arns.len() < ordered.len(),
                "case {case:?}: a nextToken must be present exactly while results remain, \
                 got {next:?} after {} of {} ARNs",
                seen.len() + arns.len(),
                ordered.len()
            );
            seen.extend(arns);
            match next {
                None => break,
                Some(next) => token = Some(next),
            }
            assert!(
                pages < MAX_PAGES,
                "case {case:?}: ListClusters never ran out of pages"
            );
        }
        // The pages concatenate, in order, to the whole set exactly once: nothing is
        // dropped, nothing repeated, and the order does not depend on the page size.
        assert_eq!(
            seen, ordered,
            "case {case:?}: expected the pages to walk {ordered:?}, got {seen:?}"
        );
        let full_pages = ordered
            .len()
            .div_ceil(usize::try_from(size).expect("a positive size"));
        assert_eq!(
            pages,
            full_pages.max(1),
            "case {case:?}: expected {full_pages} page(s) for {} ARNs",
            ordered.len()
        );
    }

    // "If this parameter isn't used, then ListClusters returns up to 100 results and a
    // nextToken value if applicable": six clusters fit in one default page.
    let case = "no maxResults";
    let (only, next) = page(&router, json!({}), case).await;
    assert_eq!(
        only, ordered,
        "case {case:?}: a default-sized page holds the whole set, got {only:?}"
    );
    assert!(
        next.is_none(),
        "case {case:?}: expected no nextToken on the last page, got {next:?}"
    );

    // A cluster created AFTER a token was minted does not disturb the page the token
    // resumes: the token names the cluster the next page starts at, not an index.
    let case = "a cluster created mid-walk";
    let (first, token) = page(&router, json!({ "maxResults": 2 }), case).await;
    assert_eq!(
        first,
        ordered[..2].to_vec(),
        "case {case:?}: the first page is the first two clusters in order"
    );
    let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a nextToken"));
    create_named(&router, "aaaa", case).await;
    let (second, _) = page(
        &router,
        json!({ "maxResults": 2, "nextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        second,
        ordered[2..4].to_vec(),
        "case {case:?}: the resumed page continues where the first stopped"
    );
}

#[tokio::test]
async fn list_clusters_refuses_an_out_of_range_page_size_and_an_unminted_token() {
    let router = service();
    let case = "setup";
    create_named(&router, "refusing", case).await;

    // "This value can be between 1 and 100": each published bound is accepted and the
    // value just outside it is refused.
    for size in [1, 100] {
        let case = format!("maxResults {size} is in range");
        ok(
            &router,
            "ListClusters",
            json!({ "maxResults": size }),
            case.as_str(),
        )
        .await;
    }
    for size in [0, 101, -1] {
        let case = format!("maxResults {size} is out of range");
        refused(
            &router,
            "ListClusters",
            json!({ "maxResults": size }),
            "InvalidParameterException",
            case.as_str(),
        )
        .await;
    }

    // A `nextToken` this operation never minted names no page: the bad parameter it is,
    // which is what `InvalidParameterException` reports, with the token the REQUEST
    // carried so a paginating client knows which cursor to abandon. The rows span the
    // ways a token can fail to name a stored cluster: empty, not an identifier at all, a
    // base64-looking opaque blob, the cluster's NAME rather than the ARN the operation
    // mints, an ARN of a cluster this account never created, and an ARN of another
    // resource kind.
    let unminted = [
        String::new(),
        "not a token".to_string(),
        "c2ltdWxhdGVkLXRva2Vu==".to_string(),
        "refusing".to_string(),
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/nosuchcluster"),
        format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:service/refusing/web"),
    ];
    for token in unminted {
        let case = format!("nextToken {token:?} was never minted");
        let case = case.as_str();
        let (code, message) = refusal(
            &router,
            "ListClusters",
            json!({ "nextToken": token }),
            case,
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected an InvalidParameterException, got {code:?}"
        );
        assert_eq!(
            message,
            format!("Invalid token {token}"),
            "case {case:?}: expected the sentence ECS answers for an unusable token, \
             got {message:?}"
        );
    }

    // A token this operation DID mint still resumes after the cluster it names has been
    // deleted out of the listing: the tombstone is stored, so the walk carries on with the
    // first cluster that sorts after it.
    let case = "a token whose cluster was deleted mid-walk";
    // A router of its own: the listing is account-wide, so the clusters this walk pages
    // through are the only ones stored.
    let router = service();
    let mut ordered: Vec<String> = Vec::new();
    for name in ["resuminga", "resumingb", "resumingc"] {
        ordered.push(create_named(&router, name, case).await);
    }
    ordered.sort();
    let (first, token) = page(&router, json!({ "maxResults": 1 }), case).await;
    assert_eq!(
        first,
        ordered[..1].to_vec(),
        "case {case:?}: the first page is the first cluster in order"
    );
    let token = token.unwrap_or_else(|| panic!("case {case:?}: expected a nextToken"));
    assert_eq!(
        token, ordered[1],
        "case {case:?}: the token names the cluster the next page starts at"
    );
    delete(&router, "resumingb", case).await;
    let resumed = all_pages(
        &router,
        json!({ "maxResults": 1, "nextToken": token }),
        case,
    )
    .await;
    assert_eq!(
        resumed,
        ordered[2..].to_vec(),
        "case {case:?}: the resumed walk skips the deleted cluster and keeps the one after \
         it, got {resumed:?}"
    );
}
