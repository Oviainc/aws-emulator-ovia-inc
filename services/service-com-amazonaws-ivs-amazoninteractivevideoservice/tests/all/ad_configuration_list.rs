// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAdConfigurations`, driven over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — on the
//! harness [`crate::channel_lifecycle`] already owns (`Ivs`, `text`, `hex_of`), so
//! the suites cannot drift apart in how they talk to the emulator.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. The list answers every persisted ad configuration, in ONE stable order (ARN),
//!    cut into pages of at most `maxResults`, with a `nextToken` that resumes exactly
//!    where the previous page stopped and is absent on the last page.
//! 2. A `nextToken` this list did not issue is refused with a 400
//!    `ValidationException`, never honoured as another list's boundary and never
//!    answered as a silently truncated page.
//! 3. A listed entry is the `AdConfigurationSummary` projection of the stored
//!    resource: the members that shape declares, read off state, so the list agrees
//!    with `GetAdConfiguration` member for member.
//! 4. Listing is a report: repeating it answers the same thing and changes nothing a
//!    later caller sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::channel_lifecycle::{hex_of, text, Ivs};

/// The ARN prefix every ad configuration this emulator mints carries, from the
/// modelled `AdConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$` plus the
/// canonical emulator account id.
const AD_CONFIGURATION_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:ad-configuration/";

/// One MediaTailor integration entry, of the form the modelled
/// `MediaTailorPlaybackConfigurationArn` pattern
/// `^arn:aws:mediatailor:[a-z0-9-]+:[0-9]+:playbackConfiguration/[a-zA-Z0-9-]+$`
/// pins, so each case's request satisfies its own constraints.
fn playback_entry(suffix: &str) -> Value {
    json!({
        "playbackConfigurationArn":
            format!("arn:aws:mediatailor:us-east-1:000000000000:playbackConfiguration/{suffix}"),
    })
}

/// Create an ad configuration and return the resource the create answered.
async fn create(ivs: &Ivs, request: Value) -> Value {
    ivs.ok("CreateAdConfiguration", request).await["adConfiguration"].clone()
}

/// `GetAdConfiguration` for `arn`.
async fn get(ivs: &Ivs, arn: &str) -> Value {
    ivs.ok("GetAdConfiguration", json!({ "arn": arn })).await["adConfiguration"].clone()
}

/// One page of the list, for the request members a case wants to send.
async fn page(ivs: &Ivs, request: Value) -> (Vec<Value>, Option<String>) {
    let page = ivs.ok("ListAdConfigurations", request).await;
    let listed = page["adConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = page
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (listed, token)
}

/// Every persisted ad configuration, walked page by page through the `nextToken` the
/// list issues, at the given page size (`None` = send no `maxResults`).
async fn list_all(ivs: &Ivs, max_results: Option<usize>) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(next) = &token {
            request["nextToken"] = json!(next);
        }
        let (listed, next) = page(ivs, request).await;
        if let Some(size) = max_results {
            assert!(
                listed.len() <= size,
                "case maxResults {size}: a page holds at most maxResults entries, got {}",
                listed.len()
            );
        }
        all.extend(listed);
        token = next;
        if token.is_none() {
            break;
        }
        assert!(
            all.len() < 1_000,
            "case maxResults {max_results:?}: the walk must terminate, got {} entries",
            all.len()
        );
    }
    all
}

