// SPDX-License-Identifier: CC0-1.0

//! `ListRealtimeLogConfigs`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "Gets a list of real-time log configurations": one `RealtimeLogConfig` per stored
//!   configuration, each the record `GetRealtimeLogConfig` answers from, so the listing
//!   reports what was persisted rather than anything derived from the request being served;
//! * "If the total number of items in the list exceeds the maximum that you specify, or the
//!   default maximum, the response is paginated. To get the next page of items, send a
//!   subsequent request that specifies the NextMarker value from the current response as the
//!   Marker value in the subsequent request" — so `MaxItems` cuts the page and is echoed as
//!   asked for, the request's `Marker` is echoed, `IsTruncated` and `NextMarker` say exactly
//!   when configurations remain, `NextMarker` names the configuration the page ended at, and
//!   a request resumes AFTER the one its `Marker` names: walking the pages at ANY admissible
//!   page size enumerates every configuration exactly once, in one order;
//! * a page size that names no page, and a `Marker` that is no resume point this service
//!   handed out, are refused with the errors the operation models and change nothing;
//! * a listing is a READ: it answers the same records again and mints nothing, and a
//!   configuration a delete removed is listed by nothing and resumes nothing.
//!
//! The rows differ along everything these rules generalise over: the number of
//! configurations stored (none, one, several), the page size (one, several, the collection
//! size, past it, this service's own ceiling and past it), the resume point (every stored
//! name in turn, the last one, one no configuration holds), and the shape of each stored
//! configuration — which is the real-time log configuration lifecycle suite's table, reused
//! rather than copied, together with its create, read and delete helpers and the
//! distribution lifecycle suite's wire harness.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE, MAX_ITEMS_REALTIME_LOG_CONFIGS,
    NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE, REALTIME_LOG_CONFIG_MARKER_INVALID_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, Answer, Node, API_VERSION};
use crate::realtime_log_config_lifecycle::{
    assert_reports, config_node, config_table, create_accepted, delete_config, get_config,
    Submitted,
};

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/realtime-log-config`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=…`), from the operation's `http` binding.
async fn list_configs(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/realtime-log-config{query}"),
        None,
        None,
    )
    .await
}

/// Percent-encode a query value, so a configuration name of any shape — punctuation or
/// non-ASCII — travels in the request URI as the one value the service decodes back.
///
/// `pub(crate)` so a sibling suite whose operation binds an input member to a query string
/// parameter encodes its value the same way rather than carrying a copy of this.
pub(crate) fn encode_query(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The `RealtimeLogConfig` elements a page reports, in document order.
///
/// `RealtimeLogConfigList`'s member carries no `xmlName`
/// (`smithy_query(query="shape",shape="RealtimeLogConfigList")`), so each item travels
/// under the default element name.
fn listed(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("member"))
        .unwrap_or_default()
}

/// The configuration names a page reports, in the order the page carries them.
fn listed_names(list: &Node) -> Vec<String> {
    listed(list)
        .into_iter()
        .map(|config| config.text_at("Name").unwrap_or_default().to_string())
        .collect()
}

/// Create every row of the shared table, in the order the table carries them.
async fn create_all(router: &MultiServiceRouter, rows: &[Submitted]) {
    for row in rows {
        create_accepted(router, row).await;
    }
}

