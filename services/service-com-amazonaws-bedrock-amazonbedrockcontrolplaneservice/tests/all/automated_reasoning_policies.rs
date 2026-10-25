// SPDX-License-Identifier: CC0-1.0

//! The Automated Reasoning policy resource end-to-end:
//! `CreateAutomatedReasoningPolicy` writes it, `GetAutomatedReasoningPolicy` reads it
//! back, over the same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A created policy round-trips**: every member the read reports comes back out of
//!    the stored record, for requests differing in whether a definition, a description, a
//!    KMS key and tags are present, and in how many types, variables and rules the
//!    definition carries. The minted `policyArn` is one `AutomatedReasoningPolicyArn`
//!    admits, and the policy's working `version` is the `DRAFT` the create documents.
//! 2. **`definitionHash` is a function of the definition alone**: two policies whose
//!    definitions agree report one hash whatever else their requests carried, any change
//!    to a definition changes it, and it is always the 128 `[0-9a-z]` digits
//!    `AutomatedReasoningPolicyHash` admits.
//! 3. **`kmsKeyId` is reported as the `KmsKeyArn` it names**, over each spelling the
//!    member's pattern admits.
//! 4. **A name is unique per account per region**, so a second create of a name in one
//!    region conflicts while the same name in another region is minted.
//! 5. **A replayed `clientRequestToken` mints nothing new**, and two distinct tokens mint
//!    two policies.
//! 6. **An ARN this account holds no policy under is 404**, including a well-formed ARN of
//!    another region and a versioned ARN of a policy that has no versions.
//! 7. **A resource carries at most 50 tags**, the bound `TooManyTagsException` documents.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="CreateAutomatedReasoningPolicy,GetAutomatedReasoningPolicy")`
//! / `query="shape"`). The requests go through `crate::support` — the ONE harness every
//! suite in this crate drives the emulator with.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

/// The working version a freshly created policy carries:
/// `CreateAutomatedReasoningPolicyResponse$version` documents "The initial version is
/// always DRAFT".
const DRAFT: &str = "DRAFT";

/// How many digits `AutomatedReasoningPolicyHash` carries (`@length(128, 128)`, pattern
/// `^[0-9a-z]{128}$`).
const HASH_DIGITS: usize = 128;

/// How many characters the resource half of an `AutomatedReasoningPolicyArn` carries
/// (`automated-reasoning-policy/[a-z0-9]{12}`), which is also
/// `AutomatedReasoningPolicyId`'s own `^[a-z0-9]{12}$`.
const POLICY_ID_DIGITS: usize = 12;

/// The most tags one resource may carry, as `TooManyTagsException` documents it ("50 tags
/// per resource").
const MAX_TAGS: usize = 50;

/// The ARN prefix every policy minted for `region` carries.
fn arn_prefix(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:automated-reasoning-policy/")
}

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/automated-reasoning-policies", Some(body))
        .await
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/automated-reasoning-policies", Some(body))
        .await
}

async fn get(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

/// A create body carrying only the required member.
fn named(name: &str) -> Value {
    json!({ "name": name })
}

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (`smithy_query(query="closure",operation="CreateAutomatedReasoningPolicy")`:
/// a type name is `^\p{L}[\p{L}\p{M}0-9_]*( [\p{L}\p{M}0-9_]+)*$`, a rule id is
/// `^[A-Z][0-9A-Z]{11}$`).
fn definition(count: usize) -> Value {
    let types: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Type{index}"),
                "description": format!("type {index}"),
                "values": [
                    { "value": format!("Value{index}A"), "description": "first" },
                    { "value": format!("Value{index}B") },
                ],
            })
        })
        .collect();
    let variables: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("variable{index}"),
                "type": format!("Type{index}"),
                "description": format!("variable {index}"),
            })
        })
        .collect();
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "id": format!("R{:011}", index),
                "expression": format!("variable{index} == Value{index}A"),
            })
        })
        .collect();
    json!({
        "version": "1",
        "types": types,
        "variables": variables,
        "rules": rules,
    })
}

/// The id half of a minted policy ARN, checked against the pattern that admits it.
fn policy_id(case: &str, arn: &str, region: &str) -> String {
    let id = arn
        .strip_prefix(&arn_prefix(region))
        .unwrap_or_else(|| panic!("case {case}: ARN {arn:?} is not under {:?}", arn_prefix(region)))
        .to_string();
    assert!(
        id.len() == POLICY_ID_DIGITS
            && id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "case {case}: the ARN resource id must be {POLICY_ID_DIGITS} digits of [a-z0-9], \
         got {id:?}"
    );
    id
}

