// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SHAPE of what this service's success bodies carry,
//! and for where the `external` flag of an association comes from.
//!
//! The two rules under test, each asserted over a table rather than one case:
//!
//! * **A success body carries exactly the members the model declares.** Every `Tag`
//!   RAM reports — in `TagResource$tagsCreated`, in `UntagResource$deletedTags` and in
//!   `ResourceShare$tags` — is the one modelled `com.amazonaws.ram#Tag`
//!   (`smithy_query(query="shape",shape="Tag")`: exactly `key` and `value`), so one
//!   stored pair serializes identically wherever it is reported and no type
//!   discriminator appears anywhere inside a 2xx body. The restJson1 discriminator
//!   `__type` belongs to the ERROR envelope (`vendor/smithy-schema-server/src/rest_json.rs`),
//!   which the last case of that test pins from the other side.
//! * **`external` is ONE value per association record, derived from the share's owner.**
//!   `ResourceShareAssociation$external` "indicates whether the principal belongs to
//!   the same organization in Organizations as the Amazon Web Services account that
//!   owns the resource share", and all three association operations report that one
//!   member of that one shape — so `AssociateResourceShare`,
//!   `GetResourceShareAssociations` and `DisassociateResourceShare` report the same
//!   value for the same record, and that value follows the entity: the owning account
//!   and anything inside the owner's Organizations hierarchy are in-organization, any
//!   other account is external.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

// ---------------------------------------------------------------------------
// A success body carries exactly the modelled members
// ---------------------------------------------------------------------------

/// A row of the tag-serialization table: the tags a share is created with, a tag
/// mutation to report, and a key to detach.
struct TagShapeCase {
    case: &'static str,
    initial: Vec<(&'static str, &'static str)>,
    add: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
}

/// Every member name `value` carries, at any depth.
fn member_names(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                into.insert(name.clone());
                member_names(child, into);
            }
        }
        Value::Array(items) => {
            for item in items {
                member_names(item, into);
            }
        }
        _ => {}
    }
}

/// Assert no member of a SUCCESS body is a type discriminator: a member whose name
/// starts with `__` (restJson1 writes `__type` only in an error envelope).
fn assert_no_discriminator(label: &str, body: &Value) {
    let mut names = BTreeSet::new();
    member_names(body, &mut names);
    let discriminators: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("__"))
        .collect();
    assert!(
        discriminators.is_empty(),
        "case {label:?}: a 2xx body must carry only modelled members, \
         got the discriminator(s) {discriminators:?} in {body:?}"
    );
}

/// The member names of one reported entry, as the wire carried them.
fn members_of(label: &str, entry: &Value) -> BTreeSet<String> {
    entry
        .as_object()
        .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {entry:?}"))
        .keys()
        .cloned()
        .collect()
}

/// The entries of a reported tag list.
fn list_of(label: &str, body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {label:?}: response carried no {member}, got {body:?}"))
}

/// The `key`/`value` members the modelled `Tag` structure declares
/// (`smithy_query(query="shape",shape="Tag")`), as a set.
fn modelled_tag_members() -> BTreeSet<String> {
    ["key", "value"].iter().map(|m| (*m).to_string()).collect()
}

