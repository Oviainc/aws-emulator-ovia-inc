// SPDX-License-Identifier: CC0-1.0

//! How AppSync refuses — and how it reports a served write — on the two neighbouring
//! surfaces that address a resource by something other than an `apiId`: the API-key
//! operations, which address a key by an id that carries the key's VERSION, and the
//! tagging operations, which address a RESOURCE by ARN.
//!
//! Five rules, each asserted over a TABLE spanning the class it generalises over rather
//! than over the single request that exposed it:
//!
//! 1. an API key id states the version of the key it names, and an operation that is not
//!    available for a version classifies the id before resolving it: `UpdateApiKey` on an
//!    id that states no version it serves is a 400 `BadRequestException` with the
//!    service's fixed sentence for an unsupported version, whether or not the addressed
//!    API or key exists, while an id that states the served version and names nothing
//!    stays a not-found;
//! 2. an operation that addresses an API key by id states which id named nothing: a 404
//!    `NotFoundException` whose fixed text is followed by the id taken from the request
//!    path, the same way for every operation that resolves a key id;
//! 3. a tag key under the prefix AWS reserves is its own refusal — one fixed sentence
//!    naming the prefix and the case-insensitive comparison, not the tag-key grammar —
//!    while a key that breaks the grammar instead keeps the grammar's wording, and a key
//!    that merely begins with those letters is an ordinary key;
//! 4. a served tag write reports no content: status 204, and a body with no members, for
//!    both write operations and whatever the write leaves behind;
//! 5. the tagging surface refuses by RESOURCE — a well-formed ARN this account holds no
//!    resource for is a 404 `NotFoundException` with the surface's fixed absent-resource
//!    text, and a value that cannot be a resource ARN at all is a 400
//!    `BadRequestException` with its malformed-ARN text — the bare sentence, or the one
//!    naming the vendor that was expected when the value is an ARN of another vendor
//!    (the rule the `tagging_arn_vendor_refusal` suite spans) — on each of the three
//!    operations the one `/v1/tags/{resourceArn}` route carries.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, the `@httpLabel` ARN binding, the boundary validator, restJson1
//! (de)serialization, the handlers and the shared engine together. The harness is the
//! error-contract suite's `call`/`Answer` over the lifecycle suite's router and create
//! helper, with the API-key and tag suites' request helpers and modelled bounds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_key_deletion_window::{create_key, listed_key, unix_now, update_key, DAY};
use crate::error_contract::{call, Answer};
use crate::graphql_api_lifecycle::{create_api, router, REGION};
use crate::resource_tags::{ABSENT_API_ID, ACCOUNT, TAG_KEY_MAX_LEN};
use crate::tag_resource::{
    list_tags, tag_map, tag_resource, RESERVED_TAG_KEY_REFUSAL, RESOURCE_NOT_FOUND,
    TAG_WRITE_STATUS,
};
use crate::tagging_arn_vendor_refusal::EXPECTED_VENDOR_REFUSAL;
use crate::untag_resource::untag_resource;

/// The fixed half of the text a 404 carries for an API key the addressed API does not
/// hold; the id the request named follows it.
const API_KEY_NOT_FOUND_PREFIX: &str = "API key not found: ";

/// The fixed text a 400 carries for a call that addresses a key of a version it is not
/// available for. Source: the `ApiKey` shape's documentation, which states
/// "UpdateApiKey is not available for this key version" for the older of the two
/// versions it describes (`smithy_query(query="doc", shape="ApiKey")`).
const UNSUPPORTED_KEY_VERSION: &str = "Unsupported API key version.";

/// The fixed text a 400 carries for a `resourceArn` that is not a resource ARN at all,
/// where the value names no other vendor than this service's own — the rows below that
/// break a field other than the ARN's vendor. A value whose vendor field names another
/// service takes the sentence that says so ([`EXPECTED_VENDOR_REFUSAL`]).
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn";

/// The API key versions the `ApiKey` shape's documentation describes, with the one whose
/// keys an update is available for marked. Source: that documentation — "There are two
/// key versions", `da1` ("UpdateApiKey is not available for this key version") and `da2`
/// ("UpdateApiKey returns the expiration time and and deletion time in seconds and
/// accepts a user-provided expiration time in seconds").
const KEY_VERSIONS: [(&str, bool); 2] = [("da1", false), ("da2", true)];

/// The version segment of the key ids this service mints, as a prefix.
fn updatable_prefix() -> String {
    let (version, _) = KEY_VERSIONS
        .into_iter()
        .find(|(_, updatable)| *updatable)
        .expect("one version admits an update");
    format!("{version}-")
}

