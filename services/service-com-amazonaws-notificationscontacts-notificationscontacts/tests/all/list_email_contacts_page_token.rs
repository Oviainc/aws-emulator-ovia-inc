// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `nextToken` of `ListEmailContacts`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handler — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a pagination token is an OPAQUE value the listing itself cut,
//! so the walk resumes only from a token this operation actually issued, and every
//! other value — however well formed the position it claims to name — is the modelled
//! 400 `ValidationException` blaming `nextToken`.

use serde_json::Value;
use service_com_amazonaws_notificationscontacts_notificationscontacts::state::CONTACT_ID_LEN;

use crate::email_contacts::{
    create_ok, delete, emulator, error_code, list, unknown_arn, Emulator, ACCOUNT,
};

/// Create `total` contacts and answer their ARNs in the order the listing walks them
/// (ascending ARN, which is the store's key order).
async fn stock(em: &Emulator, total: usize) -> Vec<String> {
    let mut arns = Vec::with_capacity(total);
    for n in 0..total {
        arns.push(
            create_ok(
                em,
                &format!("contact-{n:02}"),
                &format!("contact-{n:02}@example.com"),
            )
            .await,
        );
    }
    arns.sort();
    arns
}

/// One page of the listing: the ARNs it reported and the token it cut, asserting the
/// call succeeded.
async fn page(em: &Emulator, case: &str, query: &str) -> (Vec<String>, Option<String>) {
    let (status, body) = list(em, query).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListEmailContacts must answer 200 for {query:?}, got {status} / {body:?}"
    );
    let arns = body["emailContacts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: emailContacts is @required, got {body:?}"))
        .iter()
        .map(|contact| contact["arn"].as_str().unwrap_or_default().to_string())
        .collect();
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (arns, token)
}

/// Assert `query` is refused as a `nextToken` this operation never issued: the modelled
/// 400 `ValidationException`, blaming the member the value arrived through.
async fn assert_refused(em: &Emulator, case: &str, token: &str) {
    let (status, body) = list(em, &format!("nextToken={token}")).await;
    assert_eq!(
        status, 400,
        "case {case:?}: a token this listing did not issue must be refused with 400, got \
         {status} / {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body:?}"
    );
    let blamed: Vec<&str> = body
        .get("fieldList")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        blamed,
        vec!["nextToken"],
        "case {case:?}: the 400 must blame the member the value arrived through, got {body:?}"
    );
}

/// A well-formed id of this service's email-contact ARNs, built from `index` so a row
/// names a contact no create ever minted.
///
/// The alphabet and the length are the modelled `EmailContactArn` pattern's last
/// segment, `[a-z0-9]{27}`
/// (`smithy_query(query="shape",shape="EmailContactArn")`).
fn never_minted_id(index: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    (0..CONTACT_ID_LEN)
        .map(|position| ALPHABET[(index * 7 + position * 3) % ALPHABET.len()] as char)
        .collect()
}

