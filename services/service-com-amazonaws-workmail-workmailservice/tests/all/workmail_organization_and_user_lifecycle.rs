// SPDX-License-Identifier: CC0-1.0

//! The WorkMail organization + user lifecycle, end to end over the awsJson1_1 wire.
//!
//! Every case here drives the service the way an SDK client does: a `POST` carrying an
//! `X-Amz-Target` and a JSON body through the multi-service router, with the response
//! read back out of the JSON body — not by calling a handler directly. So a response
//! member that is not actually serialized, or an error whose HTTP status or wire code is
//! wrong, fails here.
//!
//! The suite asserts RULES over sets of inputs rather than one recorded request: the
//! organization id shape over a table of aliases, pagination over a table of page sizes,
//! the filter semantics over a table of filters, and the not-found errors over a table of
//! operations.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header: the router resolves the target service from its
/// credential scope, and the handler reads the ARN region from the same scope.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20261021/eu-west-1/workmail/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An `OrganizationId` that satisfies the modelled pattern but names no organization.
const ABSENT_ORGANIZATION: &str = "m-ffffffffffffffffffffffffffffffff";

/// One emulator: a single shared engine with the WorkMail service mounted on it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_workmail_workmailservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, returning the HTTP status and the decoded JSON body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", SIGNED_FOR_EU_WEST_1)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("WorkMailService.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let decoded = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, decoded)
    }

    /// Call `operation`, requiring HTTP 200.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, decoded) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {decoded}"
        );
        decoded
    }

    /// Call `operation`, requiring HTTP 400 and the wire error code `code`.
    async fn error(&self, operation: &str, body: Value, code: &str) -> Value {
        let (status, decoded) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "{operation}: expected HTTP 400 {code}, got {status} with body {decoded}"
        );
        assert_eq!(
            decoded["__type"].as_str(),
            Some(code),
            "{operation}: expected error code {code}, got body {decoded}"
        );
        decoded
    }

    /// `CreateOrganization` with just an alias, returning the minted id.
    async fn create_organization(&self, alias: &str) -> String {
        let out = self
            .ok("CreateOrganization", json!({ "Alias": alias }))
            .await;
        out["OrganizationId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateOrganization({alias}) returned no id: {out}"))
            .to_string()
    }

    /// `CreateUser` with the required members only, returning the minted id.
    async fn create_user(&self, organization_id: &str, name: &str, display_name: &str) -> String {
        let out = self
            .ok(
                "CreateUser",
                json!({
                    "OrganizationId": organization_id,
                    "Name": name,
                    "DisplayName": display_name,
                }),
            )
            .await;
        out["UserId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateUser({name}) returned no id: {out}"))
            .to_string()
    }
}

