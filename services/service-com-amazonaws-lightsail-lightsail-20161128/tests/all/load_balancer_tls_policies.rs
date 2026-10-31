// SPDX-License-Identifier: CC0-1.0

//! `GetLoadBalancerTlsPolicies` — the TLS security policy catalog Lightsail
//! publishes for its load balancers — driven over the wire.
//!
//! Every wire case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetLoadBalancerTlsPolicies` and an
//! `awsJson1_1` body, dispatched by the multi-service router, so the
//! modelled-constraint walk, the handler and the awsJson error envelope are all
//! exercised rather than bypassed.
//!
//! The rules under test, asserted over the WHOLE catalog and over sets of
//! requests rather than one recorded call:
//!
//! * every reported policy's members are its catalog entry's — the name, whether
//!   it is the default, the `Protocol-` prefixed protocol versions it enables and
//!   the cipher suites it offers in order of preference — and exactly one
//!   published policy is the default;
//! * the description is DERIVED from the policy's own protocol versions, so a
//!   policy added to the catalog is described the same way;
//! * a page is the catalog from the policy `pageToken` names onward, bounded by
//!   the page size, with `nextPageToken` present only while policies remain — so
//!   a walk reports each policy in order, exactly once — and a token that names
//!   no published policy is a modelled 400 `InvalidInputException`;
//! * a reported policy name is one `CreateLoadBalancer` accepts, and the load
//!   balancer then reports that policy; and
//! * the operation reports, never mutates: identical calls answer identically,
//!   and the catalog does not depend on what the store holds or on which emulator
//!   serves it.
//!
//! The paging rule is additionally exercised directly against
//! `state::load_balancer_tls_policy_page` over SYNTHETIC catalogs, at page sizes
//! the published catalog cannot reach.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The front door every suite in this test target drives the service through, the
// awsJson error-code reader, and the checked paged walk — all already
// implemented beside this suite.
use crate::instance_lifecycle::{error_code, Emulator};
use crate::paging::{expected_page_sizes, names, walk_pages};

/// The operation under test.
const OPERATION: &str = "GetLoadBalancerTlsPolicies";

/// The page size the service publishes for this operation, read from the crate
/// rather than restated here, so a page that grows stays bounded by it.
const PAGE_SIZE: usize = state::LOAD_BALANCER_TLS_POLICY_PAGE_SIZE;

/// Every member the modelled `LoadBalancerTlsPolicy` structure declares
/// (`smithy_query(query="shape",shape="LoadBalancerTlsPolicy")`), in wire
/// spelling. A reported policy carries exactly these and nothing else.
const POLICY_MEMBERS: &[&str] = &["ciphers", "description", "isDefault", "name", "protocols"];

/// The TLS protocol versions a policy may enable, oldest first.
///
/// SOURCE: the protocol tokens the recorded `GetLoadBalancerTlsPolicies` answer
/// reports, which `LoadBalancerTlsPolicy$protocols` ("The protocols used in a
/// given TLS security policy") carries — the `Protocol-` prefixed form of each
/// version, in the recorded capitalisation.
const TLS_VERSIONS: &[&str] = &["Protocol-TLSv1", "Protocol-TLSV1.1", "Protocol-TLSv1.2"];

/// One expected published policy, written out here independently of the
/// service's own `LOAD_BALANCER_TLS_POLICY_SPECS` so these rows pin the
/// published data rather than restating whatever the code happens to hold.
///
/// SOURCE: the Lightsail TLS security policies of "Configuring TLS security
/// policies on your Amazon Lightsail load balancers" in the Developer Guide, as
/// the recorded answer reports them — the default `TLS-2016-08` policy and the
/// restricted forward-secrecy (`FS`) policy, whose name carries the protocol
/// floor (`1-2`), the restricted cipher set (`Res`) and the policy's date; each
/// is the load-balancing service's own security policy of that name, whose
/// protocol versions and ordered cipher suites these rows are. The rows span
/// what the rules generalise over: the default policy against the
/// forward-secrecy one, a policy that enables three protocol versions against
/// one that enables a single version, and each published cipher set (full and
/// restricted).
struct Expected {
    name: &'static str,
    is_default: bool,
    protocols: &'static [&'static str],
    ciphers: &'static [&'static str],
    /// The sentence the policy's description carries: which TLS versions it
    /// enables.
    description: &'static str,
}

