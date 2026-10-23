// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the alias LIFECYCLE — `DeleteAlias`, `UpdateAlias` and
//! `ListAliases` — driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: an alias is a persisted binding from one name, in one Region
//! of one account, to one KMS key, and EVERY `KeyId` resolution follows the binding
//! the store holds right now. So:
//!
//! * `DeleteAlias` removes the binding, and from then on the name and its alias ARN
//!   resolve to NOTHING — every operation taking a `KeyId` answers
//!   `NotFoundException` rather than falling back to the key the alias used to name —
//!   while the key itself, the key's other names, and aliases of the same name in
//!   other Regions are untouched.
//! * `UpdateAlias` repoints an EXISTING binding, so the name resolves to the new key
//!   and keeps its own identity (name, ARN, creation date).
//! * `ListAliases` reports exactly the bindings the store holds, in creation order,
//!   narrowed by `KeyId` and paged by `Limit`/`Marker`.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the alias
//! name (length, character class, number of `/` segments), in the kind of key the
//! alias names, in the identifier form the later read uses (bare `alias/<name>`,
//! alias ARN, key id, key ARN), in the operation that resolves the identifier, in the
//! Region the request is signed for, in how many names the key holds, and in the page
//! size asked for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    alias_arn, assert_empty, call, create_alias, create_alias_in, create_key, describe, error_code,
    failure, failure_in, ok, ok_in, router, text, REGION, b64,
};

/// Delete an alias, requiring the empty 200.
async fn delete_alias(router: &MultiServiceRouter, case: &str, alias: &str) {
    delete_alias_in(router, REGION, case, alias).await;
}

/// [`delete_alias`], in `region`.
async fn delete_alias_in(router: &MultiServiceRouter, region: &str, case: &str, alias: &str) {
    let answer = ok_in(router, region, "DeleteAlias", &json!({"AliasName": alias})).await;
    assert_empty(case, "DeleteAlias", &answer);
}

/// Point an existing alias at another key, requiring the empty 200.
async fn update_alias(router: &MultiServiceRouter, case: &str, alias: &str, target: &str) {
    let answer = ok(
        router,
        "UpdateAlias",
        &json!({"AliasName": alias, "TargetKeyId": target}),
    )
    .await;
    assert_empty(case, "UpdateAlias", &answer);
}

/// The `Aliases` `ListAliases` reports for a request.
async fn list_aliases(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "ListAliases", request).await
}

/// The `AliasName` of every entry of a `ListAliases` answer, in the order reported.
fn listed_names(case: &str, answer: &Value) -> Vec<String> {
    answer["Aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an Aliases list in {answer}"))
        .iter()
        .map(|entry| text(case, entry, "AliasName"))
        .collect()
}

/// One row of the "a deleted alias resolves to nothing" table: the case name, the
/// alias name, and the `CreateKey` request for the key it names.
///
/// The names span `AliasNameType`'s modelled `len 1..256` and `^[a-zA-Z0-9:/_-]+$`
/// pattern (`smithy_query(query="operation",operation="DeleteAlias")`), and the keys
/// span the `KeySpec`/`KeyUsage` combinations alias management accepts — an alias is
/// not a property of the key, so removing one must behave the same whatever the key
/// is.
fn deletion_rows() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "plain name, default symmetric key",
            "alias/plain-name".to_string(),
            json!({"Description": "plain"}),
        ),
        ("one-character name", "alias/a".to_string(), json!({})),
        (
            "several path segments",
            "alias/team/project/key".to_string(),
            json!({"Description": "segments"}),
        ),
        (
            "every permitted character class, HMAC key",
            "alias/Mixed_Case-1:2".to_string(),
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "longest name the modelled length allows, asymmetric key",
            format!("alias/{}", "x".repeat(250)),
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "multi-Region key",
            "alias/multi-region".to_string(),
            json!({"MultiRegion": true}),
        ),
    ]
}

