// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPlaybackKeyPairs` and `DeletePlaybackKeyPair`, driven
//! over the wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs` call arrives — on the
//! harness `channel_lifecycle` owns, so both suites drive the emulator exactly one
//! way and what this asserts is HTTP behaviour, not a handler called directly.
//!
//! Importing and reading back one key pair is `playback_key_pair_import`'s subject;
//! the rules here are the LIST's and the DELETE's, each stated once and then
//! exercised over a SET of inputs:
//!
//! 1. `ListPlaybackKeyPairs` answers every key pair this account holds as a
//!    `PlaybackKeyPairSummary` — `arn`, `name` and `tags`, and nothing else the shape
//!    does not declare — and reflects what was imported and what was deleted.
//! 2. It answers in one stable order (ARN), cut into pages of at most `maxResults`,
//!    with a `nextToken` that resumes exactly where the previous page stopped; a
//!    `maxResults` outside the modelled range, and a token this service did not issue
//!    for THIS list, are refused rather than answered with a truncated page.
//! 3. `DeletePlaybackKeyPair` makes the pair observably gone from both the read and
//!    the list, leaves every other pair alone, and answers 404 for an ARN naming no
//!    key pair.
//! 4. The two reads are reporting operations: repeating them answers identically.

use std::collections::BTreeSet;

use rcgen::{KeyPair, PKCS_ECDSA_P384_SHA384};
use serde_json::{json, Value};

use super::channel_lifecycle::{arn_is_well_formed, hex_of, text, Ivs};

/// The ARN prefix every playback key pair this emulator mints must carry, from the
/// modelled `PlaybackKeyPairArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:playback-key/[a-zA-Z0-9-]+$` plus the canonical
/// emulator account id.
const PLAYBACK_KEY_ARN_PREFIX: &str = "arn:aws:ivs:us-east-1:000000000000:playback-key/";

/// The `maxResults` ceiling the model declares for this list
/// (`MaxPlaybackKeyPairResults`, `range(min: 1, max: 100)`).
const MAX_RESULTS: usize = 100;

/// The import half of the harness: the list has to have something to report, and
/// `ImportPlaybackKeyPair` is the only thing that writes a key pair.
impl Ivs {
    /// Import a key pair and return the `keyPair` the service answered.
    async fn import(&self, request: Value) -> Value {
        self.ok("ImportPlaybackKeyPair", request).await["keyPair"].clone()
    }

    /// Import a freshly generated key pair and return just its ARN.
    async fn import_arn(&self) -> String {
        text(
            &self
                .import(json!({ "publicKeyMaterial": public_key_pem() }))
                .await,
            "arn",
        )
    }
}

/// A freshly generated ECDSA P-384 public key, in the `PUBLIC KEY` PEM block
/// `openssl ec -pubout` writes.
///
/// Generated rather than hard-coded, so every row imports a real, distinct key — the
/// import refuses a key it already holds, which is what makes "distinct" matter here.
fn public_key_pem() -> String {
    let der = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384)
        .expect("the test key pair is generated")
        .public_key_der();
    pem::encode(&pem::Pem::new("PUBLIC KEY", der))
}

/// Every ARN the list answers, in the order it answered them.
fn listed_arns(page: &Value) -> Vec<String> {
    page["keyPairs"]
        .as_array()
        .expect("keyPairs is a list")
        .iter()
        .map(|summary| text(summary, "arn"))
        .collect()
}