/// Create one GraphQL API and return its `apiId` and its ARN.
async fn api(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = create_api(
        router,
        json!({ "name": name, "authenticationType": "API_KEY" }),
    )
    .await;
    (
        created["apiId"]
            .as_str()
            .unwrap_or_else(|| panic!("the created API carries an id: {created:?}"))
            .to_string(),
        created["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("the created API carries an ARN: {created:?}"))
            .to_string(),
    )
}

/// The ARN an AppSync API of this account and Region is published under, for the id
/// `api_id`: the one shape `ResourceArn` admits.
fn api_arn(api_id: &str) -> String {
    format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{api_id}")
}

/// `DELETE /v1/apis/{apiId}/apikeys/{id}`.
async fn delete_key(router: &MultiServiceRouter, api_id: &str, key_id: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/apikeys/{key_id}"),
        None,
    )
    .await
}

/// Rule 1: `UpdateApiKey` classifies the key version the id states BEFORE it resolves
/// the id, so an id stating no version the operation serves is a bad request however the
/// rest of the address resolves.
///
/// The rows differ along the id that states no served version (the older version at
/// several lengths, a version never issued, an id with no version segment at all, the
/// served version without its separator) and along what the rest of the address names: an
/// API that holds keys, an API that holds none, and an id no create ever minted.
#[tokio::test]
async fn an_update_of_an_unsupported_key_version_is_a_bad_request() {
    let router = router();
    let (api_id, _) = api(&router, "Key_version_held").await;
    let (empty_api_id, _) = api(&router, "Key_version_empty").await;

    // One stored key, so the API the rows address really does hold keys to resolve
    // against.
    let created = create_key(
        &router,
        &api_id,
        json!({ "description": "held", "expires": unix_now() + 30 * DAY }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the stored key: expected CreateApiKey 200, got {created:?}"
    );
    let held_key_id = created.body["apiKey"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The version a key id states is the segment before its first `-`; no row below
    // states the version an update is available for.
    let unsupported: &[(&str, String)] = &[
        (
            "a full-length id of the older version",
            "da1-dq5e45sgwr3mamypsk3zabjbfx".to_string(),
        ),
        ("a short id of the older version", "da1-0".to_string()),
        ("the older version with no suffix", "da1-".to_string()),
        (
            "the older version with the stored key's suffix",
            format!(
                "da1-{}",
                held_key_id
                    .strip_prefix(&updatable_prefix())
                    .unwrap_or(&held_key_id)
            ),
        ),
        (
            "a version never issued",
            "da3-dq5e45sgwr3mamypsk3zabjbfx".to_string(),
        ),
        ("an id with no version segment", "nosuchkey".to_string()),
        (
            "an id whose first segment is not a version",
            "plain-key-id".to_string(),
        ),
        (
            "the served version without its separator",
            updatable_prefix().trim_end_matches('-').to_string(),
        ),
    ];
    let addresses: [(&str, &str); 3] = [
        ("an API holding keys", api_id.as_str()),
        ("an API holding none", empty_api_id.as_str()),
        ("an apiId no create minted", ABSENT_API_ID),
    ];

    for (case, key_id) in unsupported {
        for (where_, addressed) in &addresses {
            let answer = update_key(
                &router,
                addressed,
                key_id,
                json!({ "expires": unix_now() + 2 * DAY }),
            )
            .await;
            assert_eq!(
                (
                    answer.status,
                    answer.error_type.as_str(),
                    answer.message.as_str()
                ),
                (400, "BadRequestException", UNSUPPORTED_KEY_VERSION),
                "case {case:?} on {where_}: expected 400 BadRequestException \
                 {UNSUPPORTED_KEY_VERSION:?}, got {answer:?}"
            );
        }
    }

    // Every version the catalog names, so the classification is driven by the whole of
    // it: a key of a version that can be updated and is not stored is the not-found it
    // is, and one of a version that cannot is the bad request above.
    for (version, updatable) in KEY_VERSIONS {
        let key_id = format!("{version}-dq5e45sgwr3mamypsk3zabjbfx");
        let answer = update_key(
            &router,
            &api_id,
            &key_id,
            json!({ "expires": unix_now() + 2 * DAY }),
        )
        .await;
        let expected = if updatable {
            (404, "NotFoundException")
        } else {
            (400, "BadRequestException")
        };
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            expected,
            "case the version {version:?}: expected {expected:?}, got {answer:?}"
        );
    }

    // The complement, so the refusal above is about the VERSION rather than about every
    // id the API does not hold: an id that states the served version and names no stored
    // key stays the not-found it is, at several shapes.
    let resolvable: &[(&str, String)] = &[
        (
            "a full-length id of the served version",
            format!("{}dq5e45sgwr3mamypsk3zabjbfx", updatable_prefix()),
        ),
        ("the served version with no suffix", updatable_prefix()),
        (
            "a short id of the served version",
            format!("{}0", updatable_prefix()),
        ),
    ];
    for (case, key_id) in resolvable {
        let answer = update_key(
            &router,
            &api_id,
            key_id,
            json!({ "expires": unix_now() + 2 * DAY }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {answer:?}"
        );
    }

    // A refused update writes nothing: the stored key is exactly the one the create
    // left behind.
    assert_eq!(
        listed_key(&router, &api_id, &held_key_id).await,
        created.body["apiKey"],
        "case the stored key: expected the refused updates to leave it as it was, got {:?}",
        listed_key(&router, &api_id, &held_key_id).await
    );

    // And the version the service mints is one that CAN be updated, so the rule refuses
    // nothing a caller holds.
    assert!(
        KEY_VERSIONS.iter().any(|(version, updatable)| *updatable
            && held_key_id.starts_with(&format!("{version}-"))),
        "case the minted key id: expected it to lead with a version that can be updated, \
         got {held_key_id:?}"
    );
    let answer = update_key(
        &router,
        &api_id,
        &held_key_id,
        json!({ "expires": unix_now() + 2 * DAY }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case the stored key of the current version: expected UpdateApiKey 200, got {answer:?}"
    );
}

/// Rule 2: an operation that addresses an API key by id names that id in its not-found.
///
/// The rows differ along the id that names nothing (lengths and shapes, a version an
/// update is not available for, an id with no version segment, a key another API holds, a
/// key an earlier delete removed) and along the operation that resolves it, so the
/// template is the key's rather than one operation's. `DeleteApiKey` "deletes the item
/// from the table" for either key version (`smithy_query(query="doc", shape="ApiKey")`),
/// so it resolves every row; an update is available for one version only, so it resolves
/// the rows that state it and answers rule 1's refusal for the others.
#[tokio::test]
async fn an_absent_api_key_is_reported_with_the_id_that_named_it() {
    let router = router();
    let (api_id, _) = api(&router, "Key_not_found_subject").await;
    let (other_api_id, _) = api(&router, "Key_not_found_other").await;

    let theirs = create_key(
        &router,
        &other_api_id,
        json!({ "description": "theirs", "expires": unix_now() + 30 * DAY }),
    )
    .await;
    let their_key_id = theirs.body["apiKey"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A key of this API that is deleted, so its id named something and now names
    // nothing.
    let doomed = create_key(
        &router,
        &api_id,
        json!({ "description": "doomed", "expires": unix_now() + 30 * DAY }),
    )
    .await;
    let doomed_key_id = doomed.body["apiKey"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let removed = delete_key(&router, &api_id, &doomed_key_id).await;
    assert_eq!(
        removed.status, 200,
        "case the first delete: expected DeleteApiKey 200, got {removed:?}"
    );

    // (case, the id that names nothing, whether an update is available for its version)
    let cases: &[(&str, String, bool)] = &[
        (
            "a full-length id no create minted",
            format!("{}dq5e45sgwr3mamypsk3zabjbfx", updatable_prefix()),
            true,
        ),
        ("a short id", format!("{}0", updatable_prefix()), true),
        ("an id with no suffix", updatable_prefix(), true),
        (
            "an id of the older version",
            "da1-dq5e45sgwr3mamypsk3zabjbfx".to_string(),
            false,
        ),
        (
            "an id with no version segment",
            "nosuchkey".to_string(),
            false,
        ),
        ("a key another API holds", their_key_id.clone(), true),
        (
            "a key an earlier delete removed",
            doomed_key_id.clone(),
            true,
        ),
    ];

    for (case, key_id, updatable) in cases {
        let expected = format!("{API_KEY_NOT_FOUND_PREFIX}{key_id}");
        let answer = delete_key(&router, &api_id, key_id).await;
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.message.as_str()
            ),
            (404, "NotFoundException", expected.as_str()),
            "case {case:?} on DeleteApiKey: expected 404 NotFoundException {expected:?}, \
             got {answer:?}"
        );

        if !updatable {
            continue;
        }
        let answer = update_key(
            &router,
            &api_id,
            key_id,
            json!({ "expires": unix_now() + 2 * DAY }),
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.message.as_str()
            ),
            (404, "NotFoundException", expected.as_str()),
            "case {case:?} on UpdateApiKey: expected 404 NotFoundException {expected:?}, \
             got {answer:?}"
        );
    }

    // The key that DOES exist, under the API that holds it, is untouched: the refusals
    // above are about the id rather than about the route.
    assert_eq!(
        listed_key(&router, &other_api_id, &their_key_id).await,
        theirs.body["apiKey"],
        "case the other API's key: expected it to stand, got {:?}",
        listed_key(&router, &other_api_id, &their_key_id).await
    );
}

/// Rule 3: a tag key under the reserved prefix takes its own fixed sentence, a key that
/// breaks the tag-key grammar keeps the grammar's wording, and a key that merely begins
/// with those letters is ordinary.
///
/// The reserved rows differ along the case of the prefix (the comparison the sentence
/// states is case-insensitive), along what follows it (nothing, a word, a key at the
/// modelled maximum length) and along the write operation that carries the key.
#[tokio::test]
async fn a_reserved_tag_key_takes_its_own_refusal() {
    let router = router();
    let (_, arn) = api(&router, "Reserved_tag_key").await;
    let held = json!({ "env": "prod" });
    let written = tag_resource(&router, &arn, held.clone()).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case the initial write: expected {TAG_WRITE_STATUS}, got {written:?}"
    );

    // `TagKey`'s maximum length is 128 (`smithy_query(query="shape", shape="TagKey")`),
    // so a reserved key ON that bound is refused for the prefix rather than the length.
    let long_reserved = format!("aws:{}", "k".repeat(TAG_KEY_MAX_LEN - 4));
    let reserved: &[(&str, String)] = &[
        ("the reserved prefix alone", "aws:".to_string()),
        ("a reserved key", "aws:cost-center".to_string()),
        ("the prefix in upper case", "AWS:cost-center".to_string()),
        ("the prefix in mixed case", "Aws:Secret".to_string()),
        ("the prefix in another mixed case", "aWs:x".to_string()),
        ("a reserved key at the maximum length", long_reserved),
    ];

    for (case, key) in reserved {
        let written =
            tag_resource(&router, &arn, tag_map(&[(key.clone(), "v".to_string())])).await;
        assert_eq!(
            (
                written.status,
                written.error_type.as_str(),
                written.message.as_str()
            ),
            (400, "BadRequestException", RESERVED_TAG_KEY_REFUSAL),
            "case {case:?} ({key:?}) on TagResource: expected 400 BadRequestException \
             {RESERVED_TAG_KEY_REFUSAL:?}, got {written:?}"
        );

        let removed = untag_resource(&router, &arn, std::slice::from_ref(key)).await;
        assert_eq!(
            (
                removed.status,
                removed.error_type.as_str(),
                removed.message.as_str()
            ),
            (400, "BadRequestException", RESERVED_TAG_KEY_REFUSAL),
            "case {case:?} ({key:?}) on UntagResource: expected 400 BadRequestException \
             {RESERVED_TAG_KEY_REFUSAL:?}, got {removed:?}"
        );

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.body["tags"], held,
            "case {case:?}: expected the refused calls to change nothing, got {listed:?}"
        );
    }

    // A key that breaks the GRAMMAR rather than the reserved prefix keeps the grammar's
    // wording, which names the rejected key — so the two grounds are stated apart.
    for (case, key) in [
        ("a character outside the alphabet", "bad*key"),
        ("another character outside the alphabet", "bad!key"),
        ("a non-ASCII letter", "café"),
    ] {
        let written =
            tag_resource(&router, &arn, tag_map(&[(key.to_string(), "v".to_string())])).await;
        assert_eq!(
            (written.status, written.error_type.as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {written:?}"
        );
        assert_ne!(
            written.message, RESERVED_TAG_KEY_REFUSAL,
            "case {case:?}: expected the grammar's wording rather than the reserved \
             prefix's, got {written:?}"
        );
        assert!(
            written.message.contains(key),
            "case {case:?}: expected the grammar's refusal to name the rejected key, \
             got {written:?}"
        );
    }

    // The reserved prefix is `aws:`: a key that merely begins with those letters, or
    // carries them elsewhere, is an ordinary key and is written.
    for (case, key) in [
        ("a key beginning with aws", "awsome"),
        ("the three letters alone", "aws"),
        ("a colon after something else", "awsx:y"),
        ("the letters after a prefix of their own", "my:aws:key"),
    ] {
        let written =
            tag_resource(&router, &arn, tag_map(&[(key.to_string(), "v".to_string())])).await;
        assert_eq!(
            written.status, TAG_WRITE_STATUS,
            "case {case:?} ({key:?}): expected {TAG_WRITE_STATUS}, got {written:?}"
        );
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.body["tags"][key],
            Value::String("v".to_string()),
            "case {case:?}: expected the key to be written, got {listed:?}"
        );
    }
}

