// SPDX-License-Identifier: CC0-1.0

//! The configuration-set listing, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — through the shared harness in the sibling
//! [`crate::configuration_sets`] suite, so routing, deserialization, the handler and
//! serialization are all exercised.
//!
//! The rule these cases pin: `ListConfigurationSets` REPORTS the name of every
//! configuration set the account holds, read off the records the create path persisted
//! (so the listing and each set's own read cannot disagree), ordered by
//! `ConfigurationSetName`, narrowed to the sets satisfying every key the request's
//! filter names and windowed by its `PageSize` with a `NextToken` that resumes strictly
//! after the last name the page reported; a filter key outside the enum the model binds,
//! and a token this listing never handed out, are refused `BadRequestException`, and no
//! call changes what a later caller sees.
//!
//! The tables below span what the rule generalises over: the number of sets stored
//! (none, one, several), the order they were created in against the order they are
//! reported in, the form of their names (one character, the documented 64-character
//! bound, every documented character class), the one `ConfigurationSetFilterKey` and the
//! substrings it filters on, the page size (absent, one, several, the whole listing,
//! past the whole listing, zero, negative), paging a FILTERED listing, deleting a set
//! out of the listing, and every refusal leg.

use serde_json::{json, Value};

use crate::configuration_sets::{Reply, Ses};
use crate::contact_lists::query_escaped;
use service_com_amazonaws_sesv2_simpleemailservice_v2::{ops, state};

/// The longest `ConfigurationSetName` SES accepts, taken from the crate's own constant
/// so the bound is stated once: the member's documentation states the name "can contain
/// up to 64 alphanumeric characters, including letters, numbers, hyphens (-) and
/// underscores (_) only"
/// (`smithy_query(query="shape",shape="ConfigurationSetName")`).
const MAX_NAME_LEN: usize = state::MAX_CONFIGURATION_SET_NAME_LEN;

/// Every `ConfigurationSetFilterKey` the model binds, taken from the crate's own
/// generated value set so the enum is stated once
/// (`smithy_query(query="shape",shape="ConfigurationSetFilterKey")`:
/// `CONFIGURATION_SET_NAME_CONTAINS`).
const FILTER_KEYS: &[&str] = ops::types::CONFIGURATION_SET_FILTER_KEY_VALUES;

/// Convenience calls for the configuration-set listing, over the shared harness.
trait Listing {
    async fn list_sets(&self, body: Value) -> Reply;
    async fn list_sets_get(&self, query: &str) -> Reply;
    async fn delete_set(&self, name: &str) -> Reply;
}

impl Listing for Ses {
    async fn list_sets(&self, body: Value) -> Reply {
        self.call(
            "POST",
            "/v2/email/list-configuration-sets",
            Some(body),
        )
        .await
    }

    /// `ListConfigurationSets` as an SDK sends it: a GET over the configuration-set
    /// collection with the paging members in the query string.
    async fn list_sets_get(&self, query: &str) -> Reply {
        let path = if query.is_empty() {
            "/v2/email/configuration-sets".to_string()
        } else {
            format!("/v2/email/configuration-sets?{query}")
        };
        self.call("GET", &path, None).await
    }

    async fn delete_set(&self, name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/v2/email/configuration-sets/{name}"),
            None,
        )
        .await
    }
}

/// Create every name in `names`, in the order given, and fail loudly if any create is
/// refused: a listing case is only about the listing.
async fn stored(ses: &Ses, case: &str, names: &[&str]) {
    for name in names {
        let created = ses
            .create_set(json!({"ConfigurationSetName": name}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 creating configuration set {name:?}, got {} {:?}",
            created.status,
            created.message()
        );
    }
}

/// The names a reply reported, in the order reported.
fn reported(reply: &Reply) -> Vec<String> {
    reply
        .member("ConfigurationSets")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `NextToken` a reply carried, or `None` when it carried none.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .as_str()
        .map(str::to_string)
        .filter(|token| !token.is_empty())
}

/// `names`, sorted the way the store orders them: the table is keyed by
/// `ConfigurationSetName`.
fn in_name_order(names: &[&str]) -> Vec<String> {
    let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    expected.sort();
    expected
}

/// One row of the listing table: the sets to store, in creation order.
struct ListingCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The names to create, in the order the requests arrive.
    names: Vec<String>,
}

