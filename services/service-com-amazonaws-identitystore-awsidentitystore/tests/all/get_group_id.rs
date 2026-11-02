// SPDX-License-Identifier: CC0-1.0

//! `GetGroupId`: resolving a group's primary identity from a NON-primary identifier,
//! driven end to end over HTTP.
//!
//! The rule under test: within one identity store, `GetGroupId` resolves the group an
//! `AlternateIdentifier` names — an `ExternalId` (issuer + id) stored on the group, or a
//! `UniqueAttribute` naming the one unique attribute the operation supports,
//! `displayName` — and reports that group's stored `IdentityStoreId`, `GroupId` and
//! `GroupArn`; no group matching is the not-found `ResourceNotFoundException`, an identifier
//! the operation cannot interpret is HTTP 400 `ValidationException`, and the resolution
//! writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: the
//! name's value, length, script and case, the casing of the attribute path, the form of
//! the identity-store identifier, which store holds the name, the attribute path, and
//! the JSON type of the attribute value. The `engineering` case from the issue is one
//! row.
//!
//! The wire harness (`emulator`, `ok`, `refused`, …) is [`crate::harness`]'s, shared
//! rather than copied: every call still goes through the production
//! [`MultiServiceRouter`] front door and nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{assert_resource_arn, assert_uuid};

/// The bounds of the modelled `GroupDisplayName` length
/// (`smithy_query(query="shape",shape="GroupDisplayName")`: length 1..1024).
const DISPLAY_NAME_MIN_LEN: usize = 1;
const DISPLAY_NAME_MAX_LEN: usize = 1024;

/// A `UniqueAttribute` alternate identifier for `path` carrying `value`.
fn by_unique_attribute(path: &str, value: Value) -> Value {
    json!({ "UniqueAttribute": { "AttributePath": path, "AttributeValue": value } })
}

/// A `UniqueAttribute` alternate identifier naming the `displayName` attribute.
fn by_display_name(display_name: &str) -> Value {
    by_unique_attribute("displayName", json!(display_name))
}

/// An `ExternalId` alternate identifier: the issuer and the id it assigned.
fn by_external_id(issuer: &str, id: &str) -> Value {
    json!({ "ExternalId": { "Issuer": issuer, "Id": id } })
}

/// `GetGroupId` with `identifier`, which must answer 200.
async fn get_group_id(
    router: &MultiServiceRouter,
    store: &str,
    identifier: Value,
    case: &str,
) -> Value {
    ok(
        router,
        "GetGroupId",
        json!({ "IdentityStoreId": store, "AlternateIdentifier": identifier }),
        case,
    )
    .await
}

/// `GetGroupId` with `identifier`, which must be refused with `status`/`error`.
async fn get_group_id_refused(
    router: &MultiServiceRouter,
    store: &str,
    identifier: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    refused(
        router,
        "GetGroupId",
        json!({ "IdentityStoreId": store, "AlternateIdentifier": identifier }),
        status,
        error,
        case,
    )
    .await
}

/// Create a group and return its `(GroupId, GroupArn)` as stored.
async fn create_group(
    router: &MultiServiceRouter,
    store: &str,
    display_name: &str,
    case: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateGroup",
        json!({ "IdentityStoreId": store, "DisplayName": display_name }),
        case,
    )
    .await;
    (
        string(&created, "GroupId", case),
        string(&created, "GroupArn", case),
    )
}

/// An `IdentityStoreId` the model admits that names no store this emulator serves,
/// built from `n` so a row can have one of its own: the ARN form carries the OWNING
/// ACCOUNT, and this one names a store of another account.
fn unknown_store_id(n: usize) -> String {
    format!("arn:aws:identitystore::210987654321:identitystore/d-{n:010x}")
}

/// A well-formed `IdentityStoreId` of THIS account that nothing has written to, built
/// from `n`: the modelled pattern's `d-[0-9a-f]{10}` form, which names the store its
/// caller has.
fn unwritten_store_id(n: usize) -> String {
    format!("d-{n:010x}")
}

