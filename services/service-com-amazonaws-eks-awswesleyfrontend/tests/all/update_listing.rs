// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListUpdates`: the listing of the asynchronous updates one
//! Amazon EKS resource has taken, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The operations that MINT an update (`CreateCertificateAuthority`,
//! `UpdateClusterConfig`) and the single read it pages over (`DescribeUpdate`) have
//! suites of their own; this suite is about what the LISTING adds: which update ids
//! it reports for which resource, in which order, that every id it reports is
//! readable through `DescribeUpdate`, how it pages them, and what it answers for a
//! resource that does not exist.
//!
//! Every suite asserts a RULE over a TABLE whose rows differ along what the rule
//! generalises over (how many updates the resource has taken and which operation
//! minted each, the cluster name's shape, the region the request is signed for, the
//! page size, how a token is foreign, which resource inside the cluster the listing
//! is scoped to), rather than over one recorded request; each assertion names its
//! case and prints the actual value.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the cluster,
//! certificate-authority and node-group helpers are the ones the suites that own
//! those slices already export: no suite grows a second client.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, encode_page_token, encode_scoped_page_token, CAPABILITIES_LISTING,
    CLUSTER_ACTIVATION_DELAY, CONNECTED_CLUSTER_REFUSAL_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
    UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_authority::{append_authority, create_cluster, settle, str_member, REGION};
use crate::nodegroup_lifecycle::{create_nodegroup_in, minimal_request as nodegroup_request};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// `ListUpdates`: `GET /clusters/{name}/updates`, with the query string a row drives
/// it at.
async fn list_updates_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    query: &str,
) -> Answer {
    let path = format!("/clusters/{}/updates", encode_label(cluster));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, region, "GET", &uri, "").await
}

async fn list_updates(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    list_updates_in(router, REGION, cluster, query).await
}

/// `DescribeUpdate`: the read every id the listing reports must resolve through.
async fn describe_update(router: &MultiServiceRouter, cluster: &str, update_id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/updates/{}",
            encode_label(cluster),
            encode_label(update_id)
        ),
        "",
    )
    .await
}

/// The `updateIds` of a listing answer, in the order it returned them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["updateIds"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// An `ACTIVE` cluster, which is the precondition every call in this suite needs.
async fn active_cluster(router: &MultiServiceRouter, case: &str, cluster: &str) {
    let created = create_cluster(router, REGION, cluster).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: setup CreateCluster expected 200, got {created:?}"
    );
    settle(CLUSTER_ACTIVATION_DELAY).await;
}

/// Which operation a row mints its next update with. A cluster takes one update at a
/// time, so each of these waits out the span the clock owes the update it started
/// before the next one is minted.
#[derive(Debug, Clone, Copy)]
enum Minter {
    /// `CreateCertificateAuthority`, whose update tracks the trust distribution.
    AppendAuthority,
    /// `UpdateClusterConfig` turning deletion protection on.
    DeletionProtection,
    /// `UpdateClusterConfig` enabling one control-plane log type.
    ClusterLogging,
}

/// Start the update `minter` names on `cluster` and answer the id it reports, once
/// the clock has completed it.
async fn mint(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
    minter: Minter,
) -> String {
    let answer = match minter {
        Minter::AppendAuthority => append_authority(router, REGION, cluster, None).await,
        Minter::DeletionProtection => {
            update_config(router, cluster, &serde_json::json!({ "deletionProtection": true })).await
        }
        Minter::ClusterLogging => {
            update_config(
                router,
                cluster,
                &serde_json::json!({
                    "logging": { "clusterLogging": [{ "types": ["api"], "enabled": true }] },
                }),
            )
            .await
        }
    };
    assert_eq!(
        answer.status, 200,
        "case {case:?}: setup {minter:?} expected 200, got {answer:?}"
    );
    let id = str_member(case, "update.id", &answer.body["update"]["id"]);
    settle(UPDATE_COMPLETION_DELAY).await;
    id
}

