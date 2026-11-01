// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetManagedPrefixListEntries`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, and the Region in that scope is the Region the service answers for, so
//! every call here carries one.
//!
//! The rules under test:
//!
//! 1. The operation REPORTS THE ENTRY SET OF THE ONE PREFIX LIST the request names, computed
//!    from what the service holds rather than from the request: the entries a create stored
//!    on a list of the account — each with its CIDR block in the canonical form the create
//!    stored and the description it was given — reported in ascending CIDR order whatever
//!    order they were named in. The rows of the entry table differ along what that
//!    generalises over: the address family, how many entries, whether an entry carries a
//!    description, whether a block was given in canonical form, and the order they were
//!    named in.
//! 2. The same report serves a prefix list Amazon Web Services publishes for a service of
//!    the Region: its entries are the ranges `DescribePrefixLists` reports for that list, so
//!    the two operations cannot disagree about one list's ranges, and a published list
//!    belongs to its OWN Region.
//! 3. A page is `MaxResults` entries long and carries a `NextToken` only while an entry
//!    follows it, so walking the tokens yields every entry of the list exactly once in that
//!    one order, whatever page size is asked for.
//! 4. `TargetVersion` selects the version the entries are reported for: the request that
//!    names none and the one that names the version the list stands at are answered with the
//!    same entries, and a version the list does not have is refused rather than answered
//!    with another version's entries.
//! 5. A request this operation cannot serve is REFUSED with EC2's code for that class and
//!    reports no entries: an id that is not a prefix list id, an id nothing the caller can
//!    see carries, a pagination token this service did not mint or minted for another list,
//!    and a page size outside the range the model publishes.
//! 6. Reporting entries is a READ: a `DryRun` answers 412 `DryRunOperation`, and asking
//!    twice — before and after a dry run — reports the same entries.

use ec2::state::{
    FIRST_PREFIX_LIST_VERSION, PREFIX_LIST_ADDRESS_FAMILY_IPV4, PREFIX_LIST_ADDRESS_FAMILY_IPV6,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};

/// The Region every call here is signed for unless the case asks for another.
const REGION: &str = "us-west-2";

/// A second Region, for the rule that a prefix list the Region PUBLISHES is a fact of that
/// Region: its name carries the Region, so its id names it there and nowhere else.
const OTHER_REGION: &str = "eu-west-1";

/// One entry of a create request: its CIDR block and, when it carries one, its description.
type Entry = (&'static str, Option<&'static str>);

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The form fields that ask a create for `entries`, as ec2Query flattens the `Entries` list
/// into the indexed `Entry.N` keys the EC2 API documents.
fn entry_params(entries: &[Entry]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (cidr, description)) in entries.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Entry.{position}.Cidr"), cidr));
        if let Some(description) = description {
            params.push(param(
                &format!("Entry.{position}.Description"),
                description,
            ));
        }
    }
    params
}

/// One prefix list of the account, created with `entries`; answers the id it was given.
async fn create_list(
    router: &MultiServiceRouter,
    case: &str,
    prefix_list_name: &str,
    address_family: &str,
    max_entries: usize,
    entries: &[Entry],
) -> String {
    let mut params = vec![
        param("PrefixListName", prefix_list_name),
        param("AddressFamily", address_family),
        param("MaxEntries", &max_entries.to_string()),
    ];
    params.extend(entry_params(entries));
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateManagedPrefixList", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} with body {body}"
    );
    element(&body, "prefixListId")
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no prefixListId: {body}"))
}

/// One `GetManagedPrefixListEntries` call signed for `region`, as its status and XML body.
async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "GetManagedPrefixListEntries", params).await
}

/// One `GetManagedPrefixListEntries` call in [`REGION`].
async fn get(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    get_in(router, REGION, params).await
}