/// The created configurations in the one total order every page indexes into: a
/// configuration is identified by its `Name`, so that is the order the store holds them in.
fn in_name_order(rows: &[Submitted]) -> Vec<Submitted> {
    let mut ordered = rows.to_vec();
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    ordered
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// An account holding no real-time log configurations lists none: a page with nothing in
/// it, carrying an EMPTY `Items` element, the default page size, the empty marker the
/// request named and nothing that says more remain.
#[tokio::test]
async fn an_account_holding_no_configurations_lists_no_items() {
    let router = emulator();
    let case = "empty account";

    let listed_answer = list_configs(&router, "").await;
    assert_eq!(
        listed_answer.status, 200,
        "case {case}: expected HTTP 200 from ListRealtimeLogConfigs, got {} with {}",
        listed_answer.status, listed_answer.raw
    );
    let page = listed_answer.root();
    assert_eq!(
        page.text_at("MaxItems"),
        Some(MAX_ITEMS_REALTIME_LOG_CONFIGS.to_string().as_str()),
        "case {case}: expected the default MaxItems {MAX_ITEMS_REALTIME_LOG_CONFIGS}, got {}",
        listed_answer.raw
    );
    assert_eq!(
        page.text_at("IsTruncated"),
        Some("false"),
        "case {case}: expected IsTruncated false, got {}",
        listed_answer.raw
    );
    assert_eq!(
        page.text_at("Marker"),
        Some(""),
        "case {case}: a first page names no marker, got {}",
        listed_answer.raw
    );
    let items = page
        .child("Items")
        .unwrap_or_else(|| panic!("case {case}: expected an Items element, got {}", listed_answer.raw));
    assert!(
        items.children_named("RealtimeLogConfig").is_empty(),
        "case {case}: a page that lists nothing carries an empty Items element, got {}",
        listed_answer.raw
    );
    assert!(
        page.text_at("NextMarker").is_none(),
        "case {case}: expected no NextMarker when nothing remains, got {}",
        listed_answer.raw
    );
}

/// The listing reports every stored configuration, each exactly as the record a read
/// answers from — and it is a READ, so asking again answers the same records.
#[tokio::test]
async fn a_listing_reports_every_stored_configuration_as_it_was_created() {
    let router = emulator();
    let table = config_table();
    create_all(&router, &table).await;
    let expected = in_name_order(&table);

    let answer = list_configs(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        listed_names(answer.root()),
        expected
            .iter()
            .map(|row| row.name.clone())
            .collect::<Vec<String>>(),
        "case whole collection: expected every stored name once, in name order, got {}",
        answer.raw
    );
    assert_eq!(
        answer.root().text_at("IsTruncated"),
        Some("false"),
        "case whole collection: nothing remains, got {}",
        answer.raw
    );

    for (row, item) in expected.iter().zip(listed(answer.root())) {
        // The listed configuration is the one that was stored, down to the order of its
        // fields and endpoints.
        assert_reports(item, row, "the listing", &answer.raw);

        // The listing and the read answer from ONE record.
        let fetched = get_config(&router, Some(row.name.as_str()), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {}: expected HTTP 200 from GetRealtimeLogConfig, got {} with {}",
            row.case, fetched.status, fetched.raw
        );
        let read = config_node(&fetched, row.case, "the read");
        assert_eq!(
            item.text_at("ARN"),
            read.text_at("ARN"),
            "case {}: expected the listing's ARN to be the read's {:?}, got {:?}",
            row.case,
            read.text_at("ARN"),
            item.text_at("ARN")
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_configs(&router, "").await;
    assert_eq!(
        listed_names(again.root()),
        listed_names(answer.root()),
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// Walking the pages at ANY admissible page size enumerates every configuration exactly
/// once, in the stored order, with `MaxItems` and `Marker` echoed and `IsTruncated` and
/// `NextMarker` saying exactly when configurations remain.
#[tokio::test]
async fn walking_the_pages_enumerates_every_configuration_exactly_once() {
    let router = emulator();
    let table = config_table();
    create_all(&router, &table).await;
    let order: Vec<String> = in_name_order(&table)
        .into_iter()
        .map(|row| row.name)
        .collect();

    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // At most one page per configuration plus one, so a listing that never stops
        // truncating fails here rather than looping forever.
        for page in 0..=order.len() + 1 {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Marker={}", encode_query(value)),
                None => format!("?MaxItems={size}"),
            };
            let answer = list_configs(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {size} page {page}: expected HTTP 200 for {query}, got {} with {}",
                answer.status, answer.raw
            );
            let list = answer.root();
            assert_eq!(
                list.text_at("MaxItems"),
                Some(size.to_string().as_str()),
                "case page size {size} page {page}: expected the requested MaxItems echoed, got {}",
                answer.raw
            );
            assert_eq!(
                list.text_at("Marker"),
                Some(marker.clone().unwrap_or_default().as_str()),
                "case page size {size} page {page}: expected the request's Marker echoed, got {}",
                answer.raw
            );
            let names = listed_names(list);
            assert!(
                names.len() <= size.min(ceiling),
                "case page size {size} page {page}: a page served {} items, got {}",
                names.len(),
                answer.raw
            );
            walked.extend(names.iter().cloned());

            let remaining = walked.len() < order.len();
            assert_eq!(
                list.text_at("IsTruncated"),
                Some(remaining.to_string().as_str()),
                "case page size {size} page {page}: expected IsTruncated {remaining} after {} \
                 of {} configurations, got {}",
                walked.len(),
                order.len(),
                answer.raw
            );
            let next = list.text_at("NextMarker").map(str::to_string);
            assert_eq!(
                next.is_some(),
                remaining,
                "case page size {size} page {page}: expected NextMarker present {remaining} \
                 after {} of {} configurations, got {}",
                walked.len(),
                order.len(),
                answer.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        names.last().map(String::as_str),
                        "case page size {size} page {page}: expected NextMarker to name the \
                         configuration the page ended at, got {}",
                        answer.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every configuration \
             exactly once, in order"
        );
    }

    // Every stored name is a resume point, and the page it opens is exactly what follows it
    // in the one order — so the last name resumes an empty page.
    for (index, name) in order.iter().enumerate() {
        let answer = list_configs(&router, &format!("?Marker={}", encode_query(name))).await;
        assert_eq!(
            answer.status, 200,
            "case resume after {name}: expected HTTP 200, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            listed_names(answer.root()),
            order[index + 1..].to_vec(),
            "case resume after {name}: expected the configurations ordered after it, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().text_at("Marker"),
            Some(name.as_str()),
            "case resume after {name}: expected the request's Marker echoed, got {}",
            answer.raw
        );
    }
}

/// A page size that names no page, and a `Marker` that is no resume point this service
/// handed out, are refused with the errors the operation models and change nothing.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let table = config_table();
    create_all(&router, &table).await;
    let before = listed_names(list_configs(&router, "").await.root());
    let last = before.last().cloned().expect("the table created rows");

    // Rows: every form a page size can be unusable in (negative, no integer at all,
    // past what the member can hold), plus an unusable size beside a usable marker, where
    // the size is what is refused.
    let page_size_rows: &[(&str, String)] = &[
        ("a negative page size", "?MaxItems=-1".to_string()),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number".to_string(),
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999".to_string(),
        ),
        (
            "an unusable page size beside a usable marker",
            format!("?MaxItems=-1&Marker={}", encode_query(&last)),
        ),
    ];
    for (case, query) in page_size_rows {
        let answer = list_configs(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {query}, got {}",
            answer.raw
        );
        // What the refusal states is the CONSTRAINT on the parameter, so every unusable
        // value is refused with that one sentence.
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            MAX_ITEMS_INVALID_MESSAGE,
            "case {case}: expected the page-size constraint stated for {query}, got {}",
            answer.raw
        );
    }

    // A configuration is identified by a name that is never empty, so an empty marker is no
    // resume point at all rather than a name the store holds nothing under.
    let empty = list_configs(&router, "?Marker=").await;
    assert_eq!(
        empty.status, 400,
        "case an empty marker: expected HTTP 400, got {} with {}",
        empty.status, empty.raw
    );
    assert_eq!(
        empty.error_code(),
        "InvalidArgument",
        "case an empty marker: expected InvalidArgument, got {}",
        empty.raw
    );
    assert_eq!(
        empty.root().find_text("Message").unwrap_or_default(),
        REALTIME_LOG_CONFIG_MARKER_INVALID_MESSAGE,
        "case an empty marker: expected the marker constraint stated, got {}",
        empty.raw
    );

    // A marker naming no stored configuration is a configuration that does not exist: a
    // name nothing was ever created under, one differing only in case from a stored name,
    // one that is a stored name with something appended, and a non-ASCII name the store
    // holds nothing under.
    let unknown = [
        ("a name nothing holds", "no-such-configuration".to_string()),
        ("a stored name in another case", last.to_uppercase()),
        ("a stored name with a suffix", format!("{last}-extra")),
        ("a non-ASCII name nothing holds", "журнал-absent".to_string()),
    ];
    for (case, marker) in &unknown {
        let answer = list_configs(&router, &format!("?Marker={}", encode_query(marker))).await;
        assert_eq!(
            answer.status, 404,
            "case {case} ({marker}): expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchRealtimeLogConfig",
            "case {case} ({marker}): expected NoSuchRealtimeLogConfig, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_REALTIME_LOG_CONFIG_MESSAGE,
            "case {case} ({marker}): expected the modelled sentence, got {}",
            answer.raw
        );
    }

    // Every refusal above left the store exactly as it was.
    let after = list_configs(&router, "").await;
    assert_eq!(
        listed_names(after.root()),
        before,
        "case refusals: a refused listing must change nothing, got {}",
        after.raw
    );
}

/// A configuration a delete removed is listed by nothing and resumes nothing: the listing
/// answers from the store rather than from anything the earlier pages held.
#[tokio::test]
async fn a_deleted_configuration_leaves_the_listing() {
    let router = emulator();
    let table = config_table();
    create_all(&router, &table).await;
    let order: Vec<String> = in_name_order(&table)
        .into_iter()
        .map(|row| row.name)
        .collect();

    let removed = order.first().cloned().expect("the table created rows");
    let deleted = delete_config(&router, Some(removed.as_str()), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete {removed}: expected HTTP 204 from DeleteRealtimeLogConfig, got {} with {}",
        deleted.status, deleted.raw
    );

    let answer = list_configs(&router, "").await;
    assert_eq!(
        listed_names(answer.root()),
        order[1..].to_vec(),
        "case after the delete: expected the remaining configurations, got {}",
        answer.raw
    );

    let resumed = list_configs(&router, &format!("?Marker={}", encode_query(&removed))).await;
    assert_eq!(
        resumed.status, 404,
        "case resume at a deleted name: expected HTTP 404, got {} with {}",
        resumed.status, resumed.raw
    );
    assert_eq!(
        resumed.error_code(),
        "NoSuchRealtimeLogConfig",
        "case resume at a deleted name: expected NoSuchRealtimeLogConfig, got {}",
        resumed.raw
    );
}
