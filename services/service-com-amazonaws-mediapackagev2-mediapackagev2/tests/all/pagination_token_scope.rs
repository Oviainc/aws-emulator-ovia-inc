// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every paged list this service serves: a `nextToken` is read only as a
//! position in the COLLECTION it was issued for, so a list resumes a token it issued for
//! that collection and refuses every other token — a cursor one of this service's other
//! lists handed out, a cursor for the same kind of collection under another parent or in
//! another Region, a hand-written run of hex, an edited one — with the modeled 400
//! (`ValidationException`, `INVALID_PAGINATION_TOKEN`), never with a 200 whose `Items`
//! silently start past a boundary no page of that collection ever had.
//!
//! The table spans the class in both directions. The collections are every paged list the
//! model declares (`smithy_query(query="digest")`: the four operations binding `NextToken`
//! as `httpQuery("nextToken")`), instantiated so the set differs in LEVEL (groups,
//! channels, endpoints, harvest jobs), in PARENT (two groups holding channels of the same
//! names, two channels holding endpoints of the same names) and in REGION (the same group
//! list served in two Regions) — every axis the identity of a collection runs along here.
//! Each one is crossed with every token the others issue, and with tokens that differ in
//! FORM: hex of a name that exists, hex of a name that does not, a token with its
//! collection digest edited, one with its cursor edited, one truncated and one
//! uppercased. The accepted rows are asserted beside them: the token a collection issues
//! resumes exactly that collection, and walking it with those tokens reports every item
//! once, in order — which is what the refusal protects.
//!
//! The refusal body is not spelled out here: each case is compared against the body the
//! SAME list answers a malformed token with, so one wording covers both and this suite
//! cannot drift from `pagination_token_refusal`, which pins that wording.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The Region the fixtures are created in — the one `Emulator::call` signs for.
const HOME: &str = "us-west-2";
/// A second Region holding its own groups of the SAME names: a collection is identified by
/// its Region too, so the group list of one is not the group list of the other.
const OTHER: &str = "eu-west-1";

/// Two of everything at every level, so a page of one always leaves something over and
/// each collection therefore issues a real token.
const GROUP_A: &str = "pts-grp-a";
const GROUP_B: &str = "pts-grp-b";
const CHANNEL_1: &str = "pts-chan-1";
const CHANNEL_2: &str = "pts-chan-2";
const ENDPOINT_1: &str = "pts-ep-1";
const ENDPOINT_2: &str = "pts-ep-2";

/// A token no list ever issued, refused on its form alone: the row
/// `pagination_token_refusal` pins the wording of. Every other refusal in this suite is
/// compared against the body this one produces.
const MALFORMED_TOKEN: &str = "zzzz";