#[tokio::test]
async fn only_a_next_token_this_listing_issued_resumes_the_walk() {
    let em = emulator();
    let arns = stock(&em, 5).await;

    // One token this listing really cut, so the rows below can be derived from a
    // GENUINE token as well as assembled from nothing.
    let (first, issued) = page(&em, "the first page", "maxResults=2").await;
    assert_eq!(
        first,
        arns[..2].to_vec(),
        "the first page must report the first contacts in the walk order, got {first:?}"
    );
    let issued = issued.expect("two of five contacts leave a page behind, so a token is cut");

    // A token is OPAQUE: it is not a value the same response already carried, so a
    // caller cannot build one out of a response field.
    let last_reported = first.last().expect("the page reported contacts").clone();
    for (case, forgeable) in [
        ("the arn itself", last_reported.clone()),
        ("the hex of the arn", hex::encode(&last_reported)),
    ] {
        assert_ne!(
            issued, forgeable,
            "case {case:?}: an issued token must not be a value the caller can assemble \
             from the page it ended, got {issued:?}"
        );
    }

    // Rows differ along WHERE a value that is not an issued token comes from: text that
    // is not a token at all, hex of a position the caller assembled (a stored contact,
    // one that never existed, another service's resource, an id outside the modelled
    // length), and a GENUINE token with its tag or its position tampered with.
    let bytes = hex::decode(&issued).expect("an issued token is hex");
    // The token carries the position it names plus something the caller cannot compute;
    // that remainder is what the rows below tamper with, so its length is derived from
    // the token rather than assumed.
    let tag_len = bytes
        .len()
        .checked_sub(last_reported.len())
        .filter(|tag_len| *tag_len > 0)
        .unwrap_or_else(|| {
            panic!(
                "an issued token must carry more than the position it names, got {issued:?} \
                 for {last_reported:?}"
            )
        });
    let mut flipped_tag = bytes.clone();
    flipped_tag[0] ^= 0x01;
    let mut zeroed_tag = bytes.clone();
    zeroed_tag[..tag_len].fill(0);
    let mut swapped_position = bytes[..tag_len].to_vec();
    swapped_position.extend_from_slice(arns[3].as_bytes());
    let mut extended = bytes.clone();
    extended.push(b'x');
    let truncated_position = &bytes[..tag_len + 1];
    let tag_only = &bytes[..tag_len];

    let mut rows: Vec<(String, String)> = vec![
        ("not hex at all".to_string(), "not-a-token".to_string()),
        ("an empty value".to_string(), String::new()),
        ("hex of an odd length".to_string(), "abc".to_string()),
        (
            "hex of the arn of a stored contact".to_string(),
            hex::encode(&arns[1]),
        ),
        (
            "hex of a well-formed arn no contact uses".to_string(),
            hex::encode(unknown_arn('a')),
        ),
        (
            "hex of a non-arn".to_string(),
            hex::encode("just-some-text"),
        ),
        (
            "hex of another service's arn".to_string(),
            hex::encode(format!(
                "arn:aws:aiops::{ACCOUNT}:investigation-group/abcdefghij123456"
            )),
        ),
        (
            "hex of an arn whose id is too short".to_string(),
            hex::encode(format!(
                "arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/abc"
            )),
        ),
        (
            "an issued token with its tag flipped".to_string(),
            hex::encode(&flipped_tag),
        ),
        (
            "an issued token with its tag zeroed".to_string(),
            hex::encode(&zeroed_tag),
        ),
        (
            "an issued token whose position was swapped for another contact's".to_string(),
            hex::encode(&swapped_position),
        ),
        (
            "an issued token with a byte appended".to_string(),
            hex::encode(&extended),
        ),
        (
            "an issued token truncated into its position".to_string(),
            hex::encode(truncated_position),
        ),
        (
            "an issued token carrying only its tag".to_string(),
            hex::encode(tag_only),
        ),
    ];
    // And the same class the other way round: positions that satisfy the modelled ARN
    // pattern in every field, drawn across its id alphabet, none of them ever minted.
    for index in 0..8 {
        let id = never_minted_id(index);
        rows.push((
            format!("hex of the never-minted arn ending {id}"),
            hex::encode(format!(
                "arn:aws:notifications-contacts::{ACCOUNT}:emailcontact/{id}"
            )),
        ));
    }

    for (case, token) in &rows {
        assert_refused(&em, case, token).await;
    }

    // The issued token still resumes, and resumes exactly where its page stopped:
    // refusing the rows above costs the genuine token nothing.
    let (resumed, _) = page(
        &em,
        "the issued token resumes",
        &format!("maxResults=2&nextToken={issued}"),
    )
    .await;
    assert_eq!(
        resumed,
        arns[2..4].to_vec(),
        "the issued token must resume strictly after the page that cut it, got {resumed:?}"
    );
}

#[tokio::test]
async fn every_issued_token_of_a_whole_walk_is_accepted_at_every_page_size() {
    // Rows differ along how many contacts the store holds and how large a page the
    // caller asked for: every token the walk cuts must be accepted by the next call,
    // and the walk must report each contact exactly once.
    let cases: Vec<(&str, usize, i32)> = vec![
        ("one contact, page size 1", 1, 1),
        ("four contacts, page size 1", 4, 1),
        ("five contacts, page size 2", 5, 2),
        ("six contacts, page size 3", 6, 3),
        ("seven contacts, page size at the modelled maximum", 7, 100),
    ];

    for (case, total, page_size) in cases {
        let em = emulator();
        let arns = stock(&em, total).await;

        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=total {
            let query = match &token {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let (reported, cut) = page(&em, case, &query).await;
            walked.extend(reported);
            token = cut;
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must terminate, still holding {token:?}"
        );
        assert_eq!(
            walked, arns,
            "case {case:?}: following the issued tokens must report every contact exactly \
             once, in walk order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_issued_resumes_even_after_its_contact_is_deleted() {
    // A token names a POSITION, not a record, so deleting the contact it names does not
    // turn an issued token into one this listing never issued: the next page is still
    // the contacts beyond that position.
    let em = emulator();
    let arns = stock(&em, 4).await;

    let (first, issued) = page(&em, "the first page", "maxResults=1").await;
    assert_eq!(
        first,
        arns[..1].to_vec(),
        "the first page must report the first contact, got {first:?}"
    );
    let issued = issued.expect("one of four contacts leaves a page behind, so a token is cut");

    let (status, body) = delete(&em, &arns[0]).await;
    assert_eq!(
        status, 200,
        "deleting the contact the token names must succeed, got {status} / {body:?}"
    );

    let (resumed, _) = page(
        &em,
        "a token whose contact is gone",
        &format!("nextToken={issued}"),
    )
    .await;
    assert_eq!(
        resumed,
        arns[1..].to_vec(),
        "an issued token must still resume beyond its position, got {resumed:?}"
    );
}
