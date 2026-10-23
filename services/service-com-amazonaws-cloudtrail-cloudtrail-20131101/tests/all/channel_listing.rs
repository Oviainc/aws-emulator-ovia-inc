// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListChannels`, through the shared [`Emulator`] harness.
//!
//! The rule under test, grounded in this service's Smithy model: `ListChannels`
//! reports the channels the registry holds for the Region the call was made in (a
//! channel ARN names its Region), in registry (channel-id) order, in pages of at most
//! `MaxResults`
//! (`@range 1..1000`; the service's own page size when the request sends none), with
//! `NextToken` present exactly when channels remain and resuming immediately after the
//! last channel reported — and a `NextToken` the service never issued, or one issued
//! for a different `MaxResults` or in another Region, is refused with
//! `InvalidNextTokenException` (HTTP
//! 400), the error the model documents for "a token that is not valid, or a token that
//! was previously used in a request with different parameters".
//!
//! Channels are seeded straight into the shared channel registry
//! (`crate::state::channels`), as `CreateChannel` will write them once it is
//! implemented (its own issue) — the same way `channel_deletion.rs` seeds them.

use serde_json::{json, Value};
use std::collections::BTreeMap;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::list_channels::LIST_CHANNELS_DEFAULT_PAGE_SIZE;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::Tag;
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::channels::{
    self, ChannelKey, ChannelRecord,
};

use super::emulator::{Emulator, HOME_REGION};

/// A channel id: the `8-4-4-4-12` hex UUID CloudTrail mints, built from `seed` so a
/// row can differ in the VALUE of the id without repeating its shape.
fn channel_id(seed: &str) -> String {
    format!("{seed}-30fe-4581-8bd3-351632c2be72")
}

/// Put one channel into the registry every channel operation reads — the SAME store,
/// since the engine is shared — and hand back its ARN.
fn seed_channel(emulator: &Emulator, region: &str, id: &str, name: &str, tags: Vec<Tag>) -> String {
    assert!(
        channels::is_minted_channel_id(id),
        "seed {id:?}: a stored channel carries an id CloudTrail minted"
    );
    let channel_arn = channels::channel_arn(region, id);
    let record = ChannelRecord {
        channel_id: id.to_string(),
        name: name.to_string(),
        channel_arn: channel_arn.clone(),
        // `CreateChannelRequest$Source`: "Custom" is the source of a channel a caller
        // owns.
        source: "Custom".to_string(),
        destinations: Vec::new(),
        tags,
        home_region: region.to_string(),
    };
    let state = emulator.state();
    let channels: &mut BTreeMap<ChannelKey, ChannelRecord> = &mut state.lock().channels;
    channels.insert((id.to_string(), region.to_string()), record);
    channel_arn
}

/// Seed `count` channels in `region`, and hand back the `(ARN, name)` pairs
/// `ListChannels` must report for that Region, in registry order (the registry is
/// keyed by `(channel id, Region)` and iterates id-ordered).
fn seed_channels(emulator: &Emulator, region: &str, count: usize) -> Vec<(String, String)> {
    let mut seeded: Vec<(String, String)> = Vec::new();
    for index in 0..count {
        // The first id group is zero-padded hex, so id order and seed order agree and
        // a case can assert a page's contents positionally.
        let id = channel_id(&format!("{:08x}", index + 1));
        let name = format!("channel-{region}-{index:04}");
        let arn = seed_channel(emulator, region, &id, &name, Vec::new());
        seeded.push((arn, name));
    }
    seeded
}