/// Rule 4: a served tag write answers 204 and no body at all, whichever write it is and
/// whatever it leaves behind.
///
/// The rows differ along what the write does — add a key, overwrite one, add several,
/// remove one, remove one the resource never carried, remove every key — so the status
/// belongs to the operation rather than to a particular outcome.
#[tokio::test]
async fn a_served_tag_write_reports_no_content() {
    let router = router();
    let (_, arn) = api(&router, "Tag_write_status").await;

    // (case, the write, the keys left afterwards)
    enum Write {
        Tag(Value),
        Untag(Vec<String>),
    }
    let rows: Vec<(&str, Write)> = vec![
        ("a key added", Write::Tag(json!({ "env": "prod" }))),
        ("the same key overwritten", Write::Tag(json!({ "env": "staging" }))),
        (
            "several keys at once",
            Write::Tag(json!({ "team": "search", "owner": "platform" })),
        ),
        ("one key removed", Write::Untag(vec!["team".to_string()])),
        (
            "a key the resource never carried removed",
            Write::Untag(vec!["absent".to_string()]),
        ),
        (
            "every key removed",
            Write::Untag(vec!["env".to_string(), "owner".to_string()]),
        ),
    ];

    for (case, write) in &rows {
        let answer = match write {
            Write::Tag(tags) => tag_resource(&router, &arn, tags.clone()).await,
            Write::Untag(keys) => untag_resource(&router, &arn, keys).await,
        };
        assert_eq!(
            answer.status, TAG_WRITE_STATUS,
            "case {case:?}: expected status {TAG_WRITE_STATUS}, got {answer:?}"
        );
        // `TagResourceResponse` and `UntagResourceResponse` model no members, so the
        // answer carries none.
        assert!(
            answer.body.is_null()
                || answer
                    .body
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
            "case {case:?}: expected an answer with no members, got {:?}",
            answer.body
        );

        // The read that follows is a 200 with a body, so the status above is the write's
        // own rather than the route's.
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {listed:?}"
        );
        assert!(
            listed.body["tags"].is_object(),
            "case {case:?}: expected the listing to carry a tag map, got {listed:?}"
        );
    }

    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed.body["tags"],
        json!({}),
        "case the whole walk: expected the last removal to leave an empty map, got {listed:?}"
    );
}

