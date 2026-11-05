// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyManagedPrefixList`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential scope,
//! and the Region in that scope is the Region the service answers for, so every call here
//! carries one.
//!
//! The rules under test:
//!
//! 1. A revision of the ENTRY SET is computed from the entry set the list HOLDS: the removals
//!    the request names come out, the additions go in — each block in the canonical form a
//!    prefix list stores — and the list is answered in the non-terminal `modify-in-progress`
//!    state the model publishes. Every row of the revision table is a distinct input along
//!    what that generalises over: how many entries are added, how many are removed, both at
//!    once, a range removed and added back with another description, the address family, and a
//!    block named with host bits set.
//! 2. "Adding or removing entries in a prefix list creates a new version of the prefix list":
//!    the revised entry set belongs to the NEXT version, and the version it replaced still
//!    reports ITS OWN entries when `GetManagedPrefixListEntries` is asked for it.
//! 3. "Changing the name of the prefix list does not affect the version": a request that
//!    renames the list, or that changes its maximum size, leaves the version where it stands
//!    and is still the modification it asked for.
//! 4. The clock — not a later call — moves a revised list to `modify-complete`, so a caller
//!    reading straight after the revision still sees `modify-in-progress`.
//! 5. A request this operation cannot serve is REFUSED with EC2's code for that class and
//!    changes nothing: an id that is no prefix list id, an id nothing the caller can see
//!    carries, a list Amazon Web Services publishes rather than the account owning, a request
//!    naming no modification at all, a name outside the documented constraints, a maximum size
//!    asked for together with an entry change or outside the documented floor, an entry change
//!    that would take the list past its maximum, a CIDR block of the other family, a block the
//!    list already carries, a block it does not carry, a `CurrentVersion` that is not the
//!    version the list stands at, a list whose creation is still in progress, and the
//!    synchronization flag on a list no IPAM prefix list resolver target manages.
//! 6. A `DryRun` answers 412 `DryRunOperation` and changes nothing at all: not the version,
//!    not the entries, not the name.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, FIRST_PREFIX_LIST_VERSION,
    MAX_PREFIX_LIST_NAME_CHARS, PREFIX_LIST_ADDRESS_FAMILY_IPV4, PREFIX_LIST_ADDRESS_FAMILY_IPV6,
    PREFIX_LIST_STATE_CREATE_COMPLETE, PREFIX_LIST_STATE_CREATE_IN_PROGRESS,
    RESERVED_PREFIX_LIST_NAME_PREFIX, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The state a prefix list is in while the revision a modify started is still owed, and the
/// one the clock settles it into — the two words of the modelled `PrefixListState` catalog a
/// revision moves a list through
/// (`smithy_query(query="shape",shape="PrefixListState")`).
const MODIFY_IN_PROGRESS: &str = "modify-in-progress";
const MODIFY_COMPLETE: &str = "modify-complete";

/// Long enough past [`TRANSITION_DELAY`] that a transition the clock owes is due.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// One entry of a request: its CIDR block and, when it carries one, its description.
type Entry = (&'static str, Option<&'static str>);

/// One form field of an ec2Query body.
type Field = (String, String);

/// One row of the table of modifications that are NOT an entry change: the case, the fields it
/// sends, and the name and the maximum size the list must report once it has been applied.
type AttributeRow = (&'static str, Vec<Field>, &'static str, &'static str);

/// One row of the refusal table: the case, the fields it sends, and the HTTP status and EC2
/// code it must be refused with.
type RefusalRow = (String, Vec<Field>, u16, &'static str);

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The form fields that name `entries` under the indexed key `member`, as ec2Query flattens a
/// list of entries into the `<Member>.N.<Field>` keys the EC2 API documents (`Entry` for a
/// create's `Entries`, `AddEntry` for a modify's `AddEntries`).
fn entry_params(member: &str, entries: &[Entry]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (cidr, description)) in entries.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("{member}.{position}.Cidr"), cidr));
        if let Some(description) = description {
            params.push(param(
                &format!("{member}.{position}.Description"),
                description,
            ));
        }
    }
    params
}

/// The form fields that name `cidrs` as the entries a modify removes.
fn remove_params(cidrs: &[&str]) -> Vec<(String, String)> {
    cidrs
        .iter()
        .enumerate()
        .map(|(index, cidr)| param(&format!("RemoveEntry.{}.Cidr", index + 1), cidr))
        .collect()
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
    params.extend(entry_params("Entry", entries));
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateManagedPrefixList", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must succeed, got {status} with body {body}"
    );
    element(&body, "prefixListId")
        .unwrap_or_else(|| panic!("case {case:?}: the create reported no prefixListId: {body}"))
}

