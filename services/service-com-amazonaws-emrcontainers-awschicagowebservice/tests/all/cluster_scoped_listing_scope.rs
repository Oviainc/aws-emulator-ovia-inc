// SPDX-License-Identifier: CC0-1.0

//! What a listing whose collection is SCOPED to a parent virtual cluster answers when that
//! scope resolves to no cluster.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint validation,
//! the handler and serialization all run exactly as they do for an AWS SDK client.
//!
//! The RULE under test: in a paginated list operation whose collection is scoped by a
//! required path label to a parent resource, and which binds no not-found error, the label
//! SELECTS records and never gates the answer — a well-formed label that names no parent
//! selects none, so the operation answers its success status with the paginated items
//! member present and empty (and no `nextToken`), indistinguishable from a parent that
//! holds nothing. A label value the model's own constraints refuse is still a refusal.
//!
//! Here that is `ListJobRuns` and `ListManagedEndpoints`, the two listings of this service
//! scoped by `{virtualClusterId}`, each modelling `InternalServerException` (500) and
//! `ValidationException` (400) and no not-found error at all (smithy_query: operations
//! `ListJobRuns`, `ListManagedEndpoints`).
//!
//! The tables span what the rule generalises over: the listing (both of them), the FORM of
//! the scope id (both published bounds of `ResourceIdString`, an all-digit value, a
//! plausible minted one, one of a cluster another region's endpoint holds), the QUERY the
//! listing is called with (no query, a page bound of one, a page bound past the stored
//! count, a `states` filter, each `createdAt` bound), and — for the boundary — the way a
//! scope id can violate the model's `@length` / `@pattern`.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::endpoint_body;
use crate::virtual_cluster_job_run_loop::{
    query_instant, runnable_body, spark_submit_driver, text, Answer, Emr, REGION,
};

/// The listings of this service whose collection is scoped to a parent virtual cluster by
/// the `{virtualClusterId}` path label.
#[derive(Clone, Copy, Debug)]
enum Scoped {
    JobRuns,
    ManagedEndpoints,
}

/// Both of them: the rule is about the kind of listing, not about one operation.
const EVERY_CLUSTER_SCOPED_LISTING: [Scoped; 2] = [Scoped::JobRuns, Scoped::ManagedEndpoints];

impl Scoped {
    fn label(self) -> &'static str {
        match self {
            Self::JobRuns => "ListJobRuns",
            Self::ManagedEndpoints => "ListManagedEndpoints",
        }
    }

    /// The path segment the collection hangs off the cluster at.
    fn segment(self) -> &'static str {
        match self {
            Self::JobRuns => "jobruns",
            Self::ManagedEndpoints => "endpoints",
        }
    }

    /// The JSON pointer of the member the `@paginated` `items` come back in.
    fn items(self) -> &'static str {
        match self {
            Self::JobRuns => "/jobRuns",
            Self::ManagedEndpoints => "/endpoints",
        }
    }

    /// One value of the state catalog this listing's `states` filter draws from, so a row
    /// can call it with a filter as well as without one.
    fn a_state(self) -> &'static str {
        match self {
            Self::JobRuns => state::JOB_RUNNING,
            Self::ManagedEndpoints => state::ENDPOINT_ACTIVE,
        }
    }
}

/// `GET /virtualclusters/{cluster}/{segment}{query}` at `region`'s endpoint.
async fn list_at(emr: &Emr, region: &str, listing: Scoped, cluster: &str, query: &str) -> Answer {
    emr.send_in(
        region,
        "GET",
        &format!(
            "/virtualclusters/{cluster}/{}{query}",
            listing.segment()
        ),
        None,
    )
    .await
}

/// One virtual cluster created at `region`'s endpoint. It claims no Kubernetes namespace,
/// so two fixtures never collide on one.
async fn cluster_in(emr: &Emr, region: &str, name: &str, token: &str) -> String {
    let case = format!("a virtual cluster at {region}");
    let answer = emr
        .send_in(
            region,
            "POST",
            "/virtualclusters",
            Some(json!({
                "name": name,
                "clientToken": token,
                "containerProvider": {"id": "eks-scope", "type": "EKS"},
            })),
        )
        .await;
    text(answer.ok(&case), "/id", &case)
}

/// Store one record of this listing's collection under `cluster` at `region`'s endpoint,
/// and answer with its id.
async fn store_one(emr: &Emr, region: &str, listing: Scoped, cluster: &str, tag: &str) -> String {
    let case = format!("{} fixture {tag}", listing.label());
    let body = match listing {
        Scoped::JobRuns => runnable_body(&format!("tok-scope-{tag}"), spark_submit_driver()),
        Scoped::ManagedEndpoints => endpoint_body(
            &format!("scope-{tag}"),
            &format!("tok-scope-{tag}"),
            "JUPYTER_ENTERPRISE_GATEWAY",
        ),
    };
    let answer = emr
        .send_in(
            region,
            "POST",
            &format!("/virtualclusters/{cluster}/{}", listing.segment()),
            Some(body),
        )
        .await;
    text(answer.ok(&case), "/id", &case)
}