const EXPECTED: &[Expected] = &[
    Expected {
        name: "TLS-2016-08",
        is_default: true,
        protocols: &["Protocol-TLSv1", "Protocol-TLSV1.1", "Protocol-TLSv1.2"],
        description: "Enables TLS versions 1, 1.1, and 1.2",
        ciphers: &[
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            "ECDHE-RSA-AES128-GCM-SHA256",
            "ECDHE-ECDSA-AES128-SHA256",
            "ECDHE-RSA-AES128-SHA256",
            "ECDHE-ECDSA-AES128-SHA",
            "ECDHE-RSA-AES128-SHA",
            "ECDHE-ECDSA-AES256-GCM-SHA384",
            "ECDHE-RSA-AES256-GCM-SHA384",
            "ECDHE-ECDSA-AES256-SHA384",
            "ECDHE-RSA-AES256-SHA384",
            "ECDHE-RSA-AES256-SHA",
            "ECDHE-ECDSA-AES256-SHA",
            "AES128-GCM-SHA256",
            "AES128-SHA256",
            "AES128-SHA",
            "AES256-GCM-SHA384",
            "AES256-SHA256",
            "AES256-SHA",
        ],
    },
    Expected {
        name: "TLS-FS-1-2-Res-2019-08",
        is_default: false,
        protocols: &["Protocol-TLSv1.2"],
        description: "Enables only TLS version 1.2",
        ciphers: &[
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            "ECDHE-RSA-AES128-GCM-SHA256",
            "ECDHE-ECDSA-AES128-SHA256",
            "ECDHE-RSA-AES128-SHA256",
            "ECDHE-ECDSA-AES256-GCM-SHA384",
            "ECDHE-RSA-AES256-GCM-SHA384",
            "ECDHE-ECDSA-AES256-SHA384",
            "ECDHE-RSA-AES256-SHA384",
        ],
    },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One page of the catalog for `request`: the policies it reports and the next
/// page token it hands out, if any.
async fn page(emulator: &Emulator, request: Value) -> (Vec<Value>, Option<String>) {
    let case = request.to_string();
    let body = emulator.ok(OPERATION, request).await;
    let policies = body["tlsPolicies"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a tlsPolicies array, got {body:?}"))
        .clone();
    let next = body["nextPageToken"].as_str().map(str::to_string);
    (policies, next)
}

/// Every policy reported from `request` onward, in page order.
async fn walk_from(emulator: &Emulator, request: Value) -> Vec<Value> {
    let mut policies = Vec::new();
    let mut token: Option<String> = None;
    for pages in 0..20usize {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["pageToken"] = json!(token);
        }
        let (reported, next) = page(emulator, body).await;
        policies.extend(reported);
        match next {
            Some(next) => token = Some(next),
            None => return policies,
        }
        assert!(
            pages < 19,
            "case {request:?}: the walk did not terminate within 20 pages"
        );
    }
    panic!("case {request:?}: the walk did not terminate")
}

/// Every policy the published catalog reports, with the size of each page the
/// walk took — through the checked paged walk every listing suite here uses.
async fn walk(emulator: &Emulator, case: &str) -> (Vec<Value>, Vec<usize>) {
    walk_pages(emulator, case, None, OPERATION, "tlsPolicies", PAGE_SIZE).await
}

/// One policy of a reported list, by name.
fn policy<'a>(policies: &'a [Value], name: &str) -> &'a Value {
    policies
        .iter()
        .find(|policy| policy["name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!(
                "case {name}: expected the catalog to report it, got {:?}",
                names(policies)
            )
        })
}

