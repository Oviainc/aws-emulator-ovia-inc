// SPDX-License-Identifier: CC0-1.0

//! `ListIPSets`, end to end over the wire.
//!
//! The rule asserted here: `ListIPSets` "retrieves an array of IPSetSummary
//! objects for the IP sets that you manage", so the page it answers is computed
//! from the IP sets this service has PERSISTED in the requested scope — one summary
//! per stored set, carrying that set's stored `Name`, `Id`, `Description`, `ARN`
//! and current `LockToken` — and never a value derived from the list request
//! itself. Concretely —
//!
//! * a scope is a namespace, so a listing enumerates the sets of the scope it names
//!   and no others, and a set that was deleted is no longer listed;
//! * the listing is a REPORT: calling it twice, in any order relative to other
//!   reads, answers the same summaries, and every summary matches what `GetIPSet`
//!   answers for the same set;
//! * `Limit` is "the maximum number of objects that you want WAF to return for this
//!   request" and `NextMarker` resumes "the next batch of objects" after the prior
//!   call's marker, so paging enumerates every stored set exactly once whatever the
//!   page size, and a `Limit` outside the modelled `PaginationLimit` range
//!   (`range 1..100`, per
//!   `smithy_query(query="operation",operation="ListIPSets")`) is refused rather
//!   than clamped.
//!
//! Every call goes through the same front door a real client uses (the `Waf`
//! harness in [`crate::web_acl_lifecycle`]), so what these tests observe is what an
//! SDK observes.

use serde_json::{json, Value};

use crate::ip_set_mutations::{create_ip_set, get_ip_set};
use crate::web_acl_lifecycle::{is_model_uuid, Waf, TEST_REGION};

/// The ARN region a CLOUDFRONT-scoped resource carries, whatever region the call
/// was signed for: "To work with CloudFront, you must also specify the Region US
/// East (N. Virginia)".
const CLOUDFRONT_ARN_REGION: &str = "us-east-1";

/// The ARN this service mints for an IP set, as
/// `arn:aws:wafv2:<region>:<account>:<global|regional>/ipset/<name>/<id>`.
fn ip_set_arn(scope: &str, name: &str, id: &str) -> String {
    let (region, prefix) = if scope == "CLOUDFRONT" {
        (CLOUDFRONT_ARN_REGION, "global")
    } else {
        (TEST_REGION, "regional")
    };
    let account = engine::EMULATOR_ACCOUNT_ID;
    format!("arn:aws:wafv2:{region}:{account}:{prefix}/ipset/{name}/{id}")
}

/// The summaries of one `ListIPSets` page, as an array.
fn summaries(page: &Value) -> Vec<Value> {
    page["IPSets"]
        .as_array()
        .unwrap_or_else(|| panic!("a page carries IPSets, got {page}"))
        .clone()
}

/// The `Name` of every summary on a page, in the order the page lists them.
fn listed_names(page: &Value) -> Vec<String> {
    summaries(page)
        .iter()
        .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One stored IP set a listing must report.
struct StoredSet {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The scope the set is created in.
    scope: &'static str,
    /// The set's name within its scope.
    name: &'static str,
    /// The address family the set is created with.
    version: &'static str,
    /// The addresses the create submits.
    addresses: &'static [&'static str],
    /// The description the create submits, or `None` when it omits the member.
    description: Option<&'static str>,
}

