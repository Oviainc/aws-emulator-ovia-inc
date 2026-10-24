// SPDX-License-Identifier: CC0-1.0

//! `CreateComputer` — the Active Directory computer accounts a directory holds, driven
//! end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the state
//! struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a create persists a computer account on the directory the request named and reports
//!   that account back: the name as spelled, the LDAP attributes the request applied and
//!   nothing else, and a `ComputerId` that is the account's Windows SID — the directory's
//!   own domain SID, shared by every account of that directory and different for another,
//!   plus a relative identifier from 1000 up, unique within the directory;
//! * an account name is unique within the directory and compared case-insensitively, the
//!   way Active Directory compares account names: a second create of the same name in any
//!   case is refused with `EntityAlreadyExistsException`, while the same name in another
//!   directory is admitted and gets its own SID;
//! * a create needs a directory that is in service: one still being built by the
//!   transition its create owes is refused with `DirectoryUnavailableException`, and the
//!   same call succeeds once that transition has fallen due;
//! * a named `OrganizationalUnitDistinguishedName` must be a distinguished name of the
//!   directory's own domain — over a table of DN forms — and anything else is refused with
//!   `InvalidParameterException`;
//! * the `DirectoryId` is resolved in the store, an id no directory is stored under is
//!   refused with `EntityDoesNotExistException` over a table of well-formed ids, a request
//!   the MODEL refuses never reaches that resolution, and the operation never answers the
//!   scaffold's `NotImplemented`;
//! * one directory holds at most the guarded number of accounts, and that guard is
//!   per-directory.

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::{
    FIRST_COMPUTER_RID, MAX_COMPUTERS_PER_DIRECTORY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The prefix every Active Directory domain account's SID carries: the NT authority and
/// the sub-authority that marks the next three as one domain's identifier.
const DOMAIN_SID_PREFIX: &str = "S-1-5-21";

/// How many sub-authorities a domain account's SID carries after [`DOMAIN_SID_PREFIX`]:
/// the domain's own three, then the account's relative identifier.
const SUB_AUTHORITIES_AFTER_PREFIX: usize = 4;

/// The longest `ComputerName` the model admits: `ComputerName` is `len 1..15`
/// (`smithy_query(query="operation", operation="CreateComputer")`), so this row sits on
/// the published upper bound.
const LONGEST_NAME: &str = "svr0123456789ab";

/// The shortest `ComputerName` the model admits: the lower bound of the same `len 1..15`.
const SHORTEST_NAME: &str = "c";

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="CreateComputer")`), so every row here is a
/// value the boundary validator admits and the handler must resolve in the store.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-9f8e7d6c5b", "digits and letters interleaved"),
];

/// A one-time join password of `length` characters.
///
/// The model bounds `ComputerPassword` at `len 8..64` with the pattern
/// `^[\u0020-\u00FF]+$`, so the characters cycled here are inside that range and `length`
/// is what a row varies.
fn join_password(length: usize) -> String {
    "Cmp-1-Time!Pw".chars().cycle().take(length).collect()
}

/// One `CreateComputer` call that must succeed, returning the `Computer` it reported.
async fn create_computer(router: &MultiServiceRouter, body: Value, case: &str) -> Value {
    let answer = ok(router, "CreateComputer", body, case).await;
    let computer = answer["Computer"].clone();
    assert!(
        computer.is_object(),
        "case {case:?}: expected a Computer in the answer, got {answer}"
    );
    computer
}

/// Assert `id` is a well-formed computer account SID, and return it split into the domain
/// portion and the relative identifier.
///
/// The modelled `Computer$ComputerId` targets `SID` (`len 1..256`, pattern `^[&\w+-.@]+$`),
/// and a domain account's SID is `S-1-5-21-<a>-<b>-<c>-<rid>` with the RID at or above
/// 1000, the first one Active Directory hands to an account created in the domain.
fn split_computer_id(id: &str, case: &str) -> (String, u64) {
    assert!(
        !id.is_empty() && id.len() <= 256,
        "case {case:?}: expected a SID of 1..256 characters, got {id:?}"
    );
    assert!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || "&+-.@".contains(c)),
        "case {case:?}: expected a SID matching ^[&\\w+-.@]+$, got {id:?}"
    );
    let tail = id.strip_prefix(&format!("{DOMAIN_SID_PREFIX}-")).unwrap_or_else(|| {
        panic!("case {case:?}: expected a ComputerId starting {DOMAIN_SID_PREFIX}-, got {id:?}")
    });
    let parts: Vec<&str> = tail.split('-').collect();
    assert_eq!(
        parts.len(),
        SUB_AUTHORITIES_AFTER_PREFIX,
        "case {case:?}: expected {SUB_AUTHORITIES_AFTER_PREFIX} sub-authorities after \
         {DOMAIN_SID_PREFIX}, got {id:?}"
    );
    for part in &parts {
        assert!(
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()),
            "case {case:?}: expected every sub-authority to be a decimal number, got {id:?}"
        );
    }
    let rid: u64 = parts[SUB_AUTHORITIES_AFTER_PREFIX - 1]
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric RID, got {id:?}"));
    assert!(
        rid >= u64::from(FIRST_COMPUTER_RID),
        "case {case:?}: expected a RID at or above {FIRST_COMPUTER_RID}, got {rid} in {id:?}"
    );
    let domain = id
        .rsplit_once('-')
        .map(|(domain, _)| domain.to_string())
        .unwrap_or_default();
    (domain, rid)
}

/// One row of the directory table: how the directory is created, and the domain name it
/// carries (its `Name`, which is what a distinguished name in it ends with).
struct DirectoryCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    domain: &'static str,
}

/// One row of the computer table: what the create asks for.
struct ComputerCase {
    case: &'static str,
    name: &'static str,
    password: String,
    /// The `ComputerAttributes` the request sends, or `None` when it sends the member not
    /// at all.
    attributes: Option<Value>,
    /// The components of the organizational unit's distinguished name BEFORE the domain
    /// ones, or `None` when the request names no unit. The domain components are appended
    /// per directory, so the same row is a valid DN in every directory of the table.
    organizational_unit: Option<&'static str>,
}

/// The directory table: both create operations, differing in type, size, edition and
/// network type, each with its own domain name.
fn directory_rows() -> Vec<DirectoryCase> {
    vec![
        DirectoryCase {
            case: "a minimal Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("simple.example.com"),
            domain: "simple.example.com",
        },
        DirectoryCase {
            case: "a large Simple AD directory with a dual-stack network",
            operation: "CreateDirectory",
            body: json!({
                "Name": "large.simple.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack"
            }),
            domain: "large.simple.example.com",
        },
        DirectoryCase {
            case: "a minimal Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("corp.example.com"),
            domain: "corp.example.com",
        },
        DirectoryCase {
            case: "a Standard-edition Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "standard.corp.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            domain: "standard.corp.example.com",
        },
    ]
}

/// The computer table: the rows differ along name length and case, how many LDAP
/// attributes the create applies (none at all, an empty list, one, one carrying no value,
/// three), whether the request names an organizational unit and how deep it is, and the
/// password's length at both published bounds.
fn computer_rows() -> Vec<ComputerCase> {
    vec![
        ComputerCase {
            case: "the shortest name, no attributes, no organizational unit",
            name: SHORTEST_NAME,
            password: join_password(8),
            attributes: None,
            organizational_unit: None,
        },
        ComputerCase {
            case: "the longest name, one attribute, a shallow organizational unit",
            name: LONGEST_NAME,
            password: join_password(20),
            attributes: Some(json!([{ "Name": "operatingSystem", "Value": "Windows" }])),
            organizational_unit: Some("OU=Computers"),
        },
        ComputerCase {
            case: "a mixed-case name, three attributes, a deep organizational unit",
            name: "Mixed-Case-01",
            password: join_password(64),
            attributes: Some(json!([
                { "Name": "description", "Value": "a build agent" },
                { "Name": "location", "Value": "rack 7" },
                { "Name": "operatingSystemVersion", "Value": "10.0 (20348)" }
            ])),
            organizational_unit: Some("OU=Servers,OU=Prod"),
        },
        ComputerCase {
            case: "an attribute carrying a name and no value",
            name: "attr-no-value",
            password: join_password(12),
            attributes: Some(json!([{ "Name": "description" }])),
            organizational_unit: None,
        },
        ComputerCase {
            case: "an empty attribute list",
            name: "no-attrs",
            password: join_password(9),
            attributes: Some(json!([])),
            organizational_unit: None,
        },
    ]
}

/// The `DC=` components a distinguished name in `domain` ends with.
fn domain_components(domain: &str) -> String {
    domain
        .split('.')
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The `CreateComputer` body a row asks for in `directory_id`, whose domain is `domain`.
fn create_body(directory_id: &str, domain: &str, row: &ComputerCase) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "ComputerName": row.name,
        "Password": row.password
    });
    if let Some(attributes) = &row.attributes {
        body["ComputerAttributes"] = attributes.clone();
    }
    if let Some(unit) = row.organizational_unit {
        body["OrganizationalUnitDistinguishedName"] =
            json!(format!("{unit},{}", domain_components(domain)));
    }
    body
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &DirectoryCase) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

#[tokio::test]
async fn a_computer_account_is_created_in_the_directory_the_request_names() {
    let router = service();

    let directories = directory_rows();
    let mut ids: Vec<(&DirectoryCase, String)> = Vec::new();
    for row in &directories {
        ids.push((row, create(&router, row).await));
    }
    // The stage transition each create owes falls due, so every directory is in service.
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let rows = computer_rows();
    let mut every_id: Vec<String> = Vec::new();
    let mut domains: Vec<(&str, String)> = Vec::new();

    for (directory, directory_id) in &ids {
        let mut rids: Vec<u64> = Vec::new();
        let mut domain_sids: Vec<String> = Vec::new();
        for row in &rows {
            let case = format!("{} / {}", directory.case, row.case);
            let body = create_body(directory_id, directory.domain, row);
            let computer = create_computer(&router, body, &case).await;

            assert_eq!(
                computer["ComputerName"].as_str(),
                Some(row.name),
                "case {case:?}: the account is reported under the name the request spelled, \
                 got {computer}"
            );

            // The attributes reported are exactly the ones the create applied — and a
            // create that applied none reports the member not at all, rather than an empty
            // list nothing recorded.
            let expected_attributes = match &row.attributes {
                Some(attributes) if attributes.as_array().is_some_and(|rows| !rows.is_empty()) => {
                    attributes.clone()
                }
                _ => Value::Null,
            };
            assert_eq!(
                computer["ComputerAttributes"], expected_attributes,
                "case {case:?}: expected ComputerAttributes {expected_attributes}, got \
                 {computer}"
            );

            let id = computer["ComputerId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ComputerId, got {computer}"))
                .to_string();
            let (domain_sid, rid) = split_computer_id(&id, &case);
            rids.push(rid);
            domain_sids.push(domain_sid);
            every_id.push(id);
        }

        // Every account of one directory carries that directory's domain SID, and each
        // takes its own relative identifier in the order the accounts were created.
        let first = domain_sids[0].clone();
        assert!(
            domain_sids.iter().all(|sid| *sid == first),
            "case {:?}: every account of one directory carries one domain SID, got \
             {domain_sids:?}",
            directory.case
        );
        let expected_rids: Vec<u64> = (0..rows.len())
            .map(|index| {
                u64::from(FIRST_COMPUTER_RID)
                    + u64::try_from(index).expect("a row index fits a u64")
            })
            .collect();
        assert_eq!(
            rids, expected_rids,
            "case {:?}: expected the relative identifiers {expected_rids:?}, got {rids:?}",
            directory.case
        );
        domains.push((directory.case, first));
    }

    // One directory's domain SID is its own: no two directories of the store share one.
    for (left_index, (left_case, left)) in domains.iter().enumerate() {
        for (right_case, right) in domains.iter().skip(left_index + 1) {
            assert_ne!(
                left, right,
                "case {left_case:?} vs {right_case:?}: two directories must not share a \
                 domain SID, both reported {left:?}"
            );
        }
    }

    // And no two accounts of the store share a `ComputerId`.
    let mut sorted = every_id.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        every_id.len(),
        "expected {} distinct ComputerIds, got {every_id:?}",
        every_id.len()
    );

    // An account name is unique within its directory, compared the way Active Directory
    // compares account names: a repeat in any case is refused, while the same name in
    // another directory is admitted and gets its own id.
    for (directory, directory_id) in &ids {
        for row in &rows {
            for (name, class) in [
                (row.name.to_string(), "the same spelling"),
                (row.name.to_uppercase(), "the name upper-cased"),
                (row.name.to_lowercase(), "the name lower-cased"),
            ] {
                let case = format!("{} / {} / {class}", directory.case, row.case);
                refused(
                    &router,
                    "CreateComputer",
                    json!({
                        "DirectoryId": directory_id,
                        "ComputerName": name,
                        "Password": join_password(16)
                    }),
                    "EntityAlreadyExistsException",
                    &case,
                )
                .await;
            }
        }
    }

    // The same name in a directory that does not hold it yet is a different account.
    let shared_name = "shared-name";
    let mut shared_ids: Vec<String> = Vec::new();
    for (directory, directory_id) in &ids {
        let case = format!("{} / a name shared across directories", directory.case);
        let computer = create_computer(
            &router,
            json!({
                "DirectoryId": directory_id,
                "ComputerName": shared_name,
                "Password": join_password(10)
            }),
            &case,
        )
        .await;
        let id = computer["ComputerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ComputerId, got {computer}"))
            .to_string();
        split_computer_id(&id, &case);
        assert!(
            !shared_ids.contains(&id),
            "case {case:?}: an account of another directory must not reuse the id {id:?}"
        );
        shared_ids.push(id);
    }
}

#[tokio::test]
async fn a_create_needs_a_directory_that_is_in_service() {
    let router = service();

    // Both creates leave the directory in the transient stage its create owes a
    // transition out of, so both rows exercise the window before it falls due.
    let rows = vec![
        DirectoryCase {
            case: "a Simple AD directory still being built",
            operation: "CreateDirectory",
            body: minimal_create("pending.example.com"),
            domain: "pending.example.com",
        },
        DirectoryCase {
            case: "a Managed Microsoft AD directory still being built",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("pending.corp.example.com"),
            domain: "pending.corp.example.com",
        },
    ];

    let mut ids: Vec<(&DirectoryCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        refused(
            &router,
            "CreateComputer",
            json!({
                "DirectoryId": &id,
                "ComputerName": "early",
                "Password": join_password(16)
            }),
            "DirectoryUnavailableException",
            row.case,
        )
        .await;
        ids.push((row, id));
    }

    // The refusal was about the stage, so the very same call is admitted once the
    // transition the create owed has fallen due.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in &ids {
        let computer = create_computer(
            &router,
            json!({
                "DirectoryId": id,
                "ComputerName": "early",
                "Password": join_password(16)
            }),
            row.case,
        )
        .await;
        assert_eq!(
            computer["ComputerName"].as_str(),
            Some("early"),
            "case {:?}: the account is created once the directory is in service, got \
             {computer}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_organizational_unit_must_be_a_distinguished_name_of_the_directory_domain() {
    let router = service();

    let domain = "corp.example.com";
    let directory_id = create_microsoft_ad(
        &router,
        minimal_microsoft_ad(domain),
        "the directory the units are checked against",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A distinguished name is a comma-separated sequence of `attribute=value` components,
    // most specific first, and the ones naming the domain are its last: these rows differ
    // in depth, in case, in spacing and in which attribute the unit uses — all forms a
    // directory resolves to the same place.
    let accepted = [
        (
            format!("OU=Computers,{}", domain_components(domain)),
            "a single unit",
        ),
        (
            format!("OU=Servers,OU=Prod,{}", domain_components(domain)),
            "a nested unit",
        ),
        (
            format!("ou=computers,{}", domain_components(domain).to_lowercase()),
            "lower case throughout",
        ),
        (
            format!("OU=Computers, {}", domain_components(domain).replace(',', ", ")),
            "a space after every comma",
        ),
        (
            format!("CN=Computers,{}", domain_components(domain)),
            "the default container, which is a CN rather than an OU",
        ),
        (
            domain_components(domain),
            "the domain root itself, carrying only domain components",
        ),
    ];
    for (index, (unit, class)) in accepted.iter().enumerate() {
        let computer = create_computer(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "ComputerName": format!("ok-{index}"),
                "Password": join_password(16),
                "OrganizationalUnitDistinguishedName": unit
            }),
            class,
        )
        .await;
        assert_eq!(
            computer["ComputerName"].as_str(),
            Some(format!("ok-{index}").as_str()),
            "case {class:?}: expected the account to be created in {unit:?}, got {computer}"
        );
    }

    let rejected = [
        ("Computers".to_string(), "no attribute=value component at all"),
        (
            format!("OU=,{}", domain_components(domain)),
            "a component with an empty value",
        ),
        (
            format!("=Computers,{}", domain_components(domain)),
            "a component with an empty attribute",
        ),
        (
            "OU=Computers".to_string(),
            "no domain components, so it names no domain",
        ),
        (
            format!("OU=Computers,{}", domain_components("other.example.com")),
            "the domain components of another domain",
        ),
        (
            format!("OU=Computers,{}", domain_components("example.com")),
            "a suffix of the directory's domain rather than the domain",
        ),
        (
            format!("{},OU=Computers", domain_components(domain)),
            "the domain components first, so they are not the trailing ones",
        ),
    ];
    for (index, (unit, class)) in rejected.iter().enumerate() {
        refused(
            &router,
            "CreateComputer",
            json!({
                "DirectoryId": &directory_id,
                "ComputerName": format!("no-{index}"),
                "Password": join_password(16),
                "OrganizationalUnitDistinguishedName": unit
            }),
            "InvalidParameterException",
            class,
        )
        .await;
    }

    // A refused unit created nothing: the names it was refused under are still free.
    for index in 0..rejected.len() {
        let case = format!("the name refused row {index} asked for is still free");
        create_computer(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "ComputerName": format!("no-{index}"),
                "Password": join_password(16)
            }),
            &case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_directory_id_is_resolved_in_the_store() {
    let router = service();

    // One stored directory, so the store is not empty and a refusal cannot come from
    // there being nothing at all.
    let stored = create_directory(
        &router,
        minimal_create("resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        refused(
            &router,
            "CreateComputer",
            json!({
                "DirectoryId": absent,
                "ComputerName": "anywhere",
                "Password": join_password(16)
            }),
            "EntityDoesNotExistException",
            class,
        )
        .await;
    }

    // A request the MODEL refuses never reaches that resolution: the boundary validator
    // refuses it first, so the answer names neither the directory nor the scaffold. Every
    // row breaks one published constraint of this operation's input
    // (`smithy_query(query="operation", operation="CreateComputer")`): `DirectoryId`
    // `^d-[0-9a-f]{10}$`, `ComputerName` `len 1..15`, `ComputerPassword` `len 8..64` with
    // pattern `^[\u0020-\u00FF]+$` — each tested on both sides of the bound it has.
    let malformed = [
        (
            json!({ "DirectoryId": "d-ABCDEF0123", "ComputerName": "c", "Password": join_password(16) }),
            "a DirectoryId outside the pattern's alphabet",
        ),
        (
            json!({ "DirectoryId": "d-0a1b2c3d", "ComputerName": "c", "Password": join_password(16) }),
            "a DirectoryId with too few digits",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "", "Password": join_password(16) }),
            "a ComputerName below the length bound",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "sixteen-chars-16", "Password": join_password(16) }),
            "a ComputerName above the length bound",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "c", "Password": join_password(7) }),
            "a Password below the length bound",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "c", "Password": join_password(65) }),
            "a Password above the length bound",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "c", "Password": "Ā".repeat(16) }),
            "a Password carrying a character outside the pattern's range",
        ),
        (
            json!({ "DirectoryId": &stored, "ComputerName": "c" }),
            "no Password at all, which the model requires",
        ),
        (
            json!({ "DirectoryId": &stored, "Password": join_password(16) }),
            "no ComputerName at all, which the model requires",
        ),
    ];
    for (body, class) in malformed {
        let (status, parsed) = call(&router, "CreateComputer", body).await;
        assert_eq!(
            status, 400,
            "case {class:?}: the model refuses this request, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
        assert!(
            !wire_type.contains("EntityDoesNotExistException")
                && !wire_type.contains("EntityAlreadyExistsException"),
            "case {class:?}: the input is refused before the directory is resolved, got \
             {parsed}"
        );
        assert!(
            !wire_type.contains("NotImplemented"),
            "case {class:?}: the operation must not answer the scaffold signal, got {parsed}"
        );
    }

    // None of those refusals touched the stored directory, which still accepts the name
    // every malformed row named.
    let computer = create_computer(
        &router,
        json!({
            "DirectoryId": &stored,
            "ComputerName": "c",
            "Password": join_password(16)
        }),
        "the stored directory",
    )
    .await;
    split_computer_id(
        computer["ComputerId"].as_str().unwrap_or_default(),
        "the stored directory",
    );
}

#[tokio::test]
async fn a_directory_holds_at_most_the_guarded_number_of_computer_accounts() {
    let router = service();

    let filled = create_directory(
        &router,
        minimal_create("filled.example.com"),
        "the directory that is filled",
    )
    .await;
    let spare = create_directory(
        &router,
        minimal_create("spare.example.com"),
        "the directory in the same store",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for index in 0..MAX_COMPUTERS_PER_DIRECTORY {
        let body = json!({
            "DirectoryId": &filled,
            "ComputerName": format!("c{index}"),
            "Password": join_password(16)
        });
        let (status, parsed) = call(&router, "CreateComputer", body).await;
        assert_eq!(
            status, 200,
            "case {:?}: account {index} of {MAX_COMPUTERS_PER_DIRECTORY} is inside the \
             guard, got {status} with {parsed}",
            "the directory that is filled"
        );
    }

    refused(
        &router,
        "CreateComputer",
        json!({
            "DirectoryId": &filled,
            "ComputerName": "one-too-many",
            "Password": join_password(16)
        }),
        "ClientException",
        "one account past the guard",
    )
    .await;

    // The guard is per directory: the one beside it in the same store still accepts an
    // account.
    let computer = create_computer(
        &router,
        json!({
            "DirectoryId": &spare,
            "ComputerName": "one-too-many",
            "Password": join_password(16)
        }),
        "the directory in the same store",
    )
    .await;
    let (_, rid) = split_computer_id(
        computer["ComputerId"].as_str().unwrap_or_default(),
        "the directory in the same store",
    );
    assert_eq!(
        rid,
        u64::from(FIRST_COMPUTER_RID),
        "case {:?}: the first account of a directory takes the first relative identifier, \
         got {rid}",
        "the directory in the same store"
    );
}
