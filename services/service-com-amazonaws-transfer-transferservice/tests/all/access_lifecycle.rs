// SPDX-License-Identifier: CC0-1.0

//! A server's ACCESSES — the directory groups allowed to transfer files — over the
//! whole wire path.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. An access names a group in the SERVER'S DIRECTORY, so `CreateAccess` is served
//!    only for a server whose `IdentityProviderType` is the directory-backed one
//!    (`AWS_DIRECTORY_SERVICE`) and refused for every other one with the modelled
//!    `InvalidRequestException` (HTTP 400) naming that server's own type — and the
//!    refusal persists nothing.
//! 2. The properties a create stores are what `DescribeAccess` reports back, keyed by
//!    `(ServerId, ExternalId)`: `HomeDirectoryType` resolved once by the write, the
//!    landing directory, the session policy and the POSIX profile as sent, and
//!    `HomeDirectoryMappings` absent rather than empty when the access has none.
//! 3. An access record is addressed by its own identifier: a server the store does not
//!    hold, an `ExternalId` it has no access for, and a second create of the same pair
//!    are each refused with the modelled error for that case, and a server's accesses
//!    go with the server when it is deleted.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{assert_members, member_of, Emulator, ROLE};

/// Satisfies `DirectoryId`'s modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`.
const DIRECTORY_ID: &str = "d-0123456789";

/// Satisfies `Function`'s modelled `len 1..170`, pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_IDP: &str = "arn:aws:lambda:us-east-1:000000000000:function:idp";

/// A group SID satisfying `ExternalId`'s modelled `len 1..256`, pattern
/// `^S-1-[\d-]+$` — the form `Get-ADGroup` reports, per the member's documentation.
const GROUP_SID: &str = "S-1-1-12-1234567890-123456789-1234567890-1234";

/// A second, differently shaped `ExternalId` the same pattern admits: the shortest
/// legal form.
const SHORT_SID: &str = "S-1-0";

/// A `ServerId` satisfying the modelled `^s-([0-9a-f]{17})$` that no `CreateServer`
/// ever minted.
const UNMINTED_SERVER_ID: &str = "s-0000000000000dead";

/// Create a server whose identity provider is the directory one an access needs, and
/// return its `ServerId`.
async fn directory_server(emulator: &Emulator) -> String {
    emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
        }))
        .await
}

