// SPDX-License-Identifier: CC0-1.0

//! `ListStorageLensConfigurations`, driven end to end over HTTP.
//!
//! The rule under test: a list reports ONE summary entry for every S3 Storage
//! Lens configuration the account the request names holds — the configuration's
//! id, the ARN this service assigned it, the home Region of the put that stored
//! it, and whether that document enabled it, and nothing else of the document —
//! in configuration-id order, scoped to that account, a page at a time, with a
//! continuation token returned exactly when a further configuration follows the
//! page. Listing is a read: it changes nothing a later caller sees, and the same
//! token answers the same page.
//!
//! Every row below differs along something that rule generalises over: how long
//! a configuration id is and which of the characters the wire can address it by
//! it uses,
//! whether the configuration is enabled, which Region the put was signed for,
//! which account owns it, the order the configurations were put in (which is not
//! the order they are listed in), how many configurations there are either side
//! of one page, and whether a continuation token was issued by this service at
//! all.
//!
//! The router, the request builder, the XML readers and the document writer are
//! the access point and Storage Lens configuration suites', reused rather than
//! copied.

use crate::access_point_lifecycle::{
    call_in_region, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{document_xml, leaf, leaves_of, Leaf, NO_CONTENT};
use std::collections::BTreeSet;

/// How many entries one page carries — the page size the service itself fixes,
/// read from the service rather than restated here.
const PAGE_SIZE: usize =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::STORAGE_LENS_CONFIGURATIONS_PAGE_SIZE;

/// Two further signing regions, so a listed entry's home Region is pinned
/// against the Region its put was signed for rather than one fixed value.
const OTHER_REGION: &str = "eu-central-1";
const THIRD_REGION: &str = "ap-southeast-2";

// ---------------------------------------------------------------------------
// Reading a list answer
// ---------------------------------------------------------------------------

/// One listed entry, as `ListStorageLensConfigurationEntry` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    storage_lens_arn: String,
    home_region: String,
    /// `IsEnabled` is declared with a default of `false`, so an entry whose
    /// element is absent is a disabled configuration.
    is_enabled: bool,
}

/// Every leaf path a list answer may carry: the four members of an entry, each
/// under the `StorageLensConfiguration` element the model's `xmlName` gives the
/// flattened list member, and the answer's own `NextToken`.
const ENTRY_MEMBER_PREFIX: &str = "StorageLensConfiguration/";
/// The answer's root element. [`leaves_of`] roots every path BELOW the root, so
/// this appears as a path of its own in exactly one answer: the one carrying no
/// entry and no token at all, whose root element is its own only leaf.
const RESULT_ROOT: &str = "ListStorageLensConfigurationResult";
const ALLOWED_PATHS: [&str; 6] = [
    "StorageLensConfiguration/Id",
    "StorageLensConfiguration/StorageLensArn",
    "StorageLensConfiguration/HomeRegion",
    "StorageLensConfiguration/IsEnabled",
    "NextToken",
    RESULT_ROOT,
];

/// The entries a list answer reports, in document order.
///
/// `Id` is the entry shape's first member, so an `Id` leaf opens an entry and
/// the members after it belong to the entry it opened. Anything else in the
/// answer PANICS rather than comparing as nothing.
fn reported_entries(body: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for (path, value) in leaves_of(body) {
        let Some(member) = path.strip_prefix(ENTRY_MEMBER_PREFIX) else {
            assert!(
                path == "NextToken" || path == RESULT_ROOT,
                "the emulator answered an element no part of the list shape declares: {body}"
            );
            continue;
        };
        if member == "Id" {
            entries.push(Entry {
                id: value,
                storage_lens_arn: String::new(),
                home_region: String::new(),
                is_enabled: false,
            });
            continue;
        }
        let open = entries
            .last_mut()
            .unwrap_or_else(|| panic!("a {member} outside any entry: {body}"));
        match member {
            "StorageLensArn" => open.storage_lens_arn = value,
            "HomeRegion" => open.home_region = value,
            "IsEnabled" => open.is_enabled = value == "true",
            other => panic!("the entry carried an undeclared member {other}: {body}"),
        }
    }
    entries
}