/// The listing table: one row per thing the ordering-and-projection rule generalises
/// over.
fn listing_cases() -> Vec<ListingCase> {
    vec![
        ListingCase {
            case: "no configuration sets at all",
            names: Vec::new(),
        },
        ListingCase {
            case: "one configuration set",
            names: vec!["marketing".to_string()],
        },
        ListingCase {
            case: "several sets created in an order that is not their name order",
            names: vec![
                "zeta".to_string(),
                "alpha".to_string(),
                "mu".to_string(),
                "beta".to_string(),
            ],
        },
        ListingCase {
            case: "names spanning every documented character class",
            names: vec![
                "Set_01-beta".to_string(),
                "set-02".to_string(),
                "SET_03".to_string(),
                "9lives".to_string(),
            ],
        },
        ListingCase {
            case: "the shortest allowed name beside one at the documented bound",
            names: vec!["a".to_string(), format!("s{}", "x".repeat(MAX_NAME_LEN - 1))],
        },
    ]
}

#[tokio::test]
async fn every_stored_set_is_reported_in_name_order_and_agrees_with_its_own_read() {
    for case in listing_cases() {
        let ses = Ses::new();
        let names: Vec<&str> = case.names.iter().map(String::as_str).collect();
        stored(&ses, case.case, &names).await;

        let listed = ses.list_sets(json!({})).await;
        assert_eq!(
            listed.status,
            200,
            "case {:?}: expected HTTP 200 from ListConfigurationSets, got {} {:?}",
            case.case,
            listed.status,
            listed.message()
        );
        assert_eq!(
            reported(&listed),
            in_name_order(&names),
            "case {:?}: ListConfigurationSets must report every stored set in name order, \
             got {:?}",
            case.case,
            reported(&listed)
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {:?}: an unwindowed listing has nothing left behind it, got {:?}",
            case.case,
            next_token(&listed)
        );

        // The listing is the stored records, not a list of its own: every reported name
        // must be a set this account's own read serves, under exactly that name.
        for name in reported(&listed) {
            let read = ses.get_set(&name).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected HTTP 200 from GetConfigurationSet for {name:?}, got {} {:?}",
                case.case,
                read.status,
                read.message()
            );
            assert_eq!(
                read.member("ConfigurationSetName"),
                json!(name),
                "case {:?}: the read of {name:?} must report the listed name, got {:?}",
                case.case,
                read.member("ConfigurationSetName")
            );
        }

        // Reporting only: listing twice reports the same thing, so the read changed
        // nothing a later caller sees.
        let again = ses.list_sets(json!({})).await;
        assert_eq!(
            reported(&again),
            reported(&listed),
            "case {:?}: two listings must agree, got {:?} then {:?}",
            case.case,
            reported(&listed),
            reported(&again)
        );
    }
}

/// One row of the paging table: the `PageSize` a walk asks for, and the longest page it
/// may be answered with.
struct PageCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `PageSize` member to send, or `None` to leave it out.
    page_size: Option<i32>,
    /// The most names one page may carry, or `None` when nothing truncates the page.
    longest_page: Option<usize>,
}

/// The paging table, over a listing of six sets.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            case: "no PageSize at all, so nothing truncates the page",
            page_size: None,
            longest_page: None,
        },
        PageCase {
            case: "one set per page",
            page_size: Some(1),
            longest_page: Some(1),
        },
        PageCase {
            case: "four sets per page, which does not divide the listing",
            page_size: Some(4),
            longest_page: Some(4),
        },
        PageCase {
            case: "a page size that exactly spans the listing",
            page_size: Some(6),
            longest_page: Some(6),
        },
        PageCase {
            case: "a page size past the whole listing",
            page_size: Some(60),
            longest_page: Some(60),
        },
        PageCase {
            case: "a page size of zero, read as no truncation so a walk still terminates",
            page_size: Some(0),
            longest_page: None,
        },
        PageCase {
            case: "a negative page size, read the same way",
            page_size: Some(-3),
            longest_page: None,
        },
    ]
}