/// The `(ChannelArn, Name)` pairs a `ListChannels` reply carries, in order.
fn channel_entries(body: &Value) -> Vec<(String, String)> {
    body.get("Channels")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let field = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (field("ChannelArn"), field("Name"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `NextToken` a reply carries, if it carries one.
fn next_token(body: &Value) -> Option<String> {
    body.get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A `ListChannels` request body: `MaxResults` and `NextToken` only when given.
fn list_request(max_results: Option<i32>, token: Option<&str>) -> Value {
    let mut body = json!({});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    body
}

#[tokio::test]
async fn channels_are_reported_for_the_region_the_call_was_made_in() {
    let emulator = Emulator::new();
    // Rows differ in how many channels a Region holds — none, one, several — and the
    // Regions differ in ARN order, so no case can pass by listing the whole registry.
    let rows = [
        (HOME_REGION, 0usize),
        ("us-west-2", 1),
        ("eu-central-1", 5),
        ("ap-southeast-2", 12),
    ];
    let seeded: Vec<(&str, Vec<(String, String)>)> = rows
        .iter()
        .map(|(region, count)| (*region, seed_channels(&emulator, region, *count)))
        .collect();

    for (region, expected) in &seeded {
        let reply = emulator
            .call_in("ListChannels", region, &list_request(None, None))
            .await;
        let body = reply.expect_ok(region);
        assert_eq!(
            channel_entries(body),
            *expected,
            "case {region}: expected exactly that Region's channels, got {body:?}"
        );
        assert_eq!(
            next_token(body),
            None,
            "case {region}: a single page of {} channels must carry no NextToken, got {body:?}",
            expected.len()
        );
    }

    // And a Region the registry holds nothing for sees nothing, though the registry is
    // not empty.
    let elsewhere = emulator
        .call_in("ListChannels", "sa-east-1", &list_request(None, None))
        .await;
    let body = elsewhere.expect_ok("sa-east-1");
    assert_eq!(
        channel_entries(body),
        Vec::new(),
        "case sa-east-1: a Region with no channels must report none, got {body:?}"
    );
}

#[tokio::test]
async fn pagination_walks_every_channel_exactly_once() {
    // Rows differ in the channel count and in the page size, including a page size of
    // one, an exact multiple, a remainder, a page wider than the registry, the
    // modelled maximum (1000), and no MaxResults at all — which pages at the service's
    // own default, so the last row crosses that default by one.
    let rows = [
        (0usize, Some(1)),
        (1, Some(1)),
        (5, Some(2)),
        (7, Some(3)),
        (5, Some(5)),
        (5, Some(1000)),
        (3, None),
        (LIST_CHANNELS_DEFAULT_PAGE_SIZE + 1, None),
    ];

    for (count, max_results) in rows {
        let case = format!("{count} channels, MaxResults {max_results:?}");
        // A fresh registry per row, so a row sees only its own channels.
        let emulator = Emulator::new();
        let seeded = seed_channels(&emulator, HOME_REGION, count);
        let page_size =
            max_results.map_or(LIST_CHANNELS_DEFAULT_PAGE_SIZE, |limit| limit as usize);

        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let reply = emulator
                .call("ListChannels", &list_request(max_results, token.as_deref()))
                .await;
            let body = reply.expect_ok(&case);
            let page = channel_entries(body);
            assert!(
                page.len() <= page_size,
                "case {case}: a page may carry at most {page_size} channels, got {}",
                page.len()
            );
            collected.extend(page.clone());
            token = next_token(body);
            pages += 1;
            assert!(
                pages <= count / page_size + 2,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match &token {
                // A page that reports more to come must be a full one: the token marks
                // where the NEXT page starts, never a channel this page skipped.
                Some(_) => assert_eq!(
                    page.len(),
                    page_size,
                    "case {case}: a non-final page must be full, got {page:?}"
                ),
                None => break,
            }
        }

        assert_eq!(
            collected, seeded,
            "case {case}: the pages must carry every seeded channel exactly once, in \
             registry order"
        );
        // A token is issued only when a channel remains BEYOND the page just sent, so
        // an exactly-full registry ends on its last full page rather than on an empty
        // one, and an empty registry is still one (empty) page.
        let expected_pages = count.div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} page(s) for {count} channels of \
             {page_size}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let seeded = seed_channels(&emulator, HOME_REGION, 4);

    // A real token, from a real first page.
    let first = emulator
        .call("ListChannels", &list_request(Some(2), None))
        .await;
    let first_body = first.expect_ok("first page");
    assert_eq!(
        channel_entries(first_body),
        seeded[..2].to_vec(),
        "case first page: expected the first two channels, got {first_body:?}"
    );
    let issued = next_token(first_body).expect("the first of two pages carries a NextToken");

    // Rows differ in HOW the token is wrong: not this service's coding at all, a coded
    // payload that is not text, a coded payload with no resume key, and a token this
    // service did issue but for another page size (both an explicit one and the
    // default). Every one is the same refusal. Tokens are at least 4 characters, so the
    // member's modelled `@length 4..1000` is satisfied and it is the handler, not the
    // boundary, that refuses them.
    let rows = [
        (
            "not this service's coding",
            list_request(Some(2), Some("zzzz-not-a-token")),
        ),
        (
            "coded bytes that are not text",
            list_request(Some(2), Some("abcdef")),
        ),
        (
            "coded payload with no resume key",
            list_request(Some(2), Some(&::hex::encode("no-separator-here"))),
        ),
        (
            "coded payload with only one separator",
            list_request(Some(2), Some(&::hex::encode("2|no-region-field"))),
        ),
        (
            "a resume key this service never mints",
            list_request(
                Some(2),
                Some(&::hex::encode(format!("2|{HOME_REGION}|not-a-minted-id"))),
            ),
        ),
        (
            "issued for another MaxResults",
            list_request(Some(3), Some(&issued)),
        ),
        (
            "issued for a MaxResults the request omits",
            list_request(None, Some(&issued)),
        ),
    ];
    for (case, body) in rows {
        let reply = emulator.call("ListChannels", &body).await;
        reply.expect_error(case, 400, "InvalidNextTokenException");
    }

    // Same token, same page size, another Region: the registry spans every Region, so
    // a token belongs to the walk over ONE Region's channels and is refused elsewhere
    // rather than reporting an empty page as a success.
    let elsewhere = emulator
        .call_in(
            "ListChannels",
            "eu-central-1",
            &list_request(Some(2), Some(&issued)),
        )
        .await;
    elsewhere.expect_error("issued for another Region", 400, "InvalidNextTokenException");

    // The refusals changed nothing: the token still resumes the walk it was issued for.
    let second = emulator
        .call("ListChannels", &list_request(Some(2), Some(&issued)))
        .await;
    let second_body = second.expect_ok("second page");
    assert_eq!(
        channel_entries(second_body),
        seeded[2..].to_vec(),
        "case second page: expected the remaining channels, got {second_body:?}"
    );
    assert_eq!(
        next_token(second_body),
        None,
        "case second page: the final page must carry no NextToken, got {second_body:?}"
    );
}

/// RULE: a page reports the modelled `Channel` members and nothing else — the shape
/// carries only `ChannelArn` and `Name` — whatever else the stored channel holds.
/// Rows differ in how many tags the channel carries, since a tag set is the part of a
/// channel record `ListChannels` must NOT report.
#[tokio::test]
async fn a_page_reports_the_modelled_channel_members_only() {
    let emulator = Emulator::new();
    let rows = [("no tags", 0usize), ("one tag", 1), ("several tags", 3)];

    let mut expected: Vec<(String, String)> = Vec::new();
    for (index, (case, tags)) in rows.iter().enumerate() {
        let name = format!("channel-{}", case.replace(' ', "-"));
        let arn = seed_channel(
            &emulator,
            HOME_REGION,
            &channel_id(&format!("{:08x}", index + 1)),
            &name,
            (0..*tags)
                .map(|tag| Tag {
                    key: format!("key-{tag}"),
                    value: Some(format!("value-{tag}")),
                })
                .collect(),
        );
        expected.push((arn, name));
    }

    let reply = emulator
        .call("ListChannels", &list_request(None, None))
        .await;
    let body = reply.expect_ok("modelled members");
    assert_eq!(
        channel_entries(body),
        expected,
        "case modelled members: expected each channel's ARN and name, got {body:?}"
    );
    let members: Vec<Vec<String>> = body
        .get("Channels")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let mut keys: Vec<String> = item
                        .as_object()
                        .map(|fields| fields.keys().cloned().collect())
                        .unwrap_or_default();
                    keys.sort();
                    keys
                })
                .collect()
        })
        .unwrap_or_default();
    for (index, keys) in members.iter().enumerate() {
        assert_eq!(
            keys,
            &vec!["ChannelArn".to_string(), "Name".to_string()],
            "case modelled members: channel {index} must carry only the modelled \
             members, got {keys:?}"
        );
    }
}