/// Rule 1: what the list answers is the stored record projected onto
/// `PlaybackKeyPairSummary`, for every key pair the account holds.
#[tokio::test]
async fn the_list_answers_every_imported_key_pair_as_a_summary() {
    let ivs = Ivs::new();

    // Rows differ along what the rule generalises over: whether `name` is sent, how
    // long it is, and how many tags accompany the key.
    let cases: Vec<(&str, Value)> = vec![
        ("no name and no tags", json!({})),
        ("a name", json!({ "name": "viewer-token-key" })),
        ("an empty name, which the modelled length 0..128 allows", json!({ "name": "" })),
        (
            "a 128-character name, the modelled upper bound",
            json!({ "name": "k".repeat(128) }),
        ),
        ("one tag", json!({ "name": "tagged", "tags": { "env": "test" } })),
        (
            "three tags and no name",
            json!({ "tags": { "env": "test", "team": "video", "tier": "gold" } }),
        ),
    ];

    // Each import's full record, keyed by ARN, so the list can be compared against
    // what the writer persisted rather than against a literal.
    let mut imported: Vec<(String, Value)> = Vec::new();
    for (case, extra) in &cases {
        let mut request = extra.clone();
        request["publicKeyMaterial"] = json!(public_key_pem());
        let key_pair = ivs.import(request).await;
        let arn = text(&key_pair, "arn");
        assert!(
            arn_is_well_formed(&arn, PLAYBACK_KEY_ARN_PREFIX),
            "case {case:?}: expected a modelled PlaybackKeyPairArn, got {arn:?}"
        );
        imported.push((arn, key_pair));
    }

    let page = ivs.ok("ListPlaybackKeyPairs", json!({})).await;
    let summaries = page["keyPairs"].as_array().expect("keyPairs is a list");
    assert_eq!(
        summaries.len(),
        imported.len(),
        "expected every imported key pair to be listed, got {page}"
    );
    assert!(
        page.get("nextToken").and_then(Value::as_str).is_none(),
        "a single page holding everything must issue no nextToken, got {page}"
    );

    for (arn, key_pair) in &imported {
        let summary = summaries
            .iter()
            .find(|summary| &text(summary, "arn") == arn)
            .unwrap_or_else(|| panic!("case {arn}: the imported key pair must be listed, got {page}"));
        // Exactly the members `PlaybackKeyPairSummary` declares: arn, name and tags —
        // `fingerprint` belongs to `PlaybackKeyPair` and so to the single read.
        assert_eq!(
            summary,
            &json!({
                "arn": arn,
                "name": text(key_pair, "name"),
                "tags": key_pair["tags"].clone(),
            }),
            "case {arn}: the summary must project the stored record, got {summary}"
        );
    }
}