/// A deleted alias resolves to nothing: every operation that takes a `KeyId` answers
/// `NotFoundException` for the name and for its alias ARN, however often it is asked,
/// while the key the alias used to name is still exactly where it was.
#[tokio::test]
async fn a_deleted_alias_resolves_to_nothing_for_every_key_id_member() {
    let router = router();

    for (case, alias, create_request) in deletion_rows() {
        let metadata = create_key(&router, &create_request).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        // Before the delete the name resolves to the key, so what follows is about
        // the removal and not about a name that never worked.
        create_alias(&router, case, &alias, &key_id).await;
        assert_eq!(
            describe(&router, &alias).await,
            metadata,
            "case {case}: the alias must resolve to its key before it is deleted"
        );

        delete_alias(&router, case, &alias).await;

        // Rows: the identifier form, and the operation resolving it. Every one of
        // these models `NotFoundException`, and a mutating one must refuse BEFORE it
        // writes anything.
        for identifier in [alias.clone(), alias_arn(REGION, &alias)] {
            let requests: Vec<(&str, Value)> = vec![
                ("DescribeKey", json!({"KeyId": identifier})),
                (
                    "Encrypt",
                    json!({"KeyId": identifier, "Plaintext": b64(b"after the delete")}),
                ),
                (
                    "UpdateKeyDescription",
                    json!({"KeyId": identifier, "Description": "written through a dead alias"}),
                ),
                ("ListAliases", json!({"KeyId": identifier})),
            ];
            for (operation, body) in requests {
                let (status, code) = failure(&router, operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case} / {operation} / {identifier}: expected HTTP 400, got \
                     {status} ({code})"
                );
                assert_eq!(
                    code, "NotFoundException",
                    "case {case} / {operation} / {identifier}: expected NotFoundException, \
                     got {code}"
                );
            }
        }

        // The recorded client POLLS `DescribeKey` on the alias, so the answer must
        // SETTLE on the error rather than flipping back to the old target.
        for attempt in 1..=3 {
            let (status, payload) =
                call(&router, REGION, "DescribeKey", &json!({"KeyId": alias})).await;
            assert_eq!(
                status, 400,
                "case {case} poll attempt {attempt}: expected HTTP 400, got {status} with \
                 {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "NotFoundException",
                "case {case} poll attempt {attempt}: expected NotFoundException, got {payload}"
            );
        }

        // Deleting the name left the KEY alone: "you can delete and change the
        // aliases of a KMS key without affecting the KMS key".
        for (form, identifier) in [("key id", &key_id), ("key ARN", &key_arn)] {
            let described = describe(&router, identifier).await;
            assert_eq!(
                described, metadata,
                "case {case} read by {form}: deleting an alias must not touch the key, \
                 got {described}"
            );
        }

        // The name is free again, and taking it binds it afresh.
        let (status, code) = failure(&router, "DeleteAlias", &json!({"AliasName": alias})).await;
        assert_eq!(
            status, 400,
            "case {case}: a second delete must be refused, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case}: a second delete must answer NotFoundException, got {code}"
        );
        create_alias(&router, case, &alias, &key_id).await;
        assert_eq!(
            describe(&router, &alias).await,
            metadata,
            "case {case}: a deleted name must be free to create again"
        );
    }
}

/// Removal is of ONE binding: the key's other names still resolve, another key's
/// names are untouched, and the alias reader stops reporting only the name that was
/// removed.
#[tokio::test]
async fn deleting_one_alias_leaves_every_other_binding_in_place() {
    let router = router();
    let first = create_key(&router, &json!({"Description": "first"})).await;
    let second = create_key(&router, &json!({"Description": "second"})).await;
    let first_id = text("setup", &first, "KeyId");
    let second_id = text("setup", &second, "KeyId");

    // Rows: the names of the first key, created in this order, and the one that is
    // removed — the middle one, so the rows before and after it are both exercised.
    let names = ["alias/first/one", "alias/first-2", "alias/first:3"];
    for alias in names {
        create_alias(&router, alias, alias, &first_id).await;
    }
    create_alias(&router, "second key", "alias/second", &second_id).await;

    delete_alias(&router, "middle name", names[1]).await;

    for (index, alias) in names.iter().enumerate() {
        if index == 1 {
            let (status, code) = failure(&router, "DescribeKey", &json!({"KeyId": alias})).await;
            assert_eq!(
                status, 400,
                "case {alias}: the removed name must not resolve, got {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {alias}: expected NotFoundException, got {code}"
            );
            continue;
        }
        assert_eq!(
            describe(&router, alias).await,
            first,
            "case {alias}: a name that was not removed must still resolve to its key"
        );
    }
    assert_eq!(
        describe(&router, "alias/second").await,
        second,
        "case second key: another key's name must be untouched"
    );

    // The reader reports exactly the bindings that remain, in creation order.
    let answer = list_aliases(&router, &json!({})).await;
    assert_eq!(
        listed_names("after the delete", &answer),
        vec![
            names[0].to_string(),
            names[2].to_string(),
            "alias/second".to_string()
        ],
        "case after the delete: ListAliases must report the remaining aliases in \
         creation order, got {answer}"
    );
    let answer = list_aliases(&router, &json!({"KeyId": first_id})).await;
    assert_eq!(
        listed_names("filtered after the delete", &answer),
        vec![names[0].to_string(), names[2].to_string()],
        "case filtered after the delete: the filtered list must drop the removed \
         alias too, got {answer}"
    );
}

