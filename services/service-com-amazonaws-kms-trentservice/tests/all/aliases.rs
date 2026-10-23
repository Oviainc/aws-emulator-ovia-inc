// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateAlias`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: `CreateAlias` persists a friendly name for the KMS key its
//! `TargetKeyId` names, answers an empty 200, and from then on every operation taking
//! a `KeyId` resolves that name — bare `alias/<name>` or the alias ARN — to that key.
//! The name is unique per account and Region, a key may carry many names, and the
//! call is refused (persisting nothing) when the name is not one a caller may create,
//! when the target names no key of this Region, or when the target key's key state is
//! not compatible with the operation.
//!
//! Every assertion is about that rule over a TABLE, whose rows differ in the alias
//! name (length, character class, number of `/` segments), in the kind of key the
//! alias targets, in the identifier form the request and the later read use, in the
//! Region the request is signed for, and in how many aliases one key already holds.

use serde_json::{json, Value};

use super::harness::{
    alias_arn, call, create_alias, create_alias_in, create_key, describe, error_code, failure,
    failure_in, ok, ok_in, router, text, REGION,
};

/// One row of the "which key states accept alias management" table: the case name,
/// the calls that put the target key into that state, and the error code the create
/// must then answer (`None` when the state accepts it).
type KeyStateRow = (&'static str, Vec<(&'static str, Value)>, Option<&'static str>);

/// One row of the "a created alias resolves to its key" table: the alias name, and
/// the `CreateKey` request for the key it targets.
///
/// The names span `AliasNameType`'s modelled `len 1..256` and
/// `^[a-zA-Z0-9:/_-]+$` pattern
/// (`smithy_query(query="operation",operation="CreateAlias")`): the shortest name the
/// `alias/` prefix allows, several `/` segments, every other character class the
/// pattern permits, the longest name the length allows, and a name that merely STARTS
/// with `alias/aws` without being under the reserved `alias/aws/` prefix. The keys
/// span the `KeySpec`/`KeyUsage` combinations and the key states alias management
/// accepts.
fn alias_rows() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "plain name, default symmetric key",
            "alias/plain-name".to_string(),
            json!({"Description": "plain"}),
        ),
        (
            "one-character name",
            "alias/a".to_string(),
            json!({}),
        ),
        (
            "several path segments",
            "alias/team/project/key".to_string(),
            json!({"Description": "segments"}),
        ),
        (
            "every permitted character class",
            "alias/Mixed_Case-1:2".to_string(),
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "longest name the modelled length allows",
            format!("alias/{}", "x".repeat(250)),
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "name starting with alias/aws but not the reserved prefix",
            "alias/awsome-key".to_string(),
            json!({"Description": "not reserved"}),
        ),
        (
            "multi-Region key",
            "alias/multi-region".to_string(),
            json!({"MultiRegion": true}),
        ),
    ]
}

/// A created alias is a second name for its key: `DescribeKey` reports exactly the
/// target key's metadata whether it is asked by alias name, by alias ARN, by key id
/// or by key ARN — and asking by alias changes nothing about the key.
#[tokio::test]
async fn a_created_alias_resolves_to_its_target_key() {
    let router = router();

    for (case, alias, create_request) in alias_rows() {
        let metadata = create_key(&router, &create_request).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        create_alias(&router, case, &alias, &key_id).await;

        for (form, identifier) in [
            ("alias name", alias.clone()),
            ("alias ARN", alias_arn(REGION, &alias)),
            ("key id", key_id.clone()),
            ("key ARN", key_arn.clone()),
        ] {
            let described = describe(&router, &identifier).await;
            assert_eq!(
                described, metadata,
                "case {case} read by {form}: DescribeKey must report the target key's \
                 metadata, got {described}"
            );
        }

        // The read through the alias wrote nothing: the key is byte-for-byte what
        // `CreateKey` answered.
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, metadata,
            "case {case}: resolving an alias must leave the key untouched, got {after}"
        );
    }
}