/// Hold one answer to the rule: the operation's success status, the paginated items member
/// PRESENT and carrying no record, and no `nextToken` — nothing remains after a page of
/// nothing.
fn assert_empty_page(answer: &Answer, listing: Scoped, case: &str) {
    let body = answer.ok(case);
    let items = body.pointer(listing.items()).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the {} member to be present, got {body}",
            listing.items()
        )
    });
    let records = items.as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {} to be a list, got {items}",
            listing.items()
        )
    });
    assert!(
        records.is_empty(),
        "case {case:?}: expected an empty page, got {records:?}"
    );
    assert_eq!(
        body.get("nextToken"),
        None,
        "case {case:?}: a page carrying no record leaves nothing to resume, got {body}"
    );
}

/// The ids a listed page reported, in the order it reported them.
fn page_ids(body: &Value, listing: Scoped, case: &str) -> Vec<String> {
    body.pointer(listing.items())
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: response carries no {}: {body}", listing.items()))
        .iter()
        .map(|record| text(record, "/id", case))
        .collect()
}

/// Scope ids no cluster of this store is held under, spanning the forms a `ResourceIdString`
/// takes: both published length bounds (len 1..64, `^[0-9a-z]+$` — smithy_query: shape
/// `ResourceIdString`), an all-digit value, a plausible minted one and a digit-led one.
fn absent_scopes() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model allows", "a".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a plausible minted id", "doesnotexist1".to_string()),
        ("a digit-led id of the right shape", "0zzzz9".to_string()),
    ]
}

/// The queries one listing is called with, so the empty page is asserted for a bare call
/// and for every member that narrows a page as well.
///
/// The two instants bound the stored `createdAt` from outside on either side, so neither
/// bound can be what keeps a record out: epoch zero (1970-01-01) is before every stored
/// record and 2100-01-01 (4102444800000 ms) is after every one.
fn query_forms(listing: Scoped) -> Vec<(String, String)> {
    vec![
        ("no query at all".to_string(), String::new()),
        ("a page bound of one".to_string(), "?maxResults=1".to_string()),
        (
            "a page bound past the stored count".to_string(),
            "?maxResults=50".to_string(),
        ),
        (
            format!("a states filter of {}", listing.a_state()),
            format!("?states={}", listing.a_state()),
        ),
        (
            "a createdAfter bound before every record".to_string(),
            format!("?createdAfter={}", query_instant(0)),
        ),
        (
            "a createdBefore bound after every record".to_string(),
            format!("?createdBefore={}", query_instant(4_102_444_800_000)),
        ),
    ]
}

/// THE RULE, over both cluster-scoped listings, every form an absent scope id takes and
/// every query a page can be narrowed by: the answer is the ordinary page with the items
/// member present and empty, never a refusal.
///
/// The control in the same store: the very same calls against a cluster that really holds a
/// record report it, so the rows above answered empty because the scope selected nothing
/// and not because the listing reports nothing.
#[tokio::test]
async fn a_cluster_scoped_listing_answers_an_empty_page_for_an_unresolvable_scope() {
    for listing in EVERY_CLUSTER_SCOPED_LISTING {
        let emr = Emr::new();
        let host = cluster_in(&emr, REGION, "scope-host", "tok-scope-host").await;
        let stored = store_one(&emr, REGION, listing, &host, "host").await;

        for (form, absent) in absent_scopes() {
            for (shape, query) in query_forms(listing) {
                let case = format!("{}, {form}, {shape}", listing.label());
                let answer = list_at(&emr, REGION, listing, &absent, &query).await;
                assert_empty_page(&answer, listing, &case);
            }
        }

        // The control: the cluster that holds the record still reports it, under the same
        // queries that answered empty for an absent scope.
        for (shape, query) in query_forms(listing) {
            if shape.starts_with("a states filter") {
                // A fresh record has not reached that state yet, so this one row is about
                // the filter and not about the scope; the rows above already cover it.
                continue;
            }
            let case = format!("{} of the cluster that holds a record, {shape}", listing.label());
            let answer = list_at(&emr, REGION, listing, &host, &query).await;
            let reported = page_ids(answer.ok(&case), listing, &case);
            assert_eq!(
                reported,
                vec![stored.clone()],
                "case {case:?}: expected the stored record {stored:?}, got {reported:?}"
            );
        }
    }
}