/// The rows the membership test stores: they differ along everything the rule
/// generalises over — the SCOPE (both values the model's `Scope` enum declares),
/// the NUMBER of sets per scope, the set's NAME (including the same name in both
/// scopes, which is two different resources), the address FAMILY, the NUMBER of
/// addresses (including none at all), and whether the create carried a
/// `Description`.
const STORED: &[StoredSet] = &[
    StoredSet {
        case: "a regional IPv4 set with one address and a description",
        scope: "REGIONAL",
        name: "alpha-ips",
        version: "IPV4",
        addresses: &["10.0.0.0/24"],
        description: Some("admin ranges"),
    },
    StoredSet {
        case: "a regional IPv4 set with three addresses and no description",
        scope: "REGIONAL",
        name: "bravo-ips",
        version: "IPV4",
        addresses: &["10.1.0.0/16", "172.16.0.0/12", "192.168.0.0/16"],
        description: None,
    },
    StoredSet {
        case: "a regional IPv6 set holding no address at all",
        scope: "REGIONAL",
        name: "charlie-ips",
        version: "IPV6",
        addresses: &[],
        description: Some("empty on purpose"),
    },
    StoredSet {
        case: "a regional set whose name is also taken in the other scope",
        scope: "REGIONAL",
        name: "shared-name",
        version: "IPV4",
        addresses: &["203.0.113.0/24"],
        description: Some("regional shared"),
    },
    StoredSet {
        case: "a cloudfront set under the name the other scope also holds",
        scope: "CLOUDFRONT",
        name: "shared-name",
        version: "IPV4",
        addresses: &["198.51.100.0/24", "192.0.2.0/24"],
        description: Some("edge shared"),
    },
    StoredSet {
        case: "a cloudfront IPv6 set",
        scope: "CLOUDFRONT",
        name: "zulu-ips",
        version: "IPV6",
        addresses: &["2001:db8::/32", "2001:db8:1::/48"],
        description: None,
    },
];

/// Store every row of [`STORED`] and answer, per row, the `(id, arn, lock token)`
/// the create reported.
async fn store_every_row(waf: &Waf) -> Vec<(&'static StoredSet, String, String, String)> {
    let mut created = Vec::new();
    for set in STORED {
        let (id, arn, lock) = create_ip_set(
            waf,
            set.scope,
            set.name,
            set.version,
            set.addresses,
            set.description,
        )
        .await;
        created.push((set, id, arn, lock));
    }
    created
}

/// A listing reports exactly the IP sets its scope holds, with each set's stored
/// members, and reports nothing about the sets of the other scope.
#[tokio::test]
async fn a_listing_reports_the_stored_ip_sets_of_its_scope() {
    let waf = Waf::new();
    let created = store_every_row(&waf).await;

    for scope in ["REGIONAL", "CLOUDFRONT"] {
        let page = waf.ok("ListIPSets", json!({ "Scope": scope })).await;

        // Only this scope's sets, in ascending name order: the sort key a marker
        // resumes from.
        let mut expected_names: Vec<String> = created
            .iter()
            .filter(|(set, ..)| set.scope == scope)
            .map(|(set, ..)| set.name.to_string())
            .collect();
        expected_names.sort();
        assert_eq!(
            listed_names(&page),
            expected_names,
            "case {scope:?}: a listing must report this scope's IP sets only, in \
             name order, got {page}"
        );

        for (set, id, arn, lock) in created.iter().filter(|(set, ..)| set.scope == scope) {
            let case = set.case;
            let summary = summaries(&page)
                .into_iter()
                .find(|summary| summary["Id"] == json!(id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing must report the stored set, got {page}")
                });
            assert_eq!(
                summary["Name"],
                json!(set.name),
                "case {case:?}: expected the stored name, got {summary}"
            );
            assert_eq!(
                summary["ARN"],
                json!(arn),
                "case {case:?}: expected the stored ARN, got {summary}"
            );
            assert_eq!(
                summary["ARN"],
                json!(ip_set_arn(set.scope, set.name, id)),
                "case {case:?}: expected this scope's ipset ARN, got {summary}"
            );
            assert_eq!(
                summary["LockToken"],
                json!(lock),
                "case {case:?}: expected the set's current lock token, got {summary}"
            );
            assert!(
                is_model_uuid(summary["Id"].as_str().unwrap_or_default()),
                "case {case:?}: every summary carries an Id of the modelled form, \
                 got {summary}"
            );
            // A description the create gave is reported verbatim; a create that
            // gave none has nothing to report, so the summary reports the empty
            // description every WAFv2 read answers for such an entity — never an
            // invented one.
            assert_eq!(
                summary["Description"],
                json!(set.description.unwrap_or_default()),
                "case {case:?}: expected the stored description (empty when the \
                 create omitted it), got {summary}"
            );

            // The listing and the by-id read answer the SAME stored resource.
            let got = get_ip_set(&waf, set.scope, set.name, id).await;
            assert_eq!(
                summary["Name"], got["IPSet"]["Name"],
                "case {case:?}: the summary must name the set Get answers, got {got}"
            );
            assert_eq!(
                summary["Id"], got["IPSet"]["Id"],
                "case {case:?}: the summary's Id must be the stored one, got {got}"
            );
            assert_eq!(
                summary["ARN"], got["IPSet"]["ARN"],
                "case {case:?}: the summary's ARN must be the stored one, got {got}"
            );
            assert_eq!(
                summary["Description"], got["IPSet"]["Description"],
                "case {case:?}: the summary's description must be the stored one, \
                 got {got}"
            );
            assert_eq!(
                summary["LockToken"], got["LockToken"],
                "case {case:?}: the summary's lock token must be the stored one, \
                 got {got}"
            );
        }

        // A report changes nothing a later caller sees.
        let again = waf.ok("ListIPSets", json!({ "Scope": scope })).await;
        assert_eq!(
            again, page,
            "case {scope:?}: listing twice must answer the same page, got {again}"
        );
    }
}

