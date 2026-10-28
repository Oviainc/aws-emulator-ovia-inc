// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the ACCOUNT a Domain listing is scoped to.
//!
//! The RULE under test, asserted over tables rather than one recorded page: `ListDomains`
//! is documented "Lists Domains for a given account."
//! (`smithy_query(query="doc", shape="ListDomains", model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so the listing reports exactly the persisted domains whose stored owning account is the
//! account the request was signed for. A domain another account owns is outside the answer
//! however the page is drawn and whichever `status` filter is asked for, and it is outside
//! the account's `CreateDomain` cap too — the listing and the cap are measured over ONE
//! scope.
//!
//! No request this emulator serves can create a domain for another account: every signed
//! request resolves the one canonical emulator account (`engine::emulator_account_id`), so
//! the foreign-owned rows are created through the API and then handed to another account
//! straight in this service's own store, through the shared engine the harness exposes —
//! the sibling suites' way of seeding a record no operation here writes.
//!
//! The rows differ along what the rule generalises over: how many domains the calling
//! account owns (none through the whole cap), how many records sit beside them owned by
//! another account, which account owns those (several distinct 12-digit ids), the status
//! each foreign record carries, which `status` filter the listing is asked for (none and
//! every modelled value), and how the page is drawn (one full page, the default page, or
//! walked one element at a time through `nextToken`).
//!
//! The harness (`Emulator`, `error_code`, `string_at`) is the sibling suites': one way of
//! driving this service through signed restJson1 requests, not two.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, DomainStatus, MAX_DOMAINS_PER_ACCOUNT,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The Region every request below is signed for. One Region throughout, so nothing here
/// can pass by accident of the Region scope `tests/all/region_scoping.rs` covers.
const REGION: &str = "eu-west-1";

/// Accounts that are NOT the one this emulator signs for. Each is the 12 digits every ARN
/// pattern in this model declares for the account segment (`[0-9]{12}`), and they differ in
/// the ways such an id can: repeated digits, one differing only in its last digit from the
/// emulator's own, and an arbitrary one.
const FOREIGN_ACCOUNTS: [&str; 3] = ["111111111111", "000000000001", "490735624321"];

/// Every modelled `DomainStatus` value, as the `status` query parameter spells it
/// (`smithy_query(query="operation", operation="ListDomains")`: `status` is a
/// `DomainStatus`, one of ACTIVE | DELETED | DELETING).
const STATUS_VALUES: [&str; 3] = ["ACTIVE", "DELETED", "DELETING"];

/// Far enough ahead that the purge a `DELETING` record owes does not fall due while a case
/// is running, so a seeded non-terminal record stays observable.
const BEYOND_ANY_CASE: Duration = Duration::from_secs(3_600);

/// Hand the persisted domain `domain_id` to `account_id`, carrying `status`.
///
/// Written directly into this service's store because no operation it serves can produce
/// a record of another account: the account every request resolves is the canonical
/// emulator one.
fn hand_to_account(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    account_id: &str,
    status: DomainStatus,
) {
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let domain = store.domains.get_mut(domain_id).unwrap_or_else(|| {
        panic!("case {case:?}: a persisted domain {domain_id} to hand to {account_id}")
    });
    domain.account_id = account_id.to_string();
    domain.status = status;
    // A record in the non-terminal `DELETING` is one whose purge has not fallen due yet;
    // an `ACTIVE` one owes no purge at all.
    domain.purge_due_at = match status {
        DomainStatus::Active => None,
        _ => Some(SystemTime::now() + BEYOND_ANY_CASE),
    };
}

/// The `domainId`s `ListDomains` reports for `query`, drawn as ONE page.
async fn listed_domain_ids(emulator: &Emulator, case: &str, query: &str) -> BTreeSet<String> {
    let (status, page) = emulator
        .send("GET", &format!("/domain{query}"), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}{query}: ListDomains must answer 200, got {page}"
    );
    page.get("domains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}{query}: domains must be a list, got {page}"))
        .iter()
        .map(|summary| string_at(summary, "domainId"))
        .collect()
}