/// THE RULE's BOUNDARY: the model's own constraints on the scope label are untouched. A
/// value `ResourceIdString` does not admit is refused with the modelled
/// `ValidationException` (HTTP 400) rather than answered as an empty page.
///
/// The rows walk the published `@length` bound and the character classes the `@pattern`
/// excludes (len 1..64, `^[0-9a-z]+$` — smithy_query: shape `ResourceIdString`).
#[tokio::test]
async fn a_scope_id_the_model_refuses_is_still_a_validation_error() {
    for listing in EVERY_CLUSTER_SCOPED_LISTING {
        let emr = Emr::new();
        let host = cluster_in(&emr, REGION, "scope-bounds", "tok-scope-bounds").await;
        let stored = store_one(&emr, REGION, listing, &host, "bounds").await;

        for (form, id) in [
            ("one character past the length maximum", "z".repeat(65)),
            ("an id twice the length maximum", "0".repeat(128)),
            ("an upper-case id", host.to_uppercase()),
            ("an id carrying a hyphen", format!("{host}-1")),
            ("an id carrying an underscore", format!("{host}_1")),
            ("an id carrying a dot", format!("{host}.1")),
        ] {
            let case = format!("{}, {form}", listing.label());
            list_at(&emr, REGION, listing, &id, "")
                .await
                .refused(&case, 400, "ValidationException");
        }

        // The refusals changed nothing: the cluster still reports what it held.
        let case = format!("{} after the refusals", listing.label());
        let answer = list_at(&emr, REGION, listing, &host, "").await;
        let reported = page_ids(answer.ok(&case), listing, &case);
        assert_eq!(
            reported,
            vec![stored.clone()],
            "case {case:?}: a refused listing must leave the stored set as it was, got \
             {reported:?}"
        );
    }
}

/// The rule's other half: a scope that selects nothing is answered EXACTLY as a parent that
/// holds nothing, and the empty page writes nothing — the id a listing was called with is
/// still unknown to the store afterwards, so no listing minted a cluster for it.
#[tokio::test]
async fn an_empty_page_is_the_answer_of_an_empty_cluster_and_creates_nothing() {
    for listing in EVERY_CLUSTER_SCOPED_LISTING {
        let emr = Emr::new();
        let empty = cluster_in(&emr, REGION, "scope-empty", "tok-scope-empty").await;
        let absent = "doesnotexist2";

        let from_empty = list_at(&emr, REGION, listing, &empty, "").await;
        let from_absent = list_at(&emr, REGION, listing, absent, "").await;
        let case = format!("{} of a cluster holding nothing", listing.label());
        assert_empty_page(&from_empty, listing, &case);
        let case = format!("{} of a scope that resolves to nothing", listing.label());
        assert_empty_page(&from_absent, listing, &case);
        assert_eq!(
            from_absent.body, from_empty.body,
            "case {case:?}: the two must be indistinguishable, got {} against {}",
            from_absent.body, from_empty.body
        );

        // The listing is a read: the absent id names no cluster afterwards either, so
        // nothing was created for it, and the cluster that holds nothing still holds
        // nothing.
        let case = format!("describing the scope {listing:?} was called with");
        emr.send_in(REGION, "GET", &format!("/virtualclusters/{absent}"), None)
            .await
            .refused(&case, 400, "ResourceNotFoundException");
        let case = format!("{} of the empty cluster, read again", listing.label());
        assert_empty_page(
            &list_at(&emr, REGION, listing, &empty, "").await,
            listing,
            &case,
        );
    }
}

/// THE RULE where the scope names a cluster ANOTHER region's endpoint holds: this service
/// is regional, so such an id selects no record here — which is the empty page, not a
/// refusal — while the endpoint that holds the cluster still reports its records.
#[tokio::test]
async fn a_cluster_of_another_regions_endpoint_selects_no_record_here() {
    // Two different regions, neither of them the emulator's own default alone
    // (`state::DEFAULT_REGION` is `us-east-1`).
    for (home, other) in [("us-east-1", "us-west-2"), ("eu-west-1", "ap-southeast-2")] {
        for listing in EVERY_CLUSTER_SCOPED_LISTING {
            let emr = Emr::new();
            let cluster = cluster_in(&emr, home, "scope-region", "tok-scope-region").await;
            let stored = store_one(&emr, home, listing, &cluster, "region").await;

            let case = format!("{} of a {home} cluster at {other}", listing.label());
            let answer = list_at(&emr, other, listing, &cluster, "").await;
            assert_empty_page(&answer, listing, &case);

            let case = format!("{} of a {home} cluster at {home}", listing.label());
            let answer = list_at(&emr, home, listing, &cluster, "").await;
            let reported = page_ids(answer.ok(&case), listing, &case);
            assert_eq!(
                reported,
                vec![stored.clone()],
                "case {case:?}: the endpoint that holds the cluster must report its record, \
                 got {reported:?}"
            );
        }
    }
}