#[tokio::test]
async fn page_size_windows_the_listing_and_next_token_resumes_after_the_last_reported_name() {
    const NAMES: [&str; 6] = ["delta", "alpha", "foxtrot", "charlie", "bravo", "echo"];

    for case in page_cases() {
        let ses = Ses::new();
        stored(&ses, case.case, &NAMES).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more iteration than there are sets would mean a token that never
        // terminates, which the bound turns into a failure rather than a hang.
        for step in 0..=NAMES.len() {
            let mut body = json!({});
            if let Some(size) = case.page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let page = ses.list_sets(body).await;
            assert_eq!(
                page.status, 200,
                "case {:?}: expected HTTP 200 from page {step}, got {} {:?}",
                case.case,
                page.status,
                page.message()
            );
            let page_names = reported(&page);
            if let Some(longest) = case.longest_page {
                assert!(
                    page_names.len() <= longest,
                    "case {:?}: page {step} may carry at most {longest} names, got {page_names:?}",
                    case.case
                );
            }
            walked.extend(page_names);
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {:?}: the walk must terminate, but a token was still offered: {token:?}",
            case.case
        );
        assert_eq!(
            walked,
            in_name_order(&NAMES),
            "case {:?}: walking every page must report each set exactly once, in name \
             order, got {walked:?}",
            case.case
        );
    }
}

/// One row of the filter table: the filter to send and the sets it must leave.
struct FilterCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `Filter` member to send, or `None` to leave it out.
    filter: Option<Value>,
    /// The names the filtered listing must report, in name order.
    expected: Vec<&'static str>,
}

/// The names behind the filter table: they share substrings at the front, in the middle
/// and at the end, so a substring case is not satisfied by a prefix match alone.
const FILTER_FIXTURE: [&str; 4] = ["prod-apac", "prod-emea", "test-emea", "test-us"];