/// `DescribeAccess` for one access, asserted to have been served, with the
/// `DescribedAccess` it reported.
async fn described_access(emulator: &Emulator, server_id: &str, external_id: &str) -> Value {
    let answer = emulator
        .call(
            "DescribeAccess",
            json!({"ServerId": server_id, "ExternalId": external_id}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeAccess {external_id}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.get("ServerId").as_str(),
        Some(server_id),
        "DescribeAccess {external_id}: expected ServerId {server_id:?}, got {}",
        answer.body
    );
    answer.get("Access").clone()
}

/// RULE 1: `CreateAccess` is served only on a directory-backed server, and refused on
/// every other identity provider type with HTTP 400 `InvalidRequestException` naming
/// that server's type.
///
/// The rows span the WHOLE `IdentityProviderType` value set — `[SERVICE_MANAGED,
/// API_GATEWAY, AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`, per
/// `smithy_query(query="shape",shape="IdentityProviderType")` — plus the absent member
/// that resolves to `SERVICE_MANAGED`, each with the `IdentityProviderDetails` that
/// type requires.
#[tokio::test]
async fn create_access_is_served_only_on_a_directory_backed_server() {
    let emulator = Emulator::new();

    // (case, the CreateServer request, the IdP type the server resolves to, whether an
    // access may be created on it)
    let cases: Vec<(&str, Value, &str, bool)> = vec![
        (
            "no IdentityProviderType at all, which resolves to SERVICE_MANAGED",
            json!({}),
            "SERVICE_MANAGED",
            false,
        ),
        (
            "SERVICE_MANAGED",
            json!({"IdentityProviderType": "SERVICE_MANAGED"}),
            "SERVICE_MANAGED",
            false,
        ),
        (
            "API_GATEWAY",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
            "API_GATEWAY",
            false,
        ),
        (
            "AWS_LAMBDA",
            json!({
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
            }),
            "AWS_LAMBDA",
            false,
        ),
        (
            "AWS_DIRECTORY_SERVICE",
            json!({
                "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
            }),
            "AWS_DIRECTORY_SERVICE",
            true,
        ),
    ];

    for (case, server_request, idp_type, servable) in cases {
        let server_id = emulator.create_server(server_request).await;
        let answer = emulator
            .call(
                "CreateAccess",
                json!({"ServerId": server_id, "ExternalId": GROUP_SID, "Role": ROLE}),
            )
            .await;

        if servable {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.get("ExternalId").as_str(),
                Some(GROUP_SID),
                "case {case:?}: expected the created ExternalId back, got {}",
                answer.body
            );
            assert_eq!(
                answer.get("ServerId").as_str(),
                Some(server_id.as_str()),
                "case {case:?}: expected the server id back, got {}",
                answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The refusal is the ONE rule every access operation shares
        // (`state::accesses_require_directory_provider`, which the implemented
        // `DeleteAccess` applies too): it names the member, the type an access
        // requires, and the type THIS server is configured with — so each row's
        // wording is produced from that server's own configuration.
        for fragment in [
            "IdentityProviderType",
            "AWS_DIRECTORY_SERVICE",
            idp_type,
            server_id.as_str(),
        ] {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
            );
        }
        assert!(
            !message.contains(GROUP_SID),
            "case {case:?}: the ExternalId was never looked up, so it must not appear in \
             the refusal; got {message:?}"
        );

        // The refusal persisted nothing: the access is not readable afterwards.
        let read_back = emulator
            .call(
                "DescribeAccess",
                json!({"ServerId": server_id, "ExternalId": GROUP_SID}),
            )
            .await;
        assert_eq!(
            read_back.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: a refused create must persist nothing, got {}",
            read_back.body
        );
    }
}

/// RULE 2: every property a create stores is what `DescribeAccess` reports back for
/// that `(ServerId, ExternalId)`.
///
/// The rows differ along which optional members the create carried, the
/// `HomeDirectoryType` it resolves to (sent `PATH`, sent `LOGICAL` with mappings, or
/// absent, which resolves to `PATH`), how many mappings it holds, and the shape and
/// length of the `ExternalId`.
#[tokio::test]
async fn a_created_access_reads_back_through_describe_access() {
    let emulator = Emulator::new();
    let server_id = directory_server(&emulator).await;

    let posix = json!({"Uid": 1000, "Gid": 1000, "SecondaryGids": [2000, 2001]});
    let mappings = json!([
        {"Entry": "/directory1", "Target": "/bucket_name/home/mydirectory"},
        {"Entry": "/directory2", "Target": "/bucket_name/home/other"},
    ]);

    // (case, the extra members of the CreateAccess request, the DescribedAccess the
    // reader must report — a null expectation means the member must be absent)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "only the required members",
            GROUP_SID,
            json!({}),
            json!({
                "ExternalId": GROUP_SID,
                "Role": ROLE,
                "HomeDirectoryType": "PATH",
                "HomeDirectory": null,
                "HomeDirectoryMappings": null,
                "Policy": null,
                "PosixProfile": null,
            }),
        ),
        (
            "a PATH landing directory and a session policy",
            SHORT_SID,
            json!({
                "HomeDirectoryType": "PATH",
                "HomeDirectory": "/bucket_name/home/short",
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
            json!({
                "ExternalId": SHORT_SID,
                "HomeDirectoryType": "PATH",
                "HomeDirectory": "/bucket_name/home/short",
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "HomeDirectoryMappings": null,
            }),
        ),
        (
            "LOGICAL with two mappings and a POSIX profile",
            "S-1-5-21-1111111111-2222222222-3333333333-4444",
            json!({
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": mappings,
                "PosixProfile": posix,
            }),
            json!({
                "ExternalId": "S-1-5-21-1111111111-2222222222-3333333333-4444",
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": mappings,
                "PosixProfile": posix,
                "HomeDirectory": null,
            }),
        ),
        (
            "LOGICAL with a single chroot mapping",
            "S-1-5-32-544",
            json!({
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket_name/home/root"}],
            }),
            json!({
                "ExternalId": "S-1-5-32-544",
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket_name/home/root"}],
            }),
        ),
    ];

    for (case, external_id, extra, expected) in &cases {
        let mut request = json!({"ServerId": server_id, "ExternalId": external_id, "Role": ROLE});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            object.insert(member.clone(), value.clone());
        }

        let answer = emulator.call("CreateAccess", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        let access = described_access(&emulator, &server_id, external_id).await;
        assert_members(case, &access, expected);
    }

    // Every access of the table is still readable after the later creates, each under
    // its own `ExternalId`: the records do not overwrite one another.
    for (case, external_id, _, expected) in &cases {
        let access = described_access(&emulator, &server_id, external_id).await;
        assert_members(&format!("{case} after the whole table"), &access, expected);
    }
}

/// RULE 2 (the other half of the documented pairing): a create whose
/// `HomeDirectoryType` and `HomeDirectoryMappings` break the documented pairing is
/// refused with `InvalidRequestException` and stores nothing.
///
/// The rows differ along which side of the pairing is unsatisfied and how many
/// mappings were sent.
#[tokio::test]
async fn create_access_enforces_the_home_directory_pairing() {
    let emulator = Emulator::new();
    let server_id = directory_server(&emulator).await;

    // Each row carries what the shared rule's refusal must NAME: the member always,
    // plus the condition in the direction whose rule states one.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "LOGICAL with no mappings",
            json!({"HomeDirectoryType": "LOGICAL"}),
            // This direction states `HomeDirectoryMappings`' own modelled minimum
            // (`len 1..50000`), which is what an absent or empty list violates.
            vec!["HomeDirectoryMappings"],
        ),
        (
            "mappings with no type at all",
            json!({"HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket/home"}]}),
            // The opposite direction has no modelled constraint of its own, so it
            // names the condition the value may be sent under.
            vec!["HomeDirectoryMappings", "LOGICAL"],
        ),
        (
            "two mappings with HomeDirectoryType PATH",
            json!({
                "HomeDirectoryType": "PATH",
                "HomeDirectoryMappings": [
                    {"Entry": "/a", "Target": "/bucket/a"},
                    {"Entry": "/b", "Target": "/bucket/b"},
                ],
            }),
            vec!["HomeDirectoryMappings", "LOGICAL"],
        ),
    ];

    for (case, extra, named) in cases {
        let mut request = json!({"ServerId": server_id, "ExternalId": GROUP_SID, "Role": ROLE});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            object.insert(member.clone(), value.clone());
        }

        let answer = emulator.call("CreateAccess", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        for fragment in &named {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
            );
        }

        let read_back = emulator
            .call(
                "DescribeAccess",
                json!({"ServerId": server_id, "ExternalId": GROUP_SID}),
            )
            .await;
        assert_eq!(
            read_back.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: a refused create must persist nothing, got {}",
            read_back.body
        );
    }
}