/// Rule 1: every page size — both published bounds of `maxResults`, sizes that divide
/// the set unevenly, and a size past the set — returns the WHOLE set exactly once and
/// in one order; a size outside the modelled 1..100 range is refused before any
/// handler runs.
#[tokio::test]
async fn list_pages_the_whole_set_without_duplicates_or_omissions() {
    let ivs = Ivs::new();
    let mut all = BTreeSet::new();
    for index in 0..7 {
        let created = create(
            &ivs,
            json!({
                "name": format!("paged-{index}"),
                "mediaTailorPlaybackConfigurations": [playback_entry(&format!("page-{index}"))],
            }),
        )
        .await;
        all.insert(text(&created, "arn"));
    }

    // Rows: the page size, spanning 1 (the modelled minimum), sizes that divide the
    // set unevenly, the set's own size, 100 (the modelled maximum), and no
    // `maxResults` at all (the whole page).
    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let seen: Vec<String> = list_all(&ivs, max_results)
            .await
            .iter()
            .map(|summary| text(summary, "arn"))
            .collect();
        assert_eq!(
            seen.len(),
            all.len(),
            "case maxResults {max_results:?}: expected each of the {} ad configurations \
             exactly once, got {seen:?}",
            all.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            all,
            "case maxResults {max_results:?}: the pages together are the whole set, got {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case maxResults {max_results:?}: the pages come in one stable (ARN) order, \
             got {seen:?}"
        );
    }

    // One past each published bound of `MaxAdConfigurationResults` is refused by the
    // modelled range.
    for max_results in [0, 101] {
        let reply = ivs
            .call("ListAdConfigurations", &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults {max_results}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 1's token half: a page smaller than the set carries a `nextToken`, the last
/// page carries none, and the token resumes strictly after the page it was issued for.
#[tokio::test]
async fn a_next_token_is_issued_only_while_a_further_page_exists() {
    let ivs = Ivs::new();
    let mut arns = Vec::new();
    for index in 0..5 {
        arns.push(text(
            &create(
                &ivs,
                json!({
                    "mediaTailorPlaybackConfigurations": [playback_entry(&format!("tok-{index}"))],
                }),
            )
            .await,
            "arn",
        ));
    }
    arns.sort();

    // Rows: the page size, so the number of pages the set is cut into varies.
    for size in [1, 2, 4, 5, 6] {
        let mut consumed = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "maxResults": size });
            if let Some(next) = &token {
                request["nextToken"] = json!(next);
            }
            let (listed, next) = page(&ivs, request).await;
            let expected: Vec<String> = arns
                .iter()
                .skip(consumed)
                .take(size)
                .map(String::clone)
                .collect();
            assert_eq!(
                listed
                    .iter()
                    .map(|summary| text(summary, "arn"))
                    .collect::<Vec<_>>(),
                expected,
                "case maxResults {size} after {consumed}: a page resumes strictly after the \
                 previous one, got {listed:?}"
            );
            consumed += listed.len();
            let more_remain = consumed < arns.len();
            assert_eq!(
                next.is_some(),
                more_remain,
                "case maxResults {size} after {consumed}: a nextToken is issued exactly while \
                 a further page exists, got {next:?}"
            );
            token = next;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            consumed,
            arns.len(),
            "case maxResults {size}: the walk ended having served the whole set"
        );
    }
}

/// Rule 2: a `nextToken` this list did not issue is a 400 `ValidationException`, never
/// a silently truncated or empty page; an EMPTY token names no boundary, so it is the
/// first page.
#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let ivs = Ivs::new();
    // A token the CHANNEL list issues: valid hex, valid codec, wrong list.
    for index in 0..3 {
        ivs.ok("CreateChannel", json!({ "name": format!("channel-{index}") }))
            .await;
    }
    let channel_token = text(
        &ivs.ok("ListChannels", json!({ "maxResults": 1 })).await,
        "nextToken",
    );
    assert!(
        !channel_token.is_empty(),
        "the channel list must issue a token for this case to be meaningful"
    );
    for index in 0..3 {
        create(
            &ivs,
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry(&format!("forge-{index}"))],
            }),
        )
        .await;
    }

    let forged: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        (
            "hex of a plain ARN, unstamped",
            hex_of(&format!("{AD_CONFIGURATION_ARN_PREFIX}abc")),
        ),
        (
            "the right stamp, a boundary that is no ARN",
            hex_of("ivs:1:ad-configuration:nothing"),
        ),
        (
            "another list's stamp around this list's boundary",
            hex_of(&format!("ivs:1:channel:{AD_CONFIGURATION_ARN_PREFIX}abc")),
        ),
        ("a token the channel list issued", channel_token),
    ];

    for (case, token) in forged {
        let reply = ivs
            .call("ListAdConfigurations", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case forged nextToken ({case}): expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    let (listed, token) = page(&ivs, json!({ "nextToken": "" })).await;
    assert_eq!(
        listed.len(),
        3,
        "an empty nextToken is the first page, got {listed:?}"
    );
    assert!(
        token.is_none(),
        "the whole set fits one page here, so no nextToken is issued, got {token:?}"
    );
}

/// Rule 3: each listed entry is the `AdConfigurationSummary` projection of the stored
/// resource — the members that shape declares, agreeing with `GetAdConfiguration`.
///
/// Rows differ along what the projection generalises over: how many MediaTailor
/// entries the list carries (the modelled 1..3), whether `name`,
/// `postRollConfiguration` and `tags` were sent at all, and the values they take at
/// each published bound.
#[tokio::test]
async fn a_listed_entry_is_the_summary_projection_of_the_stored_resource() {
    // 128 characters, the modelled `AdConfigurationName` maximum length, drawn from
    // every class its pattern `^[a-zA-Z0-9-_]*$` allows.
    let longest_name = "aZ9-_".repeat(26)[..128].to_string();
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required list, one entry",
            json!({ "mediaTailorPlaybackConfigurations": [playback_entry("proj-one")] }),
        ),
        (
            "two entries, a name and tags",
            json!({
                "name": "proj_two",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("proj-two-a"), playback_entry("proj-two-b"),
                ],
                "tags": { "env": "test", "team": "video" },
            }),
        ),
        (
            "three entries (the modelled maximum), post-roll at its lower bound",
            json!({
                "name": "proj-three",
                "mediaTailorPlaybackConfigurations": [
                    playback_entry("proj-three-a"),
                    playback_entry("proj-three-b"),
                    playback_entry("proj-three-c"),
                ],
                "postRollConfiguration": { "durationSeconds": 1, "enabled": true },
            }),
        ),
        (
            "post-roll at the upper bound of AdDurationSeconds, an empty tag map",
            json!({
                "mediaTailorPlaybackConfigurations": [playback_entry("proj-upper")],
                "postRollConfiguration": { "durationSeconds": 300, "enabled": false },
                "tags": {},
            }),
        ),
        (
            "a name at the modelled 128-character maximum",
            json!({
                "name": longest_name,
                "mediaTailorPlaybackConfigurations": [playback_entry("proj-longest")],
            }),
        ),
    ];

    let ivs = Ivs::new();
    let mut arns = Vec::new();
    for (case, request) in &cases {
        let created = create(&ivs, request.clone()).await;
        let arn = text(&created, "arn");
        let listed = list_all(&ivs, None)
            .await
            .into_iter()
            .find(|summary| text(summary, "arn") == arn)
            .unwrap_or_else(|| panic!("case {case:?}: the created configuration must be listed"));

        // Every member `AdConfigurationSummary` declares, read off state: the summary
        // and the detail shape declare the same members, so the listed entry is the
        // whole stored resource and `GetAdConfiguration` agrees with it.
        let stored = get(&ivs, &arn).await;
        for member in [
            "arn",
            "name",
            "mediaTailorPlaybackConfigurations",
            "postRollConfiguration",
            "tags",
        ] {
            assert_eq!(
                listed.get(member).cloned().unwrap_or(Value::Null),
                stored.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: the listed {member} must be the stored value, got {listed}"
            );
        }
        assert_eq!(
            listed, stored,
            "case {case:?}: expected the listed entry to be the stored resource {stored}, \
             got {listed}"
        );
        arns.push((*case, arn));
    }

    // A later create disturbs nothing listed earlier.
    let listed_now = list_all(&ivs, Some(2)).await;
    for (case, arn) in &arns {
        assert!(
            listed_now
                .iter()
                .any(|summary| &text(summary, "arn") == arn),
            "case {case:?}: expected {arn} to still be listed after the later creates, \
             got {listed_now:?}"
        );
    }
}