#[tokio::test]
async fn a_group_resolves_by_the_display_name_it_was_stored_with() {
    let router = emulator();

    // Rows differ along the display name itself: the recorded case, each published
    // bound of the modelled `GroupDisplayName` length (1..1024), a name differing from
    // another row only in case, and names in other scripts and with the whitespace,
    // symbol, digit and punctuation characters the modelled pattern admits
    // (`^[\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r \u{00a0}\u{3000}]+$`).
    let rows: Vec<(&str, String)> = vec![
        ("recorded case", "engineering".to_string()),
        ("minimum length", "a".repeat(DISPLAY_NAME_MIN_LEN)),
        ("maximum length", "g".repeat(DISPLAY_NAME_MAX_LEN)),
        ("same name, other case", "Engineering".to_string()),
        ("non-ASCII letters", "équipe-ingénierie".to_string()),
        ("CJK letters", "開発チーム".to_string()),
        ("spaces", "platform ops team".to_string()),
        ("digits and punctuation", "team-42_(prod)".to_string()),
        ("symbols", "team+ops".to_string()),
    ];

    for (case, display_name) in &rows {
        let (group_id, group_arn) = create_group(&router, STORE, display_name, case).await;
        assert_uuid(&group_id, "GroupId", case);
        assert_resource_arn(&group_arn, "group", &group_id, case);

        let resolved = get_group_id(&router, STORE, by_display_name(display_name), case).await;
        assert_eq!(
            string(&resolved, "GroupId", case),
            group_id,
            "case {case:?}: expected the stored GroupId, got {resolved}"
        );
        assert_eq!(
            string(&resolved, "GroupArn", case),
            group_arn,
            "case {case:?}: expected the stored GroupArn, got {resolved}"
        );
        assert_eq!(
            string(&resolved, "IdentityStoreId", case),
            STORE,
            "case {case:?}: expected the store that holds the group, got {resolved}"
        );

        // Round trip: the id the resolution reported reads back as the group that was
        // created under that name.
        let described = ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": string(&resolved, "GroupId", case) }),
            case,
        )
        .await;
        assert_eq!(
            described["DisplayName"],
            json!(display_name),
            "case {case:?}: expected the resolved group to carry the name it was \
             created with, got {}",
            described["DisplayName"]
        );
    }

    // Every row resolved to a DIFFERENT group: a display name identifies exactly one,
    // and two names that differ only in case are two groups.
    let mut resolved_ids = Vec::new();
    for (case, display_name) in &rows {
        let resolved = get_group_id(&router, STORE, by_display_name(display_name), case).await;
        resolved_ids.push(string(&resolved, "GroupId", case));
    }
    let mut unique = resolved_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        rows.len(),
        "expected each of the {} names to resolve to its own group, got {resolved_ids:?}",
        rows.len()
    );
}

#[tokio::test]
async fn the_unique_attribute_path_names_the_attribute_case_insensitively() {
    let router = emulator();
    let case = "attribute path casing";
    let (group_id, _) = create_group(&router, STORE, "engineering", case).await;

    // Rows differ along the CASE of the attribute path. SCIM attribute names are case
    // insensitive (RFC 7643 §2.1), so each of these names the one unique attribute
    // `GetGroupId` resolves a group by.
    for path in [
        "displayName",
        "displayname",
        "DisplayName",
        "DISPLAYNAME",
        "dIsPlAyNaMe",
    ] {
        let resolved = get_group_id(
            &router,
            STORE,
            by_unique_attribute(path, json!("engineering")),
            path,
        )
        .await;
        assert_eq!(
            string(&resolved, "GroupId", path),
            group_id,
            "case {path:?}: expected the path to name the displayName attribute, \
             got {resolved}"
        );
    }
}

