// SPDX-License-Identifier: CC0-1.0

//! Which ACCOUNT a tagging `ResourceArn` has to name for it to reach a resource at all.
//!
//! RULE: an ARN addresses one resource inside one account's Region-scoped namespace, so a
//! tagging `ResourceArn` reaches a resource only when its account field is the account the
//! emulator's own resources belong to (`engine::emulator_account_id`) and its Region is the
//! one the request is served in; an ARN naming any other account is well formed but names
//! nothing that is here, which is the absent-resource class — `NotFoundException` (404) with
//! the tagging front end's not-found sentence — and it must neither report nor change the
//! tags of the same path under THIS account.
//!
//! The rows span the class rather than one request:
//!
//! * the ACCOUNT ([`foreign_accounts`]) varies how the field differs from the emulator's own
//!   — another twelve-digit id, every digit shifted, one digit too many, far too few digits,
//!   and no account at all — and every row is derived from the account field of an ARN the
//!   service MINTED, so no row can pass because the table disagrees with the emulator;
//! * the LEVEL varies which resource the path addresses — channel group, channel, origin
//!   endpoint, harvest job — each ARN read back from the create that minted it, so no ARN in
//!   this suite is written out by hand;
//! * the DOOR varies which tagging operation reads the ARN: the reporting
//!   `ListTagsForResource` and both writers, `TagResource` and `UntagResource`.
//!
//! The control test keeps the verdict from being a constant: the SAME requests sent at the
//! minted ARN — the one whose account is the emulator's — are served, and their write is
//! visible on exactly that resource.
//!
//! The Region half of the same scope rule is pinned by the sibling `regional_partitioning`
//! suite (`a_resource_arn_reaches_only_the_region_that_holds_it`), and the malformed-ARN
//! class it is distinct from by `rejection_rule_and_negative_paths`.

use serde_json::Value;
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::TAGGED_RESOURCE_NOT_FOUND_MESSAGE;

// The crate's shared test harness: the control-plane client with its error-envelope reader
// (`channel_lifecycle`), the three tagging calls and the tag-map readers (`resource_tags`),
// and — from the sibling suite that judges the same tagging front end over the same
// hierarchy — the whole channel group -> channel -> origin endpoint -> harvest job chain with
// one `Target` per taggable level, its seeded-tag assertion and its envelope-message reader
// (`tagging_arn_refusal_parity`). This suite adds only what is specific to the ARN's account
// field.
use crate::channel_lifecycle::{error_type, Emulator};
use crate::resource_tags::{map_of, tag_map};
use crate::tagging_arn_refusal_parity::{
    assert_seeded_tags_untouched, live_targets, message, Target, SEEDED,
};

/// The tag a cross-account `TagResource` would add if its ARN resolved to this account's
/// resource.
const INTRUDER: (&str, &str) = ("intruder", "from another account");

/// The six fields of `minted`, which every ARN this service hands out has.
///
/// The resource field keeps everything after the fifth colon, so a resource path is never
/// split by this.
fn fields_of(minted: &str) -> Vec<&str> {
    let fields: Vec<&str> = minted.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "a minted ARN has six colon-separated fields: {minted}"
    );
    fields
}

/// The account field of `minted` — the account every resource this emulator holds belongs
/// to, read off an ARN the service itself produced rather than written out here.
fn account_of(minted: &str) -> &str {
    fields_of(minted)[4]
}

/// `minted` with its account field replaced by `account`, every other field untouched: the
/// account is then the only thing that can decide the answer.
fn with_account(minted: &str, account: &str) -> String {
    let fields = fields_of(minted);
    format!(
        "arn:{}:{}:{}:{account}:{}",
        fields[1], fields[2], fields[3], fields[5]
    )
}

/// `account` with every ASCII digit shifted one place up (`9` wrapping to `0`): a different
/// account id of the same length and alphabet, derived from the emulator's own.
fn digits_shifted(account: &str) -> String {
    account
        .chars()
        .map(|character| match character.to_digit(10) {
            Some(digit) => char::from_digit((digit + 1) % 10, 10).unwrap_or(character),
            None => character,
        })
        .collect()
}