/// An alias belongs to the Region that holds it: deleting the name through one
/// endpoint leaves the same name in every other Region resolving as it did, and a
/// Region that holds no such name refuses the delete.
#[tokio::test]
async fn an_alias_is_deleted_only_in_the_region_that_holds_it() {
    let router = router();
    let regions = ["us-east-1", "us-west-2", "eu-west-1"];
    let shared = "alias/shared-name";

    let mut keys: Vec<(&str, Value)> = Vec::new();
    for region in regions {
        let metadata = ok_in(
            &router,
            region,
            "CreateKey",
            &json!({"Description": format!("key in {region}")}),
        )
        .await["KeyMetadata"]
            .clone();
        let key_id = text(region, &metadata, "KeyId");
        create_alias_in(&router, region, region, shared, &key_id).await;
        keys.push((region, metadata));
    }

    // The name is removed in the FIRST Region only.
    delete_alias_in(&router, regions[0], regions[0], shared).await;

    for (region, metadata) in &keys {
        if *region == regions[0] {
            for identifier in [shared.to_string(), alias_arn(region, shared)] {
                let (status, code) = failure_in(
                    &router,
                    region,
                    "DescribeKey",
                    &json!({"KeyId": identifier}),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {region} / {identifier}: expected HTTP 400, got {status} ({code})"
                );
                assert_eq!(
                    code, "NotFoundException",
                    "case {region} / {identifier}: expected NotFoundException, got {code}"
                );
            }
            continue;
        }
        let described = ok_in(&router, region, "DescribeKey", &json!({"KeyId": shared}))
            .await["KeyMetadata"]
            .clone();
        assert_eq!(
            &described, metadata,
            "case {region}: another Region's delete must not remove this one's alias, \
             got {described}"
        );
    }

    // A Region that holds no alias under the name refuses the delete, and the Regions
    // that do hold one keep it.
    let (status, code) = failure_in(
        &router,
        regions[0],
        "DeleteAlias",
        &json!({"AliasName": shared}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case already deleted: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "NotFoundException",
        "case already deleted: expected NotFoundException, got {code}"
    );
    let described = ok_in(&router, regions[1], "DescribeKey", &json!({"KeyId": shared}))
        .await["KeyMetadata"]
        .clone();
    assert_eq!(
        described, keys[1].1,
        "case {}: a refused delete elsewhere must change nothing here, got {described}",
        regions[1]
    );
}

/// `DeleteAlias` is refused, with nothing removed, for every name this endpoint holds
/// no alias under.
#[tokio::test]
async fn deleting_a_name_no_alias_is_bound_to_is_refused() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "target"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    create_alias(&router, "setup", "alias/live", &key_id).await;

    // Rows: names outside any binding this endpoint holds — never created, created in
    // another Region, outside the `alias/` prefix rules, and under the reserved
    // AWS-managed prefix (which a caller can never have created).
    let other_region_alias = "alias/elsewhere";
    let elsewhere = ok_in(
        &router,
        "ap-south-1",
        "CreateKey",
        &json!({"Description": "a key of another Region"}),
    )
    .await["KeyMetadata"]
        .clone();
    let elsewhere_id = text("setup", &elsewhere, "KeyId");
    create_alias_in(
        &router,
        "ap-south-1",
        "setup",
        other_region_alias,
        &elsewhere_id,
    )
    .await;
    let names: Vec<(&str, String)> = vec![
        ("never created", "alias/absent".to_string()),
        ("created in another Region", other_region_alias.to_string()),
        ("no prefix at all", "plain-name".to_string()),
        ("prefix in the middle", "my/alias/name".to_string()),
        ("prefix without its slash", "alias".to_string()),
        ("reserved AWS-managed prefix", "alias/aws/elasticache".to_string()),
        ("a key id, not an alias name", key_id.clone()),
    ];
    for (case, alias) in names {
        let (status, code) = failure(&router, "DeleteAlias", &json!({"AliasName": alias})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected NotFoundException, got {code}"
        );
    }

    // Nothing above removed the alias that does exist, here or in the other Region.
    assert_eq!(
        describe(&router, "alias/live").await,
        metadata,
        "case refused deletes: the live alias must still resolve"
    );
    let described = ok_in(
        &router,
        "ap-south-1",
        "DescribeKey",
        &json!({"KeyId": other_region_alias}),
    )
    .await["KeyMetadata"]
        .clone();
    assert_eq!(
        text("other Region", &described, "KeyId"),
        elsewhere_id,
        "case refused deletes: the other Region's alias must still resolve, got {described}"
    );
}