/// `UpdateClusterConfig`: `POST /clusters/{name}/update-config`.
async fn update_config(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

// ---------------------------------------------------------------------------
// RULE: the listing reports exactly the ids of the updates the store holds for the
// named cluster's own control plane — every one of them, nothing else — in the
// stable order the cluster holds them in, and each of those ids resolves through
// `DescribeUpdate` to the same update; a cluster that has taken none reports an
// empty list, not a token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_reports_every_update_of_the_cluster_and_each_id_is_readable() {
    let router = emulator();

    // Rows differ along how many updates the cluster has taken, which operation
    // minted each of them, and the shape of the cluster name.
    struct Case {
        case: &'static str,
        cluster: &'static str,
        minters: Vec<Minter>,
    }
    let cases = vec![
        Case {
            case: "a cluster that has taken no update yet",
            cluster: "updates-list-none",
            minters: vec![],
        },
        Case {
            case: "one update, from the certificate-authority append",
            cluster: "updates-list-one",
            minters: vec![Minter::AppendAuthority],
        },
        Case {
            case: "two updates of different types",
            cluster: "updates-list-two",
            minters: vec![Minter::DeletionProtection, Minter::AppendAuthority],
        },
        Case {
            case: "three updates, and another cluster name shape",
            cluster: "Updates9_List-Mixed",
            minters: vec![
                Minter::AppendAuthority,
                Minter::DeletionProtection,
                Minter::ClusterLogging,
            ],
        },
    ];

    for row in &cases {
        let case = row.case;
        active_cluster(&router, case, row.cluster).await;
        let mut minted: BTreeSet<String> = BTreeSet::new();
        for minter in &row.minters {
            minted.insert(mint(&router, case, row.cluster, *minter).await);
        }
        let expected: Vec<String> = minted.iter().cloned().collect();

        let answer = list_updates(&router, row.cluster, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListUpdates, got {answer:?}"
        );
        let ids = listed_ids(&answer);
        assert_eq!(
            ids, expected,
            "case {case:?}: expected exactly the stored update ids in id order \
             {expected:?}, got {answer:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {case:?}: a single page offers no nextToken, got {answer:?}"
        );

        // The listing and the read are two projections of ONE stored record, so every
        // id the listing reports resolves, and resolves to the update it names.
        for id in &ids {
            let described = describe_update(&router, row.cluster, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: every listed update must be readable, got {described:?}"
            );
            assert_eq!(
                described.body["update"]["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: DescribeUpdate must answer the update {id:?} names, got \
                 {described:?}"
            );
        }

        // A listing is a READ: calling it again reports exactly what it reported
        // first.
        let again = list_updates(&router, row.cluster, "").await;
        assert_eq!(
            listed_ids(&again),
            ids,
            "case {case:?}: a listing must not change what the next caller sees, got \
             {again:?}"
        );
    }

    // Across the whole table no two clusters reported the same update id, and no
    // cluster's listing leaked another's history.
    let mut everything: Vec<String> = Vec::new();
    for row in &cases {
        everything.extend(listed_ids(&list_updates(&router, row.cluster, "").await));
    }
    let unique: BTreeSet<&String> = everything.iter().collect();
    assert_eq!(
        unique.len(),
        everything.len(),
        "an update belongs to exactly one cluster, but one was listed twice across \
         clusters: {everything:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: whatever page size the listing is driven at, following `nextToken` to
// exhaustion yields every update id exactly once in the listing's order and the
// last page carries no token; a page size outside the modelled range is refused,
// and so is a token this listing did not issue.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_update_exactly_once() {
    let router = emulator();
    let cluster = "updates-list-paged";
    active_cluster(&router, "paging", cluster).await;
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for minter in [
        Minter::AppendAuthority,
        Minter::DeletionProtection,
        Minter::ClusterLogging,
    ] {
        minted.insert(mint(&router, "paging", cluster, minter).await);
    }
    let stored: Vec<String> = minted.iter().cloned().collect();

    // Rows differ along page size: the modelled lower bound, every size up to the
    // number stored, one above it, and the modelled upper bound.
    for page_size in [1, 2, 3, 4, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list_updates(&router, cluster, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {answer:?}"
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page must not exceed the limit, got {page:?}"
            );
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every update id once, in the \
             listing's order; got {seen:?}"
        );
    }

    // One past each published bound of `maxResults` (1..100) is refused by the
    // modelled range, before the handler runs.
    for case in ["maxResults=0", "maxResults=101"] {
        let answer = list_updates(&router, cluster, case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: outside the modelled range expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }

    // A token this listing did not issue is a bad parameter, not an empty page. The
    // rows differ along HOW the token is foreign: not this encoding's alphabet, an
    // odd length, a well-formed encoding carrying no provenance tag, a hex encoding
    // of a REAL update id, and the two a scope-less check would wave through —
    // tokens other listings of this same service mint.
    let real_id = stored.first().expect("the setup minted three updates");
    let untagged: String = real_id.bytes().map(|byte| format!("{byte:02x}")).collect();
    let clusters_listing = encode_page_token(cluster);
    let capabilities_listing = encode_scoped_page_token(CAPABILITIES_LISTING, real_id);
    for case in [
        "not-hex",
        "zz",
        "abc",
        "ffffffffff",
        untagged.as_str(),
        clusters_listing.as_str(),
        capabilities_listing.as_str(),
    ] {
        let answer = list_updates(&router, cluster, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a foreign nextToken, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a listing narrowed to a resource INSIDE the cluster reports the updates of
// THAT resource — so the cluster's own control-plane updates are not reported as a
// node group's, an add-on's or a capability's — and a resource the cluster does not
// hold has no listing at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_scoped_to_a_resource_in_the_cluster_reports_only_that_resource() {
    let router = emulator();
    let cluster = "updates-list-scoped";
    active_cluster(&router, "scoped", cluster).await;

    // The three resources inside a cluster `ListUpdates` can narrow to, created
    // before any update is minted so each exists for the rows below.
    let nodegroup = create_nodegroup_in(
        &router,
        REGION,
        cluster,
        &nodegroup_request("scoped-nodegroup"),
    )
    .await;
    assert_eq!(
        nodegroup.status, 200,
        "setup CreateNodegroup expected 200, got {nodegroup:?}"
    );
    let addon = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/addons", encode_label(cluster)),
        &serde_json::json!({ "addonName": "vpc-cni" }).to_string(),
    )
    .await;
    assert_eq!(
        addon.status, 200,
        "setup CreateAddon expected 200, got {addon:?}"
    );
    let capability = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &serde_json::json!({
            "capabilityName": "scoped-capability",
            "type": "KRO",
            "roleArn": "arn:aws:iam::000000000000:role/eksCapabilityRole",
            "deletePropagationPolicy": "RETAIN",
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        capability.status, 200,
        "setup CreateCapability expected 200, got {capability:?}"
    );

    let cluster_update = mint(&router, "scoped", cluster, Minter::DeletionProtection).await;

    // The unscoped listing reports the cluster's own update, so the empty answers
    // below are the narrowing and not an empty store.
    let unscoped = list_updates(&router, cluster, "").await;
    assert_eq!(
        listed_ids(&unscoped),
        vec![cluster_update.clone()],
        "the cluster's own listing reports the update it took, got {unscoped:?}"
    );

    // A resource that EXISTS and has taken no update of its own: an empty list, and
    // never the cluster's update, which is not an update of that resource.
    for (case, query) in [
        ("a node group of this cluster", "nodegroupName=scoped-nodegroup"),
        ("an add-on installed in this cluster", "addonName=vpc-cni"),
        (
            "a managed capability of this cluster",
            "capabilityName=scoped-capability",
        ),
    ] {
        let answer = list_updates(&router, cluster, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            listed_ids(&answer),
            Vec::<String>::new(),
            "case {case:?}: the cluster's own update is not an update of this resource, \
             got {answer:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {case:?}: an empty page offers no nextToken, got {answer:?}"
        );
    }

    // A resource the cluster does NOT hold: the generic not-found payload every read
    // of a resource inside a cluster carries, not an empty page.
    for (case, query) in [
        ("a node group that was never created", "nodegroupName=absent"),
        ("an add-on that is not installed", "addonName=coredns"),
        ("a capability that was never created", "capabilityName=absent"),
        (
            "an existing name under the wrong member",
            "addonName=scoped-nodegroup",
        ),
    ] {
        let answer = list_updates(&router, cluster, query).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(RESOURCE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the generic not-found sentence, got {answer:?}"
        );
    }

    // The three members are alternatives — each names one resource to list the
    // updates of — so a request naming two of them names none.
    for (case, query) in [
        (
            "a node group and an add-on",
            "nodegroupName=scoped-nodegroup&addonName=vpc-cni",
        ),
        (
            "an add-on and a capability",
            "addonName=vpc-cni&capabilityName=scoped-capability",
        ),
        (
            "all three at once",
            "nodegroupName=scoped-nodegroup&addonName=vpc-cni&capabilityName=scoped-capability",
        ),
    ] {
        let answer = list_updates(&router, cluster, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster this store does not hold for the region the request is signed
// for has no listing at all — `ResourceNotFoundException` (404) naming the missing
// cluster — and a cluster Amazon EKS does not manage has no control-plane update
// history to list, which is the one `ClientException` every operation of that
// plane answers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_a_cluster_the_store_does_not_manage_is_refused() {
    let router = emulator();
    active_cluster(&router, "setup", "updates-list-present").await;

    // Rows differ along WHY the cluster is not in the store for this request: it was
    // never created, its name differs only in case, and it lives in another region.
    let cases: Vec<(&str, &str, Answer)> = vec![
        (
            "a cluster that was never created",
            "updates-list-absent",
            list_updates(&router, "updates-list-absent", "").await,
        ),
        (
            "a cluster name differing only in case",
            "UPDATES-LIST-PRESENT",
            list_updates(&router, "UPDATES-LIST-PRESENT", "").await,
        ),
        (
            "the right name in another region",
            "updates-list-present",
            list_updates_in(&router, "eu-west-1", "updates-list-present", "").await,
        ),
    ];

    for (case, cluster, answer) in &cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            error_code(answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(cluster_not_found_message(cluster).as_str()),
            "case {case:?}: expected the message to name the missing cluster, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some(*cluster),
            "case {case:?}: expected clusterName to name the missing cluster, got {answer:?}"
        );
    }

    // A cluster `RegisterCluster` connected has no Amazon EKS-managed control plane,
    // so it has no update history of this kind — the refusal, not an empty page. The
    // rows differ along the connector provider the registration names.
    for (case, cluster, provider) in [
        ("a connected cluster registered as OTHER", "updates-list-connected", "OTHER"),
        ("a connected cluster registered as EKS_ANYWHERE", "updates-list-anywhere", "EKS_ANYWHERE"),
    ] {
        let registered = call(
            &router,
            REGION,
            "POST",
            "/cluster-registrations",
            &serde_json::json!({
                "name": cluster,
                "connectorConfig": {
                    "roleArn": "arn:aws:iam::000000000000:role/AmazonEKSConnectorRole",
                    "provider": provider,
                },
            })
            .to_string(),
        )
        .await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: setup RegisterCluster expected 200, got {registered:?}"
        );

        let answer = list_updates(&router, cluster, "").await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ClientException",
            "case {case:?}: expected ClientException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(CONNECTED_CLUSTER_REFUSAL_MESSAGE),
            "case {case:?}: expected the connected-cluster refusal, got {answer:?}"
        );
    }
}