/// One collection a paged list walks: how a case names it, the Region it is served in and
/// the path it is served at.
struct Collection {
    label: String,
    region: &'static str,
    path: String,
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(body: &Value) -> &str {
    body.get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `GET path`, served in `region` (the Region travels in the SigV4 credential scope, and
/// the hostname is the regional one a client addresses).
async fn get_in(emulator: &Emulator, region: &str, path: &str) -> (u16, Value) {
    emulator
        .send_in_region(
            "GET",
            path,
            None,
            &format!("mediapackagev2.{region}.amazonaws.com"),
            region,
            &[],
        )
        .await
}

/// `POST path` with `body`, served in `region`.
async fn post_in(emulator: &Emulator, region: &str, path: &str, body: Value) -> (u16, Value) {
    emulator
        .send_in_region(
            "POST",
            path,
            Some(body),
            &format!("mediapackagev2.{region}.amazonaws.com"),
            region,
            &[],
        )
        .await
}

/// Create the hierarchy every row is paged through, and return the collections it holds.
///
/// `HOME` gets two groups; the first holds two channels, each holding two origin
/// endpoints, and two harvest jobs. The second holds two channels of the SAME names, so a
/// cursor is not interchangeable between two collections whose items are named alike.
/// `OTHER` gets two groups of the same names, for the Region axis.
async fn seed(emulator: &Emulator) -> Vec<Collection> {
    for region in [HOME, OTHER] {
        for group in [GROUP_A, GROUP_B] {
            let (status, body) = post_in(
                emulator,
                region,
                "/channelGroup",
                json!({"ChannelGroupName": group}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case \"setup\": CreateChannelGroup {group} in {region} expected 200, got \
                 {status} {body}"
            );
        }
    }
    for group in [GROUP_A, GROUP_B] {
        for channel in [CHANNEL_1, CHANNEL_2] {
            let (status, body) = post_in(
                emulator,
                HOME,
                &format!("/channelGroup/{group}/channel"),
                json!({"ChannelName": channel}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case \"setup\": CreateChannel {channel} under {group} expected 200, got \
                 {status} {body}"
            );
        }
    }
    for channel in [CHANNEL_1, CHANNEL_2] {
        for endpoint in [ENDPOINT_1, ENDPOINT_2] {
            let mut body = minimal_endpoint("TS");
            body["OriginEndpointName"] = json!(endpoint);
            let (status, response) = post_in(
                emulator,
                HOME,
                &format!("/channelGroup/{GROUP_A}/channel/{channel}/originEndpoint"),
                body,
            )
            .await;
            assert_eq!(
                status, 200,
                "case \"setup\": CreateOriginEndpoint {endpoint} under {channel} expected 200, \
                 got {status} {response}"
            );
        }
    }
    let start = epoch_seconds_from_now(600.0);
    for job in ["pts-job-1", "pts-job-2"] {
        let mut request = harvest_body(start, start + 3600.0, &["index"]);
        request["HarvestJobName"] = json!(job);
        let (status, body) = post_in(
            emulator,
            HOME,
            &format!(
                "/channelGroup/{GROUP_A}/channel/{CHANNEL_1}/originEndpoint/{ENDPOINT_1}\
                 /harvestJob"
            ),
            request,
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateHarvestJob {job} expected 200, got {status} {body}"
        );
    }

    vec![
        Collection {
            label: format!("ListChannelGroups in {HOME}"),
            region: HOME,
            path: "/channelGroup".to_string(),
        },
        Collection {
            label: format!("ListChannelGroups in {OTHER}"),
            region: OTHER,
            path: "/channelGroup".to_string(),
        },
        Collection {
            label: format!("ListChannels of {GROUP_A}"),
            region: HOME,
            path: format!("/channelGroup/{GROUP_A}/channel"),
        },
        Collection {
            label: format!("ListChannels of {GROUP_B}"),
            region: HOME,
            path: format!("/channelGroup/{GROUP_B}/channel"),
        },
        Collection {
            label: format!("ListOriginEndpoints of {CHANNEL_1}"),
            region: HOME,
            path: format!("/channelGroup/{GROUP_A}/channel/{CHANNEL_1}/originEndpoint"),
        },
        Collection {
            label: format!("ListOriginEndpoints of {CHANNEL_2}"),
            region: HOME,
            path: format!("/channelGroup/{GROUP_A}/channel/{CHANNEL_2}/originEndpoint"),
        },
        Collection {
            label: format!("ListHarvestJobs of {GROUP_A}"),
            region: HOME,
            path: format!("/channelGroup/{GROUP_A}/harvestJob"),
        },
    ]
}

/// The token `collection` issues after a page of one — every collection here holds at
/// least two items, so there is always one.
async fn issued_token(emulator: &Emulator, collection: &Collection) -> String {
    let label = &collection.label;
    let (status, body) = get_in(
        emulator,
        collection.region,
        &format!("{}?maxResults=1", collection.path),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: a first page of one expected 200, got {status} {body}"
    );
    let token = body["NextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case {label:?}: a collection of more than one item must issue a token after a page \
         of one, got {body}"
    );
    token
}

/// Assert `collection` refuses `token` exactly as it refuses a malformed one.
async fn assert_refused(emulator: &Emulator, collection: &Collection, case: &str, token: &str) {
    let (status, body) = get_in(
        emulator,
        collection.region,
        &format!("{}?nextToken={token}", collection.path),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a token this collection did not issue must be refused, got {status} \
         {body}"
    );
    assert_eq!(
        error_type(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        validation_type(&body),
        "INVALID_PAGINATION_TOKEN",
        "case {case:?}: expected ValidationExceptionType INVALID_PAGINATION_TOKEN, got {body}"
    );
    let (_, malformed) = get_in(
        emulator,
        collection.region,
        &format!("{}?nextToken={MALFORMED_TOKEN}", collection.path),
    )
    .await;
    assert_eq!(
        body, malformed,
        "case {case:?}: one refusal for every token this collection did not issue, got {body} \
         against {malformed}"
    );
}

/// Each item of a page, as the member that names it at that level.
fn names(body: &Value) -> Vec<String> {
    body["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            for member in [
                "HarvestJobName",
                "OriginEndpointName",
                "ChannelName",
                "ChannelGroupName",
            ] {
                if let Some(name) = item.get(member).and_then(Value::as_str) {
                    return name.to_string();
                }
            }
            String::new()
        })
        .collect()
}

#[tokio::test]
async fn a_token_is_read_only_in_the_collection_that_issued_it() {
    let emulator = Emulator::new();
    let collections = seed(&emulator).await;

    for issuer in &collections {
        let token = issued_token(&emulator, issuer).await;
        for reader in &collections {
            let case = format!("token of {:?} read by {:?}", issuer.label, reader.label);
            if reader.label == issuer.label {
                // Its own collection resumes it, strictly beyond the first page.
                let (status, first) = get_in(
                    &emulator,
                    reader.region,
                    &format!("{}?maxResults=1", reader.path),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a first page of one expected 200, got {status} {first}"
                );
                let (status, second) = get_in(
                    &emulator,
                    reader.region,
                    &format!("{}?maxResults=1&nextToken={token}", reader.path),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the token this collection issued must resume it, got \
                     {status} {second}"
                );
                let (first, second) = (names(&first), names(&second));
                assert_eq!(
                    (first.len(), second.len()),
                    (1, 1),
                    "case {case:?}: two pages of one expected, got {first:?} then {second:?}"
                );
                assert!(
                    second[0] > first[0],
                    "case {case:?}: the resumed page must start strictly beyond the first, got \
                     {first:?} then {second:?}"
                );
            } else {
                assert_refused(&emulator, reader, &case, &token).await;
            }
        }
    }
}

#[tokio::test]
async fn a_token_no_collection_issued_is_refused_in_every_form() {
    let emulator = Emulator::new();
    let collections = seed(&emulator).await;

    for collection in &collections {
        let issued = issued_token(&emulator, collection).await;
        // Hand-written runs of hex: the bytes of a name this collection holds, of one it
        // does not, and of a single letter — each decodes to text, and none of them is a
        // token this service ever handed out.
        let mut tokens: Vec<(String, String)> = [GROUP_A, CHANNEL_1, "m", "a", "no-such-name"]
            .iter()
            .map(|name| (format!("hex of {name:?}"), hex::encode(name.as_bytes())))
            .collect();
        // Tokens DERIVED from one this collection issued: a digest edited, a cursor
        // edited, the token truncated, and the token uppercased. None of these is a token
        // this service issued either.
        tokens.push((
            "issued token with its first character changed".to_string(),
            format!("{}{}", flip(&issued[..1]), &issued[1..]),
        ));
        tokens.push((
            "issued token with its last character changed".to_string(),
            format!(
                "{}{}",
                &issued[..issued.len() - 1],
                flip(&issued[issued.len() - 1..])
            ),
        ));
        tokens.push((
            "issued token truncated".to_string(),
            issued[..issued.len() - 2].to_string(),
        ));
        tokens.push(("issued token uppercased".to_string(), issued.to_uppercase()));

        for (form, token) in tokens {
            let case = format!("{} given {form}", collection.label);
            assert_refused(&emulator, collection, &case, &token).await;
        }
    }
}

/// Another lowercase hex character, so an edited token stays hex and is refused on what it
/// IS rather than on its alphabet.
fn flip(character: &str) -> String {
    match character {
        "0" => "1".to_string(),
        _ => "0".to_string(),
    }
}

#[tokio::test]
async fn paging_with_issued_tokens_reports_every_item_once_in_order() {
    let emulator = Emulator::new();
    let collections = seed(&emulator).await;

    for collection in &collections {
        let label = &collection.label;
        // The whole collection in one page, as the order every page of it follows.
        let (status, all) = get_in(
            &emulator,
            collection.region,
            &format!("{}?maxResults=100", collection.path),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: one page of everything expected 200, got {status} {all}"
        );
        let expected = names(&all);
        assert!(
            expected.len() >= 2,
            "case {label:?}: the fixture must hold at least two items, got {expected:?}"
        );

        for page_size in [1usize, 2, 3] {
            let mut seen: Vec<String> = Vec::new();
            let mut query = format!("?maxResults={page_size}");
            for _ in 0..=expected.len() {
                let (status, body) = get_in(
                    &emulator,
                    collection.region,
                    &format!("{}{query}", collection.path),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {label:?}/maxResults={page_size}: expected 200, got {status} {body}"
                );
                let page = names(&body);
                assert!(
                    page.len() <= page_size,
                    "case {label:?}/maxResults={page_size}: a page of {} items is longer than \
                     the page size",
                    page.len()
                );
                seen.extend(page);
                match body["NextToken"].as_str() {
                    Some(token) => {
                        query = format!("?maxResults={page_size}&nextToken={token}");
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {label:?}/maxResults={page_size}: paging must report every item once, in \
                 order, got {seen:?}"
            );
        }
    }
}