/// The filter table.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no filter at all",
            filter: None,
            expected: FILTER_FIXTURE.to_vec(),
        },
        FilterCase {
            case: "an empty filter, which narrows nothing",
            filter: Some(json!({})),
            expected: FILTER_FIXTURE.to_vec(),
        },
        FilterCase {
            case: "a name substring shared by a prefix of the names",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "prod-"})),
            expected: vec!["prod-apac", "prod-emea"],
        },
        FilterCase {
            case: "a name substring in the middle of the names",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "-eme"})),
            expected: vec!["prod-emea", "test-emea"],
        },
        FilterCase {
            case: "a name substring at the end of the names",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "emea"})),
            expected: vec!["prod-emea", "test-emea"],
        },
        FilterCase {
            case: "a whole name as the substring",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "test-us"})),
            expected: vec!["test-us"],
        },
        FilterCase {
            case: "a one-character substring",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "u"})),
            expected: vec!["test-us"],
        },
        FilterCase {
            case: "a substring no stored name carries",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "staging"})),
            expected: Vec::new(),
        },
        FilterCase {
            case: "a substring differing only in case, which no stored name carries",
            filter: Some(json!({"CONFIGURATION_SET_NAME_CONTAINS": "PROD-"})),
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_filter_reports_only_the_sets_satisfying_every_key_it_names() {
    // Each key the model binds appears in the table, so it covers the whole enum rather
    // than a chosen corner of it.
    for key in FILTER_KEYS.iter().copied() {        assert!(
            filter_cases()
                .iter()
                .any(|case| case.filter.as_ref().is_some_and(|filter| filter.get(key).is_some())),
            "case {key:?}: every modelled ConfigurationSetFilterKey must appear in the \
             filter table"
        );
    }

    for case in filter_cases() {
        let ses = Ses::new();
        stored(&ses, case.case, &FILTER_FIXTURE).await;

        let mut body = json!({});
        if let Some(filter) = &case.filter {
            body["Filter"] = filter.clone();
        }
        let listed = ses.list_sets(body).await;
        assert_eq!(
            listed.status,
            200,
            "case {:?}: expected HTTP 200 from ListConfigurationSets, got {} {:?}",
            case.case,
            listed.status,
            listed.message()
        );
        assert_eq!(
            reported(&listed),
            in_name_order(&case.expected),
            "case {:?}: the filtered listing must report exactly the sets satisfying it, \
             in name order, got {:?}",
            case.case,
            reported(&listed)
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {:?}: an unwindowed listing has nothing left behind it, got {:?}",
            case.case,
            next_token(&listed)
        );
    }
}

#[tokio::test]
async fn a_token_is_a_position_in_the_filtered_listing_it_was_handed_out_from() {
    let case = "paging a filtered listing one set at a time";
    let ses = Ses::new();
    stored(&ses, case, &FILTER_FIXTURE).await;

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=FILTER_FIXTURE.len() {
        let mut body = json!({"PageSize": 1, "Filter": {"CONFIGURATION_SET_NAME_CONTAINS": "-eme"}});
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let page = ses.list_sets(body).await;
        assert_eq!(
            page.status,
            200,
            "case {case:?}: expected HTTP 200 from page {step}, got {} {:?}",
            page.status,
            page.message()
        );
        let names = reported(&page);
        assert!(
            names.len() <= 1,
            "case {case:?}: page {step} may carry one name, got {names:?}"
        );
        walked.extend(names);
        token = next_token(&page);
        if token.is_none() {
            break;
        }
    }

    assert_eq!(
        walked,
        vec!["prod-emea".to_string(), "test-emea".to_string()],
        "case {case:?}: the walk must report the narrowed listing exactly once, in name \
         order, got {walked:?}"
    );
    assert_eq!(
        token, None,
        "case {case:?}: the walk must terminate, but a token was still offered: {token:?}"
    );
}

#[tokio::test]
async fn the_sdks_get_form_answers_what_the_modelled_route_answers() {
    // Both wire forms of the one operation, over the requests the paging members can
    // make: every set, one page, a page that does not divide the listing, a page the
    // size of the whole listing, and a page past it.
    const NAMES: [&str; 6] = ["delta", "alpha", "foxtrot", "charlie", "bravo", "echo"];
    let cases: &[(&str, &str, Value)] = &[
        ("no paging members", "", json!({})),
        ("one set a page", "PageSize=1", json!({"PageSize": 1})),
        ("four sets a page", "PageSize=4", json!({"PageSize": 4})),
        (
            "a page the size of the listing",
            "PageSize=6",
            json!({"PageSize": 6}),
        ),
        (
            "a page past the whole listing",
            "PageSize=60",
            json!({"PageSize": 60}),
        ),
    ];

    for (case, query, body) in cases {
        let ses = Ses::new();
        stored(&ses, case, &NAMES).await;
        let over_get = ses.list_sets_get(query).await;
        let over_post = ses.list_sets(body.clone()).await;
        assert_eq!(
            (over_get.status, reported(&over_get), next_token(&over_get)),
            (
                over_post.status,
                reported(&over_post),
                next_token(&over_post)
            ),
            "case {case:?}: the GET form must answer what the modelled route answers, got \
             {} {:?} against {} {:?}",
            over_get.status,
            reported(&over_get),
            over_post.status,
            reported(&over_post)
        );
        assert_eq!(
            over_get.status, 200,
            "case {case:?}: expected HTTP 200 from the GET form, got {} {:?}",
            over_get.status,
            over_get.message()
        );
    }
}

#[tokio::test]
async fn the_get_forms_token_pages_through_the_whole_listing() {
    const NAMES: [&str; 6] = ["delta", "alpha", "foxtrot", "charlie", "bravo", "echo"];

    for page_size in [1_usize, 2, 4] {
        let case = format!("the GET form, {page_size} set(s) a page");
        let ses = Ses::new();
        stored(&ses, &case, &NAMES).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for step in 0..=NAMES.len() {
            let query = match &token {
                None => format!("PageSize={page_size}"),
                Some(token) => {
                    format!("PageSize={page_size}&NextToken={}", query_escaped(token))
                }
            };
            let page = ses.list_sets_get(&query).await;
            assert_eq!(
                page.status,
                200,
                "case {case:?}: expected HTTP 200 from page {step}, got {} {:?}",
                page.status,
                page.message()
            );
            let names = reported(&page);
            assert!(
                names.len() <= page_size,
                "case {case:?}: page {step} may carry at most {page_size} names, got {names:?}"
            );
            walked.extend(names);
            token = next_token(&page);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            token, None,
            "case {case:?}: the walk must terminate, but a token was still offered: {token:?}"
        );
        assert_eq!(
            walked,
            in_name_order(&NAMES),
            "case {case:?}: walking every page of the GET form must report each set exactly \
             once, in name order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn the_neighbouring_routes_in_the_configuration_set_space_still_answer() {
    let case = "the GET form beside its neighbours";
    let ses = Ses::new();
    stored(&ses, case, &["alpha"]).await;

    // The GET form takes the configuration-set COLLECTION; one set under it is
    // `GetConfigurationSet`'s own route, a POST over the collection is
    // `CreateConfigurationSet`'s, and a deeper path is nobody's.
    let one = ses.get_set("alpha").await;
    assert_eq!(
        one.status,
        200,
        "case {case:?}: expected GetConfigurationSet to still answer, got {} {:?}",
        one.status,
        one.message()
    );
    let created = ses
        .create_set(json!({"ConfigurationSetName": "beta"}))
        .await;
    assert_eq!(
        created.status,
        200,
        "case {case:?}: expected POST over the collection to still create, got {} {:?}",
        created.status,
        created.message()
    );
    let unmodelled = ses
        .call("GET", "/v2/email/configuration-sets/a/b/c", None)
        .await;
    assert_eq!(
        unmodelled.status, 501,
        "case {case:?}: expected a path no route binds to stay unimplemented, got {}",
        unmodelled.status
    );
}

#[tokio::test]
async fn the_listing_follows_the_stored_table_as_sets_are_created_and_deleted() {
    let case = "creating and deleting sets between listings";
    let ses = Ses::new();
    stored(&ses, case, &["alpha", "beta", "gamma"]).await;

    let dropped = ses.delete_set("beta").await;
    assert_eq!(
        dropped.status,
        200,
        "case {case:?}: expected HTTP 200 deleting a stored set, got {} {:?}",
        dropped.status,
        dropped.message()
    );
    let after_delete = ses.list_sets(json!({})).await;
    assert_eq!(
        reported(&after_delete),
        in_name_order(&["alpha", "gamma"]),
        "case {case:?}: a deleted set must leave the listing, got {:?}",
        reported(&after_delete)
    );

    stored(&ses, case, &["aardvark"]).await;
    let after_create = ses.list_sets(json!({})).await;
    assert_eq!(
        reported(&after_create),
        in_name_order(&["aardvark", "alpha", "gamma"]),
        "case {case:?}: a newly created set must join the listing in name order, got {:?}",
        reported(&after_create)
    );
}

#[tokio::test]
async fn input_outside_what_the_model_binds_is_refused_and_nothing_changes() {
    let ses = Ses::new();
    stored(&ses, "the refusal fixture", &["alpha", "beta"]).await;
    let before = ses.list_sets(json!({})).await;

    // Each row is a request the listing cannot serve: a filter key the enum does not
    // name, a token this listing never handed out, and a token another listing of this
    // service handed out.
    let foreign_token =
        state::encode_tagged_page_token(state::CONTACT_LISTS_PAGE_TOKEN_TAG, "alpha");
    for (case, body) in [
        (
            "a filter key the enum does not name",
            json!({"Filter": {"CONFIGURATION_SET_NAME": "alpha"}}),
        ),
        (
            "a filter key differing only in case",
            json!({"Filter": {"configuration_set_name_contains": "alpha"}}),
        ),
        (
            "a NextToken that is not base64 at all",
            json!({"NextToken": "not a token"}),
        ),
        (
            "a NextToken that is base64 of something else",
            json!({"NextToken": ::aws_smithy_types::base64::encode("somewhere else")}),
        ),
        (
            "a NextToken another listing of this service handed out",
            json!({"NextToken": foreign_token}),
        ),
    ] {
        let refused = ses.list_sets(body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400 from ListConfigurationSets, got {}",
            refused.status
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: the refusal must say what it refused, got {:?}",
            refused.message()
        );
    }

    // A refused call stored nothing and removed nothing.
    let after = ses.list_sets(json!({})).await;
    assert_eq!(
        reported(&after),
        reported(&before),
        "case {:?}: a refused listing must leave the stored sets untouched, got {:?} \
         then {:?}",
        "the refusal fixture",
        reported(&before),
        reported(&after)
    );
}