/// One `ModifyManagedPrefixList` call carrying `params`, as its HTTP status and XML body.
async fn modify(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "ModifyManagedPrefixList", params).await
}

/// One `DescribeManagedPrefixLists` call narrowed to `prefix_list_id`, as its XML body.
async fn describe_one(router: &MultiServiceRouter, case: &str, prefix_list_id: &str) -> String {
    let (status, body) = super::ec2_query::call(
        router,
        REGION,
        "DescribeManagedPrefixLists",
        &[param("PrefixListId.1", prefix_list_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status} with body {body}"
    );
    body
}

/// The entries a `GetManagedPrefixListEntries` answer reports, as `(cidr, description)` in the
/// order it reported them.
///
/// Read off the document tree rather than by substring, so a description is paired with the
/// entry it belongs to rather than with the one at the same position in a flat list of
/// elements.
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

/// The entries the list `prefix_list_id` reports for `target_version` — the version it stands
/// at when the row names none.
async fn entries_at(
    router: &MultiServiceRouter,
    case: &str,
    prefix_list_id: &str,
    target_version: Option<i64>,
) -> Vec<(String, Option<String>)> {
    let mut params = vec![param("PrefixListId", prefix_list_id)];
    if let Some(version) = target_version {
        params.push(param("TargetVersion", &version.to_string()));
    }
    let (status, body) =
        super::ec2_query::call(router, REGION, "GetManagedPrefixListEntries", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the entry report must succeed, got {status} with body {body}"
    );
    reported_entries(case, &body)
}

/// A row of a table as the entry report answers it: ascending CIDR block, each block in the
/// canonical form the service stores.
fn expected_entries(entries: &[Entry]) -> Vec<(String, Option<String>)> {
    let mut expected: Vec<(String, Option<String>)> = entries
        .iter()
        .map(|(cidr, description)| {
            (
                (*cidr).to_string(),
                description.map(|description| description.to_string()),
            )
        })
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    expected
}

/// One row of the revision table.
struct RevisionCase {
    name: &'static str,
    address_family: &'static str,
    /// The entries the list is created with.
    created: Vec<Entry>,
    /// The maximum size it is created with.
    max_entries: usize,
    /// The entries the revision adds.
    added: Vec<Entry>,
    /// The CIDR blocks the revision removes.
    removed: Vec<&'static str>,
    /// The entry set the revised list must hold, in any order.
    expected: Vec<Entry>,
}

/// The rows the revision table is written against: an addition to a list holding nothing, two
/// additions at once, a removal, an addition and a removal in one call, a range removed and
/// added back carrying another description, and an IPv6 list whose added block is named with
/// host bits set.
fn revision_cases() -> Vec<RevisionCase> {
    vec![
        RevisionCase {
            name: "one entry added to a list holding none",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            created: Vec::new(),
            max_entries: 4,
            added: vec![("10.0.0.0/8", Some("the office range"))],
            removed: Vec::new(),
            expected: vec![("10.0.0.0/8", Some("the office range"))],
        },
        RevisionCase {
            name: "two entries added, one of them described",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            created: vec![("10.1.0.0/16", None)],
            max_entries: 4,
            added: vec![
                ("172.16.0.0/20", Some("branch two")),
                ("192.168.1.0/24", None),
            ],
            removed: Vec::new(),
            expected: vec![
                ("10.1.0.0/16", None),
                ("172.16.0.0/20", Some("branch two")),
                ("192.168.1.0/24", None),
            ],
        },
        RevisionCase {
            name: "one entry removed from a list holding three",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            created: vec![
                ("10.2.0.0/16", Some("one")),
                ("10.3.0.0/16", None),
                ("10.4.0.0/16", Some("three")),
            ],
            max_entries: 3,
            added: Vec::new(),
            removed: vec!["10.3.0.0/16"],
            expected: vec![("10.2.0.0/16", Some("one")), ("10.4.0.0/16", Some("three"))],
        },
        RevisionCase {
            name: "one entry added and another removed in the same call",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            created: vec![("10.5.0.0/16", None)],
            max_entries: 2,
            added: vec![("10.6.0.0/16", Some("the new range"))],
            removed: vec!["10.5.0.0/16"],
            expected: vec![("10.6.0.0/16", Some("the new range"))],
        },
        RevisionCase {
            name: "a range removed and added back carrying another description",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV4,
            created: vec![("10.7.0.0/16", Some("the old description"))],
            max_entries: 1,
            added: vec![("10.7.0.0/16", Some("the new description"))],
            removed: vec!["10.7.0.0/16"],
            expected: vec![("10.7.0.0/16", Some("the new description"))],
        },
        RevisionCase {
            name: "an IPv6 block named with host bits set is stored as the range",
            address_family: PREFIX_LIST_ADDRESS_FAMILY_IPV6,
            created: vec![("2001:db8:1::/48", None)],
            max_entries: 2,
            added: vec![("2001:db8:2:3::9/64", Some("documentation range"))],
            removed: Vec::new(),
            expected: vec![
                ("2001:db8:1::/48", None),
                ("2001:db8:2:3::/64", Some("documentation range")),
            ],
        },
    ]
}

/// The form fields a revision row sends.
fn revision_params(prefix_list_id: &str, case: &RevisionCase) -> Vec<(String, String)> {
    let mut params = vec![param("PrefixListId", prefix_list_id)];
    params.extend(entry_params("AddEntry", &case.added));
    params.extend(remove_params(&case.removed));
    params
}

/// Rules 1 and 2: the revised entry set is the one the list held with the request's removals
/// out and its additions in, it belongs to the NEXT version, and the version it replaced still
/// reports its own entries.
#[tokio::test]
async fn a_revision_computes_the_next_versions_entry_set_from_the_one_the_list_holds() {
    let (router, _engine) = super::ec2_query::emulator();

    // Every row's list first, then ONE wait for the creations the clock owes: a list whose
    // creation is still in progress is not one a revision may start from, which rule 5 asserts
    // in its own right.
    let mut lists = Vec::new();
    for (index, case) in revision_cases().into_iter().enumerate() {
        let prefix_list_id = create_list(
            &router,
            case.name,
            &format!("revision-case-{index}"),
            case.address_family,
            case.max_entries,
            &case.created,
        )
        .await;
        lists.push((case, prefix_list_id));
    }
    tokio::time::sleep(past_the_transition()).await;

    for (case, prefix_list_id) in &lists {
        let (status, body) = modify(&router, &revision_params(prefix_list_id, case)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "prefixListId").as_deref(),
            Some(prefix_list_id.as_str()),
            "case {name:?}: expected the list the request named, got {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "version"),
            Some((FIRST_PREFIX_LIST_VERSION + 1).to_string()),
            "case {name:?}: expected the next version, got {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(MODIFY_IN_PROGRESS),
            "case {name:?}: expected {MODIFY_IN_PROGRESS}, got {body}",
            name = case.name
        );
        assert_eq!(
            element(&body, "maxEntries"),
            Some(case.max_entries.to_string()),
            "case {name:?}: a revision of the entries must leave the maximum size alone, got \
             {body}",
            name = case.name
        );

        // Rule 1: the entry set the revision left behind, read back through the operation that
        // reports a prefix list's entries.
        let reported = entries_at(&router, case.name, prefix_list_id, None).await;
        assert_eq!(
            reported,
            expected_entries(&case.expected),
            "case {name:?}: expected the revised entry set, got {reported:?}",
            name = case.name
        );

        // Rule 2: the version the revision superseded still reports ITS entries, not the ones
        // that replaced them.
        let previous = entries_at(
            &router,
            case.name,
            prefix_list_id,
            Some(FIRST_PREFIX_LIST_VERSION),
        )
        .await;
        assert_eq!(
            previous,
            expected_entries(&case.created),
            "case {name:?}: expected the superseded version's own entries, got {previous:?}",
            name = case.name
        );

        // The version the list stands at, named explicitly, reports what the default does.
        let current = entries_at(
            &router,
            case.name,
            prefix_list_id,
            Some(FIRST_PREFIX_LIST_VERSION + 1),
        )
        .await;
        assert_eq!(
            current, reported,
            "case {name:?}: naming the current version must report the current entries, got \
             {current:?}",
            name = case.name
        );
    }
}