// ---------------------------------------------------------------------------
// The reported members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_policy_reports_its_own_protocols_and_ciphers() {
    let emulator = Emulator::new();
    let (policies, page_sizes) = walk(&emulator, "the whole catalog").await;

    assert!(
        !policies.is_empty(),
        "case the whole catalog: expected at least one TLS security policy, got none"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(policies.len(), PAGE_SIZE),
        "case the whole catalog: expected full pages until the remainder, got {page_sizes:?}"
    );

    for expected in EXPECTED {
        let case = expected.name;
        let reported = policy(&policies, case);

        assert_eq!(
            reported["isDefault"].as_bool(),
            Some(expected.is_default),
            "case {case}: expected isDefault {}, got {:?}",
            expected.is_default,
            reported["isDefault"]
        );
        // "The ciphers are listed in order of preference", so the ORDER is part
        // of what is published, not just the set.
        assert_eq!(
            reported["protocols"],
            json!(expected.protocols),
            "case {case}: expected the policy's own protocols {:?}, got {:?}",
            expected.protocols,
            reported["protocols"]
        );
        assert_eq!(
            reported["ciphers"],
            json!(expected.ciphers),
            "case {case}: expected the policy's own ciphers in order of preference {:?}, got \
             {:?}",
            expected.ciphers,
            reported["ciphers"]
        );

        // The description states which TLS versions this policy enables, and
        // nothing else.
        assert_eq!(
            reported["description"].as_str(),
            Some(expected.description),
            "case {case}: expected the description {:?}, got {:?}",
            expected.description,
            reported["description"]
        );

        // Exactly the modelled members, so nothing is reported that the shape
        // does not declare and nothing it declares is left out.
        let members: BTreeSet<&str> = reported
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: a policy is an object, got {reported:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            POLICY_MEMBERS.iter().copied().collect::<BTreeSet<_>>(),
            "case {case}: expected exactly the modelled LoadBalancerTlsPolicy members, got \
             {members:?}"
        );
    }

    let reported_names: BTreeSet<String> = names(&policies).into_iter().collect();
    let published_names: BTreeSet<String> = EXPECTED
        .iter()
        .map(|expected| expected.name.to_string())
        .collect();
    assert_eq!(
        reported_names, published_names,
        "case the whole catalog: expected exactly the published policies, got {reported_names:?}"
    );
}