/// Rule 2: every page size from the modelled lower bound up to and past the number of
/// key pairs returns the WHOLE set exactly once, in one order; `maxResults` one past
/// each published bound is refused, and so is a token this service did not issue for
/// THIS list.
#[tokio::test]
async fn the_list_pages_the_whole_set_without_duplicates_or_omissions() {
    let ivs = Ivs::new();
    let mut all = BTreeSet::new();
    for _ in 0..7 {
        all.insert(ivs.import_arn().await);
    }

    let unpaged = ivs.ok("ListPlaybackKeyPairs", json!({})).await;
    let order = listed_arns(&unpaged);
    assert_eq!(
        order,
        all.iter().cloned().collect::<Vec<_>>(),
        "case unpaged: the list must answer every imported key pair in ARN order"
    );

    for page_size in [1_usize, 2, 3, 7, MAX_RESULTS] {
        let mut seen: Vec<String> = Vec::new();
        let mut token = Value::Null;
        for _ in 0..(all.len() + 2) {
            let mut request = json!({ "maxResults": page_size });
            if let Some(token) = token.as_str() {
                request["nextToken"] = json!(token);
            }
            let page = ivs.ok("ListPlaybackKeyPairs", request).await;
            let key_pairs = page["keyPairs"].as_array().expect("keyPairs is a list");
            assert!(
                key_pairs.len() <= page_size,
                "case maxResults={page_size}: a page may hold at most maxResults entries, got {}",
                key_pairs.len()
            );
            seen.extend(listed_arns(&page));
            token = page.get("nextToken").cloned().unwrap_or(Value::Null);
            if token.as_str().is_none() {
                break;
            }
        }
        assert!(
            token.as_str().is_none(),
            "case maxResults={page_size}: paging must terminate, still holding {token}"
        );
        assert_eq!(
            seen, order,
            "case maxResults={page_size}: paging must yield the same key pairs, in the same \
             order, as the unpaged call"
        );
    }

    // One past each published bound of `MaxPlaybackKeyPairResults` (range 1..100).
    for bad in [0, 101] {
        let reply = ivs
            .call("ListPlaybackKeyPairs", &json!({ "maxResults": bad }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults={bad}: outside the modelled range 1..100, expected 400 \
             ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // Tokens this service never issued for this list: a non-hex one, one that decodes
    // to text that is not a boundary it could have issued, and a REAL token it issued
    // for the CHANNEL list.
    ivs.ok("CreateChannel", json!({ "name": "first" })).await;
    ivs.ok("CreateChannel", json!({ "name": "second" })).await;
    let channel_page = ivs.ok("ListChannels", json!({ "maxResults": 1 })).await;
    let channel_token = text(&channel_page, "nextToken");
    assert!(
        !channel_token.is_empty(),
        "the channel list must issue a token to borrow, got {channel_page}"
    );
    let forged = vec![
        ("not hex at all", "zzzz".to_string()),
        ("hex of someone else's text", hex_of("ivs:1:playback-key:nope")),
        (
            "a boundary of the right form but the wrong list",
            hex_of(&format!("ivs:1:channel:{}", order[0])),
        ),
        ("a token issued for the channel list", channel_token),
    ];
    for (case, token) in forged {
        let reply = ivs
            .call("ListPlaybackKeyPairs", &json!({ "nextToken": token }))
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
}

/// Rules 1 and 3 together: a delete makes the pair gone from the read and from the
/// list, leaves every other pair alone, and a second delete of the same ARN is a 404.
#[tokio::test]
async fn delete_makes_the_key_pair_observably_gone() {
    let ivs = Ivs::new();
    let mut arns = Vec::new();
    for _ in 0..5 {
        arns.push(ivs.import_arn().await);
    }

    // Rows: delete the first, the last and then one strictly inside what remains, so
    // the rule is exercised at both ends of the order and in between. Five imports
    // leave three entries by the third row, so its index is a real middle.
    for (case, position) in [("the first", "first"), ("the last", "last"), ("the middle", "middle")]
    {
        let mut remaining: Vec<String> = arns.clone();
        remaining.sort();
        let doomed = match position {
            "first" => remaining.first(),
            "last" => remaining.last(),
            _ => remaining.get(remaining.len() / 2),
        }
        .expect("key pairs remain to delete")
        .clone();
        assert!(
            position != "middle" || (remaining.len() >= 3 && remaining.len() / 2 > 0),
            "case {case}: the middle row must delete an entry strictly inside the order, \
             with {} remaining",
            remaining.len()
        );
        ivs.ok("DeletePlaybackKeyPair", json!({ "arn": doomed.clone() }))
            .await;
        arns.retain(|arn| arn != &doomed);

        let reply = ivs
            .call("GetPlaybackKeyPair", &json!({ "arn": doomed.clone() }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: a deleted key pair must read as 404, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let reply = ivs
            .call("DeletePlaybackKeyPair", &json!({ "arn": doomed.clone() }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: deleting it twice must be a 404, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );

        let listed = listed_arns(&ivs.ok("ListPlaybackKeyPairs", json!({})).await);
        let mut expected = arns.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case}: the list must answer exactly the key pairs that remain"
        );
    }

    // An ARN of the modelled form that names no key pair is a 404; one that cannot be
    // a `PlaybackKeyPairArn` at all is refused as a 400 before any handler runs.
    let unknown = format!("{PLAYBACK_KEY_ARN_PREFIX}abc123def456");
    for operation in ["GetPlaybackKeyPair", "DeletePlaybackKeyPair"] {
        let reply = ivs.call(operation, &json!({ "arn": unknown.clone() })).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {operation} on an unknown ARN: expected 404, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let reply = ivs
            .call(operation, &json!({ "arn": "playback-key/not-an-arn" }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {operation} on a value that is not an ARN: expected 400, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 4: `ListPlaybackKeyPairs` and `GetPlaybackKeyPair` are reporting operations,
/// so repeating them answers identically and changes nothing a later caller sees —
/// including the paged form, which must not consume what it reports.
#[tokio::test]
async fn reads_are_repeatable() {
    let ivs = Ivs::new();
    let arn = text(
        &ivs.import(json!({
            "publicKeyMaterial": public_key_pem(),
            "name": "stable",
            "tags": { "env": "test" },
        }))
        .await,
        "arn",
    );
    ivs.import_arn().await;

    let first_read = ivs
        .ok("GetPlaybackKeyPair", json!({ "arn": arn.clone() }))
        .await;
    let first_list = ivs.ok("ListPlaybackKeyPairs", json!({})).await;
    let first_page = ivs
        .ok("ListPlaybackKeyPairs", json!({ "maxResults": 1 }))
        .await;
    for round in 1..=3 {
        let read = ivs
            .ok("GetPlaybackKeyPair", json!({ "arn": arn.clone() }))
            .await;
        assert_eq!(
            read, first_read,
            "case read round {round}: a read must not change what the next read answers"
        );
        let listed = ivs.ok("ListPlaybackKeyPairs", json!({})).await;
        assert_eq!(
            listed, first_list,
            "case list round {round}: a list must not change what the next list answers"
        );
        let page = ivs
            .ok("ListPlaybackKeyPairs", json!({ "maxResults": 1 }))
            .await;
        assert_eq!(
            page, first_page,
            "case first page round {round}: paging must not consume what it reports"
        );
    }
}