/// Every leaf path of `body` that an entry does not declare, so a list answer
/// leaking the document's metric selections or its tags fails loudly.
fn undeclared_paths(body: &str) -> Vec<String> {
    leaves_of(body)
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| !ALLOWED_PATHS.contains(&path.as_str()))
        .collect()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One stored configuration: the id it is put under, whether its document
/// enables it, and the Region its put is signed for.
struct Case {
    case: &'static str,
    config_id: String,
    is_enabled: bool,
    region: &'static str,
}

impl Case {
    /// The entry this configuration must be listed with: the ARN and the home
    /// Region both follow the Region the put was signed for, and `IsEnabled`
    /// follows the document.
    fn expected_entry(&self) -> Entry {
        Entry {
            id: self.config_id.clone(),
            storage_lens_arn: expected_arn(&self.config_id, self.region),
            home_region: self.region.to_string(),
            is_enabled: self.is_enabled,
        }
    }
}

/// The ARN a configuration of this id must be listed with, in the Region its put
/// was signed for: the `storage-lens/<config-id>` resource in the `s3` ARN
/// namespace under the emulator's canonical account — the format
/// `ListStorageLensConfigurationEntry$StorageLensArn`'s pattern
/// (`^arn:[a-z\-]+:s3:[a-z0-9\-]+:\d{12}:storage\-lens\/`) admits.
fn expected_arn(config_id: &str, region: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:storage-lens/{config_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The configurations every assertion below is run over.
///
/// The ids differ in length and in which of the characters `ConfigId`'s pattern
/// allows they use (`^[a-zA-Z0-9\-\_\.]+$`), at both published length bounds
/// (1..64); the rows are deliberately NOT in id order, because the order a list
/// reports them in is the configuration-id order and not the order they were
/// put in.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a one-character id, the published minimum",
            config_id: "a".to_string(),
            is_enabled: true,
            region: REGION,
        },
        Case {
            case: "an id at the published 64-character maximum",
            config_id: "x".repeat(64),
            is_enabled: false,
            region: REGION,
        },
        Case {
            case: "a disabled configuration in another region",
            config_id: "reports_weekly".to_string(),
            is_enabled: false,
            region: OTHER_REGION,
        },
        Case {
            case: "a digit-only id",
            config_id: "0".to_string(),
            is_enabled: true,
            region: THIRD_REGION,
        },
        Case {
            case: "an id carrying every addressable character class",
            config_id: "MixedCase_Id-With_Every-Addressable-Character_0123456789".to_string(),
            is_enabled: true,
            region: THIRD_REGION,
        },
        Case {
            case: "an underscored id in another region",
            config_id: "reports-daily".to_string(),
            is_enabled: true,
            region: OTHER_REGION,
        },
        Case {
            case: "a hyphenated id",
            config_id: "Dashboard-1".to_string(),
            is_enabled: false,
            region: REGION,
        },
    ]
}

/// The entries the table must be listed as: one per row, in configuration-id
/// order.
fn expected_entries(cases: &[Case]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = cases.iter().map(Case::expected_entry).collect();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    entries
}

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// The smallest document the model allows, carrying the id and the enabled flag
/// the row names: `AccountLevel` with its required (and all-optional)
/// `BucketLevel`, plus `IsEnabled`.
fn put_body(config_id: &str, is_enabled: bool) -> String {
    let leaves: Vec<Leaf> = vec![
        leaf("Id", config_id),
        leaf("AccountLevel/BucketLevel", ""),
        leaf("IsEnabled", if is_enabled { "true" } else { "false" }),
    ];
    format!(
        "<PutStorageLensConfigurationRequest xmlns=\"{XMLNS}\">\
         <StorageLensConfiguration>{}</StorageLensConfiguration>\
         </PutStorageLensConfigurationRequest>",
        document_xml(&leaves)
    )
}