/// Rule 3: renaming the list, and changing its maximum size, are modifications that leave the
/// version where it stands.
#[tokio::test]
async fn a_rename_and_a_resize_leave_the_version_where_it_stands() {
    let (router, _engine) = super::ec2_query::emulator();

    let prefix_list_id = create_list(
        &router,
        "a rename",
        "the-first-name",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        4,
        &[("10.8.0.0/16", Some("kept"))],
    )
    .await;
    tokio::time::sleep(past_the_transition()).await;

    // Each row is one modification that is not an entry change, with what it must leave the
    // list reporting.
    let rows: Vec<AttributeRow> = vec![
        (
            "a new name",
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", "the-second-name"),
            ],
            "the-second-name",
            "4",
        ),
        (
            "a larger maximum size",
            vec![
                param("PrefixListId", &prefix_list_id),
                param("MaxEntries", "9"),
            ],
            "the-second-name",
            "9",
        ),
        (
            "a name and a maximum size at once",
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", "the-third-name"),
                param("MaxEntries", "7"),
            ],
            "the-third-name",
            "7",
        ),
    ];

    for (case, params, expected_name, expected_max) in &rows {
        let (status, body) = modify(&router, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "version"),
            Some(FIRST_PREFIX_LIST_VERSION.to_string()),
            "case {case:?}: a modification that is not an entry change must not move the \
             version, got {body}"
        );
        assert_eq!(
            element(&body, "prefixListName").as_deref(),
            Some(*expected_name),
            "case {case:?}: expected the name the request named, got {body}"
        );
        assert_eq!(
            element(&body, "maxEntries").as_deref(),
            Some(*expected_max),
            "case {case:?}: expected the maximum size the list stands at, got {body}"
        );
        // The entries are untouched by any of these, and still belong to the one version.
        let reported = entries_at(&router, case, &prefix_list_id, None).await;
        assert_eq!(
            reported,
            expected_entries(&[("10.8.0.0/16", Some("kept"))]),
            "case {case:?}: expected the entries to be left alone, got {reported:?}"
        );
        // A revision has to be able to follow one: the state the clock owes is settled first.
        tokio::time::sleep(past_the_transition()).await;
    }
}