/// Rule 4: the list is a report — repeating it answers the same thing, and it never
/// changes what a later caller sees.
#[tokio::test]
async fn listing_is_repeatable_and_changes_nothing() {
    let ivs = Ivs::new();
    let mut stored = Vec::new();
    for index in 0..4 {
        let created = create(
            &ivs,
            json!({
                "name": format!("repeat-{index}"),
                "mediaTailorPlaybackConfigurations": [playback_entry(&format!("rep-{index}"))],
                "postRollConfiguration": { "durationSeconds": 30, "enabled": true },
                "tags": { "round": format!("{index}") },
            }),
        )
        .await;
        stored.push(created);
    }

    let first = list_all(&ivs, None).await;
    // Rows: the page size the repeat is served at, so a repeat through several pages
    // answers the same set as a repeat in one.
    for max_results in [None, Some(1), Some(3), Some(100)] {
        for round in 1..=3 {
            let again = list_all(&ivs, max_results).await;
            assert_eq!(
                again, first,
                "case maxResults {max_results:?} round {round}: expected the same answer as \
                 the first list {first:?}, got {again:?}"
            );
        }
    }

    // And every resource still reads back exactly as it was created: listing wrote
    // nothing.
    for created in &stored {
        let arn = text(created, "arn");
        let fetched = get(&ivs, &arn).await;
        assert_eq!(
            &fetched, created,
            "case {arn}: expected the resource to be untouched by the lists, got {fetched}"
        );
    }
}

/// An empty store lists as an empty page with no `nextToken` — the modelled
/// `adConfigurations` member is required, so it is present and empty rather than
/// absent.
#[tokio::test]
async fn an_empty_store_lists_as_an_empty_page() {
    let ivs = Ivs::new();
    // A channel exists, so the answer is empty because no AD CONFIGURATION was
    // created, not because the store was never touched.
    ivs.ok("CreateChannel", json!({ "name": "unrelated" })).await;

    for request in [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": 100 }),
        json!({ "nextToken": "" }),
    ] {
        let answer = ivs.ok("ListAdConfigurations", request.clone()).await;
        assert_eq!(
            answer["adConfigurations"],
            json!([]),
            "case {request}: expected an empty list, got {answer}"
        );
        assert!(
            answer.get("nextToken").is_none(),
            "case {request}: an answer with no further page carries no nextToken, got {answer}"
        );
    }
}
