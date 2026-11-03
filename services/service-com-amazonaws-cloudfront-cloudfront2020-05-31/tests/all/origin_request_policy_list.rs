// SPDX-License-Identifier: CC0-1.0

//! `ListOriginRequestPolicies`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones CloudFront states for this listing:
//!
//! * "Gets a list of origin request policies": one `OriginRequestPolicySummary` per stored
//!   policy, each carrying the `OriginRequestPolicy` the same record answers
//!   `GetOriginRequestPolicy` with, and the `Type` the policy was created under — "either
//!   `managed` (created by Amazon Web Services) or `custom` (created in this Amazon Web
//!   Services account)";
//! * "If the total number of items in the list exceeds the maximum that you specify, or the
//!   default maximum, the response is paginated. To get the next page of items, send a
//!   subsequent request that specifies the NextMarker value from the current response as the
//!   Marker value" — so `MaxItems` cuts the page and is echoed as asked for, `NextMarker` is
//!   present exactly when policies remain and names the id the page ended at, and a request
//!   resumes AFTER the policy its `Marker` names: walking the pages at ANY admissible page
//!   size enumerates every policy exactly once, in one order;
//! * "A filter to return only the specified kinds of origin request policies" selects by
//!   where a policy came from, so a filter that admits the policies created in this account
//!   lists all of them and one that admits only the policies Amazon Web Services creates
//!   lists the published catalog instead ([`crate::managed_policy_catalogs`] asserts that
//!   catalog's own content);
//! * a page size that names no page, and a `Marker` that is no resume point this service
//!   handed out, are refused with the errors the operation models and change nothing;
//! * a listing is a READ: it answers the same records again and mints nothing.
//!
//! The table rows differ along everything these rules generalise over: the number of
//! policies stored (none, one, several), the page size (one, several, the collection size,
//! past it and past the server's own ceiling), the resume point, the filter value, and the
//! shape of each stored configuration — which is the origin request policy lifecycle suite's
//! table, reused rather than copied, together with its create and get helpers and the
//! distribution lifecycle suite's wire harness.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::ops::types::
    ORIGIN_REQUEST_POLICY_TYPE_VALUES;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    MANAGED_ORIGIN_REQUEST_POLICIES, MAX_ITEMS, MAX_ITEMS_INVALID_MESSAGE,
    NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, emulator, is_minted_uuid, Answer, Node, API_VERSION};