/// Assert `hash` is one `AutomatedReasoningPolicyHash` admits.
fn assert_hash_shape(case: &str, hash: &str) {
    assert!(
        hash.len() == HASH_DIGITS
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "case {case}: definitionHash must be {HASH_DIGITS} digits of [0-9a-z], got {hash:?} \
         ({} chars)",
        hash.len()
    );
}

// ---------------------------------------------------------------------------------------
// 1. What a create stores is what a read reports.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_created_policy_round_trips_through_get() {
    // Rows differ along what the create carries: whether a definition is present and how
    // large it is, whether a description and tags are present, and the policy's name.
    struct Row {
        name: &'static str,
        description: Option<&'static str>,
        definition: Option<Value>,
        tags: Vec<(&'static str, &'static str)>,
    }

    let rows = vec![
        Row {
            name: "minimal policy",
            description: None,
            definition: None,
            tags: Vec::new(),
        },
        Row {
            name: "described-policy",
            description: Some("validates the returns rules"),
            definition: None,
            tags: vec![("team", "search")],
        },
        Row {
            name: "one_rule policy 1",
            description: Some("a single rule"),
            definition: Some(definition(1)),
            tags: Vec::new(),
        },
        Row {
            name: "ThreeRulePolicy",
            description: Some("three types, variables and rules"),
            definition: Some(definition(3)),
            tags: vec![("team", "search"), ("env", "test")],
        },
    ];

    let emu = emulator();
    for row in &rows {
        let case = row.name;
        let mut body = named(row.name);
        if let Some(description) = row.description {
            body["description"] = json!(description);
        }
        if let Some(definition) = &row.definition {
            body["policyDefinition"] = definition.clone();
        }
        if !row.tags.is_empty() {
            body["tags"] = Value::Array(
                row.tags
                    .iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }

        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
            created.status, created.body
        );
        let arn = created.text("policyArn");
        let id = policy_id(case, &arn, REGION);
        assert_eq!(
            created.text("version"),
            DRAFT,
            "case {case:?}: a freshly created policy reports the DRAFT version, got {:?}",
            created.get("version")
        );
        assert_eq!(
            created.text("name"),
            row.name,
            "case {case:?}: the create must report the name it stored"
        );
        assert_eq!(
            created.get("description"),
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: the create must report the description it stored, got {:?}",
            created.get("description")
        );
        assert_hash_shape(case, &created.text("definitionHash"));
        let created_at = created.instant("createdAt");
        let updated_at = created.instant("updatedAt");
        assert!(
            (created_at - updated_at).abs() < f64::EPSILON,
            "case {case:?}: a policy nothing has updated reports one instant for both, \
             got createdAt={created_at} updatedAt={updated_at}"
        );

        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetAutomatedReasoningPolicy must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("policyArn"),
            arn,
            "case {case:?}: the read must report the ARN the create minted"
        );
        assert_eq!(
            read.text("policyId"),
            id,
            "case {case:?}: policyId is the resource half of the ARN, got {:?}",
            read.get("policyId")
        );
        assert_eq!(
            read.text("name"),
            row.name,
            "case {case:?}: the read must report the stored name"
        );
        assert_eq!(
            read.text("version"),
            DRAFT,
            "case {case:?}: the unversioned ARN names the DRAFT, got {:?}",
            read.get("version")
        );
        assert_eq!(
            read.get("description"),
            row.description.map_or(Value::Null, |text| json!(text)),
            "case {case:?}: the read must report the stored description, got {:?}",
            read.get("description")
        );
        assert_eq!(
            read.text("definitionHash"),
            created.text("definitionHash"),
            "case {case:?}: the read must report the hash the create answered"
        );
        assert_eq!(
            read.get("kmsKeyArn"),
            Value::Null,
            "case {case:?}: a create that named no KMS key reports none, got {:?}",
            read.get("kmsKeyArn")
        );
        assert!(
            (read.instant("createdAt") - created_at).abs() < f64::EPSILON
                && (read.instant("updatedAt") - updated_at).abs() < f64::EPSILON,
            "case {case:?}: the read must report the instants the create answered, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. The hash is a function of the definition, and of nothing else.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_definition_hash_is_a_function_of_the_definition_alone() {
    // (case, the definition the create carries, the GROUP definitions that must hash
    // alike). Rows span: no definition at all, the empty definition, the definition whose
    // `version` is left to its modelled `@default("1")`, and definitions differing in
    // element count, in one value and in one ordering.
    struct Row {
        case: &'static str,
        definition: Option<Value>,
        group: &'static str,
    }

    let one_rule = definition(1);
    let mut renamed_value = definition(1);
    renamed_value["types"][0]["values"][0]["value"] = json!("Renamed");
    let mut reordered = definition(2);
    let swapped = json!([reordered["rules"][1].clone(), reordered["rules"][0].clone()]);
    reordered["rules"] = swapped;

    let rows = vec![
        Row {
            case: "no definition",
            definition: None,
            group: "empty",
        },
        Row {
            case: "empty definition",
            definition: Some(json!({ "version": "1", "types": [], "variables": [], "rules": [] })),
            group: "empty",
        },
        Row {
            case: "empty definition, default version",
            definition: Some(json!({ "types": [], "variables": [], "rules": [] })),
            group: "empty",
        },
        Row {
            case: "one rule",
            definition: Some(one_rule.clone()),
            group: "one rule",
        },
        Row {
            case: "one rule again, under another name",
            definition: Some(one_rule.clone()),
            group: "one rule",
        },
        Row {
            case: "one rule, one type value renamed",
            definition: Some(renamed_value),
            group: "renamed value",
        },
        Row {
            case: "two rules",
            definition: Some(definition(2)),
            group: "two rules",
        },
        Row {
            case: "two rules, reordered",
            definition: Some(reordered),
            group: "two rules reordered",
        },
        Row {
            case: "three rules",
            definition: Some(definition(3)),
            group: "three rules",
        },
    ];

    let emu = emulator();
    let mut by_group: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let mut body = named(&format!("hash policy {index}"));
        if let Some(definition) = &row.definition {
            body["policyDefinition"] = definition.clone();
        }
        // Each row carries a different name, description and tag set, so a hash that
        // depended on anything but the definition could not match across a group.
        body["description"] = json!(format!("row {index}"));
        body["tags"] = json!([{ "key": "row", "value": format!("{index}") }]);

        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
            created.status, created.body
        );
        let hash = created.text("definitionHash");
        assert_hash_shape(case, &hash);

        match by_group.get(row.group) {
            Some((first_case, first_hash)) => assert_eq!(
                &hash, first_hash,
                "case {case:?}: definitions that agree must hash alike, but this differs \
                 from {first_case:?}: {hash:?}"
            ),
            None => {
                by_group.insert(row.group, (case, hash));
            }
        }
    }

    // Every group is a DIFFERENT definition, so every group's hash must differ from every
    // other group's.
    let groups: Vec<(&str, (&str, String))> = by_group.into_iter().collect();
    for (index, (left_group, (left_case, left_hash))) in groups.iter().enumerate() {
        for (right_group, (right_case, right_hash)) in groups.iter().skip(index + 1) {
            assert_ne!(
                left_hash, right_hash,
                "case {left_case:?} ({left_group:?}) and {right_case:?} ({right_group:?}) \
                 carry different definitions, so their hashes must differ, got {left_hash:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3. The KMS key a create names is reported as the key ARN it names.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_kms_key_a_create_names_is_reported_as_a_key_arn() {
    // (case, kmsKeyId, the kmsKeyArn the read must report). A CATALOG of the spellings
    // `KmsKeyId`'s pattern admits — a key ARN, a bare 36-character key id, an alias — read
    // against `KmsKeyArn`, which admits only `arn:…:kms:<region>:<account>:key/<id>`: an
    // alias resolves to a key id only inside KMS, so none is reported for it.
    let key_id = "1234abcd-12ab-34cd-56ef-1234567890ab";
    let foreign_key_arn = format!("arn:aws:kms:{REGION}:111122223333:key/{key_id}");
    let rows: Vec<(&str, Option<String>, Value)> = vec![
        ("no key", None, Value::Null),
        (
            "a bare key id",
            Some(key_id.to_string()),
            json!(format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{key_id}")),
        ),
        (
            "a key ARN of another account",
            Some(foreign_key_arn.clone()),
            json!(foreign_key_arn),
        ),
        (
            "an alias",
            Some("alias/automated-reasoning".to_string()),
            Value::Null,
        ),
    ];

    let emu = emulator();
    for (index, (case, kms_key_id, expected)) in rows.iter().enumerate() {
        let mut body = named(&format!("kms policy {index}"));
        if let Some(kms_key_id) = kms_key_id {
            body["kmsKeyId"] = json!(kms_key_id);
        }
        let created = create(&emu, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
            created.status, created.body
        );
        let read = get(&emu, &created.text("policyArn")).await;
        assert_eq!(
            &read.get("kmsKeyArn"),
            expected,
            "case {case:?}: expected kmsKeyArn {expected:?}, got {:?}",
            read.get("kmsKeyArn")
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. A name is unique per account per region.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_name_is_unique_within_an_account_and_region() {
    // Names differing in the characters `AutomatedReasoningPolicyName`
    // (`^[0-9a-zA-Z-_ ]+$`) admits, so the rule is not keyed to one spelling.
    let names = ["returns policy", "Returns-Policy_2", "0123456789"];
    let other_region = "eu-west-1";

    let emu = emulator();
    for name in names {
        let first = create(&emu, named(name)).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the first create must answer 200, got {} {:?}",
            first.status, first.body
        );

        let again = create(&emu, named(name)).await;
        assert_eq!(
            again.status, 400,
            "case {name:?}: a second create of one name must be refused, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error.as_deref(),
            Some("ConflictException"),
            "case {name:?}: expected ConflictException, got {:?}",
            again.error
        );

        let elsewhere = create_in_region(&emu, other_region, named(name)).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {name:?}: the same name in another region names another policy, got {} {:?}",
            elsewhere.status, elsewhere.body
        );
        let minted = elsewhere.text("policyArn");
        assert!(
            minted.starts_with(&arn_prefix(other_region)),
            "case {name:?}: expected an ARN under {:?}, got {minted:?}",
            arn_prefix(other_region)
        );
        assert_ne!(
            minted,
            first.text("policyArn"),
            "case {name:?}: the policy of another region is another policy"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. The idempotency token.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_request_token_mints_nothing_new() {
    // Tokens differing in the characters `IdempotencyToken`
    // (`^[a-zA-Z0-9]([-a-zA-Z0-9]{0,254}[a-zA-Z0-9])?$`) admits.
    let tokens = ["token1", "A-token-2", "0123456789abcdef"];

    let emu = emulator();
    let mut minted: Vec<String> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let body = json!({
            "name": format!("token policy {index}"),
            "clientRequestToken": token,
            "policyDefinition": definition(1),
        });

        let first = create(&emu, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {token:?}: the first create must answer 200, got {} {:?}",
            first.status, first.body
        );
        let replay = create(&emu, body).await;
        assert_eq!(
            replay.status, 200,
            "case {token:?}: a replay must answer 200, got {} {:?}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, first.body,
            "case {token:?}: a replay must answer with the policy the token already minted"
        );

        let arn = first.text("policyArn");
        assert!(
            !minted.contains(&arn),
            "case {token:?}: a distinct token must mint a distinct policy, got {arn:?} twice"
        );
        minted.push(arn);
    }
    assert_eq!(
        minted.len(),
        tokens.len(),
        "case all: {} distinct tokens must mint {} policies, got {minted:?}",
        tokens.len(),
        tokens.len()
    );
}

// ---------------------------------------------------------------------------------------
// 6. An ARN this account holds no policy under.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_policy_of_this_account_is_not_found() {
    let emu = emulator();
    let created = create(&emu, named("found policy")).await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    let arn = created.text("policyArn");
    let id = policy_id("setup", &arn, REGION);

    // Rows: an id nothing minted, the minted id in another region, and the minted policy's
    // own versioned ARN — `AutomatedReasoningPolicyArn` admits a `:<version>` suffix, and
    // this policy has no versions.
    let rows: Vec<(&str, String)> = vec![
        (
            "an id nothing minted",
            format!("{}{}", arn_prefix(REGION), "00000000000z"),
        ),
        (
            "the minted id in another region",
            format!("{}{id}", arn_prefix("eu-west-1")),
        ),
        ("a version nothing minted", format!("{arn}:1")),
    ];

    for (case, policy_arn) in &rows {
        let read = get(&emu, policy_arn).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: {policy_arn:?} names no policy of this account, so the read \
             must answer 404, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            read.error
        );
    }

    // The ARN the create minted still reads back, so the rows above refused on the ARN
    // rather than on the store.
    let read = get(&emu, &arn).await;
    assert_eq!(
        read.status, 200,
        "case the minted ARN: the read must answer 200, got {} {:?}",
        read.status, read.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. The tag bound.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn more_tags_than_one_resource_may_hold_is_refused() {
    // The published bound, and one past it. `TagList` admits up to 200 entries, so only
    // the per-resource bound can refuse the second row.
    let rows: &[(usize, bool)] = &[(0, true), (1, true), (MAX_TAGS, true), (MAX_TAGS + 1, false)];

    let emu = emulator();
    for (count, accepted) in rows {
        let case = format!("{count} tags");
        let tags: Vec<Value> = (0..*count)
            .map(|index| json!({ "key": format!("key{index}"), "value": format!("value{index}") }))
            .collect();
        let mut body = named(&format!("tagged policy {count}"));
        body["tags"] = Value::Array(tags);

        let created = create(&emu, body).await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: at most {MAX_TAGS} tags must be accepted, got {} {:?}",
                created.status, created.body
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: more than {MAX_TAGS} tags must be refused, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error.as_deref(),
                Some("TooManyTagsException"),
                "case {case:?}: expected TooManyTagsException, got {:?}",
                created.error
            );
        }
    }
}
