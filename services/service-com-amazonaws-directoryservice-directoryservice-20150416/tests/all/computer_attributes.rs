// SPDX-License-Identifier: CC0-1.0

//! `CreateComputer`'s `ComputerAttributes` — the attributes of the ACCOUNT the create
//! made, not the list the request sent, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`).
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * `Computer$ComputerAttributes` is "the LDAP attributes that belong to the computer
//!   account" (`smithy_query(query="doc", shape="Computer", member="ComputerAttributes")`),
//!   so an attribute a create applies is reported under the one name the directory holds
//!   it by: a recognized LDAP attribute name, which LDAP compares case-insensitively, is
//!   reported in its display spelling whatever casing the request wrote it in, and a name
//!   the service does not recognize is reported as it was written;
//! * the attributes the domain derives for the account itself are reported beside the
//!   applied ones — the SAM account name, which is the account name with the `$` suffix a
//!   machine account's SAM name carries, and the distinguished name, which is the
//!   account's `CN=` component under the unit the create named or under the directory's
//!   own computers container — so even a create that applies nothing reports them;
//! * a create cannot set a derived attribute: one that lists it is answered with the
//!   value the account derives, not the one it sent.
//!
//! The model declares no read of a computer account, so what the store holds is observed
//! the only way this service exposes it: a second create of the same name is refused
//! against the persisted account.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    create_directory, minimal_create, ok, refused, service, PASSWORD, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// A one-time join password satisfying the modelled `ComputerPassword`
/// (`len 8..64`, pattern `^[\u0020-\u00FF]+$`). Never asserted on: nothing reports it.
const JOIN_PASSWORD: &str = "Cmp-1-Time!Pw";

/// The name the SAM (pre-Windows 2000) account name of a machine account is reported
/// under, and the name its distinguished name is reported under — the two attributes the
/// domain derives for the account itself.
///
/// Source: the recorded `CreateComputer` answer this suite's rules come from, which keys
/// them `WindowsSamName` and `DistinguishedName`.
const WINDOWS_SAM_NAME: &str = "WindowsSamName";
const DISTINGUISHED_NAME: &str = "DistinguishedName";

/// LDAP attribute names as a request may write them, each with the spelling the answer
/// must report it under, as `(as sent, as reported)`.
///
/// The display spellings are the Active Directory attribute names as the Directory
/// Service APIs spell them (`Description` is the one the recorded answer carries; the
/// rest are the same attributes of a computer object, spelled the same way). The rows
/// vary the casing a request writes — all lower, all upper, display, alternating — and
/// the last two rows are names this service recognizes no spelling of, which must come
/// back exactly as written.
const ATTRIBUTE_NAMES: &[(&str, &str)] = &[
    ("description", "Description"),
    ("DESCRIPTION", "Description"),
    ("Description", "Description"),
    ("dEsCrIpTiOn", "Description"),
    ("location", "Location"),
    ("LOCATION", "Location"),
    ("operatingSystem", "OperatingSystem"),
    ("operatingsystemversion", "OperatingSystemVersion"),
    ("OperatingSystemServicePack", "OperatingSystemServicePack"),
    ("dnshostname", "DnsHostName"),
    ("managedBy", "ManagedBy"),
    ("userprincipalname", "UserPrincipalName"),
    ("displayName", "DisplayName"),
    ("serviceprincipalname", "ServicePrincipalName"),
    ("objectClass", "ObjectClass"),
    ("whencreated", "WhenCreated"),
    ("whenChanged", "WhenChanged"),
    ("objectsid", "ObjectSid"),
    ("sim2realOwnAttr", "sim2realOwnAttr"),
    ("x-not-an-ldap-name", "x-not-an-ldap-name"),
];

/// The spellings that name an attribute the domain derives for the account itself, as
/// `(as sent, the derived attribute it names)` — the LDAP name and this service's own.
/// A create that lists one of these does not get to set it, so it is never reported from
/// the request; the account's own value is reported instead.
const DERIVED_ATTRIBUTE_SPELLINGS: &[(&str, &str)] = &[
    ("samaccountname", WINDOWS_SAM_NAME),
    ("windowssamname", WINDOWS_SAM_NAME),
    ("distinguishedname", DISTINGUISHED_NAME),
];

/// The spelling an attribute written as `sent` must be reported under: its row in
/// [`ATTRIBUTE_NAMES`] or [`DERIVED_ATTRIBUTE_SPELLINGS`], or `sent` itself when no row
/// names it, since a name the service recognizes no spelling of has none other to report.
fn reported_name(sent: &str) -> String {
    ATTRIBUTE_NAMES
        .iter()
        .chain(DERIVED_ATTRIBUTE_SPELLINGS)
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(sent))
        .map_or_else(|| sent.to_string(), |(_, reported)| (*reported).to_string())
}