/// An alias resolves for every operation that takes a `KeyId`, not only for
/// `DescribeKey`: a symmetric key named by alias encrypts, and the ciphertext decrypts
/// back to the plaintext while reporting the key's ARN.
#[tokio::test]
async fn an_alias_names_the_key_for_a_cryptographic_operation() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "aliased"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");

    // Rows: the identifier form `Encrypt` is given, and the plaintext it seals.
    let alias = "alias/encrypting-key";
    create_alias(&router, "setup", alias, &key_id).await;
    let rows: Vec<(&str, String, &str)> = vec![
        ("alias name", alias.to_string(), "first"),
        ("alias ARN", alias_arn(REGION, alias), "second"),
        ("key id", key_id.clone(), "third"),
    ];

    for (case, identifier, plaintext) in rows {
        let encrypted = ok(
            &router,
            "Encrypt",
            &json!({
                "KeyId": identifier,
                "Plaintext": aws_smithy_types::base64::encode(plaintext.as_bytes()),
            }),
        )
        .await;
        assert_eq!(
            encrypted["KeyId"],
            json!(key_arn),
            "case {case}: Encrypt must report the resolved key's ARN, got {encrypted}"
        );

        let decrypted = ok(
            &router,
            "Decrypt",
            &json!({"CiphertextBlob": encrypted["CiphertextBlob"].clone()}),
        )
        .await;
        assert_eq!(
            decrypted["Plaintext"],
            json!(aws_smithy_types::base64::encode(plaintext.as_bytes())),
            "case {case}: Decrypt must return the plaintext that was sealed, got {decrypted}"
        );
        assert_eq!(
            decrypted["KeyId"],
            json!(key_arn),
            "case {case}: Decrypt must report the key's ARN, got {decrypted}"
        );
    }
}