/// `UpdateAlias` moves an existing binding to another key: the name resolves to the
/// new key for every operation, keeps its own name, ARN and creation date, and the key
/// it used to name is untouched.
#[tokio::test]
async fn update_alias_repoints_the_name_at_the_new_key() {
    let router = router();

    // Rows: the identifier form the new target is named by, and the alias name.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("target by key id", "alias/moved/by-id", false),
        ("target by key ARN", "alias/moved-by-arn", true),
    ];
    for (case, alias, by_arn) in rows {
        let first = create_key(&router, &json!({"Description": format!("{case} first")})).await;
        let second = create_key(&router, &json!({"Description": format!("{case} second")})).await;
        let first_id = text(case, &first, "KeyId");
        let second_id = text(case, &second, "KeyId");
        let second_arn = text(case, &second, "Arn");

        create_alias(&router, case, alias, &first_id).await;
        let before = list_aliases(&router, &json!({"KeyId": first_id})).await["Aliases"][0].clone();

        let target = if by_arn { &second_arn } else { &second_id };
        update_alias(&router, case, alias, target).await;

        // Every `KeyId` member follows the new binding.
        for identifier in [alias.to_string(), alias_arn(REGION, alias)] {
            let described = describe(&router, &identifier).await;
            assert_eq!(
                described, second,
                "case {case} / {identifier}: the name must resolve to the new key, got \
                 {described}"
            );
        }
        let encrypted = ok(
            &router,
            "Encrypt",
            &json!({"KeyId": alias, "Plaintext": b64(b"after the update")}),
        )
        .await;
        assert_eq!(
            encrypted["KeyId"],
            json!(second_arn),
            "case {case}: a cryptographic operation must reach the new key, got {encrypted}"
        );

        // The old key is still there, and no longer carries the name.
        assert_eq!(
            describe(&router, &first_id).await,
            first,
            "case {case}: the previous target must be untouched"
        );
        let answer = list_aliases(&router, &json!({"KeyId": first_id})).await;
        assert_eq!(
            listed_names(case, &answer),
            Vec::<String>::new(),
            "case {case}: the previous target must have no names left, got {answer}"
        );

        // The alias keeps its identity; only the target and the last-updated instant
        // move.
        let after = list_aliases(&router, &json!({"KeyId": second_id})).await["Aliases"][0].clone();
        for member in ["AliasName", "AliasArn", "CreationDate"] {
            assert_eq!(
                after[member], before[member],
                "case {case}: UpdateAlias must not change {member}, got {after}"
            );
        }
        assert_eq!(
            after["TargetKeyId"],
            json!(second_id),
            "case {case}: the entry must report the new target, got {after}"
        );
        let created = after["CreationDate"].as_f64().unwrap_or_default();
        let updated = after["LastUpdatedDate"].as_f64().unwrap_or_default();
        assert!(
            updated >= created,
            "case {case}: LastUpdatedDate must be at or after CreationDate, got {after}"
        );
    }
}