/// The `ComputerAttributes` the account `request` creates must report, in order, in a
/// directory whose default computers container is `container`.
///
/// The derived SAM name comes first and the derived distinguished name last, with the
/// attributes the request applied — canonicalized, and with any the domain derives
/// dropped — in between, which is the order the recorded answer carries them in.
pub(crate) fn expected_attributes(request: &Value, container: &str) -> Value {
    let computer_name = request["ComputerName"].as_str().unwrap_or_default();
    let mut expected = vec![json!({
        "Name": WINDOWS_SAM_NAME,
        "Value": format!("{computer_name}$")
    })];
    for applied in request["ComputerAttributes"].as_array().into_iter().flatten() {
        let Some(name) = applied["Name"].as_str() else {
            expected.push(applied.clone());
            continue;
        };
        let name = reported_name(name);
        if name == WINDOWS_SAM_NAME || name == DISTINGUISHED_NAME {
            continue;
        }
        let mut entry = json!({ "Name": name });
        if let Some(value) = applied["Value"].as_str() {
            entry["Value"] = json!(value);
        }
        expected.push(entry);
    }
    let unit = request["OrganizationalUnitDistinguishedName"]
        .as_str()
        .map_or_else(|| container.to_string(), str::to_string);
    expected.push(json!({
        "Name": DISTINGUISHED_NAME,
        "Value": format!("CN={computer_name},{unit}")
    }));
    Value::Array(expected)
}

/// One `CreateComputer` that must succeed, asserting the attributes it reports are the
/// ones `container`'s directory derives for the account plus the ones the request applied.
async fn create_and_assert(
    router: &MultiServiceRouter,
    request: Value,
    container: &str,
    case: &str,
) -> Value {
    let answer = ok(router, "CreateComputer", request.clone(), case).await;
    let computer = answer["Computer"].clone();
    let expected = expected_attributes(&request, container);
    assert_eq!(
        computer["ComputerAttributes"], expected,
        "case {case:?}: expected ComputerAttributes {expected}, got {computer}"
    );
    computer
}

/// The `DC=` components a distinguished name in `domain` ends with.
fn domain_components(domain: &str) -> String {
    domain
        .split('.')
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// One row of the directory table: how the directory is created, and the container its
/// computer accounts land in when a create names no organizational unit.
struct DirectoryCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    domain: &'static str,
    container: String,
}

/// The directory table: both create operations, differing in whether the directory
/// carries a NetBIOS short name — defaulted from the domain name, named by the request,
/// or absent — which is what its default computers container follows from.
fn directory_rows() -> Vec<DirectoryCase> {
    vec![
        DirectoryCase {
            case: "a Managed Microsoft AD directory, short name defaulted",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("corp.example.com"),
            domain: "corp.example.com",
            container: format!(
                "OU=Computers,OU=CORP,{}",
                domain_components("corp.example.com")
            ),
        },
        DirectoryCase {
            case: "a Managed Microsoft AD directory with a requested short name",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "named.example.com",
                "Password": PASSWORD,
                "ShortName": "NAMED-AD",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            domain: "named.example.com",
            container: format!(
                "OU=Computers,OU=NAMED-AD,{}",
                domain_components("named.example.com")
            ),
        },
        DirectoryCase {
            case: "a Simple AD directory carrying no short name",
            operation: "CreateDirectory",
            body: minimal_create("simple.example.com"),
            domain: "simple.example.com",
            container: format!("CN=Computers,{}", domain_components("simple.example.com")),
        },
        DirectoryCase {
            case: "a Simple AD directory with a short name and a deeper domain",
            operation: "CreateDirectory",
            body: json!({
                "Name": "ad.eu.simple.example.com",
                "Password": PASSWORD,
                "Size": "Small",
                "ShortName": "EUSIMPLE"
            }),
            domain: "ad.eu.simple.example.com",
            container: format!(
                "OU=Computers,OU=EUSIMPLE,{}",
                domain_components("ad.eu.simple.example.com")
            ),
        },
    ]
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &DirectoryCase) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