/// Whether `value` is an `OrganizationId`: the `^m-[0-9a-f]{32}$` the model declares.
fn is_organization_id(value: &str) -> bool {
    value.strip_prefix("m-").is_some_and(|hex| {
        hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Whether `value` is a `DirectoryId`: the `^d-[0-9a-f]{10}$` the model declares.
fn is_directory_id(value: &str) -> bool {
    value.strip_prefix("d-").is_some_and(|hex| {
        hex.len() == 10 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// The `Name` of every user in a `ListUsers` page, in the order returned.
fn page_names(page: &Value) -> Vec<String> {
    page["Users"]
        .as_array()
        .expect("ListUsers returns a Users list")
        .iter()
        .map(|user| {
            user["Name"]
                .as_str()
                .expect("every listed user carries a Name")
                .to_string()
        })
        .collect()
}

/// The whole happy path of the issue: create an organization, read it back, create a
/// user, read it back, register it, list, delete, and confirm the delete.
#[tokio::test]
async fn the_organization_and_user_lifecycle_round_trips() {
    let emulator = Emulator::new();

    let organization_id = emulator.create_organization("bootstrap-org").await;
    assert!(
        is_organization_id(&organization_id),
        "CreateOrganization must mint ^m-[0-9a-f]{{32}}$, got {organization_id:?}"
    );

    let described = emulator
        .ok(
            "DescribeOrganization",
            json!({ "OrganizationId": organization_id }),
        )
        .await;
    assert_eq!(
        described["OrganizationId"].as_str(),
        Some(organization_id.as_str()),
        "DescribeOrganization must report the id it was asked for: {described}"
    );
    assert_eq!(
        described["Alias"].as_str(),
        Some("bootstrap-org"),
        "DescribeOrganization must read the stored alias back: {described}"
    );
    assert_eq!(
        described["State"].as_str(),
        Some("Active"),
        "a created organization is Active: {described}"
    );
    assert_eq!(
        described["ARN"].as_str(),
        Some(format!("arn:aws:workmail:eu-west-1:{ACCOUNT}:organization/{organization_id}").as_str()),
        "the ARN is built from the request's signing region and the minted id: {described}"
    );
    assert_eq!(
        described["InteroperabilityEnabled"], json!(false),
        "interoperability was not requested: {described}"
    );
    assert!(
        described["CompletedDate"].is_number(),
        "DescribeOrganization reports when the organization completed: {described}"
    );
    let mail_domain = described["DefaultMailDomain"]
        .as_str()
        .unwrap_or_else(|| panic!("an organization has a default mail domain: {described}"))
        .to_string();

    // Two users, so the delete below is observable against a list with a survivor.
    let alice = emulator
        .create_user(&organization_id, "alice", "Alice A")
        .await;
    let bob = emulator.create_user(&organization_id, "bob", "Bob B").await;
    assert_ne!(alice, bob, "each CreateUser mints its own id");
    for (name, id) in [("alice", &alice), ("bob", &bob)] {
        assert!(
            id.len() >= 12 && id.len() <= 256,
            "case {name}: a WorkMailIdentifier is 12..256 characters, got {id:?}"
        );
    }

    let described_alice = emulator
        .ok(
            "DescribeUser",
            json!({ "OrganizationId": organization_id, "UserId": alice }),
        )
        .await;
    assert_eq!(
        described_alice["Name"].as_str(),
        Some("alice"),
        "DescribeUser reads the stored name back: {described_alice}"
    );
    assert_eq!(
        described_alice["DisplayName"].as_str(),
        Some("Alice A"),
        "DescribeUser reads the stored display name back: {described_alice}"
    );
    assert_eq!(
        described_alice["State"].as_str(),
        Some("DISABLED"),
        "a created-but-unregistered user is DISABLED: {described_alice}"
    );
    assert_eq!(
        described_alice["UserRole"].as_str(),
        Some("USER"),
        "a user created without a Role defaults to USER: {described_alice}"
    );
    assert!(
        described_alice["Email"].is_null(),
        "an unregistered user has no email address: {described_alice}"
    );

    // RegisterToWorkMail is what makes the state transition observable.
    let address = format!("alice@{mail_domain}");
    emulator
        .ok(
            "RegisterToWorkMail",
            json!({
                "OrganizationId": organization_id,
                "EntityId": alice,
                "Email": address,
            }),
        )
        .await;
    let registered = emulator
        .ok(
            "DescribeUser",
            json!({ "OrganizationId": organization_id, "UserId": alice }),
        )
        .await;
    assert_eq!(
        registered["State"].as_str(),
        Some("ENABLED"),
        "a registered user is ENABLED: {registered}"
    );
    assert_eq!(
        registered["Email"].as_str(),
        Some(address.as_str()),
        "a registered user carries the address it was registered with: {registered}"
    );
    assert!(
        registered["EnabledDate"].is_number(),
        "a registered user carries an EnabledDate: {registered}"
    );

    let listed = emulator
        .ok("ListUsers", json!({ "OrganizationId": organization_id }))
        .await;
    assert_eq!(
        page_names(&listed),
        vec!["alice".to_string(), "bob".to_string()],
        "ListUsers returns the organization's stored users: {listed}"
    );
    assert!(
        listed["NextToken"].is_null(),
        "a single page that holds everything returns no cursor: {listed}"
    );

    // "Before you can delete a user, the user state must be DISABLED."
    emulator
        .error(
            "DeleteUser",
            json!({ "OrganizationId": organization_id, "UserId": alice }),
            "EntityStateException",
        )
        .await;

    emulator
        .ok(
            "DeleteUser",
            json!({ "OrganizationId": organization_id, "UserId": bob }),
        )
        .await;
    emulator
        .error(
            "DescribeUser",
            json!({ "OrganizationId": organization_id, "UserId": bob }),
            "EntityNotFoundException",
        )
        .await;
    let after_delete = emulator
        .ok("ListUsers", json!({ "OrganizationId": organization_id }))
        .await;
    assert_eq!(
        page_names(&after_delete),
        vec!["alice".to_string()],
        "a deleted user is gone from ListUsers: {after_delete}"
    );
    // `@idempotent`: deleting what is already deleted succeeds.
    emulator
        .ok(
            "DeleteUser",
            json!({ "OrganizationId": organization_id, "UserId": bob }),
        )
        .await;
}

/// Every accepted alias mints its own organization id of the modelled shape, and the
/// alias is read back from the store. The rows span the `OrganizationName` pattern:
/// shortest accepted value, a single run, digits, several hyphen-joined runs, mixed case,
/// and the 62-character maximum.
#[tokio::test]
async fn every_alias_mints_its_own_organization() {
    let emulator = Emulator::new();
    let long_alias = "a".repeat(62);
    let aliases = [
        "a",
        "bootstrap",
        "0rg2",
        "x1-y2-z3",
        "MixedCaseOrg",
        long_alias.as_str(),
    ];

    let mut minted: Vec<String> = Vec::new();
    for alias in aliases {
        let id = emulator.create_organization(alias).await;
        assert!(
            is_organization_id(&id),
            "case {alias:?}: expected ^m-[0-9a-f]{{32}}$, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {alias:?}: id {id:?} was already minted for another alias"
        );
        let described = emulator
            .ok("DescribeOrganization", json!({ "OrganizationId": id }))
            .await;
        assert_eq!(
            described["Alias"].as_str(),
            Some(alias),
            "case {alias:?}: DescribeOrganization must read the stored alias back: {described}"
        );
        minted.push(id);
    }
    assert_eq!(
        minted.len(),
        aliases.len(),
        "one organization per alias, got {minted:?}"
    );
}

/// An alias names at most one organization: a second create with a taken alias is
/// refused, and refusing it leaves the first organization untouched.
#[tokio::test]
async fn a_taken_alias_is_refused() {
    let emulator = Emulator::new();
    for alias in ["a", "bootstrap-org", "MixedCaseOrg"] {
        let first = emulator.create_organization(alias).await;
        emulator
            .error(
                "CreateOrganization",
                json!({ "Alias": alias }),
                "NameAvailabilityException",
            )
            .await;
        let described = emulator
            .ok("DescribeOrganization", json!({ "OrganizationId": first }))
            .await;
        assert_eq!(
            described["Alias"].as_str(),
            Some(alias),
            "case {alias:?}: the refused create must not disturb the first organization: \
             {described}"
        );
    }
}

/// `ClientToken` is an `@idempotencyToken`: a retry carrying the same token replays the
/// first call's id instead of minting a second organization.
#[tokio::test]
async fn a_repeated_client_token_replays_the_same_organization() {
    let emulator = Emulator::new();
    for (token, alias) in [
        ("token-one", "retry-one"),
        ("TOKEN-2", "retry-two"),
        ("~token!3", "retry-three"),
    ] {
        let request = json!({ "Alias": alias, "ClientToken": token });
        let first = emulator.ok("CreateOrganization", request.clone()).await;
        let retried = emulator.ok("CreateOrganization", request).await;
        assert_eq!(
            first["OrganizationId"], retried["OrganizationId"],
            "case {token:?}: a retry must replay the first id, got {first} then {retried}"
        );
        // A second organization would have taken the alias, so a *different* token with
        // the same alias is still refused — proving the retry created nothing new.
        emulator
            .error(
                "CreateOrganization",
                json!({ "Alias": alias, "ClientToken": format!("{token}-other") }),
                "NameAvailabilityException",
            )
            .await;
    }
}

/// Where the organization's directory came from decides what `DescribeOrganization`
/// reports: WorkMail mints a directory when the caller associates none, and a directory
/// serves at most one organization.
#[tokio::test]
async fn the_directory_source_decides_what_describe_reports() {
    let emulator = Emulator::new();

    let minted_org = emulator.create_organization("minted-directory").await;
    let minted = emulator
        .ok(
            "DescribeOrganization",
            json!({ "OrganizationId": minted_org }),
        )
        .await;
    let minted_directory = minted["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("WorkMail creates a directory for you: {minted}"));
    assert!(
        is_directory_id(minted_directory),
        "a minted directory id matches ^d-[0-9a-f]{{10}}$, got {minted_directory:?}"
    );

    // Each row is a distinct caller-supplied directory id, echoed back as given.
    for (alias, directory_id) in [
        ("brought-one", "d-0123456789"),
        ("brought-two", "d-abcdef0123"),
    ] {
        let out = emulator
            .ok(
                "CreateOrganization",
                json!({ "Alias": alias, "DirectoryId": directory_id }),
            )
            .await;
        let id = out["OrganizationId"].as_str().expect("an organization id");
        let described = emulator
            .ok("DescribeOrganization", json!({ "OrganizationId": id }))
            .await;
        assert_eq!(
            described["DirectoryId"].as_str(),
            Some(directory_id),
            "case {alias:?}: the associated directory is reported as given: {described}"
        );
        assert_ne!(
            described["DirectoryType"], minted["DirectoryType"],
            "case {alias:?}: an associated directory is a different DirectoryType from the \
             one WorkMail mints: {described}"
        );
        assert!(
            described["DirectoryType"].is_string(),
            "case {alias:?}: an organization reports a DirectoryType: {described}"
        );

        // "The directory is already in use by another WorkMail organization."
        emulator
            .error(
                "CreateOrganization",
                json!({ "Alias": format!("{alias}-again"), "DirectoryId": directory_id }),
                "DirectoryInUseException",
            )
            .await;
    }
}

/// "When true, allows organization interoperability ... If true, you must include a AD
/// Connector directory ID in the request."
#[tokio::test]
async fn interoperability_requires_a_directory_and_is_reported_as_created() {
    let emulator = Emulator::new();

    for alias in ["interop-none", "interop-other"] {
        emulator
            .error(
                "CreateOrganization",
                json!({ "Alias": alias, "EnableInteroperability": true }),
                "InvalidParameterException",
            )
            .await;
    }

    for (alias, directory_id, interoperability) in [
        ("interop-on", "d-1111111111", true),
        ("interop-off", "d-2222222222", false),
    ] {
        let out = emulator
            .ok(
                "CreateOrganization",
                json!({
                    "Alias": alias,
                    "DirectoryId": directory_id,
                    "EnableInteroperability": interoperability,
                }),
            )
            .await;
        let id = out["OrganizationId"].as_str().expect("an organization id");
        let described = emulator
            .ok("DescribeOrganization", json!({ "OrganizationId": id }))
            .await;
        assert_eq!(
            described["InteroperabilityEnabled"],
            json!(interoperability),
            "case {alias:?}: InteroperabilityEnabled is reported as created: {described}"
        );
    }
}

/// `ListUsers` walks every user of the organization exactly once, in one order, whatever
/// the page size — and only that organization's users.
#[tokio::test]
async fn list_users_pages_through_every_user_in_a_stable_order() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("paged").await;
    let other_id = emulator.create_organization("other").await;
    for name in ["ann", "bob", "cal", "dee", "eve"] {
        emulator
            .create_user(&organization_id, name, &format!("{name} display"))
            .await;
    }
    emulator.create_user(&other_id, "zed", "Zed").await;

    let whole = page_names(
        &emulator
            .ok("ListUsers", json!({ "OrganizationId": organization_id }))
            .await,
    );
    assert_eq!(
        whole.len(),
        5,
        "only this organization's five users are listed, got {whole:?}"
    );
    assert!(
        !whole.contains(&"zed".to_string()),
        "another organization's user must not leak in, got {whole:?}"
    );

    // Rows: every page size from one-per-page up to more than the whole collection.
    for page_size in [1_u64, 2, 3, 4, 5, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({
                "OrganizationId": organization_id,
                "MaxResults": page_size,
            });
            if let Some(cursor) = &token {
                request["NextToken"] = json!(cursor);
            }
            let page = emulator.ok("ListUsers", request).await;
            let names = page_names(&page);
            assert!(
                names.len() as u64 <= page_size,
                "case MaxResults={page_size}: a page returned {} users: {page}",
                names.len()
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults={page_size}: pagination did not terminate, saw {seen:?}"
            );
            match page["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, whole,
            "case MaxResults={page_size}: paging must yield the same users in the same \
             order as one page"
        );
    }
}

/// A cursor this service did not mint is a bad parameter, not an empty page.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("tokens").await;
    emulator.create_user(&organization_id, "ann", "Ann").await;

    for token in ["not-a-token", "zz", "///"] {
        emulator
            .error(
                "ListUsers",
                json!({ "OrganizationId": organization_id, "NextToken": token }),
                "InvalidParameterException",
            )
            .await;
    }
}

/// `ListUsersFilters` selects on a prefix of the user name, display name or primary email
/// address, and on an exact `EntityState`. Each row is one filter and the names it must
/// select out of the same five users.
#[tokio::test]
async fn list_users_filters_select_on_prefix_and_state() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("filtered").await;
    let described = emulator
        .ok(
            "DescribeOrganization",
            json!({ "OrganizationId": organization_id }),
        )
        .await;
    let domain = described["DefaultMailDomain"]
        .as_str()
        .expect("a default mail domain")
        .to_string();

    for (name, display) in [
        ("anna", "Anna Smith"),
        ("annika", "Annika Jones"),
        ("bob", "Bob Smith"),
        ("carla", "Carla Smith"),
        ("dave", "Dave Jones"),
    ] {
        emulator.create_user(&organization_id, name, display).await;
    }
    // Register two of them, so the state filter has both classes to separate and the
    // email filter has addresses to match.
    for name in ["anna", "bob"] {
        emulator
            .ok(
                "RegisterToWorkMail",
                json!({
                    "OrganizationId": organization_id,
                    "EntityId": name,
                    "Email": format!("{name}@{domain}"),
                }),
            )
            .await;
    }

    let cases: [(Value, Vec<&str>); 7] = [
        (json!({ "UsernamePrefix": "ann" }), vec!["anna", "annika"]),
        (json!({ "UsernamePrefix": "annik" }), vec!["annika"]),
        (json!({ "UsernamePrefix": "z" }), vec![]),
        (
            json!({ "DisplayNamePrefix": "Anna" }),
            vec!["anna"],
        ),
        (
            json!({ "PrimaryEmailPrefix": "bob@" }),
            vec!["bob"],
        ),
        (
            json!({ "State": "ENABLED" }),
            vec!["anna", "bob"],
        ),
        (
            json!({ "State": "DISABLED" }),
            vec!["annika", "carla", "dave"],
        ),
    ];

    for (filters, expected) in cases {
        let page = emulator
            .ok(
                "ListUsers",
                json!({ "OrganizationId": organization_id, "Filters": filters }),
            )
            .await;
        let names = page_names(&page);
        let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
        assert_eq!(
            names, expected,
            "case {filters}: expected {expected:?}, got {names:?}"
        );
    }
}

/// Registration binds an address inside one of the organization's own mail domains, and
/// binds it to exactly one entity.
#[tokio::test]
async fn registration_enforces_the_organizations_domains_and_address_uniqueness() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("mailrules").await;
    let described = emulator
        .ok(
            "DescribeOrganization",
            json!({ "OrganizationId": organization_id }),
        )
        .await;
    let domain = described["DefaultMailDomain"]
        .as_str()
        .expect("a default mail domain")
        .to_string();
    emulator.create_user(&organization_id, "ann", "Ann").await;
    emulator.create_user(&organization_id, "bob", "Bob").await;

    // "The domain specified is not found in your organization."
    for foreign in ["example.com", "mail.example.org", "another.awsapps.com"] {
        emulator
            .error(
                "RegisterToWorkMail",
                json!({
                    "OrganizationId": organization_id,
                    "EntityId": "ann",
                    "Email": format!("ann@{foreign}"),
                }),
                "MailDomainNotFoundException",
            )
            .await;
    }

    let address = format!("ann@{domain}");
    emulator
        .ok(
            "RegisterToWorkMail",
            json!({
                "OrganizationId": organization_id,
                "EntityId": "ann",
                "Email": address,
            }),
        )
        .await;

    // "The email address that you're trying to assign is already created for a different
    // user" — including a case variant, since mail addresses are case-insensitive.
    for taken in [address.clone(), address.to_uppercase()] {
        emulator
            .error(
                "RegisterToWorkMail",
                json!({
                    "OrganizationId": organization_id,
                    "EntityId": "bob",
                    "Email": taken,
                }),
                "EmailAddressInUseException",
            )
            .await;
    }

    // "It performs no change if the user, group, or resource is enabled."
    emulator
        .ok(
            "RegisterToWorkMail",
            json!({
                "OrganizationId": organization_id,
                "EntityId": "ann",
                "Email": address,
            }),
        )
        .await;
    let still = emulator
        .ok(
            "DescribeUser",
            json!({ "OrganizationId": organization_id, "UserId": "ann" }),
        )
        .await;
    assert_eq!(
        still["Email"].as_str(),
        Some(address.as_str()),
        "re-registering an enabled user changes nothing: {still}"
    );
}

/// A user name is unique within its organization, and only within it.
#[tokio::test]
async fn a_user_name_is_unique_within_its_organization() {
    let emulator = Emulator::new();
    let first = emulator.create_organization("names-one").await;
    let second = emulator.create_organization("names-two").await;

    for name in ["ann", "bob.smith", "carla-3"] {
        emulator.create_user(&first, name, "Display").await;
        emulator
            .error(
                "CreateUser",
                json!({ "OrganizationId": first, "Name": name, "DisplayName": "Other" }),
                "NameAvailabilityException",
            )
            .await;
        // The same name in another organization is free.
        emulator.create_user(&second, name, "Display").await;
    }
}

/// "You cannot pass SYSTEM_USER or RESOURCE role in a single request. When a user role is
/// not selected, the default role of USER is selected."
#[tokio::test]
async fn create_user_honours_the_role_rules() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("roles").await;

    for role in ["SYSTEM_USER", "RESOURCE"] {
        emulator
            .error(
                "CreateUser",
                json!({
                    "OrganizationId": organization_id,
                    "Name": format!("user-{role}"),
                    "DisplayName": "Display",
                    "Role": role,
                }),
                "InvalidParameterException",
            )
            .await;
    }

    // Rows: an explicit accepted role, and no role at all (which defaults to USER).
    for (name, requested, expected) in [
        ("remote", Some("REMOTE_USER"), "REMOTE_USER"),
        ("plain", Some("USER"), "USER"),
        ("defaulted", None, "USER"),
    ] {
        let mut request = json!({
            "OrganizationId": organization_id,
            "Name": name,
            "DisplayName": "Display",
        });
        if let Some(role) = requested {
            request["Role"] = json!(role);
        }
        let created = emulator.ok("CreateUser", request).await;
        let user_id = created["UserId"].as_str().expect("a user id");
        let described = emulator
            .ok(
                "DescribeUser",
                json!({ "OrganizationId": organization_id, "UserId": user_id }),
            )
            .await;
        assert_eq!(
            described["UserRole"].as_str(),
            Some(expected),
            "case {name:?}: expected UserRole {expected}, got {described}"
        );
    }
}

/// Every operation that resolves an organization or an entity reports the modelled
/// not-found error for an id nothing in the store carries — never a 501 and never an
/// empty 200.
#[tokio::test]
async fn an_unknown_id_reports_the_modelled_not_found_error() {
    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("present").await;
    let absent_user = "00000000-0000-4000-8000-000000000000";

    let cases: [(&str, Value, &str); 7] = [
        (
            "DescribeOrganization",
            json!({ "OrganizationId": ABSENT_ORGANIZATION }),
            "OrganizationNotFoundException",
        ),
        (
            "CreateUser",
            json!({
                "OrganizationId": ABSENT_ORGANIZATION,
                "Name": "ann",
                "DisplayName": "Ann",
            }),
            "OrganizationNotFoundException",
        ),
        (
            "DescribeUser",
            json!({ "OrganizationId": ABSENT_ORGANIZATION, "UserId": absent_user }),
            "OrganizationNotFoundException",
        ),
        (
            "ListUsers",
            json!({ "OrganizationId": ABSENT_ORGANIZATION }),
            "OrganizationNotFoundException",
        ),
        (
            "DeleteUser",
            json!({ "OrganizationId": ABSENT_ORGANIZATION, "UserId": absent_user }),
            "OrganizationNotFoundException",
        ),
        (
            "DescribeUser",
            json!({ "OrganizationId": organization_id, "UserId": absent_user }),
            "EntityNotFoundException",
        ),
        (
            "RegisterToWorkMail",
            json!({
                "OrganizationId": organization_id,
                "EntityId": absent_user,
                "Email": "ann@present.awsapps.com",
            }),
            "EntityNotFoundException",
        ),
    ];

    for (operation, body, expected) in cases {
        emulator.error(operation, body, expected).await;
    }
}

/// The store's growth is bounded: nothing evicts an organization, so `CreateOrganization`
/// refuses past the emulator's own ceiling — and the reads keep working at the ceiling.
#[tokio::test]
async fn the_organization_store_refuses_growth_past_its_ceiling() {
    use service_com_amazonaws_workmail_workmailservice::state::MAX_ORGANIZATIONS;

    let emulator = Emulator::new();
    let mut last = String::new();
    for index in 0..MAX_ORGANIZATIONS {
        last = emulator.create_organization(&format!("org{index}")).await;
    }
    emulator
        .error(
            "CreateOrganization",
            json!({ "Alias": "one-too-many" }),
            "LimitExceededException",
        )
        .await;
    // A read at the ceiling still answers what was created.
    let described = emulator
        .ok("DescribeOrganization", json!({ "OrganizationId": last }))
        .await;
    assert_eq!(
        described["Alias"].as_str(),
        Some(format!("org{}", MAX_ORGANIZATIONS - 1).as_str()),
        "the last organization created is still readable at the ceiling: {described}"
    );
}

/// The second bound: one organization holds at most `MAX_USERS_PER_ORGANIZATION` users.
/// `CreateUser` models no limit error, so the refusal is the framework's
/// `ValidationException`.
#[tokio::test]
async fn the_user_store_refuses_growth_past_its_ceiling() {
    use service_com_amazonaws_workmail_workmailservice::state::MAX_USERS_PER_ORGANIZATION;

    let emulator = Emulator::new();
    let organization_id = emulator.create_organization("capped").await;
    for index in 0..MAX_USERS_PER_ORGANIZATION {
        emulator
            .create_user(&organization_id, &format!("user{index}"), "Display")
            .await;
    }
    emulator
        .error(
            "CreateUser",
            json!({
                "OrganizationId": organization_id,
                "Name": "one-too-many",
                "DisplayName": "Display",
            }),
            "ValidationException",
        )
        .await;
    // A read at the ceiling still answers, and still one page at a time.
    let page = emulator
        .ok(
            "ListUsers",
            json!({ "OrganizationId": organization_id, "MaxResults": 100 }),
        )
        .await;
    assert_eq!(
        page_names(&page).len(),
        100,
        "a full page is still served at the ceiling: {}",
        page["NextToken"]
    );
}

/// Two organizations in one emulator keep their users apart: an id minted in one is not
/// resolvable from the other.
#[tokio::test]
async fn users_are_scoped_to_their_organization() {
    let emulator = Emulator::new();
    let first = emulator.create_organization("scope-one").await;
    let second = emulator.create_organization("scope-two").await;
    let user = emulator.create_user(&first, "ann", "Ann").await;

    emulator
        .error(
            "DescribeUser",
            json!({ "OrganizationId": second, "UserId": user }),
            "EntityNotFoundException",
        )
        .await;
    let listed = emulator
        .ok("ListUsers", json!({ "OrganizationId": second }))
        .await;
    assert_eq!(
        page_names(&listed),
        Vec::<String>::new(),
        "the second organization owns no users: {listed}"
    );
}