/// `UpdateAlias` is refused — leaving the binding exactly where it was — for a name
/// bound to nothing, a target that is not an available key of this Region, and a
/// target whose type or key usage differs from the current one.
#[tokio::test]
async fn update_alias_refuses_what_it_cannot_honour() {
    let router = router();
    let account = engine::emulator_account_id();
    let symmetric = create_key(&router, &json!({"Description": "symmetric"})).await;
    let symmetric_id = text("setup", &symmetric, "KeyId");
    let hmac = create_key(&router, &json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}))
        .await;
    let hmac_id = text("setup", &hmac, "KeyId");
    let rsa_encrypt = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "ENCRYPT_DECRYPT"}),
    )
    .await;
    let rsa_encrypt_id = text("setup", &rsa_encrypt, "KeyId");
    let rsa_sign =
        create_key(&router, &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"})).await;
    let rsa_sign_id = text("setup", &rsa_sign, "KeyId");
    let pending = create_key(&router, &json!({"Description": "pending deletion"})).await;
    let pending_id = text("setup", &pending, "KeyId");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_id, "PendingWindowInDays": 7}),
    )
    .await;

    create_alias(&router, "setup", "alias/symmetric", &symmetric_id).await;
    create_alias(&router, "setup", "alias/rsa-encrypt", &rsa_encrypt_id).await;

    // Rows: the alias, the target, and the error code expected. A name bound to
    // nothing and a target that is not an available key are both `NotFoundException`;
    // a target of another TYPE or another key usage breaks a documented restriction
    // KMS models no error for, so it is the synthetic `ValidationException`; a target
    // whose deletion is scheduled is a key-state refusal.
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "a name bound to nothing",
            "alias/absent",
            symmetric_id.clone(),
            "NotFoundException",
        ),
        (
            "a name outside the prefix rules",
            "plain-name",
            symmetric_id.clone(),
            "NotFoundException",
        ),
        (
            "an unknown target key id",
            "alias/symmetric",
            "11111111-2222-3333-4444-555555555555".to_string(),
            "NotFoundException",
        ),
        (
            "a target key ARN of another Region",
            "alias/symmetric",
            format!("arn:aws:kms:us-west-2:{account}:key/{symmetric_id}"),
            "NotFoundException",
        ),
        (
            "another service's ARN as the target",
            "alias/symmetric",
            format!("arn:aws:s3:::bucket/{account}"),
            "NotFoundException",
        ),
        (
            "an alias as the target",
            "alias/symmetric",
            "alias/rsa-encrypt".to_string(),
            "ValidationException",
        ),
        (
            "symmetric name moved to an HMAC key",
            "alias/symmetric",
            hmac_id.clone(),
            "ValidationException",
        ),
        (
            "symmetric name moved to an asymmetric key",
            "alias/symmetric",
            rsa_encrypt_id.clone(),
            "ValidationException",
        ),
        (
            "asymmetric name moved to the same type with another key usage",
            "alias/rsa-encrypt",
            rsa_sign_id.clone(),
            "ValidationException",
        ),
        (
            "a target whose deletion is scheduled",
            "alias/symmetric",
            pending_id.clone(),
            "KMSInvalidStateException",
        ),
    ];
    for (case, alias, target, expected) in rows {
        let (status, code) = failure(
            &router,
            "UpdateAlias",
            &json!({"AliasName": alias, "TargetKeyId": target}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code}"
        );
    }

    // Every refusal above left both bindings on the key they named.
    assert_eq!(
        describe(&router, "alias/symmetric").await,
        symmetric,
        "case refused updates: the binding must not have moved"
    );
    assert_eq!(
        describe(&router, "alias/rsa-encrypt").await,
        rsa_encrypt,
        "case refused updates: the binding must not have moved"
    );

    // The same type and the same key usage IS accepted — the restriction is about the
    // pair, not about moving at all — and so is a no-op update onto the current key.
    let other_symmetric = create_key(&router, &json!({"Description": "another symmetric"})).await;
    let other_symmetric_id = text("setup", &other_symmetric, "KeyId");
    update_alias(
        &router,
        "same type and usage",
        "alias/symmetric",
        &other_symmetric_id,
    )
    .await;
    assert_eq!(
        describe(&router, "alias/symmetric").await,
        other_symmetric,
        "case same type and usage: the binding must have moved"
    );
    update_alias(
        &router,
        "onto the current target",
        "alias/symmetric",
        &other_symmetric_id,
    )
    .await;
    assert_eq!(
        describe(&router, "alias/symmetric").await,
        other_symmetric,
        "case onto the current target: the binding must stay where it is"
    );
}