async fn put_configuration(
    emulator: &Emulator,
    region: &str,
    account_id: &str,
    config_id: &str,
    is_enabled: bool,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        &format!("/v20180820/storagelens/{config_id}"),
        account_id,
        &put_body(config_id, is_enabled),
    )
    .await
}

async fn list_configurations(
    emulator: &Emulator,
    account_id: &str,
    next_token: Option<&str>,
) -> Reply {
    let uri = match next_token {
        None => "/v20180820/storagelens".to_string(),
        Some(token) => format!("/v20180820/storagelens?nextToken={token}"),
    };
    emulator.call("GET", &uri, account_id, "").await
}

/// Put every row of the table, and one configuration of another account under an
/// id the table also uses, so an account's list is pinned to its own records.
async fn put_every_case(emulator: &Emulator, cases: &[Case]) {
    for case in cases {
        let put =
            put_configuration(emulator, case.region, ACCOUNT, &case.config_id, case.is_enabled)
                .await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {:?}: expected the put to be accepted with {NO_CONTENT}, got {} {}",
            case.case, put.status, put.body
        );
    }
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_one_entry_per_configuration_the_account_holds() {
    let emulator = Emulator::start();

    // Before anything is put the account holds nothing, and the answer is an
    // empty list rather than a refusal.
    let empty = list_configurations(&emulator, ACCOUNT, None).await;
    assert_eq!(
        empty.status, 200,
        "expected an account holding no configuration to be answered 200, got {} {}",
        empty.status, empty.body
    );
    assert_eq!(
        reported_entries(&empty.body),
        Vec::<Entry>::new(),
        "expected no entries before any put, got body {}",
        empty.body
    );
    assert!(
        empty.text("NextToken").is_none(),
        "expected no continuation token on an empty list, got body {}",
        empty.body
    );

    let cases = cases();
    put_every_case(&emulator, &cases).await;
    // The same put addressed in another account is refused, so nothing of that
    // account's can enter this listing.
    let refused =
        put_configuration(&emulator, REGION, OTHER_ACCOUNT, &cases[0].config_id, false).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (403, "AccessDenied"),
        "expected the put for another account to be refused, got {} {}",
        refused.status,
        refused.body
    );

    // Read twice: the entries are the stored configurations, and reporting them
    // changes nothing for the next reader.
    for attempt in ["first read", "second read"] {
        let listed = list_configurations(&emulator, ACCOUNT, None).await;
        assert_eq!(
            listed.status, 200,
            "case {attempt:?}: expected 200, got {} {}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_entries(&listed.body),
            expected_entries(&cases),
            "case {attempt:?}: expected one entry per configuration in id order, got body {}",
            listed.body
        );
        assert_eq!(
            undeclared_paths(&listed.body),
            Vec::<String>::new(),
            "case {attempt:?}: a list answer carries only the entry members, got body {}",
            listed.body
        );
        assert!(
            listed.text("NextToken").is_none(),
            "case {attempt:?}: expected no token when one page holds every configuration, got body {}",
            listed.body
        );
    }

    // A listing for another account is refused rather than answered, so no
    // listing reaches across the account boundary.
    let other = list_configurations(&emulator, OTHER_ACCOUNT, None).await;
    assert_eq!(
        (other.status, other.code().as_str()),
        (403, "AccessDenied"),
        "expected a listing for another account to be refused, got {} {}",
        other.status,
        other.body
    );
    assert!(
        other.text("StorageLensConfigurationList").is_none()
            && other.text("StorageLensArn").is_none(),
        "expected the refusal to carry no listing, got body {}",
        other.body
    );
}