#[tokio::test]
async fn a_display_name_resolves_only_within_the_store_that_holds_it() {
    let router = emulator();
    let case = "per-store scope";
    let (here, _) = create_group(&router, STORE, "engineering", case).await;

    // Rows differ along the FORM of the identity-store identifier: the modelled
    // `IdentityStoreId` pattern admits the bare id and the store's ARN, and both name
    // the same store.
    let store_forms = [
        ("bare id", STORE.to_string()),
        (
            "store ARN",
            format!("arn:aws:identitystore::000000000000:identitystore/{STORE}"),
        ),
    ];
    for (form, identifier) in &store_forms {
        let resolved = get_group_id(&router, identifier, by_display_name("engineering"), form).await;
        assert_eq!(
            string(&resolved, "GroupId", form),
            here,
            "case {form:?}: expected {identifier} to resolve within store {STORE}, \
             got {resolved}"
        );
        assert_eq!(
            string(&resolved, "IdentityStoreId", form),
            STORE,
            "case {form:?}: expected the store id, got {resolved}"
        );
    }

    // Rows that name no group: a name nothing was created under, and a name longer than
    // the modelled `GroupDisplayName` bound (1024) so no stored group can carry it.
    let misses: Vec<(&str, String)> = vec![
        ("unused name", "nobody".to_string()),
        ("longer than any stored name", "g".repeat(DISPLAY_NAME_MAX_LEN + 1)),
    ];
    for (case, display_name) in &misses {
        let refusal = get_group_id_refused(
            &router,
            STORE,
            by_display_name(display_name),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            refusal["ResourceType"],
            json!("GROUP"),
            "case {case:?}: expected the refusal to name the GROUP resource type, \
             got {refusal}"
        );
    }

    // And a lookup addressed to an identity store this emulator does not serve is
    // refused for the STORE's sake: the store is the missing resource, not the group.
    for (case, identifier) in [
        ("another account's store", MISSING_STORE.to_string()),
        ("another account's other store", unknown_store_id(0xff)),
    ] {
        let refusal = get_group_id_refused(
            &router,
            &identifier,
            by_display_name("engineering"),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            refusal["ResourceType"],
            json!("IDENTITY_STORE"),
            "case {case:?}: expected the identity store to be the missing resource, \
             got {refusal}"
        );
    }
}

#[tokio::test]
async fn an_external_id_resolves_only_a_group_that_carries_it() {
    let router = emulator();
    let case = "external id";
    create_group(&router, STORE, "engineering", case).await;

    // No modelled operation of this slice stores a group's external ids — they come
    // from an identity provider's provisioning — so a group created here carries none,
    // and an `ExternalId` identifier resolves nothing. Rows differ along the issuer and
    // id, including a pair whose values are a stored display name, so a hit could only
    // come from matching the wrong attribute.
    let rows = [
        ("provider URL issuer", "https://idp.example.com", "group-123"),
        ("urn issuer", "urn:ietf:params:scim:schemas:core:2.0", "9999"),
        ("values of a stored display name", "engineering", "engineering"),
    ];
    for (case, issuer, id) in &rows {
        let refusal = get_group_id_refused(
            &router,
            STORE,
            by_external_id(issuer, id),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            refusal["ResourceType"],
            json!("GROUP"),
            "case {case:?}: expected the refusal to name the GROUP resource type, \
             got {refusal}"
        );
        // The refusal carries no `ResourceId`: an external id is not a value the
        // modelled `ResourceId` member can hold.
        assert!(
            refusal.get("ResourceId").unwrap_or(&Value::Null).is_null(),
            "case {case:?}: expected no ResourceId on a lookup by alternate identifier, \
             got {refusal}"
        );
    }

    let described = ok(
        &router,
        "DescribeGroup",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": string(
                &get_group_id(&router, STORE, by_display_name("engineering"), case).await,
                "GroupId",
                case,
            ),
        }),
        case,
    )
    .await;
    assert!(
        described.get("ExternalIds").unwrap_or(&Value::Null).is_null(),
        "case {case:?}: expected a group created here to carry no external ids, \
         got {described}"
    );
}