/// The entries a successful answer reported, as `(cidr, description)` in the order it
/// reported them.
///
/// Read off the document tree rather than by substring, so a description is paired with the
/// entry it belongs to rather than with the one at the same position in a flat list of
/// elements, and an entry carrying no description is told apart from one carrying an empty
/// one.
fn reported_entries(case: &str, body: &str) -> Vec<(String, Option<String>)> {
    let document = super::wire::document(case, body);
    super::wire::set_items(document.root_element(), "entrySet")
        .into_iter()
        .map(|item| {
            (
                super::wire::child_text(item, "cidr").unwrap_or_default(),
                super::wire::child_text(item, "description"),
            )
        })
        .collect()
}

/// The `nextToken` of an answer's envelope: `None` when it carries none.
fn reported_token(body: &str) -> Option<String> {
    super::ec2_query::member(body, "nextToken")
}

/// The entries a successful answer reported, as `(cidr, description)`, having first pinned
/// the status.
async fn entries_of(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<(String, Option<String>)> {
    let (status, body) = get(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_entries(case, &body)
}

/// A row of the entry table as the response reports it.
fn expected_entries(entries: &[Entry]) -> Vec<(String, Option<String>)> {
    entries
        .iter()
        .map(|(cidr, description)| {
            (
                (*cidr).to_string(),
                description.map(|description| description.to_string()),
            )
        })
        .collect()
}

/// One row of the entry table.
struct EntryCase {
    name: &'static str,
    address_family: &'static str,
    /// The entries the create names, in the order it names them.
    given: Vec<Entry>,
    /// The entries the report must carry, in the order it must carry them: ascending CIDR
    /// block, each block in the canonical form the create stored.
    expected: Vec<Entry>,
}

/// The rows the entry table is written against: a different address family, a different
/// number of entries, an entry with and without a description, a block named with host bits
/// set, and entries named in an order that is not the reported one.
fn entry_cases() -> Vec<EntryCase> {
    vec![
        EntryCase {
            name: "a list created with no entry at all",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            given: Vec::new(),
            expected: Vec::new(),
        },
        EntryCase {
            name: "one IPv4 entry carrying a description",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            given: vec![("10.0.0.0/8", Some("the office range"))],
            expected: vec![("10.0.0.0/8", Some("the office range"))],
        },
        EntryCase {
            name: "three IPv4 entries named in descending order, two described",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            given: vec![
                ("192.168.1.0/24", Some("branch three")),
                ("172.16.0.0/20", None),
                ("10.1.0.0/16", Some("branch one")),
            ],
            expected: vec![
                ("10.1.0.0/16", Some("branch one")),
                ("172.16.0.0/20", None),
                ("192.168.1.0/24", Some("branch three")),
            ],
        },
        EntryCase {
            name: "an IPv4 block named with host bits set is reported as the range",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            given: vec![("198.51.100.37/26", Some("a host inside the range"))],
            expected: vec![("198.51.100.0/26", Some("a host inside the range"))],
        },
        EntryCase {
            name: "two IPv6 entries, one described",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV6,
            given: vec![
                ("2001:db8:2::/48", None),
                ("2001:db8:1:2::8/64", Some("documentation range")),
            ],
            expected: vec![
                ("2001:db8:1:2::/64", Some("documentation range")),
                ("2001:db8:2::/48", None),
            ],
        },
    ]
}

/// Rule 1: the entry set reported for a list of the account is the entries that list holds,
/// in ascending CIDR order, with the description each entry was given — and reporting them
/// twice reports the same thing.
#[tokio::test]
async fn the_reported_entry_set_is_the_lists_own_entries_in_cidr_order() {
    let (router, _engine) = super::ec2_query::emulator();

    for (index, case) in entry_cases().iter().enumerate() {
        let prefix_list_id = create_list(
            &router,
            case.name,
            &format!("entry-case-{index}"),
            case.address_family,
            // Room for every entry the row names, so the row's own entries are never what
            // the create refuses.
            case.given.len().max(1),
            &case.given,
        )
        .await;
        let params = vec![param("PrefixListId", &prefix_list_id)];

        let reported = entries_of(&router, case.name, &params).await;
        assert_eq!(
            reported,
            expected_entries(&case.expected),
            "case {:?}: expected the list's entries in CIDR order, got {reported:?}",
            case.name
        );

        // A read: asking again reports exactly the same entry set.
        let again = entries_of(&router, case.name, &params).await;
        assert_eq!(
            again, reported,
            "case {:?}: a second read must report the same entries, got {again:?} after \
             {reported:?}",
            case.name
        );

        // Every entry of the row fits one page, so the answer names no further page.
        let (_, body) = get(&router, &params).await;
        assert_eq!(
            reported_token(&body),
            None,
            "case {:?}: a whole entry set in one page must carry no nextToken, got {body}",
            case.name
        );
    }
}

/// Rule 4: `TargetVersion` names the version the entries are reported for. A created list
/// stands at its first version, so naming that version reports what naming none reports.
#[tokio::test]
async fn the_current_version_reports_what_naming_no_version_reports() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "TargetVersion naming the version the list stands at";

    let entries = vec![("10.2.0.0/16", Some("one")), ("10.3.0.0/16", None)];
    let prefix_list_id = create_list(
        &router,
        CASE,
        "versioned",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        entries.len(),
        &entries,
    )
    .await;

    let without = entries_of(&router, CASE, &[param("PrefixListId", &prefix_list_id)]).await;
    let with = entries_of(
        &router,
        CASE,
        &[
            param("PrefixListId", &prefix_list_id),
            param("TargetVersion", &FIRST_PREFIX_LIST_VERSION.to_string()),
        ],
    )
    .await;
    assert_eq!(
        with, without,
        "case {CASE:?}: expected the same entries as naming no version ({without:?}), got \
         {with:?}"
    );
    // Not vacuous: both answers carry the list's entries rather than nothing.
    assert_eq!(
        with,
        expected_entries(&entries),
        "case {CASE:?}: expected the list's own entries, got {with:?}"
    );
}

/// The prefix lists Region `region` publishes as `DescribePrefixLists` reports them, as
/// `(prefixListId, cidrs)` — the operation that reports a published list's ranges, so this
/// suite compares the entry report against that rather than against a catalog of its own.
async fn published_lists(router: &MultiServiceRouter, region: &str) -> Vec<(String, Vec<String>)> {
    let case = format!("DescribePrefixLists in {region}");
    let (status, body) = super::ec2_query::call(router, region, "DescribePrefixLists", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribePrefixLists must succeed, got {status} with body {body}"
    );
    let document = super::wire::document(&case, &body);
    let set = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "prefixListSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <prefixListSet> in {body}"));
    set.children()
        .filter(|node| node.is_element())
        .map(|item| {
            let id = super::wire::child_text(item, "prefixListId")
                .unwrap_or_else(|| panic!("case {case:?}: a reported list carries no id: {body}"));
            let cidrs = super::wire::set_items(item, "cidrSet")
                .into_iter()
                .map(|cidr| cidr.text().unwrap_or_default().to_string())
                .collect();
            (id, cidrs)
        })
        .collect()
}