/// The listing tracks the store: an update rotates the reported lock token, and a
/// deleted set is no longer listed while its siblings still are.
#[tokio::test]
async fn the_listing_follows_every_mutation_of_the_stored_sets() {
    let waf = Waf::new();
    let created = store_every_row(&waf).await;

    // Rotate one REGIONAL set's lock token; its summary must report the new one.
    let (updated_set, updated_id, _arn, updated_lock) = created
        .iter()
        .find(|(set, ..)| set.name == "bravo-ips")
        .expect("the row the update targets is stored");
    let updated = waf
        .ok(
            "UpdateIPSet",
            json!({
                "Name": updated_set.name,
                "Scope": updated_set.scope,
                "Id": updated_id,
                "LockToken": updated_lock,
                "Addresses": ["10.4.0.0/16"],
                "Description": "rotated",
            }),
        )
        .await;
    let next_lock = updated["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();

    // Delete another REGIONAL set outright.
    let (deleted_set, deleted_id, _deleted_arn, deleted_lock) = created
        .iter()
        .find(|(set, ..)| set.name == "charlie-ips")
        .expect("the row the delete targets is stored");
    waf.ok(
        "DeleteIPSet",
        json!({
            "Name": deleted_set.name,
            "Scope": deleted_set.scope,
            "Id": deleted_id,
            "LockToken": deleted_lock,
        }),
    )
    .await;

    let page = waf.ok("ListIPSets", json!({ "Scope": "REGIONAL" })).await;
    assert_eq!(
        listed_names(&page),
        vec![
            "alpha-ips".to_string(),
            "bravo-ips".to_string(),
            "shared-name".to_string(),
        ],
        "case \"after an update and a delete\": the listing must report the sets \
         the store still holds, got {page}"
    );
    let rotated = summaries(&page)
        .into_iter()
        .find(|summary| summary["Id"] == json!(updated_id))
        .unwrap_or_else(|| panic!("the updated set must still be listed, got {page}"));
    assert_eq!(
        rotated["LockToken"],
        json!(&next_lock),
        "case \"after an update\": the summary must report the rotated lock token, \
         got {rotated}"
    );
    assert_eq!(
        rotated["Description"],
        json!("rotated"),
        "case \"after an update\": the summary must report the replaced \
         description, got {rotated}"
    );

    // The other scope is untouched by either mutation.
    let edge = waf.ok("ListIPSets", json!({ "Scope": "CLOUDFRONT" })).await;
    assert_eq!(
        listed_names(&edge),
        vec!["shared-name".to_string(), "zulu-ips".to_string()],
        "case \"the other scope\": a mutation in one scope must not change the \
         other scope's listing, got {edge}"
    );
}

/// A scope holding no IP set at all answers an empty page and no marker — the
/// paginator's terminal page.
#[tokio::test]
async fn an_empty_scope_answers_an_empty_page_and_no_marker() {
    let waf = Waf::new();
    // One REGIONAL set, so the CLOUDFRONT listing is empty because that scope
    // holds nothing, not because the store does.
    create_ip_set(
        &waf,
        "REGIONAL",
        "only-regional",
        "IPV4",
        &["10.0.0.0/8"],
        None,
    )
    .await;

    for (case, scope, expected) in [
        ("a scope holding one set", "REGIONAL", 1_usize),
        ("a scope holding none", "CLOUDFRONT", 0),
    ] {
        let page = waf.ok("ListIPSets", json!({ "Scope": scope })).await;
        assert_eq!(
            summaries(&page).len(),
            expected,
            "case {case:?}: expected {expected} summaries, got {page}"
        );
        assert_eq!(
            page["NextMarker"].is_null(),
            expected == 0,
            "case {case:?}: only an empty page carries no NextMarker, got {page}"
        );
    }
}

/// Paging: at most `Limit` summaries per call, resumed with the `NextMarker` of the
/// previous page. WAF answers every non-empty page with a `NextMarker` — the name
/// of its last summary, whether or not the page is full — so the paginator's loop
/// ends on the empty page that follows the last one.
///
/// One row per page size, including a page size equal to and larger than the whole
/// collection, over a set count that is neither a multiple nor a divisor of most of
/// them.
#[tokio::test]
async fn paging_enumerates_every_stored_ip_set_exactly_once() {
    let waf = Waf::new();
    let all_names: Vec<String> = (1..=5).map(|index| format!("paged-ips-{index}")).collect();
    for name in &all_names {
        create_ip_set(&waf, "REGIONAL", name, "IPV4", &["10.0.0.0/8"], None).await;
    }
    // A set in the other scope must never appear on a REGIONAL page.
    create_ip_set(
        &waf,
        "CLOUDFRONT",
        "paged-ips-0",
        "IPV4",
        &["10.0.0.0/8"],
        None,
    )
    .await;

    for limit in [1_usize, 2, 3, 5, 8] {
        let case = format!("Limit={limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;

        loop {
            pages += 1;
            assert!(
                pages <= all_names.len() + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({ "Scope": "REGIONAL", "Limit": limit });
            if let Some(marker) = marker.as_deref() {
                request["NextMarker"] = json!(marker);
            }
            let page = waf.ok("ListIPSets", request).await;
            let names = listed_names(&page);
            let remaining = all_names.len() - seen.len();
            assert_eq!(
                names.len(),
                remaining.min(limit),
                "case {case:?}: page {pages} must hold min(limit, remaining) \
                 summaries, got {page}"
            );
            marker = page["NextMarker"].as_str().map(str::to_string);
            assert_eq!(
                marker.as_deref(),
                names.last().map(String::as_str),
                "case {case:?}: page {pages} must carry the last summary's name as \
                 its NextMarker, got {page}"
            );
            seen.extend(names);
            if marker.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, all_names,
            "case {case:?}: paging must enumerate every stored set of the scope \
             exactly once, in order"
        );
        assert_eq!(
            pages,
            all_names.len().div_ceil(limit) + 1,
            "case {case:?}: expected ceil(5 / limit) full pages plus the empty page \
             that ends the loop, got {pages}"
        );
    }
}

/// A `Limit` outside the model's `PaginationLimit` range (`range 1..100`, per
/// `smithy_query(query="operation",operation="ListIPSets")`) is refused, not
/// clamped: a caller never receives a silently empty terminal page. One row per
/// published bound and one past each.
#[tokio::test]
async fn a_limit_outside_the_modelled_range_is_refused() {
    let waf = Waf::new();
    create_ip_set(
        &waf,
        "REGIONAL",
        "limit-ips",
        "IPV4",
        &["10.0.0.0/8"],
        None,
    )
    .await;

    for limit in [1_i64, 100] {
        let page = waf
            .ok("ListIPSets", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            summaries(&page).len(),
            1,
            "case Limit={limit}: a limit on the bound must be honoured, got {page}"
        );
    }

    for limit in [0_i64, -1, 101] {
        let (status, body) = waf
            .call("ListIPSets", json!({ "Scope": "REGIONAL", "Limit": limit }))
            .await;
        assert_eq!(
            status, 400,
            "case Limit={limit}: a limit outside 1..100 must be refused, got status \
             {status} and body {body}"
        );
        // `PaginationLimit`'s `range 1..100` is a MODELLED constraint, so the
        // request boundary refuses the value before the handler runs: the refusal is
        // the framework's `ValidationException`, naming the member and the bound it
        // missed.
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case Limit={limit}: a limit outside the modelled range must be refused \
             by the request boundary, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains("limit"),
            "case Limit={limit}: the refusal must name the member it rejected, got \
             {body}"
        );
        assert!(
            body["IPSets"].is_null(),
            "case Limit={limit}: a refusal must carry no page, got {body}"
        );
    }
}

/// `Scope` names one of the two namespaces the model's `Scope` enum declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`); any
/// other value names no namespace and is refused with no page.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    create_ip_set(
        &waf,
        "REGIONAL",
        "scoped-ips",
        "IPV4",
        &["10.0.0.0/8"],
        None,
    )
    .await;

    // A row whose scope carries JSON metacharacters and a newline: the refusal
    // body must still be a well-formed, parsed JSON envelope carrying the modelled
    // error code, so the value is encoded rather than spliced in.
    for scope in [
        "GLOBAL",
        "regional",
        "",
        "REGIONAL ",
        "REGIONAL\"},{\"x\":1",
        "back\\slash\nnewline",
    ] {
        let (status, body) = waf.call("ListIPSets", json!({ "Scope": scope })).await;
        assert_eq!(
            status, 400,
            "case Scope={scope:?}: an undeclared scope must be refused, got status \
             {status} and body {body}"
        );
        assert!(
            body.is_object(),
            "case Scope={scope:?}: a refusal must answer a parsed JSON envelope, \
             got {body}"
        );
        // `Scope` is a modelled enum, so the request boundary refuses an undeclared
        // value before the handler runs, with the framework's `ValidationException`
        // naming the member and the value set it had to be drawn from.
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case Scope={scope:?}: an undeclared scope must be refused by the \
             request boundary, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains("scope"),
            "case Scope={scope:?}: the refusal must name the member it rejected, \
             got {body}"
        );
        assert!(
            body["IPSets"].is_null(),
            "case Scope={scope:?}: a refusal must carry no page, got {body}"
        );
    }

    // The refusals left the stored set listable.
    let page = waf.ok("ListIPSets", json!({ "Scope": "REGIONAL" })).await;
    assert_eq!(
        listed_names(&page),
        vec!["scoped-ips".to_string()],
        "a refused listing must leave the stored set listable, got {page}"
    );
}