#[tokio::test]
async fn an_alternate_identifier_this_operation_cannot_interpret_is_refused() {
    let router = emulator();
    let case = "unsupported identifier";
    let (group_id, _) = create_group(&router, STORE, "engineering", case).await;

    // Rows differ along the attribute PATH. `displayName` is the only unique attribute
    // that identifies a group (`GetGroupIdRequest$AlternateIdentifier`: "For the unique
    // attribute, the only valid path is displayName"), so every other path the modelled
    // `AttributePath` pattern admits is a bad request, not a miss — including the paths
    // that identify a USER and other members of the group itself.
    let unsupported_paths = [
        "description",
        "userName",
        "emails.value",
        "externalIds.id",
        "id",
        "name.givenName",
        "displayNames",
    ];
    for path in unsupported_paths {
        get_group_id_refused(
            &router,
            STORE,
            by_unique_attribute(path, json!("engineering")),
            400,
            "ValidationException",
            path,
        )
        .await;
    }

    // Rows differ along the JSON TYPE of the attribute value. `displayName` is a
    // string, so a value of any other type cannot name a group.
    let wrong_types = [
        ("number", json!(42)),
        ("boolean", json!(true)),
        ("array", json!(["engineering"])),
        ("object", json!({ "Value": "engineering" })),
    ];
    for (case, value) in &wrong_types {
        get_group_id_refused(
            &router,
            STORE,
            by_unique_attribute("displayName", value.clone()),
            400,
            "ValidationException",
            case,
        )
        .await;
    }

    // Every refusal above stored nothing and broke nothing: the group still resolves.
    let resolved = get_group_id(&router, STORE, by_display_name("engineering"), case).await;
    assert_eq!(
        string(&resolved, "GroupId", case),
        group_id,
        "case {case:?}: expected the refusals to leave the group resolvable, got {resolved}"
    );
}

#[tokio::test]
async fn resolving_a_group_id_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let case = "repeated resolution";
    let (group_id, group_arn) = create_group(&router, STORE, "engineering", case).await;
    let describe = json!({ "IdentityStoreId": STORE, "GroupId": group_id.clone() });
    let before = ok(&router, "DescribeGroup", describe.clone(), case).await;

    // A reporting operation is idempotent: three resolutions answer the same thing, and
    // the group's own record — revision, timestamps, name — is untouched by them.
    for attempt in 0..3 {
        let case = format!("{case} {attempt}");
        let resolved = get_group_id(&router, STORE, by_display_name("engineering"), &case).await;
        assert_eq!(
            (
                string(&resolved, "GroupId", &case),
                string(&resolved, "GroupArn", &case)
            ),
            (group_id.clone(), group_arn.clone()),
            "case {case:?}: expected every resolution to answer the same identity, \
             got {resolved}"
        );
    }
    let after = ok(&router, "DescribeGroup", describe, case).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the group's record to be unchanged by the resolutions, \
         got {after} after {before}"
    );

    // And a resolution never writes: the group's record is untouched above, and a miss
    // in a store nothing has written to stays a miss that leaves the store empty — were
    // the read a write, the group it minted would be listed afterwards.
    let reader = emulator();
    for index in 0..4 {
        let case = format!("miss in a store nothing has written to {index}");
        let store = unwritten_store_id(index);
        for attempt in 0..2 {
            let case = format!("{case}, attempt {attempt}");
            let refusal = get_group_id_refused(
                &reader,
                &store,
                by_display_name("engineering"),
                NOT_FOUND,
                "ResourceNotFoundException",
                &case,
            )
            .await;
            assert_eq!(
                refusal["ResourceType"],
                json!("GROUP"),
                "case {case:?}: expected the GROUP to be the missing resource in a store \
                 that is served, got {refusal}"
            );
        }
        let listed = ok(
            &reader,
            "ListGroups",
            json!({ "IdentityStoreId": store.clone() }),
            &case,
        )
        .await;
        assert_eq!(
            listed["Groups"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected the misses to have written nothing, got {listed}"
        );
        // A lookup addressed to a store of ANOTHER account is refused instead, and that
        // refusal writes nothing either.
        get_group_id_refused(
            &reader,
            &unknown_store_id(index),
            by_display_name("engineering"),
            NOT_FOUND,
            "ResourceNotFoundException",
            &case,
        )
        .await;
        refused(
            &reader,
            "CreateGroup",
            json!({
                "IdentityStoreId": unknown_store_id(index),
                "DisplayName": "engineering",
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
            &case,
        )
        .await;
    }
}