/// RULE 3: an access is addressed by `(ServerId, ExternalId)` — a pair the store does
/// not hold is refused rather than invented, the same pair twice is refused rather
/// than replaced, and the same `ExternalId` on two servers is two independent records.
#[tokio::test]
async fn an_access_is_addressed_by_its_server_and_external_id() {
    let emulator = Emulator::new();
    let first = directory_server(&emulator).await;
    let second = directory_server(&emulator).await;

    // A server the store does not hold: refused with the service's not-found answer
    // (HTTP 400 `ResourceNotFoundException`, `ResourceType` `Server`), for both the
    // create and the read.
    for (case, op, request) in [
        (
            "CreateAccess on an unminted server",
            "CreateAccess",
            json!({"ServerId": UNMINTED_SERVER_ID, "ExternalId": GROUP_SID, "Role": ROLE}),
        ),
        (
            "DescribeAccess on an unminted server",
            "DescribeAccess",
            json!({"ServerId": UNMINTED_SERVER_ID, "ExternalId": GROUP_SID}),
        ),
    ] {
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "ResourceType").as_str(),
            Some("Server"),
            "case {case:?}: expected ResourceType Server, got {}",
            answer.body
        );
    }

    // An `ExternalId` the server has no access for: the access itself is the missing
    // resource.
    let answer = emulator
        .call(
            "DescribeAccess",
            json!({"ServerId": first, "ExternalId": GROUP_SID}),
        )
        .await;
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case \"DescribeAccess for an ExternalId with no access\": expected \
         ResourceNotFoundException, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "ResourceType").as_str(),
        Some("Access"),
        "case \"DescribeAccess for an ExternalId with no access\": expected ResourceType \
         Access, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Resource").as_str(),
        Some(GROUP_SID),
        "case \"DescribeAccess for an ExternalId with no access\": expected the requested \
         ExternalId in Resource, got {}",
        answer.body
    );

    // The same `ExternalId` on two servers: two records, each with its own properties.
    for (server_id, home) in [(&first, "/bucket/first"), (&second, "/bucket/second")] {
        let answer = emulator
            .call(
                "CreateAccess",
                json!({
                    "ServerId": server_id,
                    "ExternalId": GROUP_SID,
                    "Role": ROLE,
                    "HomeDirectory": home,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"the same ExternalId on {server_id}\": expected 200, got {} with body {}",
            answer.status, answer.body
        );
    }
    for (server_id, home) in [(&first, "/bucket/first"), (&second, "/bucket/second")] {
        let access = described_access(&emulator, server_id, GROUP_SID).await;
        assert_members(
            &format!("the access of {server_id}"),
            &access,
            &json!({"HomeDirectory": home}),
        );
    }

    // The same pair twice: refused with the modelled `ResourceExistsException`
    // (HTTP 409), and the stored record is untouched.
    let answer = emulator
        .call(
            "CreateAccess",
            json!({
                "ServerId": first,
                "ExternalId": GROUP_SID,
                "Role": ROLE,
                "HomeDirectory": "/bucket/replacement",
            }),
        )
        .await;
    assert_eq!(
        answer.status, 409,
        "case \"the same pair twice\": expected HTTP 409, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceExistsException",
        "case \"the same pair twice\": expected ResourceExistsException, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Resource").as_str(),
        Some(GROUP_SID),
        "case \"the same pair twice\": expected the ExternalId in Resource, got {}",
        answer.body
    );
    let access = described_access(&emulator, &first, GROUP_SID).await;
    assert_members(
        "the stored record after a refused duplicate",
        &access,
        &json!({"HomeDirectory": "/bucket/first"}),
    );

    // The accesses belong to the server: deleting it takes them with it, while the
    // other server's identically named access stays readable.
    let deleted = emulator
        .call("DeleteServer", json!({"ServerId": first}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"DeleteServer\": expected 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let answer = emulator
        .call(
            "DescribeAccess",
            json!({"ServerId": first, "ExternalId": GROUP_SID}),
        )
        .await;
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case \"DescribeAccess after DeleteServer\": expected ResourceNotFoundException, got {}",
        answer.body
    );
    let access = described_access(&emulator, &second, GROUP_SID).await;
    assert_members(
        "the surviving server's access",
        &access,
        &json!({"HomeDirectory": "/bucket/second"}),
    );
}

/// RULE 3 (the bound on the store): a server's access set is bounded — past
/// `MAX_ACCESSES_PER_SERVER` a create is refused with the modelled
/// `ServiceUnavailableException` instead of growing, and the refusal does not disturb
/// the stored records.
#[tokio::test]
async fn a_servers_access_set_is_bounded() {
    use service_com_amazonaws_transfer_transferservice::state::MAX_ACCESSES_PER_SERVER;

    let emulator = Emulator::new();
    let server_id = directory_server(&emulator).await;

    for n in 0..MAX_ACCESSES_PER_SERVER {
        let answer = emulator
            .call(
                "CreateAccess",
                json!({
                    "ServerId": server_id,
                    "ExternalId": format!("S-1-5-21-{n}"),
                    "Role": ROLE,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"access {n} of {MAX_ACCESSES_PER_SERVER}\": expected 200 up to the bound, \
             got {} with body {}",
            answer.status, answer.body
        );
    }

    let answer = emulator
        .call(
            "CreateAccess",
            json!({
                "ServerId": server_id,
                "ExternalId": format!("S-1-5-21-{MAX_ACCESSES_PER_SERVER}"),
                "Role": ROLE,
            }),
        )
        .await;
    assert_eq!(
        answer.status, 503,
        "case \"one access past the bound\": expected HTTP 503, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ServiceUnavailableException",
        "case \"one access past the bound\": expected ServiceUnavailableException, got {}",
        answer.body
    );

    // The bound refused the new record and kept every stored one.
    let access = described_access(&emulator, &server_id, "S-1-5-21-0").await;
    assert_members(
        "the first access after the bound was hit",
        &access,
        &json!({"ExternalId": "S-1-5-21-0", "Role": ROLE}),
    );
    let answer = emulator
        .call(
            "DescribeAccess",
            json!({
                "ServerId": server_id,
                "ExternalId": format!("S-1-5-21-{MAX_ACCESSES_PER_SERVER}"),
            }),
        )
        .await;
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case \"the access the bound refused\": expected it to be absent, got {}",
        answer.body
    );
}