/// Every directory of the table, created and past the transition its create owes, as
/// `(row, directory id)`.
async fn active_directories(router: &MultiServiceRouter) -> Vec<(DirectoryCase, String)> {
    let mut created = Vec::new();
    for row in directory_rows() {
        let id = create(router, &row).await;
        created.push((row, id));
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    created
}

#[tokio::test]
async fn a_recognized_attribute_name_is_reported_in_the_spelling_the_directory_holds_it_by() {
    let router = service();
    let directories = active_directories(&router).await;

    for (directory, directory_id) in &directories {
        for (index, (sent, reported)) in ATTRIBUTE_NAMES.iter().enumerate() {
            let case = format!("{} / the name {sent:?}", directory.case);
            // The `ComputerName` is the model's `len 1..15`, so the row index is what
            // keeps each account of one directory distinct.
            let name = format!("attr{index:02}");
            let value = format!("value-of-{sent}");
            let request = json!({
                "DirectoryId": directory_id,
                "ComputerName": name,
                "Password": JOIN_PASSWORD,
                "ComputerAttributes": [{ "Name": sent, "Value": value }]
            });
            let computer = create_and_assert(&router, request, &directory.container, &case).await;

            // Stated once more directly: the applied attribute is readable under the
            // reported spelling, carrying the value the request supplied.
            let found = computer["ComputerAttributes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {computer}"))
                .iter()
                .find(|attribute| attribute["Name"].as_str() == Some(*reported))
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                found["Value"].as_str(),
                Some(value.as_str()),
                "case {case:?}: expected {reported:?} to carry {value:?}, got {computer}"
            );
        }
    }
}

#[tokio::test]
async fn the_reported_attributes_are_the_accounts_own() {
    let router = service();
    let directories = active_directories(&router).await;

    for (directory, directory_id) in &directories {
        // The rows differ in how many attributes the create applies (none at all, an
        // empty list, one, several), in whether an applied name is one the domain derives
        // for the account itself, in whether an applied attribute carries a value, and in
        // whether the request names an organizational unit and how deep it is.
        let unit = format!("OU=Servers,OU=Prod,{}", domain_components(directory.domain));
        let rows: Vec<(&str, Value)> = vec![
            (
                "no ComputerAttributes member at all",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-none",
                    "Password": JOIN_PASSWORD
                }),
            ),
            (
                "an empty attribute list",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-empty",
                    "Password": JOIN_PASSWORD,
                    "ComputerAttributes": []
                }),
            ),
            (
                "three attributes, in the order applied",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-three",
                    "Password": JOIN_PASSWORD,
                    "ComputerAttributes": [
                        { "Name": "description", "Value": "a build agent" },
                        { "Name": "LOCATION", "Value": "rack 7" },
                        { "Name": "operatingSystemVersion", "Value": "10.0 (20348)" }
                    ]
                }),
            ),
            (
                "an attribute carrying a name and no value",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-novalue",
                    "Password": JOIN_PASSWORD,
                    "ComputerAttributes": [{ "Name": "description" }]
                }),
            ),
            (
                "an organizational unit the request names",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-unit",
                    "Password": JOIN_PASSWORD,
                    "OrganizationalUnitDistinguishedName": unit,
                    "ComputerAttributes": [{ "Name": "description", "Value": "in a unit" }]
                }),
            ),
            (
                "a request that tries to set the derived attributes",
                json!({
                    "DirectoryId": directory_id,
                    "ComputerName": "own-derived",
                    "Password": JOIN_PASSWORD,
                    "ComputerAttributes": [
                        { "Name": "WindowsSamName", "Value": "NOT-THE-ACCOUNT$" },
                        { "Name": "sAMAccountName", "Value": "NOT-THE-ACCOUNT$" },
                        { "Name": "distinguishedName", "Value": "CN=elsewhere,DC=other" },
                        { "Name": "description", "Value": "kept" }
                    ]
                }),
            ),
        ];

        for (class, request) in rows {
            let case = format!("{} / {class}", directory.case);
            let computer =
                create_and_assert(&router, request.clone(), &directory.container, &case).await;

            // The account persists: this service models no read of a computer account, so
            // the stored account is observed through the refusal a second create of the
            // same name gets.
            refused(
                &router,
                "CreateComputer",
                request.clone(),
                "EntityAlreadyExistsException",
                &case,
            )
            .await;

            // Nothing the request wrote reaches the derived attributes: both are computed
            // from the account, so the value a request offered for one is not reported.
            let attributes = computer["ComputerAttributes"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {computer}"))
                .clone();
            let expected_name = request["ComputerName"].as_str().unwrap_or_default();
            for (name, expected) in [
                (WINDOWS_SAM_NAME, format!("{expected_name}$")),
                (
                    DISTINGUISHED_NAME,
                    format!(
                        "CN={expected_name},{container}",
                        container = request["OrganizationalUnitDistinguishedName"]
                            .as_str()
                            .map_or_else(|| directory.container.clone(), str::to_string)
                    ),
                ),
            ] {
                let carried: Vec<&Value> = attributes
                    .iter()
                    .filter(|attribute| attribute["Name"].as_str() == Some(name))
                    .collect();
                assert_eq!(
                    carried.len(),
                    1,
                    "case {case:?}: expected exactly one {name} attribute, got {computer}"
                );
                assert_eq!(
                    carried[0]["Value"].as_str(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {name} {expected:?}, got {computer}"
                );
            }
        }
    }
}