/// Catalog-wide invariants, asserted over whatever the catalog holds rather than
/// over one entry by hand: a policy negotiates TLS 1.2 and the versions above its
/// floor, offers a non-empty set of distinct cipher suites, and exactly one
/// policy is the default ("A Boolean value that indicates whether the TLS
/// security policy is the default").
#[tokio::test]
async fn the_catalog_has_one_default_and_every_policy_negotiates_tls_1_2() {
    let emulator = Emulator::new();
    let (policies, _) = walk(&emulator, "the catalog's invariants").await;

    let mut defaults: Vec<String> = Vec::new();
    for reported in &policies {
        let case = reported["name"].as_str().unwrap_or_default().to_string();
        if reported["isDefault"].as_bool() == Some(true) {
            defaults.push(case.clone());
        }

        let protocols: Vec<String> = reported["protocols"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a protocols array, got {reported:?}"))
            .iter()
            .map(|protocol| protocol.as_str().unwrap_or_default().to_string())
            .collect();
        // A policy accepts its floor version and every later one, so its list is
        // a trailing run of the published versions and always ends at TLS 1.2.
        let floor = TLS_VERSIONS.len().saturating_sub(protocols.len());
        assert_eq!(
            protocols,
            TLS_VERSIONS[floor..]
                .iter()
                .map(|version| (*version).to_string())
                .collect::<Vec<_>>(),
            "case {case}: expected the policy's floor version and every later one, got \
             {protocols:?}"
        );

        let ciphers: Vec<String> = reported["ciphers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a ciphers array, got {reported:?}"))
            .iter()
            .map(|cipher| cipher.as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !ciphers.is_empty(),
            "case {case}: a policy offers at least one cipher suite, got {reported:?}"
        );
        let distinct: BTreeSet<&String> = ciphers.iter().collect();
        assert_eq!(
            distinct.len(),
            ciphers.len(),
            "case {case}: a cipher suite must not be offered twice, got {ciphers:?}"
        );

        // A reported name resolves in the published catalog, and the entry it
        // resolves to is the one that was reported.
        let spec = state::load_balancer_tls_policy_spec(&case)
            .unwrap_or_else(|| panic!("case {case}: a reported name must resolve in the catalog"));
        assert_eq!(
            spec.ciphers.len(),
            ciphers.len(),
            "case {case}: expected the catalog entry's own ciphers, got {ciphers:?}"
        );
    }

    assert_eq!(
        defaults.len(),
        1,
        "case the catalog: exactly one published policy is the default, got {defaults:?}"
    );

    // A name the catalog does not publish resolves to nothing — the lookup
    // answers from the catalog rather than from the shape of what it is asked.
    for absent in ["TLS-2016-01", "tls-2016-08", "TLS-FS", "", "TLS-FS-1-3-2099-01"] {
        assert!(
            state::load_balancer_tls_policy_spec(absent).is_none(),
            "case {absent:?}: a name the catalog does not publish must not resolve"
        );
    }
}