#[tokio::test]
async fn every_reported_tag_carries_exactly_the_modelled_members() {
    let router = emulator();

    // The rule generalises over the tag PAIR, so the rows differ along the form of the
    // key and the value: a plain pair, an empty value ("The value can be an empty
    // string" — `Tag$value`), two keys differing only in case ("Key names are case
    // sensitive" — `Tag$key`), several pairs at once, and a pair whose characters JSON
    // must escape or encode.
    let cases = vec![
        TagShapeCase {
            case: "one plain pair on an untagged share",
            initial: vec![],
            add: vec![("cost-center", "42")],
            remove: vec!["cost-center"],
        },
        TagShapeCase {
            case: "an empty value",
            initial: vec![("team", "platform")],
            add: vec![("owner", "")],
            remove: vec!["owner"],
        },
        TagShapeCase {
            case: "keys differing only in case",
            initial: vec![("Team", "platform")],
            add: vec![("team", "network")],
            remove: vec!["Team"],
        },
        TagShapeCase {
            case: "several pairs in one call",
            initial: vec![("env", "test")],
            add: vec![("a", "1"), ("b", "2"), ("c", "3")],
            remove: vec!["a", "c"],
        },
        TagShapeCase {
            case: "characters JSON must escape or encode",
            initial: vec![],
            add: vec![
                ("quote\"and\\slash", "line\nbreak"),
                ("ключ", "значение"),
                ("emoji-🙂", "值"),
            ],
            remove: vec!["ключ"],
        },
    ];

    for TagShapeCase {
        case,
        initial,
        add,
        remove,
    } in cases
    {
        let initial_tags: Vec<Value> = initial
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let (status, created_body) = create(
            &router,
            &json!({
                "name": format!("tag-shape-{}", case.replace(' ', "-")),
                "tags": initial_tags,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: create expected 200, got {status} with {created_body:?}"
        );
        assert_no_discriminator(case, &created_body);
        let created = created_body
            .get("resourceShare")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: create reported no share: {created_body:?}"));
        for entry in list_of(case, &created, "tags") {
            assert_eq!(
                members_of(case, &entry),
                modelled_tag_members(),
                "case {case:?}: a tag the create reported carries unmodelled members: {entry:?}"
            );
        }

        let arn = arn_of(case, &created);

        // TagResource: every reported entry is the modelled Tag, and the entry for a
        // key is byte-identical to the entry the SAME pair gets inside the share — one
        // shape, serialized one way, wherever it is reported.
        let tags: Vec<Value> = add
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect();
        let (status, tagged) =
            tag_resource(&router, &json!({ "resourceShareArn": arn, "tags": tags })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} with {tagged:?}"
        );
        assert_no_discriminator(case, &tagged);
        let created_entries = list_of(case, &tagged, "tagsCreated");
        assert_eq!(
            created_entries.len(),
            add.len(),
            "case {case:?}: expected one reported entry per attached pair, got {tagged:?}"
        );
        for entry in &created_entries {
            assert_eq!(
                members_of(case, entry),
                modelled_tag_members(),
                "case {case:?}: tagsCreated entry carries unmodelled members: {entry:?}"
            );
        }

        let (status, listed) = get_shares(
            &router,
            &json!({ "resourceOwner": "SELF", "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShares expected 200, got {status} with {listed:?}"
        );
        assert_no_discriminator(case, &listed);
        let share = only_share(case, &listed);
        let share_entries = list_of(case, &share, "tags");
        for entry in &created_entries {
            assert!(
                share_entries.contains(entry),
                "case {case:?}: the pair {entry:?} TagResource reported is serialized \
                 differently inside the share: {share_entries:?}"
            );
        }

        // UntagResource: the pair is gone, so an entry names the key alone — still the
        // modelled Tag, with no member the model does not declare.
        let (status, untagged) = untag_resource(
            &router,
            &json!({ "resourceShareArn": arn, "tagKeys": remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} with {untagged:?}"
        );
        assert_no_discriminator(case, &untagged);
        let deleted_entries = list_of(case, &untagged, "deletedTags");
        assert_eq!(
            deleted_entries.len(),
            remove.len(),
            "case {case:?}: expected one entry per removed key, got {untagged:?}"
        );
        for entry in &deleted_entries {
            assert_eq!(
                members_of(case, entry),
                ["key"].iter().map(|m| (*m).to_string()).collect(),
                "case {case:?}: deletedTags entry carries unmodelled members: {entry:?}"
            );
        }
    }

    // The other side of the same rule: the `__type` discriminator belongs to the ERROR
    // envelope, where restJson1 writes it, and a failed tag mutation carries it.
    let (status, refused) = tag_resource(
        &router,
        &json!({
            "resourceShareArn": format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/00000000-0000-0000-0000-000000000000"),
            "tags": [{ "key": "cost-center", "value": "42" }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"tagging an unknown share\": expected 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case \"tagging an unknown share\": wrong error code in {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// `external` is one derived value per association record
// ---------------------------------------------------------------------------

/// A row of the `external` table: one entity, and whether the share's owner can place
/// it inside its own organization.
struct ExternalCase {
    case: &'static str,
    association_type: &'static str,
    entity: &'static str,
    external: bool,
}

/// The entity forms `ResourceShareAssociation$associatedEntity` names, each with the
/// value the documented rule gives `external` for a share owned by [`ACCOUNT`]: the
/// owning account and anything inside its Organizations hierarchy are in-organization,
/// and an entity this service cannot place there is external.
fn external_cases() -> Vec<ExternalCase> {
    vec![
        ExternalCase {
            case: "the owning account itself",
            association_type: "PRINCIPAL",
            entity: ACCOUNT,
            external: false,
        },
        ExternalCase {
            case: "another account",
            association_type: "PRINCIPAL",
            entity: "111122223333",
            external: true,
        },
        ExternalCase {
            case: "a role in the owning account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::000000000000:role/local-consumer",
            external: false,
        },
        ExternalCase {
            case: "a role in another account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:iam::777788889999:role/consumer",
            external: true,
        },
        ExternalCase {
            case: "an organization id",
            association_type: "PRINCIPAL",
            entity: "o-abc123exam",
            external: false,
        },
        ExternalCase {
            case: "an organizational unit id",
            association_type: "PRINCIPAL",
            entity: "ou-ab12-cd34efgh",
            external: false,
        },
        ExternalCase {
            case: "an organization ARN in the management account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:organization/o-abc123exam",
            external: false,
        },
        ExternalCase {
            case: "an organizational-unit ARN in the management account",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:ou/o-abc123exam/ou-ab12-cd34efgh",
            external: false,
        },
        ExternalCase {
            case: "an Organizations account ARN",
            association_type: "PRINCIPAL",
            entity: "arn:aws:organizations::111122223333:account/o-abc123exam/999988887777",
            external: false,
        },
        ExternalCase {
            case: "a service principal",
            association_type: "PRINCIPAL",
            entity: "lambda.amazonaws.com",
            external: true,
        },
        ExternalCase {
            case: "a resource in the owning account",
            association_type: "RESOURCE",
            entity: LOCAL_RESOURCE,
            external: false,
        },
        ExternalCase {
            case: "a resource in another account",
            association_type: "RESOURCE",
            entity: FOREIGN_RESOURCE,
            external: true,
        },
        ExternalCase {
            case: "a source account inside the owning account",
            association_type: "SOURCE",
            entity: ACCOUNT,
            external: false,
        },
        ExternalCase {
            case: "a source account elsewhere",
            association_type: "SOURCE",
            entity: "111122223333",
            external: true,
        },
    ]
}

/// A request body naming `entity` under the list its association type is taken from.
fn entity_request(association_type: &str, entity: &str, arn: &str) -> Value {
    let member = match association_type {
        "RESOURCE" => "resourceArns",
        "SOURCE" => "sources",
        _ => "principals",
    };
    json!({ "resourceShareArn": arn, member: [entity] })
}

/// The one association a response reports for `entity`.
fn association_for(label: &str, body: &Value, entity: &str) -> Value {
    associations_of(label, body)
        .into_iter()
        .find(|association| {
            association.get("associatedEntity").and_then(Value::as_str) == Some(entity)
        })
        .unwrap_or_else(|| panic!("case {label:?}: no association reported for {entity}: {body:?}"))
}

/// The `external` member of a reported association.
fn external_of(label: &str, association: &Value) -> bool {
    association
        .get("external")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {label:?}: association reported no external: {association:?}"))
}

#[tokio::test]
async fn external_is_derived_from_the_owning_organization_and_is_one_value_per_record() {
    let router = emulator();

    for (index, row) in external_cases().into_iter().enumerate() {
        let ExternalCase {
            case,
            association_type,
            entity,
            external,
        } = row;

        let created = create_ok(
            &router,
            case,
            &json!({ "name": format!("external-{index}") }),
        )
        .await;
        let arn = arn_of(case, &created);
        let request = entity_request(association_type, entity, &arn);

        // 1. The call that makes the association.
        let (status, associated) = associate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: AssociateResourceShare expected 200, got {status} with {associated:?}"
        );
        let on_associate = external_of(case, &association_for(case, &associated, entity));
        assert_eq!(
            on_associate, external,
            "case {case:?}: AssociateResourceShare expected external {external}, \
             got {on_associate} for {entity}"
        );

        // 2. The read of the stored record.
        let (status, read) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResourceShareAssociations expected 200, got {status} with {read:?}"
        );
        let on_read = external_of(case, &association_for(case, &read, entity));
        assert_eq!(
            on_read, external,
            "case {case:?}: GetResourceShareAssociations expected external {external}, \
             got {on_read} for {entity}"
        );

        // 3. The call that tears it down: the same member of the same record, so the
        // same value — the operation reporting it cannot change what the flag says.
        let (status, disassociated) = disassociate(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisassociateResourceShare expected 200, got {status} \
             with {disassociated:?}"
        );
        let on_disassociate = external_of(case, &association_for(case, &disassociated, entity));
        assert_eq!(
            on_disassociate, external,
            "case {case:?}: DisassociateResourceShare expected external {external}, \
             got {on_disassociate} for {entity}"
        );
        assert_eq!(
            (on_associate, on_read),
            (on_disassociate, on_disassociate),
            "case {case:?}: external must be one value per record, got \
             associate={on_associate}, read={on_read}, disassociate={on_disassociate}"
        );

        // And the teardown left the record where a later read still finds the same flag.
        let (_, after) = get_associations(
            &router,
            &json!({ "associationType": association_type, "resourceShareArns": [arn] }),
        )
        .await;
        let after_teardown = external_of(case, &association_for(case, &after, entity));
        assert_eq!(
            after_teardown, external,
            "case {case:?}: the read after the teardown expected external {external}, \
             got {after_teardown}"
        );
    }
}