use crate::origin_request_policy_lifecycle::{
    assert_config, create_policy, get_policy, policy_table, Policy,
};
use crate::response_headers_policy_lifecycle::text;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/origin-request-policy`, with `query` appended verbatim
/// (`?MaxItems=2&Marker=…&Type=custom`), from the operation's `http` binding.
async fn list_policies(router: &MultiServiceRouter, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-request-policy{query}"),
        None,
        None,
    )
    .await
}

/// The `OriginRequestPolicySummary` elements a page reports, in document order.
fn summaries(list: &Node) -> Vec<&Node> {
    list.child("Items")
        .map(|items| items.children_named("OriginRequestPolicySummary"))
        .unwrap_or_default()
}

/// The policy ids a page reports, in the order the page carries them.
fn listed_ids(list: &Node) -> Vec<String> {
    summaries(list)
        .into_iter()
        .map(|summary| {
            summary
                .text_at("OriginRequestPolicy/Id")
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The value every summary this store can report carries, from the model's enum
/// (`ops::types::ORIGIN_REQUEST_POLICY_TYPE_VALUES`, which
/// `smithy_query(query="shape",shape="OriginRequestPolicyType")` reports the same values
/// for): a policy created through this API is one "created in this Amazon Web Services
/// account".
const TYPE_CUSTOM: &str = "custom";

/// The other value the same enum admits: a policy "created by Amazon Web Services", which
/// no call of this API can put in an account.
const TYPE_MANAGED: &str = "managed";

/// Create every row of the shared table and report `(policy, id)` in creation order.
async fn create_all(router: &MultiServiceRouter, rows: &[Policy]) -> Vec<(Policy, String)> {
    let mut created: Vec<(Policy, String)> = Vec::new();
    for policy in rows {
        let answer = create_policy(router, &policy.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case {}: expected HTTP 201 from CreateOriginRequestPolicy, got {} with {}",
            policy.case, answer.status, answer.raw
        );
        let id = text(answer.root(), "Id", policy.case, "the create", &answer.raw);
        assert!(
            is_minted_uuid(&id),
            "case {}: expected a minted UUID id, got {id:?}",
            policy.case
        );
        created.push((policy.clone(), id));
    }
    created
}

/// The created policies in the one total order every page indexes into: the stored order
/// over ids.
fn in_id_order(created: &[(Policy, String)]) -> Vec<(Policy, String)> {
    let mut ordered = created.to_vec();
    ordered.sort_by(|left, right| left.1.cmp(&right.1));
    ordered
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// The model admits exactly the two kinds this suite distinguishes, so the filter rows
/// below span the whole enum rather than a chosen pair.
#[test]
fn the_filter_values_are_the_modelled_ones() {
    let mut modelled = ORIGIN_REQUEST_POLICY_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        modelled,
        vec![TYPE_CUSTOM, TYPE_MANAGED],
        "case the modelled enum: expected the two kinds this suite covers, got {:?}",
        ORIGIN_REQUEST_POLICY_TYPE_VALUES
    );
}

/// An account holding no origin request policies of its OWN lists none of its own: a page
/// with nothing in it, carrying no `Items` element at all, the default page size, and
/// nothing that says more remain. (The policies AWS published are a different kind, so the
/// filter that names this account's kind excludes them.)
#[tokio::test]
async fn an_account_holding_no_policies_lists_no_items() {
    let router = emulator();
    let case = "empty account";

    let listed = list_policies(&router, "?Type=custom").await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected HTTP 200 from ListOriginRequestPolicies, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some("0"),
        "case {case}: expected Quantity 0, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("MaxItems"),
        Some(MAX_ITEMS.to_string().as_str()),
        "case {case}: expected the default MaxItems {MAX_ITEMS}, got {}",
        listed.raw
    );
    assert!(
        listed.root().child("Items").is_none(),
        "case {case}: a page that lists nothing carries no Items element, got {}",
        listed.raw
    );
    assert!(
        listed.root().text_at("NextMarker").is_none(),
        "case {case}: expected no NextMarker when nothing remains, got {}",
        listed.raw
    );
}

/// The listing reports every stored policy — each summary carrying the `OriginRequestPolicy`
/// the same record answers a get with, under the kind the policy was created as.
#[tokio::test]
async fn a_listing_reports_every_stored_policy_as_this_account_created_it() {
    let router = emulator();
    let created = create_all(&router, &policy_table()).await;
    let expected = in_id_order(&created);

    let listed = list_policies(&router, "?Type=custom").await;
    assert_eq!(
        listed.status, 200,
        "case whole collection: expected HTTP 200, got {} with {}",
        listed.status, listed.raw
    );
    assert_eq!(
        listed_ids(listed.root()),
        expected
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<String>>(),
        "case whole collection: expected every stored id once, in order, got {}",
        listed.raw
    );
    assert_eq!(
        listed.root().text_at("Quantity"),
        Some(expected.len().to_string().as_str()),
        "case whole collection: expected Quantity to state the items served, got {}",
        listed.raw
    );

    for ((policy, id), summary) in expected.iter().zip(summaries(listed.root())) {
        let case = policy.case;
        let kind = summary
            .text_at("Type")
            .unwrap_or_else(|| panic!("case {case}: the summary carries no Type: {}", listed.raw));
        assert_eq!(
            kind, TYPE_CUSTOM,
            "case {case}: expected a policy created in this account to be listed as \
             {TYPE_CUSTOM}, got {kind:?}"
        );
        let policy_node = summary.at("OriginRequestPolicy").unwrap_or_else(|| {
            panic!(
                "case {case}: the summary carries no OriginRequestPolicy: {}",
                listed.raw
            )
        });
        assert_eq!(
            policy_node.text_at("Id"),
            Some(id.as_str()),
            "case {case}: expected the stored id {id}, got {}",
            listed.raw
        );
        let config = policy_node
            .at("OriginRequestPolicyConfig")
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the summary carries no OriginRequestPolicyConfig: {}",
                    listed.raw
                )
            });
        assert_config(config, policy, "the listing", &listed.raw);

        // The summary and the get answer from ONE record.
        let fetched = get_policy(&router, id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {}",
            fetched.status, fetched.raw
        );
        assert_eq!(
            policy_node.text_at("LastModifiedTime"),
            fetched.root().text_at("LastModifiedTime"),
            "case {case}: expected the listing's LastModifiedTime to be the get's {:?}, got {:?}",
            fetched.root().text_at("LastModifiedTime"),
            policy_node.text_at("LastModifiedTime")
        );
    }

    // A listing is a READ: the same request answers the same records again.
    let again = list_policies(&router, "?Type=custom").await;
    assert_eq!(
        listed_ids(again.root()),
        listed_ids(listed.root()),
        "case repeated listing: a read must not change what a later caller sees, got {}",
        again.raw
    );
}

/// Walking the pages at ANY admissible page size enumerates every policy exactly once, in
/// the stored order, with `MaxItems` echoed, `Quantity` stating the page's length and
/// `NextMarker` present exactly when policies remain.
#[tokio::test]
async fn walking_the_pages_enumerates_every_policy_exactly_once() {
    let router = emulator();
    let created = create_all(&router, &policy_table()).await;
    let order: Vec<String> = in_id_order(&created)
        .into_iter()
        .map(|(_, id)| id)
        .collect();

    let ceiling = usize::try_from(MAX_ITEMS).expect("the ceiling fits a usize");
    let sizes = [
        1usize,
        2,
        3,
        order.len() - 1,
        order.len(),
        order.len() + 1,
        ceiling,
        ceiling + 1,
    ];
    for size in sizes {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // At most one page per policy plus one, so a listing that never stops truncating
        // fails here rather than looping forever.
        for page in 0..=order.len() + 1 {
            let query = match &marker {
                Some(value) => format!("?MaxItems={size}&Type=custom&Marker={value}"),
                None => format!("?MaxItems={size}&Type=custom"),
            };
            let answer = list_policies(&router, &query).await;
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
            let ids = listed_ids(list);
            assert!(
                ids.len() <= size.min(ceiling),
                "case page size {size} page {page}: a page served {} items, got {}",
                ids.len(),
                answer.raw
            );
            assert_eq!(
                list.text_at("Quantity"),
                Some(ids.len().to_string().as_str()),
                "case page size {size} page {page}: expected Quantity to state the page's \
                 length, got {}",
                answer.raw
            );
            walked.extend(ids.iter().cloned());

            let next = list.text_at("NextMarker").map(str::to_string);
            let remaining = walked.len() < order.len();
            assert_eq!(
                next.is_some(),
                remaining,
                "case page size {size} page {page}: expected NextMarker present {remaining} \
                 after {} of {} policies, got {}",
                walked.len(),
                order.len(),
                answer.raw
            );
            match next {
                Some(value) => {
                    assert_eq!(
                        Some(value.as_str()),
                        ids.last().map(String::as_str),
                        "case page size {size} page {page}: expected NextMarker to name the id \
                         the page ended at, got {}",
                        answer.raw
                    );
                    marker = Some(value);
                }
                None => break,
            }
        }
        assert_eq!(
            walked, order,
            "case page size {size}: walking the pages must enumerate every policy exactly once, \
             in order"
        );
    }

    // A `Marker` naming the last stored policy resumes after it: nothing of this account's
    // kind is ordered there.
    let last = order.last().cloned().expect("the table created policies");
    let after_last = list_policies(&router, &format!("?Type=custom&Marker={last}")).await;
    assert_eq!(
        after_last.status, 200,
        "case resume at the last id: expected HTTP 200, got {} with {}",
        after_last.status, after_last.raw
    );
    assert!(
        listed_ids(after_last.root()).is_empty(),
        "case resume at the last id: nothing is ordered after it, got {}",
        after_last.raw
    );
}

/// The filter selects by where a policy came from: the kind this account's creates produce
/// lists all of them, at every page size, and the kind only Amazon Web Services creates
/// lists the catalog AWS published instead.
#[tokio::test]
async fn the_type_filter_selects_the_kind_of_policy_it_names() {
    let router = emulator();
    let created = create_all(&router, &policy_table()).await;
    let order: Vec<String> = in_id_order(&created)
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    // The published catalog, in the order Amazon publishes it — the order a listing serves
    // it in, ahead of the policies this account created.
    let published: Vec<String> = MANAGED_ORIGIN_REQUEST_POLICIES
        .iter()
        .map(|managed| managed.id.to_string())
        .collect();
    let both: Vec<String> = published.iter().chain(order.iter()).cloned().collect();

    // Rows: no filter at all, the kind this account created, and the kind AWS creates —
    // each at a page size that cuts the collection and one that does not.
    let rows: &[(&str, Option<&str>, usize)] = &[
        ("no filter, whole collection", None, both.len()),
        ("no filter, a cut page", None, 2),
        ("the account's own kind, whole collection", Some(TYPE_CUSTOM), order.len()),
        ("the account's own kind, a cut page", Some(TYPE_CUSTOM), 2),
        ("the Amazon Web Services kind, whole collection", Some(TYPE_MANAGED), published.len()),
        ("the Amazon Web Services kind, a cut page", Some(TYPE_MANAGED), 2),
    ];
    for (case, filter, size) in rows {
        let query = match filter {
            Some(kind) => format!("?MaxItems={size}&Type={kind}"),
            None => format!("?MaxItems={size}"),
        };
        let answer = list_policies(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 for {query}, got {} with {}",
            answer.status, answer.raw
        );
        let source = match *filter {
            Some(TYPE_MANAGED) => &published,
            Some(TYPE_CUSTOM) => &order,
            _ => &both,
        };
        let expected: Vec<String> = source.iter().take(*size).cloned().collect();
        assert_eq!(
            listed_ids(answer.root()),
            expected,
            "case {case}: expected {expected:?} for {query}, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().text_at("Quantity"),
            Some(expected.len().to_string().as_str()),
            "case {case}: expected Quantity {} for {query}, got {}",
            expected.len(),
            answer.raw
        );
        if expected.is_empty() {
            assert!(
                answer.root().child("Items").is_none(),
                "case {case}: a page that lists nothing carries no Items element, got {}",
                answer.raw
            );
            assert!(
                answer.root().text_at("NextMarker").is_none(),
                "case {case}: expected no NextMarker when nothing is listed, got {}",
                answer.raw
            );
        }
    }

    // A filter value outside the model's enum names no kind of policy, so it is refused
    // rather than served.
    for kind in ["Custom", "CUSTOM", "aws-managed", ""] {
        let answer = list_policies(&router, &format!("?Type={kind}")).await;
        assert_eq!(
            answer.status, 400,
            "case filter {kind:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// A page size that names no page, and a `Marker` that is no resume point this service
/// handed out, are refused with the errors the operation models and store nothing.
#[tokio::test]
async fn an_unusable_page_size_or_marker_is_refused() {
    let router = emulator();
    let created = create_all(&router, &policy_table()[..3]).await;
    let before = listed_ids(list_policies(&router, "").await.root());

    // Rows: every form a page size can be unusable in (zero, negative, no integer at all,
    // past what the member can hold), and every form a marker can fail the id shape in (no
    // id at all, too short, too long, upper-case hex, another namespace's token,
    // characters outside hex) — plus an unusable size beside a usable marker, where the
    // size is what is refused.
    let last = before.last().cloned().expect("the fixture created policies");
    let rows: &[(&str, String, bool)] = &[
        ("a page of no items", "?MaxItems=0".to_string(), false),
        ("a negative page size", "?MaxItems=-1".to_string(), false),
        (
            "a page size that is no integer",
            "?MaxItems=not-a-number".to_string(),
            false,
        ),
        (
            "a page size past what the member can hold",
            "?MaxItems=99999999999".to_string(),
            false,
        ),
        ("a marker that is no id at all", "?Marker=nope".to_string(), true),
        (
            "a marker too short to be an id",
            "?Marker=0000-0000-0000-0000".to_string(),
            true,
        ),
        (
            "a marker too long to be an id",
            "?Marker=000000000-0000-0000-0000-000000000000".to_string(),
            true,
        ),
        (
            "a marker in upper-case hex",
            "?Marker=216ADEF6-5C7F-47E4-B989-5492EAFA07D3".to_string(),
            true,
        ),
        (
            "a marker from another resource's namespace",
            "?Marker=E1AAAAAAAAAAAA".to_string(),
            true,
        ),
        (
            "a marker carrying characters outside hex",
            "?Marker=zzzzzzzz-0000-0000-0000-000000000000".to_string(),
            true,
        ),
        (
            "an unusable page size beside a usable marker",
            format!("?MaxItems=0&Marker={last}"),
            false,
        ),
    ];
    for (case, query, over_marker) in rows {
        let answer = list_policies(&router, query).await;
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
        if !over_marker {
            // What the refusal states is the CONSTRAINT on the parameter, so every
            // unusable value is refused with that one sentence.
            let message = answer.root().find_text("Message").unwrap_or_default();
            assert_eq!(
                message, MAX_ITEMS_INVALID_MESSAGE,
                "case {case}: expected the page-size constraint stated for {query}, got {}",
                answer.raw
            );
        }
    }

    // A marker with the shape of an id this service mints, naming no stored policy, is a
    // policy that does not exist.
    for marker in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ] {
        let answer = list_policies(&router, &format!("?Marker={marker}")).await;
        assert_eq!(
            answer.status, 404,
            "case unknown marker {marker}: expected HTTP 404, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchOriginRequestPolicy",
            "case unknown marker {marker}: expected NoSuchOriginRequestPolicy, got {}",
            answer.raw
        );
        assert_eq!(
            answer.root().find_text("Message").unwrap_or_default(),
            NO_SUCH_ORIGIN_REQUEST_POLICY_MESSAGE,
            "case unknown marker {marker}: expected the modelled sentence, got {}",
            answer.raw
        );
    }

    // Every refusal above left the store exactly as it was.
    let listed = list_policies(&router, "").await;
    assert_eq!(
        listed_ids(listed.root()),
        before,
        "case refusals: a refused listing must store nothing, got {}",
        listed.raw
    );
    assert_eq!(
        created.len() + MANAGED_ORIGIN_REQUEST_POLICIES.len(),
        before.len(),
        "case refusals: expected the created policies and the published catalog to be the \
         listed ones"
    );
}