/// A `NextMarker` naming a position the store does not hold is still a position:
/// the page resumes at the first stored name after it, so a marker past every
/// stored name answers the terminal empty page rather than an error.
#[tokio::test]
async fn a_marker_resumes_at_the_first_name_after_it() {
    let waf = Waf::new();
    for name in ["m-bravo", "m-delta", "m-foxtrot"] {
        create_ip_set(&waf, "REGIONAL", name, "IPV4", &["10.0.0.0/8"], None).await;
    }

    // (case, marker, the names the page must report)
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "a marker before every stored name",
            "m-alpha",
            &["m-bravo", "m-delta", "m-foxtrot"],
        ),
        (
            "a marker equal to a stored name",
            "m-bravo",
            &["m-delta", "m-foxtrot"],
        ),
        (
            "a marker between two stored names",
            "m-charlie",
            &["m-delta", "m-foxtrot"],
        ),
        ("a marker equal to the last stored name", "m-foxtrot", &[]),
        ("a marker past every stored name", "m-zulu", &[]),
    ];

    for (case, marker, expected) in cases {
        let page = waf
            .ok(
                "ListIPSets",
                json!({ "Scope": "REGIONAL", "NextMarker": marker }),
            )
            .await;
        assert_eq!(
            listed_names(&page),
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the names after the marker, got {page}"
        );
        assert_eq!(
            page["NextMarker"].is_null(),
            expected.is_empty(),
            "case {case:?}: only an empty page carries no NextMarker, got {page}"
        );
    }
}