/// The description is DERIVED from the policy's own protocol versions: the rows
/// differ along how many versions the policy enables (none, one, two, three) and
/// which, so the English list, the singular "only" form and the serial comma are
/// each exercised. The version is read out of the `Protocol-` prefixed token,
/// whichever capitalisation it carries.
#[test]
fn the_description_states_the_tls_versions_the_policy_enables() {
    let rows: &[(&str, &[&str], &str)] = &[
        (
            "the three versions of the default policy",
            &["Protocol-TLSv1", "Protocol-TLSV1.1", "Protocol-TLSv1.2"],
            "Enables TLS versions 1, 1.1, and 1.2",
        ),
        (
            "a floor of TLS 1.1",
            &["Protocol-TLSV1.1", "Protocol-TLSv1.2"],
            "Enables TLS versions 1.1 and 1.2",
        ),
        (
            "TLS 1.2 only",
            &["Protocol-TLSv1.2"],
            "Enables only TLS version 1.2",
        ),
        (
            "a single older version",
            &["Protocol-TLSv1"],
            "Enables only TLS version 1",
        ),
        (
            "a version beyond the published ones",
            &["Protocol-TLSv1.2", "Protocol-TLSv1.3"],
            "Enables TLS versions 1.2 and 1.3",
        ),
        ("no protocol version at all", &[], "Enables no TLS version"),
    ];

    for (case, protocols, expected) in rows {
        let described = state::load_balancer_tls_policy_description(protocols);
        assert_eq!(
            described, *expected,
            "case {case}: expected {expected:?}, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_token_resumes_the_catalog_at_the_policy_it_names() {
    let emulator = Emulator::new();
    let (whole, page_sizes) = walk(&emulator, "resuming the catalog").await;
    let all_names = names(&whole);

    // Every reported name is a token, including the first and the last: the page
    // it opens is the catalog from there on.
    for (index, token) in all_names.iter().enumerate() {
        let suffix = walk_from(&emulator, json!({"pageToken": token})).await;
        assert_eq!(
            names(&suffix),
            all_names[index..],
            "case pageToken {token:?}: expected the catalog from there on, got {:?}",
            names(&suffix)
        );
    }

    // An absent token and an empty one both ask for the first page.
    for token in [None, Some("")] {
        let request = match token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let (first_page, next) = page(&emulator, request).await;
        assert_eq!(
            names(&first_page),
            all_names[..page_sizes[0]],
            "case pageToken {token:?}: expected the first page, got {:?}",
            names(&first_page)
        );
        assert_eq!(
            next.is_some(),
            all_names.len() > page_sizes[0],
            "case pageToken {token:?}: a next page token is handed out only while policies \
             remain, got {next:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_that_names_no_published_policy_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the ways a token can fail to name a published policy: a
    // policy name of another vintage, the wrong case, a prefix of a name, a
    // suffix of one, whitespace and an opaque cursor from elsewhere.
    for token in [
        "TLS-2016-01",
        "tls-2016-08",
        "TLS-FS",
        "2020-10",
        " ",
        "AAAAAAAA",
    ] {
        let (status, body) = emulator.call(OPERATION, json!({"pageToken": token})).await;
        assert_eq!(
            status, 400,
            "case pageToken {token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case pageToken {token:?}: expected the modelled InvalidInputException, got {body:?}"
        );
        // The refusal names what a page token may be, so the caller can recover.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("TLS-2016-08"),
            "case pageToken {token:?}: expected the refusal to name the published policies, got \
             {message:?}"
        );
    }

    // A token that IS a published name opens a page at it.
    let (resumed, _) = page(&emulator, json!({"pageToken": "TLS-FS-1-2-Res-2019-08"})).await;
    assert_eq!(
        names(&resumed).first().map(String::as_str),
        Some("TLS-FS-1-2-Res-2019-08"),
        "case pageToken TLS-FS-1-2-Res-2019-08: expected the page to open at it, got {resumed:?}"
    );
}

#[test]
fn a_paged_walk_reports_every_policy_once_at_any_page_size() {
    let reported = state::load_balancer_tls_policies();
    let all_names: Vec<String> = reported
        .iter()
        .map(|policy| policy.name.clone().unwrap_or_default())
        .collect();

    // Rows: page sizes below, at and above the catalog's size, plus 0 — which
    // cannot mean "an empty page forever", so it carries one entry.
    for page_size in [0usize, 1, 2, 5, 6, 25] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let (page, next) = state::load_balancer_tls_policy_page(
                &reported,
                page_size,
                token.as_deref(),
            )
            .unwrap_or_else(|| panic!("case page size {page_size}: expected a page"));
            pages += 1;
            assert!(
                page.len() <= page_size.max(1),
                "case page size {page_size}: a page carries at most {page_size} policies, got {}",
                page.len()
            );
            assert!(
                pages < 100,
                "case page size {page_size}: the walk did not terminate"
            );
            walked.extend(
                page.iter()
                    .map(|policy| policy.name.clone().unwrap_or_default()),
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, all_names,
            "case page size {page_size}: expected every policy once, in order"
        );
    }

    // A token that names no entry of the list has no page, whatever its form.
    for token in ["", "TLS", "TLS-2016-07", "tls-fs-2018-06"] {
        assert!(
            state::load_balancer_tls_policy_page(&reported, 2, Some(token)).is_none(),
            "case pageToken {token:?}: expected no page"
        );
    }

    // Pathological inputs are TOTAL, not panics: a very long token names no
    // entry, and the largest page size answers the whole catalog with no next
    // page.
    let long_token = "a".repeat(10_000);
    assert!(
        state::load_balancer_tls_policy_page(&reported, usize::MAX, Some(&long_token)).is_none(),
        "case a 10000-character pageToken: expected no page"
    );
    let (whole, next) = state::load_balancer_tls_policy_page(&reported, usize::MAX, None)
        .unwrap_or_else(|| panic!("case page size usize::MAX: expected the whole catalog"));
    assert_eq!(
        whole.len(),
        reported.len(),
        "case page size usize::MAX: expected every policy in one page, got {}",
        whole.len()
    );
    assert_eq!(
        next, None,
        "case page size usize::MAX: a page holding everything has no next page, got {next:?}"
    );

    // The walk is over the list it is given, not over the published catalog: a
    // name outside that list opens no page, and a catalog of one pages once.
    let synthetic = state::load_balancer_tls_policies_from(&state::LOAD_BALANCER_TLS_POLICY_SPECS
        [..1]);
    assert!(
        state::load_balancer_tls_policy_page(&synthetic, 2, Some("TLS-FS-1-2-Res-2019-08"))
            .is_none(),
        "case a one-entry catalog: a name outside it opens no page"
    );
    let (only, next) = state::load_balancer_tls_policy_page(&synthetic, 2, None)
        .unwrap_or_else(|| panic!("case a one-entry catalog: expected a page"));
    assert_eq!(
        (only.len(), next.clone()),
        (1, None),
        "case a one-entry catalog: expected one page holding it, got {only:?} and {next:?}"
    );
}

// ---------------------------------------------------------------------------
// The catalog against the rest of the service
// ---------------------------------------------------------------------------

/// A reported policy name is one `CreateLoadBalancer` accepts — "Use the
/// GetLoadBalancerTlsPolicies action to get a list of TLS policy names that you
/// can specify" — and the load balancer then reports that policy, both as
/// `tlsPolicyName` and among its configuration options.
#[tokio::test]
async fn a_reported_policy_name_can_be_applied_to_a_load_balancer() {
    let emulator = Emulator::new();
    let (policies, _) = walk(&emulator, "applying a reported policy").await;

    // One row per published policy: the rule holds for every entry of the
    // catalog, not only for the default one.
    for (index, name) in names(&policies).iter().enumerate() {
        let load_balancer_name = format!("tls-policy-lb-{index}");
        emulator
            .ok(
                "CreateLoadBalancer",
                json!({
                    "loadBalancerName": load_balancer_name,
                    "instancePort": 443,
                    "tlsPolicyName": name,
                }),
            )
            .await;
        let read = emulator
            .ok(
                "GetLoadBalancer",
                json!({"loadBalancerName": load_balancer_name}),
            )
            .await["loadBalancer"]
            .clone();

        assert_eq!(
            read["tlsPolicyName"].as_str(),
            Some(name.as_str()),
            "case {name}: expected the load balancer to report the policy it was created with, \
             got {read:?}"
        );
        assert_eq!(
            read["configurationOptions"]["TlsPolicyName"].as_str(),
            Some(name.as_str()),
            "case {name}: expected the policy among the configuration options, got {read:?}"
        );
    }
}

/// Reporting the catalog is a read: it answers the same thing every time,
/// whatever else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let requests = [
        json!({}),
        json!({"pageToken": ""}),
        json!({"pageToken": "TLS-FS-1-2-Res-2019-08"}),
    ];

    let mut first = Vec::new();
    for request in &requests {
        first.push(walk_from(&emulator, request.clone()).await);
    }

    // A mutation of unrelated state, then the same reads again: the published
    // catalog is not per-caller and not affected by what the store holds.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["policy-reader"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_3_0"
            }),
        )
        .await;

    for (index, request) in requests.iter().enumerate() {
        let again = walk_from(&emulator, request.clone()).await;
        assert_eq!(
            again, first[index],
            "case {request:?}: two reads of the catalog must agree, got {again:?}"
        );
    }

    // The instance the mutation created is still there: reading the catalog
    // consumed nothing.
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "policy-reader"}))
        .await;
    assert_eq!(
        instance["instance"]["name"].as_str(),
        Some("policy-reader"),
        "case store unchanged: expected the created instance to survive the reads, got \
         {instance:?}"
    );

    // A second emulator, with its own store, publishes the same catalog.
    let other = Emulator::new();
    for (index, request) in requests.iter().enumerate() {
        let elsewhere = walk_from(&other, request.clone()).await;
        assert_eq!(
            elsewhere, first[index],
            "case {request:?}: the catalog must not depend on which emulator serves it, got \
             {elsewhere:?}"
        );
    }
}