/// Rule 5: the tagging surface refuses by resource — an absent one with its fixed
/// absent-resource text, a value that is no resource ARN with its fixed malformed-ARN
/// text — on each of the three operations the one tag route carries.
///
/// The absent rows differ along why the ARN names nothing (an id never minted, a deleted
/// resource, another Region, another account, a Region long enough to put the ARN on the
/// modelled maximum length). The malformed rows differ along what makes the value no ARN
/// at all: each published length bound crossed by one character, and the pattern broken
/// in the partition, the service, the account, the resource type and the id.
#[tokio::test]
async fn the_tagging_surface_refuses_by_resource() {
    let router = router();
    let (api_id, arn) = api(&router, "Tag_arn_refusals").await;
    let written = tag_resource(&router, &arn, json!({ "env": "prod" })).await;
    assert_eq!(
        written.status, TAG_WRITE_STATUS,
        "case the initial write: expected {TAG_WRITE_STATUS}, got {written:?}"
    );

    let (doomed_id, doomed_arn) = api(&router, "Tag_arn_deleted").await;
    let deleted = call(&router, "DELETE", &format!("/v1/apis/{doomed_id}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "case the deleted API: expected DeleteGraphqlApi 200, got {deleted:?}"
    );

    // `ResourceArn` admits ARNs of 70 to 75 characters (`smithy_query(query="operation",
    // operation="ListTagsForResource")` — `@length` min 70, max 75, with the Region the
    // only segment whose width varies). A 14-character Region puts a well-formed ARN ON
    // the maximum, so it is answered as the absent resource it names rather than as a
    // malformed value.
    let longest_region = "eu-west-1-abcd";
    let longest_arn =
        format!("arn:aws:appsync:{longest_region}:{ACCOUNT}:apis/{ABSENT_API_ID}");
    assert_eq!(
        longest_arn.chars().count(),
        75,
        "case the longest well-formed ARN: expected it on the modelled maximum, got {longest_arn:?}"
    );

    let absent: &[(&str, String)] = &[
        ("an id no create minted", api_arn(ABSENT_API_ID)),
        ("a deleted resource", doomed_arn),
        (
            "another Region",
            format!("arn:aws:appsync:us-east-1:{ACCOUNT}:apis/{api_id}"),
        ),
        (
            "another account",
            format!("arn:aws:appsync:{REGION}:123456789012:apis/{api_id}"),
        ),
        ("the longest well-formed ARN", longest_arn),
    ];

    for (case, candidate) in absent {
        for (operation, answer) in tagging_calls(&router, candidate).await {
            assert_eq!(
                (
                    answer.status,
                    answer.error_type.as_str(),
                    answer.message.as_str()
                ),
                (404, "NotFoundException", RESOURCE_NOT_FOUND),
                "case {case:?} ({candidate}) on {operation}: expected 404 NotFoundException \
                 {RESOURCE_NOT_FOUND:?}, got {answer:?}"
            );
        }
    }

    // One character under the minimum length and one over the maximum, then the pattern
    // broken in each segment at a length the bounds admit.
    let too_short = format!(
        "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}",
        &ABSENT_API_ID[..25]
    );
    let too_long = format!(
        "arn:aws:appsync:{longest_region}x:{ACCOUNT}:apis/{ABSENT_API_ID}"
    );
    // Each row carries the sentence its ground takes: a broken VENDOR field is the one
    // the refusal diagnoses by name, every other broken field leaves the bare sentence.
    let malformed: &[(&str, String, &str)] = &[
        (
            "one character under the minimum length",
            too_short,
            INVALID_RESOURCE_ARN,
        ),
        (
            "one character over the maximum length",
            too_long,
            INVALID_RESOURCE_ARN,
        ),
        (
            "another partition",
            format!("arn:xws:appsync:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
            INVALID_RESOURCE_ARN,
        ),
        (
            "another service",
            format!("arn:aws:appsynx:{REGION}:{ACCOUNT}:apis/{ABSENT_API_ID}"),
            EXPECTED_VENDOR_REFUSAL,
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:appsync:{REGION}:00000000000a:apis/{ABSENT_API_ID}"),
            INVALID_RESOURCE_ARN,
        ),
        (
            "another resource type",
            format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apiz/{ABSENT_API_ID}"),
            INVALID_RESOURCE_ARN,
        ),
        (
            "an id outside the id alphabet",
            format!("arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}!", &ABSENT_API_ID[..25]),
            INVALID_RESOURCE_ARN,
        ),
        (
            "no ARN at all",
            "not-an-arn".to_string(),
            INVALID_RESOURCE_ARN,
        ),
    ];

    for (case, candidate, expected) in malformed {
        for (operation, answer) in tagging_calls(&router, candidate).await {
            assert_eq!(
                (
                    answer.status,
                    answer.error_type.as_str(),
                    answer.message.as_str()
                ),
                (400, "BadRequestException", *expected),
                "case {case:?} ({candidate}) on {operation}: expected 400 BadRequestException \
                 {expected:?}, got {answer:?}"
            );
        }
    }

    // The resource that DOES exist is still served, so the refusals above are about the
    // ARN rather than about the route: its ARN sits on the modelled minimum length.
    assert_eq!(
        arn.chars().count(),
        70,
        "case the stored resource: expected its ARN on the modelled minimum length, got {arn:?}"
    );
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        (listed.status, &listed.body["tags"]),
        (200, &json!({ "env": "prod" })),
        "case the stored resource: expected its tags, got {listed:?}"
    );
}

/// The three operations the one `/v1/tags/{resourceArn}` route carries, each answered
/// for `candidate`, paired with the operation's name.
pub(crate) async fn tagging_calls(
    router: &MultiServiceRouter,
    candidate: &str,
) -> Vec<(&'static str, Answer)> {
    vec![
        ("ListTagsForResource", list_tags(router, candidate).await),
        (
            "TagResource",
            tag_resource(router, candidate, json!({ "env": "prod" })).await,
        ),
        (
            "UntagResource",
            untag_resource(router, candidate, &["env".to_string()]).await,
        ),
    ]
}