#[tokio::test]
async fn a_replacing_put_changes_the_entry_the_configuration_is_listed_with() {
    let emulator = Emulator::start();
    let config_id = "replaced";

    // Each row replaces the one before it under ONE id: the entry follows the
    // last put — its enabled flag, and the Region that put was signed for —
    // and the account still holds exactly one configuration.
    for (case, region, is_enabled) in [
        ("first put: enabled, home region", REGION, true),
        ("disabled by the next put", REGION, false),
        ("re-enabled in another region", OTHER_REGION, true),
        ("disabled again in a third region", THIRD_REGION, false),
    ] {
        let put = put_configuration(&emulator, region, ACCOUNT, config_id, is_enabled).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected the put to be accepted with {NO_CONTENT}, got {} {}",
            put.status, put.body
        );

        let listed = list_configurations(&emulator, ACCOUNT, None).await;
        assert_eq!(
            reported_entries(&listed.body),
            vec![Entry {
                id: config_id.to_string(),
                storage_lens_arn: expected_arn(config_id, region),
                home_region: region.to_string(),
                is_enabled,
            }],
            "case {case:?}: expected the entry to follow the last put, got body {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_list_pages_through_every_configuration_and_a_token_names_a_position() {
    let emulator = Emulator::start();

    // Three more configurations than one page carries, so the walk crosses a
    // page boundary and ends on a partial page. The ids are zero-padded, so
    // their id order is the order they are numbered in.
    let total = PAGE_SIZE + 3;
    let ids: Vec<String> = (0..total).map(|index| format!("page-{index:04}")).collect();
    for (index, id) in ids.iter().enumerate() {
        let put = put_configuration(&emulator, REGION, ACCOUNT, id, index % 2 == 0).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {id:?}: expected the put to be accepted with {NO_CONTENT}, got {} {}",
            put.status, put.body
        );
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let page = list_configurations(&emulator, ACCOUNT, token.as_deref()).await;
        assert_eq!(
            page.status, 200,
            "case page {pages}: expected 200, got {} {}",
            page.status, page.body
        );
        let entries = reported_entries(&page.body);
        assert!(
            !entries.is_empty() && entries.len() <= PAGE_SIZE,
            "case page {pages}: expected between one and {PAGE_SIZE} entries, got {}",
            entries.len()
        );

        // The same token must answer the same page: a token is a position in a
        // scan, not a cursor the service consumes.
        if let Some(token) = &token {
            let replayed = list_configurations(&emulator, ACCOUNT, Some(token)).await;
            assert_eq!(
                reported_entries(&replayed.body),
                entries,
                "case page {pages}: expected a token to answer the same page twice, got body {}",
                replayed.body
            );
        }

        seen.extend(entries.iter().map(|entry| entry.id.clone()));
        pages += 1;
        assert!(
            pages <= total,
            "case page {pages}: expected paging to finish, saw {pages} pages"
        );
        match page.text("NextToken") {
            Some(next) => token = Some(next),
            None => break,
        }
    }

    let mut ascending = ids.clone();
    ascending.sort();
    assert_eq!(
        seen, ascending,
        "expected the pages to walk every configuration exactly once, in id order"
    );
    assert_eq!(
        seen.iter().cloned().collect::<BTreeSet<String>>().len(),
        total,
        "expected no configuration on two pages, got {} ids",
        seen.len()
    );
    assert_eq!(
        pages,
        total.div_ceil(PAGE_SIZE),
        "expected {} pages of at most {PAGE_SIZE} entries, got {pages}",
        total.div_ceil(PAGE_SIZE)
    );

    // A continuation token this service never issued cannot resume a scan.
    for (case, foreign) in [
        ("not the token encoding", "not-a-token"),
        ("valid encoding of invalid bytes", "ffff"),
    ] {
        let refused = list_configurations(&emulator, ACCOUNT, Some(foreign)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a foreign continuation token to be refused, got {} {}",
            refused.status, refused.body
        );
    }
}
