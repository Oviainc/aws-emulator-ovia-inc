// SPDX-License-Identifier: CC0-1.0

//! What `UntagResource` decides BEFORE it writes, and the tag set a refused call leaves
//! behind, over the awsJson1_1 wire.
//!
//! The rule under test: a tagging operation of this service decides, in this order and
//! before the store is written, (1) whether the caller's identity-based IAM policies allow
//! that operation's OWN action on the resource it names — otherwise the modelled
//! `AccessDeniedException` (HTTP 403) — and (2) whether the `ResourceArn` names the
//! caller's own account — otherwise the modelled `ValidationException` (HTTP 400) — and
//! only then looks the resource up, answering either the change or the modelled
//! `ResourceNotFoundException`. A call refused at either layer changes no tag, which is
//! what `ListTagsForResource` reports afterwards. `UntagResource` is the operation this
//! suite drives it through; its two siblings `TagResource` and `ListTagsForResource` are
//! covered by `tests/all/front_door_authorization.rs` and
//! `tests/all/tagging_arn_validation.rs`, which assert the same two layers.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine, through the harness
//! [`crate::front_door_authorization::Emulator`], so the documents this service reads are
//! the ones those services wrote.
//!
//! It is asserted over a TABLE that differs along what the rule generalises over: the kind
//! of principal (the account's own published credential, an IAM user, an assumed-role
//! session, an access key id no store holds), what its policy says (nothing, an `Allow`
//! for this action on this resource, a wildcard `Allow`, an `Allow` for a different action
//! of the same service, an `Allow` for this action on a different resource, an explicit
//! `Deny` beside a wildcard `Allow`), and how many keys the one request removes (none, one,
//! several) — since the refusal cannot depend on what the request would have removed.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::RESOURCE_NOT_TAGGABLE_MESSAGE;

use crate::export_lifecycle::{error_code, NOT_FOUND_STATUS};
use crate::front_door_authorization::{
    allow, allow_all_but, create_request, export_arn, Emulator, ACCOUNT_KEY,
};

/// The export every call below names.
const EXPORT_NAME: &str = "untag_authorization_export";

/// How many tags each row of the table seeds on the export before its call.
const SEEDED_KEYS: usize = 3;

/// One row of the table: a principal, whether the rule must serve it, and how many of the
/// seeded keys its one `UntagResource` call asks to remove.
struct Row {
    label: &'static str,
    /// The access key id the call presents; `None` is the account's own credential.
    access_key_id: Option<String>,
    allowed: bool,
    remove: usize,
}

/// The keys this suite seeds for row `index`, in the order it removes them.
fn seeded_keys(index: usize) -> Vec<String> {
    (0..SEEDED_KEYS)
        .map(|slot| format!("probe-{index}-{slot}"))
        .collect()
}

