// SPDX-License-Identifier: CC0-1.0

//! `ListServiceVersions`: the Snow on-device service version catalog, driven end to end
//! over HTTP.
//!
//! The rule under test: `ListServiceVersions` reports, for the on-device service its
//! required `ServiceName` names, the versions of that service this emulator's stored
//! catalog holds — one `ServiceVersion` per supported version, newest first — echoing the
//! requested `ServiceName`, narrowing the answer to the versions that run each dependent
//! service the request pinned, reporting the dependent-service pairings those versions
//! run, and paging the result with `MaxResults`/`NextToken` the way this service's other
//! list operations page.
//!
//! Every suite is a TABLE over the axis the rule generalises along — the on-device
//! service, the page size, the token's form, the dependent-service pin — and the
//! expectation of each row is computed from the crate's own catalog
//! (`state::ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState::default()`), so a
//! response that is not computed from that catalog cannot pass and the table cannot drift
//! from the support matrix it tests.
//!
//! The harness (`router`, `call_in_region`, `ok`, `refused`, `string`) is the one
//! `job_lifecycle` already drives the service with, so every suite talks to the emulator
//! through the same front door.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SERVICE_NAME_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    encode_cursor, ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState as SnowballState,
    StoredServiceVersion, DEFAULT_MAX_RESULTS, ON_DEVICE_SERVICE_EKS_ANYWHERE,
    ON_DEVICE_SERVICE_KUBERNETES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{call_in_region, ok, refused, router, string};

/// The Region every call in this suite signs for. The version catalog is the service's
/// own, not a per-Region resource, so the Region is only what makes the request routable.
const SIGNED_REGION: &str = "us-west-2";

/// `MaxResults` on `ListServiceVersions` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListServiceVersions")`).
const MAX_RESULTS_MIN: i64 = 0;
const MAX_RESULTS_MAX: i64 = 100;

/// `NextToken` and `ServiceVersion$Version` are both modelled `len 1..1024`
/// (`smithy_query(query="operation", operation="ListServiceVersions")`,
/// `smithy_query(query="shape", shape="ServiceVersion")`).
const STRING_LEN_MIN: usize = 1;
const STRING_LEN_MAX: usize = 1024;

/// The versions of `service_name` this emulator's catalog holds, newest first, read from
/// the crate rather than restated here.
fn catalog(service_name: &str) -> Vec<StoredServiceVersion> {
    let state = SnowballState::default();
    state
        .service_versions(service_name)
        .unwrap_or_else(|| panic!("the emulator's catalog holds the service {service_name}"))
        .to_vec()
}

/// The versions in `held`, in catalog order.
fn version_strings(held: &[StoredServiceVersion]) -> Vec<String> {
    held.iter().map(|version| version.version.clone()).collect()
}

/// The distinct `(dependent service, version)` pairings the catalog entries in `page`
/// run, in page order, dropping a pairing whose dependent service `pins` pinned to a
/// different version.
fn pairings(page: &[StoredServiceVersion], pins: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut reported: Vec<(String, String)> = Vec::new();
    for version in page {
        for (dependency, supported) in &version.dependencies {
            let pinned_away = pins.iter().any(|(name, pinned)| {
                *name == dependency.as_str() && *pinned != supported.as_str()
            });
            let pairing = (dependency.clone(), supported.clone());
            if !pinned_away && !reported.contains(&pairing) {
                reported.push(pairing);
            }
        }
    }
    reported
}

/// One `ListServiceVersions` that must succeed.
async fn list(router: &MultiServiceRouter, body: Value, case: &str) -> Value {
    ok(router, "ListServiceVersions", body, case).await
}

/// The `Version` of every reported `ServiceVersion`, in wire order.
fn reported_versions(body: &Value, case: &str) -> Vec<String> {
    body["ServiceVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ServiceVersions list, got {body}"))
        .iter()
        .map(|entry| {
            entry["Version"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a Version in each ServiceVersion, got {body}")
                })
                .to_string()
        })
        .collect()
}

/// The reported `DependentServices`, in wire order; an absent member reads as none.
fn reported_dependent_services(body: &Value, case: &str) -> Vec<(String, String)> {
    let Some(list) = body.get("DependentServices") else {
        return Vec::new();
    };
    list.as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DependentServices list, got {body}"))
        .iter()
        .map(|entry| {
            let name = entry["ServiceName"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a ServiceName in each DependentService, got {body}")
            });
            let version = entry["ServiceVersion"]["Version"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a ServiceVersion.Version in each \
                         DependentService, got {body}"
                    )
                });
            (name.to_string(), version.to_string())
        })
        .collect()
}