/// The `domainId`s `ListDomains` reports when the collection is walked one element at a
/// time, feeding back only the cursors the service itself issued.
async fn paged_domain_ids(emulator: &Emulator, case: &str) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    // At most one step per record the store could hold for every account in this suite,
    // so a cursor that failed to advance cannot loop forever.
    for _ in 0..=(MAX_DOMAINS_PER_ACCOUNT * (FOREIGN_ACCOUNTS.len() + 1)) {
        let path = match &token {
            None => "/domain?maxResults=1".to_string(),
            Some(token) => format!("/domain?maxResults=1&nextToken={token}"),
        };
        let (status, page) = emulator.send("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a paged ListDomains must answer 200, got {page}"
        );
        for summary in page
            .get("domains")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: domains must be a list, got {page}"))
        {
            let id = string_at(summary, "domainId");
            assert!(
                seen.insert(id.clone()),
                "case {case:?}: {id} was paged twice, after {seen:?}"
            );
        }
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return seen,
        }
    }
    panic!("case {case:?}: paging never terminated after {seen:?}")
}

/// One row: how many domains the calling account owns, and the (account, status) pair of
/// every record beside them owned by somebody else.
struct Row {
    case: &'static str,
    own: usize,
    foreign: Vec<(&'static str, DomainStatus)>,
}

#[tokio::test]
async fn list_domains_reports_only_the_calling_accounts_domains() {
    let rows = vec![
        Row {
            case: "nothing of our own, one foreign ACTIVE domain",
            own: 0,
            foreign: vec![(FOREIGN_ACCOUNTS[0], DomainStatus::Active)],
        },
        Row {
            case: "one of our own beside one foreign domain",
            own: 1,
            foreign: vec![(FOREIGN_ACCOUNTS[1], DomainStatus::Active)],
        },
        Row {
            case: "several of our own beside three accounts' domains",
            own: 3,
            foreign: vec![
                (FOREIGN_ACCOUNTS[0], DomainStatus::Active),
                (FOREIGN_ACCOUNTS[1], DomainStatus::Deleting),
                (FOREIGN_ACCOUNTS[2], DomainStatus::Active),
            ],
        },
        Row {
            case: "only foreign domains, one per account and per status",
            own: 0,
            foreign: vec![
                (FOREIGN_ACCOUNTS[0], DomainStatus::Deleting),
                (FOREIGN_ACCOUNTS[1], DomainStatus::Active),
                (FOREIGN_ACCOUNTS[2], DomainStatus::Deleting),
            ],
        },
        Row {
            case: "our own domains with no foreign record at all",
            own: 2,
            foreign: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new(REGION);

        let mut own: BTreeSet<String> = BTreeSet::new();
        for index in 0..row.own {
            let created = emulator
                .create_domain(&json!({ "name": format!("clinic-own-{index}") }))
                .await;
            own.insert(string_at(&created, "domainId"));
        }
        let mut foreign: BTreeSet<String> = BTreeSet::new();
        for (index, (account, status)) in row.foreign.iter().enumerate() {
            let created = emulator
                .create_domain(&json!({ "name": format!("clinic-other-{index}") }))
                .await;
            let domain_id = string_at(&created, "domainId");
            hand_to_account(&emulator, case, &domain_id, account, *status);
            foreign.insert(domain_id);
        }

        // Every own domain is `ACTIVE`, so the unfiltered page and the `ACTIVE` page are
        // exactly the account's domains and every other filter is empty — the foreign
        // `DELETING` records must not fill the `DELETING` page.
        for query in ["", "?maxResults=100", "?maxResults=1"] {
            let listed = listed_domain_ids(&emulator, case, query).await;
            let expected: BTreeSet<String> = own.iter().take(page_size(query)).cloned().collect();
            assert_eq!(
                listed, expected,
                "case {case:?}{query}: ListDomains must report the calling account's \
                 domains alone, got {listed:?}"
            );
            assert!(
                listed.is_disjoint(&foreign),
                "case {case:?}{query}: a domain of another account must not be listed, \
                 got {listed:?} against {foreign:?}"
            );
        }
        for status in STATUS_VALUES {
            let query = format!("?status={status}&maxResults=100");
            let listed = listed_domain_ids(&emulator, case, &query).await;
            let expected: BTreeSet<String> = if status == "ACTIVE" {
                own.clone()
            } else {
                BTreeSet::new()
            };
            assert_eq!(
                listed, expected,
                "case {case:?}{query}: a status filter must select inside the calling \
                 account's domains only, got {listed:?}"
            );
        }

        let paged = paged_domain_ids(&emulator, case).await;
        assert_eq!(
            paged, own,
            "case {case:?}: walking the cursor one domain at a time must stay inside the \
             calling account, got {paged:?}"
        );
    }
}

/// The most domains the page `query` asks for, so the expectation of a one-element page is
/// the first element rather than the whole set.
fn page_size(query: &str) -> usize {
    match query {
        "?maxResults=1" => 1,
        // No `maxResults`, or this service's own page cap: more than any row creates.
        _ => usize::MAX,
    }
}

#[tokio::test]
async fn a_domain_handed_to_another_account_leaves_the_listing_and_the_cap_together() {
    let emulator = Emulator::new(REGION);
    let account = engine::emulator_account_id().to_string();
    // The text `CreateDomain` refuses a create past the account's cap with.
    let refusal = format!(
        "Account {account} has reached the maximum limit of {MAX_DOMAINS_PER_ACCOUNT} domains."
    );

    // Fill the calling account's cap, then confirm the next create is refused: the state
    // every row below starts from.
    let mut created: Vec<String> = Vec::new();
    for ordinal in 0..MAX_DOMAINS_PER_ACCOUNT {
        let domain = emulator
            .create_domain(&json!({ "name": format!("clinic-{ordinal}") }))
            .await;
        created.push(string_at(&domain, "domainId"));
    }
    let (status, refused) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "clinic-past-the-cap" })))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "a create past the account's cap must be refused, got {status} {refused}"
    );
    assert_eq!(
        refused.get("Message").and_then(Value::as_str),
        Some(refusal.as_str()),
        "the refusal must name the account and the cap, got {refused}"
    );

    // Hand a few of them away, one per foreign account: the rows differ along how much of
    // the account's cap has been handed over.
    let handed: Vec<String> = created
        .iter()
        .take(FOREIGN_ACCOUNTS.len())
        .cloned()
        .collect();
    for (domain_id, account) in handed.iter().zip(FOREIGN_ACCOUNTS) {
        hand_to_account(
            &emulator,
            "handed to another account",
            domain_id,
            account,
            DomainStatus::Active,
        );
    }
    let ours: BTreeSet<String> = created
        .iter()
        .filter(|domain_id| !handed.contains(domain_id))
        .cloned()
        .collect();

    let listed = listed_domain_ids(&emulator, "after handing domains away", "?maxResults=100").await;
    assert_eq!(
        listed, ours,
        "the listing must drop every domain now owned elsewhere, got {listed:?}"
    );

    // And the cap agrees with the listing: exactly as many creates succeed again as were
    // handed away, and the one after that is refused once more.
    for ordinal in 0..handed.len() {
        let created = emulator
            .create_domain(&json!({ "name": format!("clinic-again-{ordinal}") }))
            .await;
        assert_eq!(
            created.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {ordinal}: a create inside the freed cap must succeed, got {created}"
        );
    }
    let (status, refused) = emulator
        .send("POST", "/domain", Some(&json!({ "name": "clinic-past-again" })))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "the create past the refilled cap must be refused, got {status} {refused}"
    );
    assert_eq!(
        refused.get("Message").and_then(Value::as_str),
        Some(refusal.as_str()),
        "the refusal must name the account and the cap, got {refused}"
    );
    // The listing now reports the account's refilled cap, and none of the handed records.
    let listed = listed_domain_ids(&emulator, "after refilling the cap", "?maxResults=100").await;
    assert_eq!(
        listed.len(),
        MAX_DOMAINS_PER_ACCOUNT,
        "the listing must report the account's whole cap, got {listed:?}"
    );
    for domain_id in &handed {
        assert!(
            !listed.contains(domain_id),
            "a domain owned elsewhere must stay out of the listing, got {listed:?}"
        );
    }
}