/// `ListAliases` reports exactly the bindings the store holds, in creation order,
/// narrowed by `KeyId` and paged by `Limit` and `Marker`.
#[tokio::test]
async fn list_aliases_reports_every_binding_in_creation_order_and_pages() {
    let router = router();
    let first = create_key(&router, &json!({"Description": "first"})).await;
    let second = create_key(&router, &json!({"Description": "second"})).await;
    let first_id = text("setup", &first, "KeyId");
    let first_arn = text("setup", &first, "Arn");
    let second_id = text("setup", &second, "KeyId");

    // Rows: the names, in the order they are created, and the key each one names.
    let created: Vec<(String, &str)> = vec![
        ("alias/one".to_string(), "first"),
        ("alias/two".to_string(), "second"),
        ("alias/three".to_string(), "first"),
        ("alias/four".to_string(), "second"),
        ("alias/five".to_string(), "first"),
    ];
    for (alias, owner) in &created {
        let target = if *owner == "first" { &first_id } else { &second_id };
        create_alias(&router, alias, alias, target).await;
    }
    let all: Vec<String> = created.iter().map(|(alias, _)| alias.clone()).collect();
    let of_first: Vec<String> = created
        .iter()
        .filter(|(_, owner)| *owner == "first")
        .map(|(alias, _)| alias.clone())
        .collect();

    // An empty request reports every binding of this Region, in creation order.
    let answer = list_aliases(&router, &json!({})).await;
    assert_eq!(
        listed_names("every alias", &answer),
        all,
        "case every alias: expected creation order, got {answer}"
    );
    assert_eq!(
        answer["Truncated"],
        json!(false),
        "case every alias: a complete list is not truncated, got {answer}"
    );
    assert!(
        answer["NextMarker"].is_null(),
        "case every alias: a complete list carries no NextMarker, got {answer}"
    );

    // Each entry reports the binding it stands for.
    let entry = answer["Aliases"][0].clone();
    assert_eq!(
        entry["AliasArn"],
        json!(alias_arn(REGION, &all[0])),
        "case first entry: expected this endpoint's alias ARN, got {entry}"
    );
    assert_eq!(
        entry["TargetKeyId"],
        json!(first_id),
        "case first entry: expected the target key id, got {entry}"
    );
    assert!(
        entry["CreationDate"].as_f64().unwrap_or_default() > 0.0,
        "case first entry: expected a creation date, got {entry}"
    );

    // `KeyId` narrows the list to one key's bindings, whatever form names it.
    for (form, identifier) in [
        ("key id", first_id.clone()),
        ("key ARN", first_arn.clone()),
        ("an alias of that key", all[0].clone()),
    ] {
        let answer = list_aliases(&router, &json!({"KeyId": identifier})).await;
        assert_eq!(
            listed_names(form, &answer),
            of_first,
            "case filtered by {form}: expected only that key's aliases, got {answer}"
        );
    }

    // Paging: every page size walks the same list in the same order, exactly once.
    for limit in [1usize, 2, 3, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut request = json!({"Limit": limit});
            if let Some(token) = &marker {
                request["Marker"] = json!(token);
            }
            let answer = list_aliases(&router, &request).await;
            let page = listed_names(&format!("limit {limit}"), &answer);
            assert!(
                page.len() <= limit,
                "case limit {limit}: a page must not exceed the limit, got {answer}"
            );
            seen.extend(page);
            if answer["Truncated"] == json!(true) {
                marker = Some(text(&format!("limit {limit}"), &answer, "NextMarker"));
            } else {
                assert!(
                    answer["NextMarker"].is_null(),
                    "case limit {limit}: the last page carries no NextMarker, got {answer}"
                );
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case limit {limit}: paging must walk every alias once, in creation order"
        );
    }

    // The published page bound is accepted; one past it is refused, and so is a marker
    // this service did not mint.
    let answer = list_aliases(&router, &json!({"Limit": 100})).await;
    assert_eq!(
        listed_names("limit at the published bound", &answer),
        all,
        "case limit at the published bound: expected every alias, got {answer}"
    );
    let (status, code) = failure(&router, "ListAliases", &json!({"Limit": 101})).await;
    assert_eq!(
        status, 400,
        "case one past the published bound: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "ValidationException",
        "case one past the published bound: expected ValidationException, got {code}"
    );
    for (case, marker) in [
        ("not a token at all", "not-a-marker".to_string()),
        ("a token for another list", hex::encode("keys/1/whatever")),
        (
            "a token naming an alias that is gone",
            hex::encode(format!("aliases/1/{}", alias_arn(REGION, "alias/absent"))),
        ),
    ] {
        let (status, code) = failure(&router, "ListAliases", &json!({"Marker": marker})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidMarkerException",
            "case {case}: expected InvalidMarkerException, got {code}"
        );
    }

    // A Region of its own: the list is per endpoint, and an empty one is an empty
    // list rather than an error.
    let answer = ok_in(&router, "eu-west-1", "ListAliases", &json!({})).await;
    assert_eq!(
        listed_names("another Region", &answer),
        Vec::<String>::new(),
        "case another Region: expected no aliases, got {answer}"
    );
}