/// Assert every reported version satisfies the modelled `ServiceVersion$Version` bound.
fn assert_versions_well_formed(versions: &[String], case: &str) {
    for version in versions {
        let length = version.chars().count();
        assert!(
            (STRING_LEN_MIN..=STRING_LEN_MAX).contains(&length),
            "case {case:?}: expected a Version of 1..1024 characters, got {version:?} \
             ({length} characters)"
        );
        assert_eq!(
            version.trim(),
            version,
            "case {case:?}: expected a Version without surrounding whitespace, got {version:?}"
        );
    }
}

/// Walk every page of `service_name`'s catalog at page size `max_results`, returning the
/// versions the pages reported in order.
async fn page_through(
    router: &MultiServiceRouter,
    service_name: &str,
    max_results: Option<i64>,
    expected_total: usize,
    case: &str,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({ "ServiceName": service_name });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let listed = list(router, body, case).await;
        assert_eq!(
            listed["ServiceName"].as_str(),
            Some(service_name),
            "case {case:?}: expected every page to report the requested ServiceName, got {listed}"
        );
        let page = reported_versions(&listed, case);
        assert_versions_well_formed(&page, case);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: the walk produced {pages} pages for {expected_total} versions"
        );
        if let Some(max) = max_results.filter(|max| *max > 0) {
            assert!(
                page.len() <= usize::try_from(max).expect("a positive page size"),
                "case {case:?}: expected at most {max} versions in a page, got {}",
                page.len()
            );
        }
        seen.extend(page);
        next_token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(token) = &next_token {
            let length = token.chars().count();
            assert!(
                (STRING_LEN_MIN..=STRING_LEN_MAX).contains(&length),
                "case {case:?}: expected a NextToken of 1..1024 characters, got {token:?}"
            );
        } else {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn each_modelled_service_reports_its_stored_version_catalog() {
    let router = router();

    // One row per value the modelled `ServiceName` enum admits, read from the generated
    // catalog rather than written out here: every on-device service this API names has a
    // stored version catalog, and reports it.
    for service_name in SERVICE_NAME_VALUES {
        let case = format!("ServiceName {service_name}");
        let held = catalog(service_name);
        assert!(
            !held.is_empty(),
            "case {case:?}: the emulator must hold at least one supported version"
        );
        assert!(
            held.len() <= DEFAULT_MAX_RESULTS,
            "case {case:?}: the catalog holds {} versions, more than the default page of \
             {DEFAULT_MAX_RESULTS}, so this row would need a second page",
            held.len()
        );

        let body = list(&router, json!({ "ServiceName": service_name }), &case).await;
        assert_eq!(
            string(&body, "ServiceName", &case),
            *service_name,
            "case {case:?}: expected the requested ServiceName to be reported, got {body}"
        );
        let reported = reported_versions(&body, &case);
        assert_eq!(
            reported,
            version_strings(&held),
            "case {case:?}: expected the stored catalog in catalog order, got {body}"
        );
        assert_versions_well_formed(&reported, &case);
        assert!(
            body.get("NextToken").is_none(),
            "case {case:?}: a page holding the whole catalog carries no NextToken, got {body}"
        );
        assert_eq!(
            reported_dependent_services(&body, &case),
            pairings(&held, &[]),
            "case {case:?}: expected the dependent services the reported versions run, got {body}"
        );
    }
}

#[tokio::test]
async fn the_kubernetes_catalog_is_the_versions_eks_anywhere_runs() {
    let router = router();

    // Both catalogs come from one support matrix, so the two services can never report
    // version sets that contradict each other: every Kubernetes version EKS Anywhere is
    // reported against is a supported Kubernetes version, and every supported Kubernetes
    // version is run by some EKS Anywhere version.
    let case = "the two catalogs agree";
    let kubernetes = list(
        &router,
        json!({ "ServiceName": ON_DEVICE_SERVICE_KUBERNETES }),
        case,
    )
    .await;
    let eks_anywhere = list(
        &router,
        json!({ "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE }),
        case,
    )
    .await;

    let supported: BTreeSet<String> = reported_versions(&kubernetes, case).into_iter().collect();
    let dependencies = reported_dependent_services(&eks_anywhere, case);
    assert!(
        !dependencies.is_empty(),
        "case {case:?}: EKS Anywhere runs on Kubernetes, so it reports dependent services, \
         got {eks_anywhere}"
    );
    let run_against: BTreeSet<String> = dependencies
        .iter()
        .map(|(name, version)| {
            assert_eq!(
                name, ON_DEVICE_SERVICE_KUBERNETES,
                "case {case:?}: expected every dependent service of EKS Anywhere to be \
                 Kubernetes, got {name}"
            );
            version.clone()
        })
        .collect();
    assert_eq!(
        run_against, supported,
        "case {case:?}: expected the Kubernetes catalog and the versions EKS Anywhere runs \
         to be the same set"
    );

    // Kubernetes on a Snow device depends on no further on-device service, so it reports
    // no dependent services at all rather than an empty list.
    assert!(
        kubernetes.get("DependentServices").is_none(),
        "case {case:?}: expected no DependentServices for a service that depends on \
         nothing, got {kubernetes}"
    );
}

#[tokio::test]
async fn the_catalog_pages_at_every_page_size() {
    let router = router();

    for service_name in SERVICE_NAME_VALUES {
        let held = version_strings(&catalog(service_name));
        let total = held.len();

        // Each row is a page size: the published bounds, sizes that divide the catalog
        // and sizes that do not, the catalog's own length, one more than it, and a
        // request that declares none.
        let rows: Vec<(String, Option<i64>)> = vec![
            ("no MaxResults".to_string(), None),
            (
                "MaxResults at the published minimum".to_string(),
                Some(MAX_RESULTS_MIN),
            ),
            ("MaxResults 1".to_string(), Some(1)),
            ("MaxResults 2".to_string(), Some(2)),
            ("MaxResults 3".to_string(), Some(3)),
            (
                format!("MaxResults at the catalog's length ({total})"),
                Some(i64::try_from(total).expect("a small catalog")),
            ),
            (
                format!("MaxResults one past the catalog's length ({})", total + 1),
                Some(i64::try_from(total + 1).expect("a small catalog")),
            ),
            (
                "MaxResults at the published maximum".to_string(),
                Some(MAX_RESULTS_MAX),
            ),
        ];

        for (row, max_results) in rows {
            let case = format!("{service_name}, {row}");
            let seen = page_through(&router, service_name, max_results, total, &case).await;
            assert_eq!(
                seen, held,
                "case {case:?}: expected the pages to report every supported version once, \
                 in catalog order"
            );
        }

        // One past the published maximum, and one below the published minimum, are
        // outside the modelled range and are refused before the handler runs.
        for (row, max_results) in [
            ("one past the published maximum", MAX_RESULTS_MAX + 1),
            ("one below the published minimum", MAX_RESULTS_MIN - 1),
        ] {
            let case = format!("{service_name}, {row}");
            refused(
                &router,
                "ListServiceVersions",
                json!({ "ServiceName": service_name, "MaxResults": max_results }),
                "ValidationException",
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let router = router();

    for service_name in SERVICE_NAME_VALUES {
        // The token the service itself minted, from a walk that has more pages to go.
        let first_page = list(
            &router,
            json!({ "ServiceName": service_name, "MaxResults": 1 }),
            service_name,
        )
        .await;
        let token = string(&first_page, "NextToken", service_name);

        // A token this service minted for the OTHER on-device service's walk: a cursor
        // belongs to the walk it was minted for, not to whatever list is being paged.
        let other_service = SERVICE_NAME_VALUES
            .iter()
            .find(|name| *name != service_name)
            .expect("the model admits more than one service name");
        let foreign_page = list(
            &router,
            json!({ "ServiceName": other_service, "MaxResults": 1 }),
            other_service,
        )
        .await;
        let foreign_token = string(&foreign_page, "NextToken", other_service);

        // Each row is a token this walk did not issue: not a cursor at all, a hex cursor
        // naming a bare version with no walk behind it, a cursor for a version no catalog
        // holds, the other service's own cursor, and a cursor altered in transit.
        let mut altered = token.clone();
        altered.pop();
        altered.push(if token.ends_with('0') { '1' } else { '0' });

        let rows: Vec<(String, String)> = vec![
            ("not a cursor at all".to_string(), "zzzz".to_string()),
            (
                "a cursor naming a bare version".to_string(),
                encode_cursor(&catalog(service_name)[1].version),
            ),
            (
                "a cursor for a version the catalog does not hold".to_string(),
                encode_cursor("99.99.99"),
            ),
            (
                "the other service's minted cursor".to_string(),
                foreign_token,
            ),
            ("an altered cursor".to_string(), altered),
        ];
        for (row, next_token) in rows {
            let case = format!("{service_name}, {row}");
            refused(
                &router,
                "ListServiceVersions",
                json!({ "ServiceName": service_name, "NextToken": next_token }),
                "InvalidNextTokenException",
                &case,
            )
            .await;
        }

        // A token outside the modelled `len 1..1024` is refused before the handler runs.
        for (row, next_token) in [
            ("one below the published minimum length", String::new()),
            (
                "one past the published maximum length",
                "a".repeat(STRING_LEN_MAX + 1),
            ),
        ] {
            let case = format!("{service_name}, {row}");
            refused(
                &router,
                "ListServiceVersions",
                json!({ "ServiceName": service_name, "NextToken": next_token }),
                "ValidationException",
                &case,
            )
            .await;
        }

        // The token this service did mint still resumes the walk where the page stopped.
        let case = format!("{service_name}, resuming with the minted token");
        let second_page = list(
            &router,
            json!({ "ServiceName": service_name, "MaxResults": 1, "NextToken": token }),
            &case,
        )
        .await;
        assert_eq!(
            reported_versions(&second_page, &case),
            version_strings(&catalog(service_name))[1..2].to_vec(),
            "case {case:?}: expected the second version of the catalog, got {second_page}"
        );
    }
}

#[tokio::test]
async fn a_token_resumes_only_the_walk_it_was_minted_for() {
    let router = router();

    // A walk is its `ServiceName` AND its `DependentServices`: a cursor minted while one
    // filter was in force must not resume a differently filtered list, which would skip
    // or repeat entries rather than refuse.
    let held = catalog(ON_DEVICE_SERVICE_EKS_ANYWHERE);
    let kubernetes = version_strings(&catalog(ON_DEVICE_SERVICE_KUBERNETES));
    let pin = kubernetes
        .iter()
        .find(|version| {
            held.iter()
                .filter(|release| {
                    release.depends_on(ON_DEVICE_SERVICE_KUBERNETES, Some(version))
                })
                .count()
                > 1
        })
        .expect("a Kubernetes version more than one release runs, so its walk has pages");
    let other_pin = kubernetes
        .iter()
        .find(|version| *version != pin)
        .expect("a second supported Kubernetes version");

    let pinned_request = |next_token: Option<&str>, pinned: Option<&str>| {
        let mut body = json!({
            "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE,
            "MaxResults": 1,
        });
        if let Some(pinned) = pinned {
            body["DependentServices"] = json!([{
                "ServiceName": ON_DEVICE_SERVICE_KUBERNETES,
                "ServiceVersion": { "Version": pinned },
            }]);
        }
        if let Some(token) = next_token {
            body["NextToken"] = json!(token);
        }
        body
    };

    let unpinned_token = string(
        &list(&router, pinned_request(None, None), "an unpinned first page").await,
        "NextToken",
        "an unpinned first page",
    );
    let pinned_token = string(
        &list(&router, pinned_request(None, Some(pin)), "a pinned first page").await,
        "NextToken",
        "a pinned first page",
    );

    // Each row is a minted token replayed against a different walk.
    let rows: Vec<(String, Value)> = vec![
        (
            "an unpinned cursor replayed with a pin".to_string(),
            pinned_request(Some(&unpinned_token), Some(pin)),
        ),
        (
            "a pinned cursor replayed without the pin".to_string(),
            pinned_request(Some(&pinned_token), None),
        ),
        (
            "a pinned cursor replayed with another pin".to_string(),
            pinned_request(Some(&pinned_token), Some(other_pin)),
        ),
    ];
    for (case, body) in rows {
        refused(
            &router,
            "ListServiceVersions",
            body,
            "InvalidNextTokenException",
            &case,
        )
        .await;
    }

    // Replayed against the walk that minted it, each token resumes where its page stopped.
    let expected_pinned: Vec<String> = version_strings(
        &held
            .iter()
            .filter(|release| release.depends_on(ON_DEVICE_SERVICE_KUBERNETES, Some(pin)))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let case = "the pinned cursor replayed on its own walk";
    let resumed = list(
        &router,
        pinned_request(Some(&pinned_token), Some(pin)),
        case,
    )
    .await;
    assert_eq!(
        reported_versions(&resumed, case),
        expected_pinned[1..2].to_vec(),
        "case {case:?}: expected the second version of the pinned walk, got {resumed}"
    );
}

#[tokio::test]
async fn a_dependent_service_pin_narrows_the_answer_to_the_versions_that_run_it() {
    let router = router();

    let held = catalog(ON_DEVICE_SERVICE_EKS_ANYWHERE);
    let kubernetes = catalog(ON_DEVICE_SERVICE_KUBERNETES);

    // One row per supported Kubernetes version — every published entry of the dependency
    // catalog, not just one — plus the entry that names the dependent service without
    // pinning a version of it.
    let mut rows: Vec<(String, Value, Vec<&str>)> = vec![(
        "KUBERNETES with no version pinned".to_string(),
        json!([{ "ServiceName": ON_DEVICE_SERVICE_KUBERNETES }]),
        Vec::new(),
    )];
    for version in &kubernetes {
        rows.push((
            format!("KUBERNETES {}", version.version),
            json!([{
                "ServiceName": ON_DEVICE_SERVICE_KUBERNETES,
                "ServiceVersion": { "Version": version.version },
            }]),
            vec![version.version.as_str()],
        ));
    }

    for (case, dependent_services, pinned) in rows {
        let body = list(
            &router,
            json!({
                "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE,
                "DependentServices": dependent_services,
            }),
            &case,
        )
        .await;

        // The rule: the versions reported are exactly those that run every pinned
        // dependent service, in catalog order.
        let expected: Vec<StoredServiceVersion> = held
            .iter()
            .filter(|version| {
                pinned.iter().all(|pin| {
                    version.depends_on(ON_DEVICE_SERVICE_KUBERNETES, Some(pin))
                }) && version.depends_on(ON_DEVICE_SERVICE_KUBERNETES, None)
            })
            .cloned()
            .collect();
        assert!(
            !expected.is_empty(),
            "case {case:?}: the pin must match some supported version for this row to say \
             anything"
        );
        assert_eq!(
            reported_versions(&body, &case),
            version_strings(&expected),
            "case {case:?}: expected the supported versions that run the pinned dependent \
             service, got {body}"
        );

        // And the dependent services reported are the pairings those versions run, with
        // a pinned dependency reported at the version it was pinned to.
        let pins: Vec<(&str, &str)> = pinned
            .iter()
            .map(|version| (ON_DEVICE_SERVICE_KUBERNETES, *version))
            .collect();
        assert_eq!(
            reported_dependent_services(&body, &case),
            pairings(&expected, &pins),
            "case {case:?}: expected the dependent services of the reported versions, got {body}"
        );
    }
}

#[tokio::test]
async fn pins_no_supported_version_satisfies_report_nothing() {
    let router = router();

    // Two versions of the same dependent service, each supported on its own but with no
    // single EKS Anywhere release running both: nothing matches, so the answer is the
    // empty list the required member admits — not an error, and not a page with a token
    // that would never terminate.
    let kubernetes = version_strings(&catalog(ON_DEVICE_SERVICE_KUBERNETES));
    let newest = kubernetes.first().expect("a supported Kubernetes version");
    let oldest = kubernetes.last().expect("a supported Kubernetes version");
    let case = format!("KUBERNETES {newest} and {oldest} at once");
    assert_ne!(
        newest, oldest,
        "case {case:?}: the catalog must hold more than one Kubernetes version for this row"
    );
    assert!(
        catalog(ON_DEVICE_SERVICE_EKS_ANYWHERE).iter().all(|version| {
            !(version.depends_on(ON_DEVICE_SERVICE_KUBERNETES, Some(newest))
                && version.depends_on(ON_DEVICE_SERVICE_KUBERNETES, Some(oldest)))
        }),
        "case {case:?}: some release runs both versions, so this row does not exercise an \
         unsatisfiable pin"
    );

    let body = list(
        &router,
        json!({
            "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE,
            "DependentServices": [
                { "ServiceName": ON_DEVICE_SERVICE_KUBERNETES, "ServiceVersion": { "Version": newest } },
                { "ServiceName": ON_DEVICE_SERVICE_KUBERNETES, "ServiceVersion": { "Version": oldest } },
            ],
        }),
        &case,
    )
    .await;
    assert!(
        reported_versions(&body, &case).is_empty(),
        "case {case:?}: expected no supported version, got {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case {case:?}: an empty answer carries no NextToken, got {body}"
    );
    assert!(
        reported_dependent_services(&body, &case).is_empty(),
        "case {case:?}: an empty answer has no dependent services, got {body}"
    );
}

#[tokio::test]
async fn a_dependent_service_the_requested_service_does_not_have_is_refused() {
    let router = router();

    // Each row is a `DependentServices` entry that names no dependency of the requested
    // service: a service that depends on nothing cannot be given one, a service is not
    // its own dependency, a dependency pinned to a version the catalog does not hold
    // names no version, and an entry naming nothing at all names no service.
    let unsupported_kubernetes = "1.99";
    assert!(
        catalog(ON_DEVICE_SERVICE_KUBERNETES)
            .iter()
            .all(|version| version.version != unsupported_kubernetes),
        "{unsupported_kubernetes} is a supported Kubernetes version, so it does not \
         exercise an unsupported pin"
    );

    // The last row's name is outside the modelled `ServiceName` enum, so the boundary
    // validator refuses it before the handler runs: a different error code for the same
    // logical failure, which is why each row carries the code it must answer with.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "KUBERNETES given a dependent service",
            ON_DEVICE_SERVICE_KUBERNETES,
            json!([{ "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE }]),
            "InvalidResourceException",
        ),
        (
            "EKS_ANYWHERE as its own dependent service",
            ON_DEVICE_SERVICE_EKS_ANYWHERE,
            json!([{ "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE }]),
            "InvalidResourceException",
        ),
        (
            "KUBERNETES pinned to a version the catalog does not hold",
            ON_DEVICE_SERVICE_EKS_ANYWHERE,
            json!([{
                "ServiceName": ON_DEVICE_SERVICE_KUBERNETES,
                "ServiceVersion": { "Version": unsupported_kubernetes },
            }]),
            "InvalidResourceException",
        ),
        (
            "a dependent service naming no service",
            ON_DEVICE_SERVICE_EKS_ANYWHERE,
            json!([{ "ServiceVersion": { "Version": "1.21" } }]),
            "InvalidResourceException",
        ),
        (
            "a second entry that names no dependency",
            ON_DEVICE_SERVICE_EKS_ANYWHERE,
            json!([
                { "ServiceName": ON_DEVICE_SERVICE_KUBERNETES },
                { "ServiceName": ON_DEVICE_SERVICE_EKS_ANYWHERE },
            ]),
            "InvalidResourceException",
        ),
        (
            "a dependent service whose name is whitespace",
            ON_DEVICE_SERVICE_EKS_ANYWHERE,
            json!([{ "ServiceName": "   " }]),
            "ValidationException",
        ),
    ];

    for (case, service_name, dependent_services, error_code) in rows {
        refused(
            &router,
            "ListServiceVersions",
            json!({ "ServiceName": service_name, "DependentServices": dependent_services }),
            error_code,
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_service_name_outside_the_modelled_catalog_is_refused() {
    let router = router();

    // `ServiceName` is required and modelled as an enum, so a request that names no
    // service, or one outside the catalog, is refused rather than answered with some
    // default service's versions.
    let rows: Vec<(&str, Value)> = vec![
        ("no ServiceName at all", json!({})),
        ("an empty ServiceName", json!({ "ServiceName": "" })),
        (
            "a near-miss on a catalog value",
            json!({ "ServiceName": "KUBERNETE" }),
        ),
        ("the wrong case", json!({ "ServiceName": "kubernetes" })),
        (
            "a service of a different enum",
            json!({ "ServiceName": "NFS_ON_DEVICE_SERVICE" }),
        ),
    ];
    for (case, body) in rows {
        if let Some(name) = body.get("ServiceName").and_then(Value::as_str) {
            assert!(
                !SERVICE_NAME_VALUES.contains(&name),
                "case {case:?}: {name:?} is a value the modelled ServiceName admits, so it \
                 does not belong in this table"
            );
        }
        refused(
            &router,
            "ListServiceVersions",
            body,
            "ValidationException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn listing_versions_changes_nothing_a_later_caller_sees() {
    let router = router();

    // A report, not a mutation: the same request answers the same thing however often it
    // is made, and a walk of the catalog leaves the next walk unchanged.
    for service_name in SERVICE_NAME_VALUES {
        let case = format!("{service_name} read twice");
        let request = json!({ "ServiceName": service_name });
        let first = list(&router, request.clone(), &case).await;
        let paged = page_through(
            &router,
            service_name,
            Some(1),
            catalog(service_name).len(),
            &case,
        )
        .await;
        let (status, second) =
            call_in_region(&router, SIGNED_REGION, "ListServiceVersions", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} with {second}"
        );
        assert_eq!(
            second, first,
            "case {case:?}: expected the repeated read to answer exactly the same body"
        );
        assert_eq!(
            paged,
            reported_versions(&first, &case),
            "case {case:?}: expected a paged walk to report the same versions as one page"
        );
    }
}