/// A key carries many aliases, and a name belongs to one key: the names of one key all
/// resolve to it, a name already taken is refused however it is targeted, and the
/// refusal leaves the existing alias pointing where it did.
#[tokio::test]
async fn a_key_may_have_many_aliases_but_a_name_names_one_key() {
    let router = router();
    let first = create_key(&router, &json!({"Description": "first"})).await;
    let second = create_key(&router, &json!({"Description": "second"})).await;
    let first_id = text("setup", &first, "KeyId");
    let first_arn = text("setup", &first, "Arn");
    let second_id = text("setup", &second, "KeyId");

    // Rows: the names given to the FIRST key, and the identifier form used to target
    // it — a key is named by key id or by key ARN, and both must work.
    let names: Vec<(&str, &str, String)> = vec![
        ("by key id", "alias/first/one", first_id.clone()),
        ("by key ARN", "alias/first/two", first_arn.clone()),
        ("by key id again", "alias/first-3", first_id.clone()),
    ];
    for (case, alias, target) in &names {
        create_alias(&router, case, alias, target).await;
    }
    for (case, alias, _) in &names {
        let described = describe(&router, alias).await;
        assert_eq!(
            described, first,
            "case {case}: every alias of a key must resolve to it, got {described}"
        );
    }

    // A different key takes a name of its own, and the first key's names are untouched.
    create_alias(&router, "second key", "alias/second", &second_id).await;
    let described = describe(&router, "alias/second").await;
    assert_eq!(
        described, second,
        "case second key: its alias must resolve to it, got {described}"
    );
    for (case, alias, _) in &names {
        let described = describe(&router, alias).await;
        assert_eq!(
            described, first,
            "case {case}: another key's alias must not move this one, got {described}"
        );
    }

    // A name already taken is refused — whether the request points it at another key
    // or at the key it already names — and nothing moves.
    for (case, alias, target) in [
        ("taken name, another key", "alias/first/one", second_id.clone()),
        ("taken name, same key", "alias/first/one", first_id.clone()),
        ("taken name, targeted by ARN", "alias/second", first_arn.clone()),
    ] {
        let (status, code) = failure(
            &router,
            "CreateAlias",
            &json!({"AliasName": alias, "TargetKeyId": target}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "AlreadyExistsException",
            "case {case}: expected AlreadyExistsException, got {code}"
        );
    }
    assert_eq!(
        describe(&router, "alias/first/one").await,
        first,
        "case taken name: a refused create must leave the alias on its original key"
    );
    assert_eq!(
        describe(&router, "alias/second").await,
        second,
        "case taken name: a refused create must leave the alias on its original key"
    );
}

/// A name a caller may not create is refused with `InvalidAliasNameException`, and
/// nothing is persisted under it.
///
/// The rows are the two rules the model cannot express
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="AliasName")`): the
/// value "must begin with alias/ followed by a name", and "the alias name cannot
/// begin with alias/aws/".
#[tokio::test]
async fn an_alias_name_outside_the_documented_rules_is_refused() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "target"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    let names: Vec<(&str, String)> = vec![
        ("no prefix at all", "plain-name".to_string()),
        ("prefix in the middle", "my/alias/name".to_string()),
        ("prefix without its slash", "alias".to_string()),
        ("prefix with nothing after it", "alias/".to_string()),
        ("prefix in the wrong case", "ALIAS/name".to_string()),
        ("reserved AWS-managed prefix", "alias/aws/elasticache".to_string()),
        ("reserved prefix with nothing after it", "alias/aws/".to_string()),
        (
            "reserved prefix, deeper name",
            "alias/aws/team/project".to_string(),
        ),
    ];
    for (case, alias) in names {
        let (status, code) = failure(
            &router,
            "CreateAlias",
            &json!({"AliasName": alias, "TargetKeyId": key_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidAliasNameException",
            "case {case}: expected InvalidAliasNameException, got {code}"
        );
        let (status, payload) = call(&router, REGION, "DescribeKey", &json!({"KeyId": alias})).await;
        assert_ne!(
            status, 200,
            "case {case}: a refused name must resolve to nothing, got {payload}"
        );
    }

    // One character past the modelled `len 1..256` the request never reaches the
    // handler: the boundary validator refuses it.
    for (case, alias) in [
        ("one past the modelled maximum", format!("alias/{}", "x".repeat(251))),
        ("empty, below the modelled minimum", String::new()),
    ] {
        let (status, payload) = call(
            &router,
            REGION,
            "CreateAlias",
            &json!({"AliasName": alias, "TargetKeyId": key_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {payload} ({})",
            error_code(&payload)
        );
    }
}

/// A target that is not a key of this Region, or a key whose key state alias
/// management does not accept, is refused — and the name stays free.
#[tokio::test]
async fn a_target_that_is_not_an_available_key_is_refused() {
    let router = router();
    let account = engine::emulator_account_id();
    let metadata = create_key(&router, &json!({"Description": "target"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    create_alias(&router, "setup", "alias/existing", &key_id).await;

    // `CreateAlias` models no `InvalidArnException`, so an identifier that is not a
    // KMS ARN at all is reported the way the operations without that error report it:
    // as a target that was not found.
    let absent: Vec<(&str, String)> = vec![
        (
            "unknown key id",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "key ARN of another Region",
            format!("arn:aws:kms:us-west-2:{account}:key/{key_id}"),
        ),
        ("another service's ARN", format!("arn:aws:s3:::bucket/{account}")),
        ("too few ARN segments", "arn:aws:kms".to_string()),
    ];
    for (case, target) in absent {
        let (status, code) = failure(
            &router,
            "CreateAlias",
            &json!({"AliasName": "alias/fresh", "TargetKeyId": target}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected NotFoundException, got {code}"
        );
    }

    // "Specify the key ID or key ARN of the KMS key"
    // (`smithy_query(query="doc",shape="CreateAliasRequest",member="TargetKeyId")`):
    // an alias is not a key, so it cannot be a target.
    for (case, target) in [
        ("an alias name as the target", "alias/existing".to_string()),
        ("an alias ARN as the target", alias_arn(REGION, "alias/existing")),
    ] {
        let (status, code) = failure(
            &router,
            "CreateAlias",
            &json!({"AliasName": "alias/fresh", "TargetKeyId": target}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected ValidationException, got {code}"
        );
    }

    // Nothing above persisted the name it asked for.
    let (status, payload) = call(
        &router,
        REGION,
        "DescribeKey",
        &json!({"KeyId": "alias/fresh"}),
    )
    .await;
    assert_ne!(
        status, 200,
        "case refused targets: no alias may have been created, got {payload}"
    );

    // A key whose deletion is scheduled is not in a key state alias management
    // accepts; a disabled key is, and a cancelled deletion returns the key to one.
    let states: Vec<KeyStateRow> = vec![
        ("enabled", vec![], None),
        ("disabled", vec![("DisableKey", json!({}))], None),
        (
            "pending deletion",
            vec![("ScheduleKeyDeletion", json!({"PendingWindowInDays": 7}))],
            Some("KMSInvalidStateException"),
        ),
        (
            "deletion cancelled",
            vec![("CancelKeyDeletion", json!({}))],
            None,
        ),
    ];
    for (index, (case, setup, expected_code)) in states.into_iter().enumerate() {
        for (operation, extra) in setup {
            let mut request = json!({"KeyId": key_id});
            for (member, value) in extra.as_object().expect("an object") {
                request[member] = value.clone();
            }
            ok(&router, operation, &request).await;
        }
        let alias = format!("alias/state-{index}");
        match expected_code {
            None => {
                create_alias(&router, case, &alias, &key_id).await;
                let described = describe(&router, &alias).await;
                assert_eq!(
                    text(case, &described, "KeyId"),
                    key_id,
                    "case {case}: the new alias must resolve to the key, got {described}"
                );
            }
            Some(expected) => {
                let (status, code) = failure(
                    &router,
                    "CreateAlias",
                    &json!({"AliasName": alias, "TargetKeyId": key_id}),
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
                let (status, payload) =
                    call(&router, REGION, "DescribeKey", &json!({"KeyId": alias})).await;
                assert_ne!(
                    status, 200,
                    "case {case}: a refused create must persist no alias, got {payload}"
                );
            }
        }
    }
}

/// An alias belongs to one Region: "The alias must be unique in the account and
/// Region, but you can have aliases with the same name in different Regions"
/// (`smithy_query(query="doc",shape="CreateAlias")`). So the same name may name a
/// different key through each endpoint, and an alias of one Region resolves to nothing
/// through another.
#[tokio::test]
async fn an_alias_is_unique_per_region_and_visible_only_there() {
    let router = router();

    // Rows: the Region, and the key the shared name points at there.
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

    for (region, metadata) in &keys {
        for (form, identifier) in [
            ("alias name", shared.to_string()),
            ("alias ARN", alias_arn(region, shared)),
        ] {
            let described = ok_in(&router, region, "DescribeKey", &json!({"KeyId": identifier}))
                .await["KeyMetadata"]
                .clone();
            assert_eq!(
                &described, metadata,
                "case {region} read by {form}: the alias must resolve to that Region's \
                 key, got {described}"
            );
        }
        // The same name in the OTHER Regions is a different alias, and that Region's
        // alias ARN is not servable here at all.
        for other in regions.iter().filter(|other| *other != region) {
            let (status, code) = failure_in(
                &router,
                region,
                "DescribeKey",
                &json!({"KeyId": alias_arn(other, shared)}),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {region} asked for {other}'s alias ARN: expected HTTP 400, got \
                 {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {region} asked for {other}'s alias ARN: expected NotFoundException, \
                 got {code}"
            );
        }
    }

    // A name taken in one Region is still free in a Region that has not taken it.
    let fresh = ok_in(
        &router,
        "ap-south-1",
        "CreateKey",
        &json!({"Description": "late Region"}),
    )
    .await["KeyMetadata"]
        .clone();
    let fresh_id = text("late Region", &fresh, "KeyId");
    create_alias_in(&router, "ap-south-1", "late Region", shared, &fresh_id).await;
    let described = ok_in(
        &router,
        "ap-south-1",
        "DescribeKey",
        &json!({"KeyId": shared}),
    )
    .await["KeyMetadata"]
        .clone();
    assert_eq!(
        described, fresh,
        "case late Region: the shared name must resolve to that Region's key, got {described}"
    );
}

/// A key carries at most the published number of aliases: KMS's resource quota
/// "Aliases per KMS key: 50". At the bound the create succeeds; one past it the
/// request is refused with the modelled `LimitExceededException`, and the bound is
/// per key, so another key still takes its own aliases.
#[tokio::test]
async fn a_key_holds_at_most_the_published_number_of_aliases() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "many aliases"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let limit = 50;

    for index in 1..=limit {
        let alias = format!("alias/bounded-{index}");
        create_alias(&router, &format!("alias {index} of {limit}"), &alias, &key_id).await;
    }
    let described = describe(&router, &format!("alias/bounded-{limit}")).await;
    assert_eq!(
        described, metadata,
        "case at the bound: the last alias must resolve to the key, got {described}"
    );

    let past = format!("alias/bounded-{}", limit + 1);
    let (status, code) = failure(
        &router,
        "CreateAlias",
        &json!({"AliasName": past, "TargetKeyId": key_id}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "LimitExceededException",
        "case one past the bound: expected LimitExceededException, got {code}"
    );

    // The bound counts THIS key's aliases: a different key is unaffected.
    let other = create_key(&router, &json!({"Description": "unaffected"})).await;
    let other_id = text("other key", &other, "KeyId");
    create_alias(&router, "other key", &past, &other_id).await;
    let described = describe(&router, &past).await;
    assert_eq!(
        described, other,
        "case other key: the bound is per key, got {described}"
    );
}