/// The tag set `ListTagsForResource` reports for `arn`, read with the account's own
/// credential so a refused row's answer cannot hide behind its own refusal.
async fn listed_tags(emulator: &Emulator, label: &str, arn: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call(
            ACCOUNT_KEY,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: the account's own tag read must be served, got {status} with {body}"
    );
    body["ResourceTags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: expected a ResourceTags list, got {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create the export this suite tags, with the account's own credential, and answer its
/// ARN.
async fn seeded_export(emulator: &Emulator) -> String {
    let (status, created) = emulator
        .call(ACCOUNT_KEY, "CreateExport", &create_request(EXPORT_NAME))
        .await;
    assert_eq!(status, 200, "the setup create must succeed, got {created}");
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    assert_eq!(
        arn,
        export_arn(EXPORT_NAME),
        "the minted ARN is the one the table's resource-scoped policies name"
    );
    arn
}

/// Put `keys` on `arn` with the account's own credential.
async fn seed_tags(emulator: &Emulator, label: &str, arn: &str, keys: &[String]) {
    let pairs: Vec<Value> = keys
        .iter()
        .map(|key| json!({ "Key": key, "Value": format!("{key}-value") }))
        .collect();
    let (status, body) = emulator
        .call(
            ACCOUNT_KEY,
            "TagResource",
            &json!({ "ResourceArn": arn, "ResourceTags": pairs }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: the setup tagging must succeed, got {status} with {body}"
    );
}

#[tokio::test]
async fn untagging_is_authorized_on_its_own_action_before_anything_is_written() {
    let emulator = Emulator::new();
    let arn = seeded_export(&emulator).await;

    let rows: Vec<Row> = vec![
        // The account's own credential holds no IAM policies and is allowed, like AWS
        // account root credentials.
        Row {
            label: "the account's own credential",
            access_key_id: None,
            allowed: true,
            remove: 1,
        },
        // An assumed-role session with no policy at all: nothing allows the action.
        Row {
            label: "an assumed-role session with no policy",
            access_key_id: Some(emulator.assumed_role_key("bcmde-untag-bare", None).await),
            allowed: false,
            remove: 1,
        },
        // Exactly this action on exactly this export.
        Row {
            label: "an assumed-role session allowed this action on this export",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-allowed",
                        Some(&allow(
                            "bcm-data-exports:UntagResource",
                            &export_arn(EXPORT_NAME),
                        )),
                    )
                    .await,
            ),
            allowed: true,
            remove: 2,
        },
        // A wildcard grant over the service's actions and resources.
        Row {
            label: "an assumed-role session with a wildcard grant",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-wildcard",
                        Some(&allow("bcm-data-exports:*", "*")),
                    )
                    .await,
            ),
            allowed: true,
            remove: SEEDED_KEYS,
        },
        // A grant for the SIBLING tagging write: each operation is judged on its own
        // action, so this one does not carry over.
        Row {
            label: "an assumed-role session allowed only TagResource",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-other-action",
                        Some(&allow("bcm-data-exports:TagResource", "*")),
                    )
                    .await,
            ),
            allowed: false,
            remove: 2,
        },
        // This action on a DIFFERENT resource.
        Row {
            label: "an assumed-role session allowed this action on another export",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-other-resource",
                        Some(&allow(
                            "bcm-data-exports:UntagResource",
                            &export_arn("somebody_elses_export"),
                        )),
                    )
                    .await,
            ),
            allowed: false,
            remove: 1,
        },
        // An explicit deny beside a wildcard allow: the deny wins.
        Row {
            label: "an assumed-role session explicitly denied this action",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-denied",
                        Some(&allow_all_but("bcm-data-exports:UntagResource")),
                    )
                    .await,
            ),
            allowed: false,
            remove: SEEDED_KEYS,
        },
        // An IAM USER signing with a long-term key IAM minted for it, judged on its own
        // inline policies, with and without a grant.
        Row {
            label: "an IAM user with no policy",
            access_key_id: Some(emulator.user_key("bcmde-untag-user-bare", None).await),
            allowed: false,
            remove: 1,
        },
        Row {
            label: "an IAM user allowed this action",
            access_key_id: Some(
                emulator
                    .user_key(
                        "bcmde-untag-user-allowed",
                        Some(&allow("bcm-data-exports:UntagResource", "*")),
                    )
                    .await,
            ),
            allowed: true,
            remove: 1,
        },
        // A refusal cannot depend on what the request would have removed: a request naming
        // NO key is still refused, and an allowed one changes nothing.
        Row {
            label: "an assumed-role session with no policy, removing no key",
            access_key_id: Some(emulator.assumed_role_key("bcmde-untag-bare-empty", None).await),
            allowed: false,
            remove: 0,
        },
        Row {
            label: "a wildcard grant removing no key",
            access_key_id: Some(
                emulator
                    .assumed_role_key(
                        "bcmde-untag-wildcard-empty",
                        Some(&allow("bcm-data-exports:*", "*")),
                    )
                    .await,
            ),
            allowed: true,
            remove: 0,
        },
        // An access key id neither STS nor IAM holds a record for: the account's own
        // published credential, which holds no policies and is allowed.
        Row {
            label: "an id no store holds",
            access_key_id: Some("AKIDNOBODYHOLDS".to_string()),
            allowed: true,
            remove: SEEDED_KEYS,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let keys = seeded_keys(index);
        seed_tags(&emulator, label, &arn, &keys).await;
        let before = listed_tags(&emulator, label, &arn).await;

        let access_key_id = row
            .access_key_id
            .clone()
            .unwrap_or_else(|| ACCOUNT_KEY.to_string());
        let removed: Vec<String> = keys.iter().take(row.remove).cloned().collect();
        let (status, body) = emulator
            .call(
                &access_key_id,
                "UntagResource",
                &json!({ "ResourceArn": arn, "ResourceTagKeys": removed }),
            )
            .await;
        let after = listed_tags(&emulator, label, &arn).await;

        if row.allowed {
            assert_eq!(
                status, 200,
                "case {label:?}: an allowed caller must be served, got {status} with {body}"
            );
            let expected: Vec<(String, String)> = before
                .iter()
                .filter(|(key, _)| !removed.contains(key))
                .cloned()
                .collect();
            assert_eq!(
                after, expected,
                "case {label:?}: exactly the {} named key(s) must be gone, got {after:?}",
                row.remove
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: a caller with no grant must be refused, got {status} with {body}"
            );
            // The refusal names the principal, the action and the resource it was decided
            // about — what a caller has to fix its policy from.
            let message = body["Message"]
                .as_str()
                .or_else(|| body["message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains("bcm-data-exports:UntagResource")
                    && message.contains("not authorized to perform"),
                "case {label:?}: the refusal must name the action it was decided about, got {message:?}"
            );
            assert_eq!(
                after, before,
                "case {label:?}: a refused UntagResource must change no tag, got {after:?}"
            );
        }

        // Leave the export as the next row finds it: its own keys, and nothing of this one.
        let leftovers: Vec<String> = after.iter().map(|(key, _)| key.clone()).collect();
        if !leftovers.is_empty() {
            let (status, body) = emulator
                .call(
                    ACCOUNT_KEY,
                    "UntagResource",
                    &json!({ "ResourceArn": arn, "ResourceTagKeys": leftovers }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: the account's own cleanup must be served, got {status} with {body}"
            );
        }
        assert!(
            listed_tags(&emulator, label, &arn).await.is_empty(),
            "case {label:?}: the cleanup must leave the export untagged"
        );
    }
}

#[tokio::test]
async fn the_refusal_layers_of_untagging_are_decided_in_order() {
    let emulator = Emulator::new();
    let arn = seeded_export(&emulator).await;
    let keys = seeded_keys(0);
    seed_tags(&emulator, "the ordering setup", &arn, &keys).await;

    // The three ARNs the layers separate: one of another account, one of this account
    // naming no export, and the export that really exists.
    let foreign = format!(
        "arn:aws:bcm-data-exports:us-east-1:111122223333:export/{EXPORT_NAME}"
    );
    let absent = export_arn("never_created_export");

    // LAYER 1 — a caller with no grant is told it is refused, whatever ARN it names: the
    // decision is reached before the ARN is judged and before anything is looked up, so a
    // refused caller learns nothing about what this account holds.
    let bare = emulator
        .assumed_role_key("bcmde-untag-ordering-bare", None)
        .await;
    for (label, candidate) in [
        ("a foreign ARN", foreign.as_str()),
        ("an own-account ARN naming nothing", absent.as_str()),
        ("the export that exists", arn.as_str()),
    ] {
        let (status, body) = emulator
            .call(
                &bare,
                "UntagResource",
                &json!({ "ResourceArn": candidate, "ResourceTagKeys": ["probe-0-0"] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDeniedException"),
            "case {label:?}: authorization comes first, got {status} with {body}"
        );
    }

    // LAYER 2 and the lookup — the same three ARNs, now from a caller the policies allow:
    // the foreign ARN is a validation failure decided without a lookup, the own-account
    // ARN that names nothing is the not-found refusal, and the export is untagged.
    let granted = emulator
        .assumed_role_key(
            "bcmde-untag-ordering-granted",
            Some(&allow("bcm-data-exports:*", "*")),
        )
        .await;

    let (status, body) = emulator
        .call(
            &granted,
            "UntagResource",
            &json!({ "ResourceArn": &foreign, "ResourceTagKeys": ["probe-0-0"] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str(), body["Message"].as_str()),
        (
            400,
            "ValidationException",
            Some(RESOURCE_NOT_TAGGABLE_MESSAGE)
        ),
        "case \"a foreign ARN\": the ARN's account is judged before any lookup, got {status} with {body}"
    );
    assert!(
        !body["Message"]
            .as_str()
            .unwrap_or_default()
            .contains(&foreign),
        "case \"a foreign ARN\": the refusal must not restate the ARN, got {body}"
    );

    let (status, body) = emulator
        .call(
            &granted,
            "UntagResource",
            &json!({ "ResourceArn": &absent, "ResourceTagKeys": ["probe-0-0"] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case \"an own-account ARN naming nothing\": an ARN of this account is looked up, got {status} with {body}"
    );

    // Neither refusal touched the tag set.
    let listed = listed_tags(&emulator, "after both refusals", &arn).await;
    assert_eq!(
        listed.len(),
        keys.len(),
        "a refused UntagResource must leave every seeded tag in place, got {listed:?}"
    );

    let (status, body) = emulator
        .call(
            &granted,
            "UntagResource",
            &json!({ "ResourceArn": &arn, "ResourceTagKeys": [keys[0].clone()] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the export that exists\": an allowed caller naming this account's export must be served, got {status} with {body}"
    );
    let listed = listed_tags(&emulator, "after the served call", &arn).await;
    assert!(
        listed.iter().all(|(key, _)| key != &keys[0]) && listed.len() == keys.len() - 1,
        "case \"the export that exists\": exactly the named key must be gone, got {listed:?}"
    );
}