/// Rule 4: the clock — not a later call — settles a revised list into `modify-complete`.
#[tokio::test]
async fn the_clock_settles_a_revised_list_into_modify_complete() {
    let (router, _engine) = super::ec2_query::emulator();

    let prefix_list_id = create_list(
        &router,
        "the clock",
        "settling-list",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[],
    )
    .await;
    tokio::time::sleep(past_the_transition()).await;
    let created = describe_one(&router, "before the revision", &prefix_list_id).await;
    assert_eq!(
        element(&created, "state").as_deref(),
        Some(PREFIX_LIST_STATE_CREATE_COMPLETE),
        "before the revision: expected {PREFIX_LIST_STATE_CREATE_COMPLETE}, got {created}"
    );

    let (status, body) = modify(
        &router,
        &[
            param("PrefixListId", &prefix_list_id),
            param("AddEntry.1.Cidr", "10.9.0.0/16"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the revision: expected HTTP 200, got {status} with body {body}"
    );
    let straight_after = describe_one(&router, "straight after the revision", &prefix_list_id).await;
    assert_eq!(
        element(&straight_after, "state").as_deref(),
        Some(MODIFY_IN_PROGRESS),
        "straight after the revision: expected {MODIFY_IN_PROGRESS}, got {straight_after}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let settled = describe_one(&router, "past the transition", &prefix_list_id).await;
    assert_eq!(
        element(&settled, "state").as_deref(),
        Some(MODIFY_COMPLETE),
        "past the transition: expected {MODIFY_COMPLETE}, got {settled}"
    );
    assert_eq!(
        element(&settled, "version"),
        Some((FIRST_PREFIX_LIST_VERSION + 1).to_string()),
        "past the transition: the version the revision minted must stand, got {settled}"
    );
}

/// Rule 5: a request this operation cannot serve is refused with EC2's code for that class and
/// changes nothing.
#[tokio::test]
async fn a_request_the_operation_cannot_serve_is_refused_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let prefix_list_id = create_list(
        &router,
        "the refusal rows",
        "refusal-list",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[("10.10.0.0/16", Some("held")), ("10.20.0.0/16", None)],
    )
    .await;
    // The creation the clock owes is settled first: a list still being created is refused for
    // its STATE whatever else the row names, which is the case asserted on its own next.
    tokio::time::sleep(past_the_transition()).await;

    // A list whose creation is still owed to the clock has no settled entry set for a revision
    // to start from: created and revised with no wait in between, so the state it is refused
    // for is the one its create answered.
    let still_creating_id = create_list(
        &router,
        "a list still being created",
        "creating-list",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[],
    )
    .await;
    let (status, body) = modify(
        &router,
        &[
            param("PrefixListId", &still_creating_id),
            param("PrefixListName", "renamed"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a list still being created: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a list still being created: expected IncorrectState, got {body}"
    );
    assert!(
        error_message(&body).contains(PREFIX_LIST_STATE_CREATE_IN_PROGRESS),
        "a list still being created: expected the message to name the state it is in, got {body}"
    );

    // A prefix list Amazon Web Services publishes for a service of this Region: visible to the
    // caller, and not the account's to revise.
    let (status, published_body) =
        super::ec2_query::call(&router, REGION, "DescribePrefixLists", &[]).await;
    assert_eq!(
        status, 200,
        "the published lists must be reported, got {status} with body {published_body}"
    );
    let published_id = elements(&published_body, "prefixListId")
        .into_iter()
        .next()
        .expect("the Region must publish a prefix list for the AWS-managed row");

    let too_long_name = "n".repeat(MAX_PREFIX_LIST_NAME_CHARS + 1);
    let reserved_name = format!("{RESERVED_PREFIX_LIST_NAME_PREFIX}.us-west-2.s3");
    let rows: Vec<RefusalRow> = vec![
        (
            "an id that is no prefix list id".to_string(),
            vec![
                param("PrefixListId", "vpc-0a1b2c3d"),
                param("PrefixListName", "renamed"),
            ],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "a well-formed id nothing the caller can see carries".to_string(),
            vec![
                param("PrefixListId", "pl-0a1b2c3d"),
                param("PrefixListName", "renamed"),
            ],
            400,
            "InvalidPrefixListID.NotFound",
        ),
        (
            "a prefix list Amazon Web Services publishes".to_string(),
            vec![
                param("PrefixListId", &published_id),
                param("PrefixListName", "renamed"),
            ],
            400,
            "UnsupportedOperation",
        ),
        (
            "a request naming no modification at all".to_string(),
            vec![param("PrefixListId", &prefix_list_id)],
            400,
            "MissingParameter",
        ),
        (
            "an empty name".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", ""),
            ],
            400,
            "MissingParameter",
        ),
        (
            "a name one character past the documented bound".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", &too_long_name),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a name in the namespace the published lists are named in".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", &reserved_name),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a maximum size asked for together with an entry change".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("MaxEntries", "5"),
                param("AddEntry.1.Cidr", "10.11.0.0/16"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "a maximum size below the documented floor".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("MaxEntries", "0"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a maximum size below the entries the list holds".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("MaxEntries", "1"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "entries that would take the list past its maximum".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("AddEntry.1.Cidr", "10.12.0.0/16"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a block of the other address family".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("AddEntry.1.Cidr", "2001:db8::/32"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a value that is no CIDR block at all".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("AddEntry.1.Cidr", "not-a-block"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a block the list already carries".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("AddEntry.1.Cidr", "10.10.0.0/16"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a block the list does not carry".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("RemoveEntry.1.Cidr", "10.14.0.0/16"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one range named twice in the additions".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("AddEntry.1.Cidr", "10.15.0.0/16"),
                param("AddEntry.2.Cidr", "10.15.0.0/16"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a current version that is not the one the list stands at".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("CurrentVersion", "7"),
                param("PrefixListName", "renamed"),
            ],
            400,
            "IncorrectState",
        ),
        (
            "the synchronization flag on a list no resolver target manages".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("IpamPrefixListResolverSyncEnabled", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("PrefixListName", "renamed"),
                param("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in &rows {
        let (status, body) = modify(&router, params).await;
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
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {body}"
        );
        // Nothing the rows asked for was applied: the list still carries the entry, the name
        // and the version it was created with.
        let stored = engine.state::<Ec2State>();
        let guard = stored.lock();
        let record = guard
            .managed_prefix_lists
            .get(&prefix_list_id)
            .unwrap_or_else(|| panic!("case {case:?}: the list under test must still be stored"));
        assert_eq!(
            record.view.prefix_list_name.as_deref(),
            Some("refusal-list"),
            "case {case:?}: a refused request must not rename the list, got {name:?}",
            name = record.view.prefix_list_name
        );
        assert_eq!(
            record.view.version,
            Some(FIRST_PREFIX_LIST_VERSION),
            "case {case:?}: a refused request must not move the version, got {version:?}",
            version = record.view.version
        );
        assert_eq!(
            record.view.max_entries,
            Some(2),
            "case {case:?}: a refused request must not resize the list, got {max:?}",
            max = record.view.max_entries
        );
        let stored_blocks: Vec<Option<String>> = record
            .entries
            .iter()
            .map(|entry| entry.cidr.clone())
            .collect();
        assert_eq!(
            stored_blocks,
            vec![
                Some("10.10.0.0/16".to_string()),
                Some("10.20.0.0/16".to_string())
            ],
            "case {case:?}: a refused request must not touch the entries, got {stored_blocks:?}"
        );
    }
}

/// Rule 6: a dry run that would have succeeded changes nothing, and the revision that follows
/// it is the one that does.
#[tokio::test]
async fn a_dry_run_changes_nothing_and_the_revision_after_it_does() {
    let (router, _engine) = super::ec2_query::emulator();

    let prefix_list_id = create_list(
        &router,
        "a dry run",
        "dry-run-list",
        PREFIX_LIST_ADDRESS_FAMILY_IPV4,
        2,
        &[("10.16.0.0/16", None)],
    )
    .await;
    tokio::time::sleep(past_the_transition()).await;

    let (status, body) = modify(
        &router,
        &[
            param("PrefixListId", &prefix_list_id),
            param("AddEntry.1.Cidr", "10.17.0.0/16"),
            param("DryRun", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 412,
        "a dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run: expected DryRunOperation, got {body}"
    );
    let after_dry_run = entries_at(&router, "after the dry run", &prefix_list_id, None).await;
    assert_eq!(
        after_dry_run,
        expected_entries(&[("10.16.0.0/16", None)]),
        "after the dry run: expected the entries the create stored, got {after_dry_run:?}"
    );
    let described = describe_one(&router, "after the dry run", &prefix_list_id).await;
    assert_eq!(
        element(&described, "version"),
        Some(FIRST_PREFIX_LIST_VERSION.to_string()),
        "after the dry run: the version must stand where it was, got {described}"
    );

    let (status, applied) = modify(
        &router,
        &[
            param("PrefixListId", &prefix_list_id),
            param("AddEntry.1.Cidr", "10.17.0.0/16"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the revision after the dry run: expected HTTP 200, got {status} with body {applied}"
    );
    let reported = entries_at(&router, "after the revision", &prefix_list_id, None).await;
    assert_eq!(
        reported,
        expected_entries(&[("10.16.0.0/16", None), ("10.17.0.0/16", None)]),
        "after the revision: expected both entries, got {reported:?}"
    );
}

/// Rule 5, the WORDING of one refusal class: an id no prefix list the caller can see carries
/// is refused `Prefix List <id> not found.` — the fixed text EC2 words
/// `InvalidPrefixListID.NotFound` with, carrying only the id the request named, and the same
/// from every operation that resolves a prefix list id.
#[tokio::test]
async fn an_unresolvable_prefix_list_id_is_refused_with_one_wording() {
    let (router, _engine) = super::ec2_query::emulator();

    // Both id forms EC2 mints for a prefix list — the short one and the long one — neither of
    // which this emulator holds a list under.
    for unknown in ["pl-0a1b2c3d", "pl-00000000000000000"] {
        let expected = format!("Prefix List {unknown} not found.");
        let calls: Vec<(&str, Vec<Field>)> = vec![
            (
                "ModifyManagedPrefixList",
                vec![
                    param("PrefixListId", unknown),
                    param("PrefixListName", "renamed"),
                ],
            ),
            (
                "GetManagedPrefixListEntries",
                vec![param("PrefixListId", unknown)],
            ),
            (
                "DescribeManagedPrefixLists",
                vec![param("PrefixListId.1", unknown)],
            ),
            (
                "DescribePrefixLists",
                vec![param("PrefixListId.1", unknown)],
            ),
        ];
        for (action, params) in &calls {
            let (status, body) = super::ec2_query::call(&router, REGION, action, params).await;
            assert_eq!(
                status, 400,
                "case {action:?} with {unknown:?}: expected HTTP 400, got {status} with body \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidPrefixListID.NotFound",
                "case {action:?} with {unknown:?}: expected InvalidPrefixListID.NotFound, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {action:?} with {unknown:?}: expected the message {expected:?}, got {actual:?}",
                actual = error_message(&body)
            );
        }
    }
}