/// Rule 2: the entries of a prefix list Amazon Web Services publishes are the ranges
/// `DescribePrefixLists` reports for that list, and a published list belongs to the Region
/// that publishes it.
#[tokio::test]
async fn a_published_list_reports_the_ranges_its_region_publishes() {
    let (router, _engine) = super::ec2_query::emulator();

    let here = published_lists(&router, REGION).await;
    assert!(
        !here.is_empty(),
        "case {REGION:?}: the Region must publish at least one prefix list to report entries \
         for"
    );

    for (prefix_list_id, cidrs) in &here {
        let case = format!("the published list {prefix_list_id}");
        let (status, body) = get_in(
            &router,
            REGION,
            &[param("PrefixListId", prefix_list_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        // One entry per published range, in ascending CIDR order, and no description: a
        // description is something an account writes on an entry of its own list.
        let mut expected: Vec<(String, Option<String>)> =
            cidrs.iter().map(|cidr| (cidr.clone(), None)).collect();
        expected.sort();
        let reported = reported_entries(&case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the ranges DescribePrefixLists reports, got {reported:?}"
        );
    }

    // A published list is a fact of its own Region: its id names it there, and in another
    // Region nothing carries that id.
    let (first_id, _) = here
        .first()
        .expect("the Region publishes at least one prefix list");
    let elsewhere = published_lists(&router, OTHER_REGION).await;
    assert!(
        !elsewhere.iter().any(|(id, _)| id == first_id),
        "another Region must not publish the list {first_id:?}, got {elsewhere:?}"
    );
    let case = format!("the published list {first_id} read in {OTHER_REGION}");
    let (status, body) = get_in(
        &router,
        OTHER_REGION,
        &[param("PrefixListId", first_id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPrefixListID.NotFound",
        "case {case:?}: expected the not-found code, got {body}"
    );
}

/// The entries a walk of the whole report yields with `max_results` as its page size, and
/// how many calls the walk took.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    prefix_list_id: &str,
    max_results: Option<usize>,
) -> (Vec<(String, Option<String>)>, usize) {
    let mut collected: Vec<(String, Option<String>)> = Vec::new();
    let mut token: Option<String> = None;
    let mut calls = 0usize;
    loop {
        let mut params = vec![param("PrefixListId", prefix_list_id)];
        if let Some(limit) = max_results {
            params.push(param("MaxResults", &limit.to_string()));
        }
        if let Some(carried) = &token {
            params.push(param("NextToken", carried));
        }
        let (status, body) = get(router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {calls} expected HTTP 200, got {status} with body {body}"
        );
        calls += 1;
        let page = reported_entries(case, &body);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit,
                "case {case:?}: page {calls} carries {} entries, above the page size of \
                 {limit}: {body}",
                page.len()
            );
        }
        collected.extend(page);
        token = reported_token(&body);
        if token.is_none() {
            return (collected, calls);
        }
        assert!(
            calls < 32,
            "case {case:?}: the walk did not end after {calls} pages"
        );
    }
}

/// Rule 3: a page is at most `MaxResults` entries long and names a further page only while
/// one follows, so a walk yields every entry of the list once, in the one order, whatever
/// page size it asks for.
#[tokio::test]
async fn paging_yields_every_entry_once_whatever_the_page_size() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "paging";

    // Five entries named out of order, so the walk's order is the report's rather than the
    // request's.
    let given: Vec<Entry> = vec![
        ("10.40.0.0/16", None),
        ("10.10.0.0/16", Some("ten")),
        ("10.50.0.0/16", Some("fifty")),
        ("10.20.0.0/16", None),
        ("10.30.0.0/16", Some("thirty")),
    ];
    let prefix_list_id = create_list(
        &router,
        CASE,
        "paged",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        given.len(),
        &given,
    )
    .await;
    let whole = entries_of(&router, CASE, &[param("PrefixListId", &prefix_list_id)]).await;
    assert_eq!(
        whole.len(),
        given.len(),
        "case {CASE:?}: expected {} entries in one answer, got {whole:?}",
        given.len()
    );

    // The page sizes the model's `PrefixListMaxResults` range admits, at both published
    // bounds (1 and 100), inside it, exactly the number of entries, and no page size at all.
    for max_results in [None, Some(1), Some(2), Some(5), Some(100)] {
        let case = format!("{CASE} with MaxResults {max_results:?}");
        let (walked, calls) = walk_pages(&router, &case, &prefix_list_id, max_results).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the whole entry set in the one order, got {walked:?}"
        );
        // A page size of N takes ceil(len / N) calls, and the last of them carries no token:
        // the walk above only returns once a page named no further page.
        let expected_calls = match max_results {
            None => 1,
            Some(limit) => whole.len().div_ceil(limit),
        };
        assert_eq!(
            calls, expected_calls,
            "case {case:?}: expected {expected_calls} call(s) to walk {} entries, took {calls}",
            whole.len()
        );
    }

    // A page size outside the range the model publishes (`PrefixListMaxResults`, 1..100) is
    // refused rather than served as the nearest page size that is inside it.
    for max_results in ["0", "101"] {
        let case = format!("{CASE} with MaxResults {max_results}");
        let (status, body) = get(
            &router,
            &[
                param("PrefixListId", &prefix_list_id),
                param("MaxResults", max_results),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}

/// Rule 5: a pagination token is only good for the report that minted it — a token this
/// service did not mint, and one minted while paging another prefix list, are both refused.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_minted_for() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "a token minted for another list";

    let first = create_list(
        &router,
        CASE,
        "token-source",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[("10.60.0.0/16", None), ("10.61.0.0/16", None)],
    )
    .await;
    let second = create_list(
        &router,
        CASE,
        "token-target",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[("10.70.0.0/16", None), ("10.71.0.0/16", None)],
    )
    .await;

    let (status, body) = get(
        &router,
        &[param("PrefixListId", &first), param("MaxResults", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {CASE:?}: the first page must succeed, got {status} with body {body}"
    );
    let token = reported_token(&body)
        .unwrap_or_else(|| panic!("case {CASE:?}: the first page must name a next page: {body}"));

    for (case, prefix_list_id, carried) in [
        (
            "a token this service did not mint",
            first.as_str(),
            "bm90LWEtdG9rZW4=".to_string(),
        ),
        (
            "a token minted while paging another list",
            second.as_str(),
            token.clone(),
        ),
    ] {
        let (status, body) = get(
            &router,
            &[
                param("PrefixListId", prefix_list_id),
                param("NextToken", &carried),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected the pagination-token code, got {body}"
        );
    }
}

/// One row of the refusal table: the case, the fields it sends, the status and the code.
type RefusalCase = (String, Vec<(String, String)>, u16, &'static str);

/// Rule 5 and rule 6: a request this operation cannot serve is refused with EC2's code for
/// that class, naming the value it refused, and reports no entry.
#[tokio::test]
async fn a_request_outside_what_the_operation_serves_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "refusals";

    let prefix_list_id = create_list(
        &router,
        CASE,
        "refusals",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        1,
        &[("10.80.0.0/16", Some("the only entry"))],
    )
    .await;

    let rows: Vec<RefusalCase> = vec![
        (
            "a value that is no resource id at all".to_string(),
            vec![param("PrefixListId", "the-office-list")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "an id of another kind of resource".to_string(),
            vec![param("PrefixListId", "vpc-0123456789abcdef0")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "a prefix list id whose suffix is not hexadecimal".to_string(),
            vec![param("PrefixListId", "pl-zzzzzzzz")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "a well-formed short id nothing the caller can see carries".to_string(),
            vec![param("PrefixListId", "pl-0a1b2c3d")],
            400,
            "InvalidPrefixListID.NotFound",
        ),
        (
            "a well-formed long id nothing the caller can see carries".to_string(),
            vec![param("PrefixListId", "pl-00112233445566778")],
            400,
            "InvalidPrefixListID.NotFound",
        ),
        (
            "a version past the one the list stands at".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("TargetVersion", "2"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a version below the first".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("TargetVersion", "0"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in &rows {
        let (status, body) = get(&router, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
        assert!(
            reported_entries(case, &body).is_empty(),
            "case {case:?}: a refusal must report no entry, got {body}"
        );
        // The message is built from the REQUEST: every value it refused is a value the
        // request sent, the prefix list id and the version alike.
        if *expected_status == 400 {
            for (_, refused) in params
                .iter()
                .filter(|(name, _)| name == "PrefixListId" || name == "TargetVersion")
            {
                assert!(
                    error_message(&body).contains(refused.as_str()),
                    "case {case:?}: expected the message to name {refused:?}, got {body}"
                );
            }
        }
    }

    // Rule 6: the dry run above read nothing — the entries are still the list's own.
    let reported = entries_of(&router, CASE, &[param("PrefixListId", &prefix_list_id)]).await;
    assert_eq!(
        reported,
        expected_entries(&[("10.80.0.0/16", Some("the only entry"))]),
        "case {CASE:?}: expected the list's one entry after the refusals, got {reported:?}"
    );
}
