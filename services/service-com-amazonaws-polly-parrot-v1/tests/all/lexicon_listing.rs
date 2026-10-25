// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that `ListLexicons` is not a paged read: one call
//! reports the whole lexicon list of the Region it was signed for, in name order, and
//! returns no `NextToken` (the model declares no page-size input for it and does not
//! mark it `@paginated`).
//!
//! The rule is checked over a TABLE of store sizes rather than the one size an issue
//! happens to quote: empty, one, several sizes either side of any page a reader might
//! expect, and the per-Region count quota itself — the largest list a Region can hold,
//! so the table spans every store size the operation can be asked about. Each row also
//! lists in a Region of its own, so a rule that held only for the default Region could
//! not pass.

use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::MAX_LEXICONS;

use crate::lexicons_and_speech::{error_code, json, listed_names_in_order, pls, Polly};

/// The store sizes and the Region each is stored in. `MAX_LEXICONS` is the per-Region
/// count quota declared in `src/ops/put_lexicon.rs`, so the last row is the largest
/// lexicon list a Region can ever hold.
const STORES: [(&str, usize); 7] = [
    ("us-east-1", 0),
    ("eu-west-1", 1),
    ("ap-southeast-2", 2),
    ("us-west-2", 19),
    ("eu-central-1", 20),
    ("ca-central-1", 25),
    ("sa-east-1", MAX_LEXICONS),
];

/// The name of the `index`th stored lexicon. Zero-padded so the name order and the
/// storing order are the same, which is what lets the assertions compare them.
fn lexicon_name(index: usize) -> String {
    format!("Lex{index:03}")
}

/// RULE: one `ListLexicons` call reports the Region's whole lexicon list and no
/// `NextToken`, whatever the store holds.
#[tokio::test]
async fn one_call_reports_every_lexicon_of_the_region() {
    let content = pls("ipa", "en-US", 1);

    for (region, size) in STORES {
        let case = format!("{region} with {size} lexicons");
        let polly = Polly::new();
        for index in 0..size {
            let name = lexicon_name(index);
            let put = polly.put_lexicon_in(region, &name, &content).await;
            assert_eq!(
                put.status(),
                200,
                "case {case:?}: PutLexicon {name} failed, body {:?}",
                json(&put)
            );
        }

        let response = polly.list_lexicons_in(region, "").await;
        assert_eq!(
            response.status(),
            200,
            "case {case:?}: ListLexicons failed, body {:?}",
            json(&response)
        );
        let body = json(&response);
        let expected: Vec<String> = (0..size).map(lexicon_name).collect();
        let listed = listed_names_in_order(&body);
        assert_eq!(
            listed, expected,
            "case {case:?}: one call must report every lexicon in name order, got {} entries",
            listed.len()
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "case {case:?}: the whole list fits in the response, so no NextToken is owed, \
             got {:?}",
            body["NextToken"]
        );

        // The listing must not contradict the reads: the lexicon a truncated list
        // would have dropped is both reported above and readable here.
        if size > 0 {
            let last = lexicon_name(size - 1);
            let got = polly.get_lexicon_in(region, &last).await;
            assert_eq!(
                got.status(),
                200,
                "case {case:?}: GetLexicon {last} must agree with the list, body {:?}",
                json(&got)
            );
        }
    }
}

/// RULE: the attributes a listed description carries are the stored lexicon's own, for
/// every entry of the list and not only the first few.
#[tokio::test]
async fn every_listed_description_carries_its_own_attributes() {
    let polly = Polly::new();
    // Documents that differ in alphabet, language and lexeme count, cycled over a
    // store large enough that a truncated list would miss most of them.
    let documents: [(&str, &str, usize); 3] = [
        ("ipa", "en-US", 1),
        ("x-sampa", "de-DE", 2),
        ("ipa", "fr-FR", 3),
    ];
    let size = 30;
    for index in 0..size {
        let (alphabet, language, lexemes) = documents[index % documents.len()];
        let name = lexicon_name(index);
        assert_eq!(
            polly
                .put_lexicon(&name, &pls(alphabet, language, lexemes))
                .await
                .status(),
            200,
            "PutLexicon {name} failed"
        );
    }

    let body = json(&polly.list_lexicons("").await);
    let entries = body["Lexicons"].as_array().expect("Lexicons is a list");
    assert_eq!(
        entries.len(),
        size,
        "all {size} lexicons must be reported in one response"
    );
    for (index, entry) in entries.iter().enumerate() {
        let (alphabet, language, lexemes) = documents[index % documents.len()];
        let name = lexicon_name(index);
        let attributes = &entry["Attributes"];
        assert_eq!(
            entry["Name"].as_str(),
            Some(name.as_str()),
            "entry {index}: wrong name, got {:?}",
            entry["Name"]
        );
        assert_eq!(
            attributes["Alphabet"].as_str(),
            Some(alphabet),
            "lexicon {name}: wrong Alphabet, got {:?}",
            attributes["Alphabet"]
        );
        assert_eq!(
            attributes["LanguageCode"].as_str(),
            Some(language),
            "lexicon {name}: wrong LanguageCode, got {:?}",
            attributes["LanguageCode"]
        );
        assert_eq!(
            attributes["LexemesCount"].as_u64(),
            Some(lexemes as u64),
            "lexicon {name}: wrong LexemesCount, got {:?}",
            attributes["LexemesCount"]
        );
    }
}

/// RULE: a `NextToken` this service could not have minted is
/// `InvalidNextTokenException` (HTTP 400), and a well-formed one resumes after the
/// name it carries — still in one untruncated response.
#[tokio::test]
async fn a_next_token_is_checked_and_never_needed() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    let size = 25;
    for index in 0..size {
        assert_eq!(
            polly.put_lexicon(&lexicon_name(index), &content).await.status(),
            200,
            "PutLexicon {} failed",
            lexicon_name(index)
        );
    }

    // Not hex at all, odd-length hex, and hex of bytes that are not UTF-8: none of
    // them is a token this service could have issued.
    for bad in ["not-a-token", "zzzz", "abc", "ff", "fffe"] {
        let response = polly.list_lexicons(&format!("?NextToken={bad}")).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (400, "InvalidNextTokenException".to_string()),
            "token {bad:?}: expected a refusal, body {:?}",
            json(&response)
        );
    }

    // A well-formed cursor: the rest of the list, whole, with no token of its own.
    for after in [0usize, 7, size - 1] {
        let token = hex::encode(lexicon_name(after));
        let body = json(&polly.list_lexicons(&format!("?NextToken={token}")).await);
        let expected: Vec<String> = (after + 1..size).map(lexicon_name).collect();
        assert_eq!(
            listed_names_in_order(&body),
            expected,
            "cursor after {after}: the remainder must be reported whole"
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "cursor after {after}: the response is never truncated, so no NextToken is owed, \
             got {:?}",
            body["NextToken"]
        );
    }
}