/// The account fields that are NOT this emulator's, as `(label, account)`.
///
/// The rows differ in HOW the field differs from `mine`: another twelve-digit id, the same
/// digits shifted, one digit too many, far too few, and the empty field AWS writes for an
/// account-less resource. Each is derived from `mine`, except the plain twelve-digit id,
/// which is a literal account id of the published form (12 digits).
fn foreign_accounts(mine: &str) -> Vec<(&'static str, String)> {
    vec![
        ("another twelve-digit account", "999999999999".to_string()),
        ("every digit of this account shifted", digits_shifted(mine)),
        ("this account with one digit too many", format!("{mine}7")),
        (
            "an account id far too short",
            mine.chars().take(4).collect::<String>(),
        ),
        ("no account at all", String::new()),
    ]
}

/// Assert one answer is the absent-resource class: 404, `NotFoundException`, and the tagging
/// front end's own not-found sentence — never the malformed-ARN class, which this ARN is not
/// in, and never a success.
fn assert_names_no_resource(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: an ARN naming another account names no resource here, so it must be \
         refused with HTTP 404, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body}"
    );
    assert_eq!(
        message(body),
        TAGGED_RESOURCE_NOT_FOUND_MESSAGE,
        "case {case:?}: expected the tagging front end's not-found sentence, got {body}"
    );
}

/// RULE: a tagging `ResourceArn` whose account is not the one this emulator's resources
/// belong to names no resource that is here — so the reporting operation reports nothing for
/// it, at every taggable level and for every way the account field can differ.
#[tokio::test]
async fn a_resource_arn_naming_another_account_reports_no_tags() {
    let emulator = Emulator::new();
    let targets: Vec<Target> = live_targets(&emulator).await;

    for target in &targets {
        for (label, account) in foreign_accounts(account_of(&target.live)) {
            let case = format!("ListTagsForResource with {label:?} at a {} ARN", target.kind);
            let arn = with_account(&target.live, &account);
            let (status, body) = emulator.list_tags(&arn).await;
            assert_names_no_resource(&case, status, &body);
        }
    }
}

/// RULE, at the two writers: the same ARN reaches nothing to WRITE either, so neither
/// `TagResource` nor `UntagResource` touches the tags of the same path under this account.
#[tokio::test]
async fn a_resource_arn_naming_another_account_writes_no_tags() {
    let emulator = Emulator::new();
    let targets: Vec<Target> = live_targets(&emulator).await;
    let intruder = map_of(&[INTRUDER]);

    for target in &targets {
        for (label, account) in foreign_accounts(account_of(&target.live)) {
            let arn = with_account(&target.live, &account);

            // The writer that would ADD a tag.
            let case = format!("TagResource with {label:?} at a {} ARN", target.kind);
            let (status, body) = emulator.tag(&arn, &intruder).await;
            assert_names_no_resource(&case, status, &body);
            assert_seeded_tags_untouched(&emulator, target, &case).await;

            // The writer that would REMOVE the tag the resource was created with.
            let case = format!("UntagResource with {label:?} at a {} ARN", target.kind);
            let (status, body) = emulator.untag(&arn, &[SEEDED.0]).await;
            assert_names_no_resource(&case, status, &body);
            assert_seeded_tags_untouched(&emulator, target, &case).await;
        }
    }
}

/// Control, so neither verdict above can be a constant: the SAME three requests at the ARN
/// the service MINTED — whose account is this emulator's — are served, and the write lands on
/// exactly the resource that ARN names.
#[tokio::test]
async fn the_minted_resource_arn_still_reaches_the_resource_it_names() {
    let emulator = Emulator::new();
    let targets: Vec<Target> = live_targets(&emulator).await;
    let intruder = map_of(&[INTRUDER]);

    for target in &targets {
        let case = format!("TagResource at this account's {} ARN", target.kind);
        let (status, body) = emulator.tag(&target.live, &intruder).await;
        assert_eq!(
            status, 204,
            "case {case:?}: an ARN naming this account must be served, got {status} {body}"
        );

        let (status, listed) = emulator.list_tags(&target.live).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} {listed}"
        );
        assert_eq!(
            tag_map(&listed, "tags"),
            map_of(&[SEEDED, INTRUDER]),
            "case {case:?}: the accepted write must be merged into the created tags, got \
             {listed}"
        );

        let case = format!("UntagResource at this account's {} ARN", target.kind);
        let (status, body) = emulator.untag(&target.live, &[INTRUDER.0]).await;
        assert_eq!(
            status, 204,
            "case {case:?}: an ARN naming this account must be served, got {status} {body}"
        );
        assert_seeded_tags_untouched(&emulator, target, &case).await;
    }
}
