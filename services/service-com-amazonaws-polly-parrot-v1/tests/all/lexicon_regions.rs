// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that a pronunciation lexicon is a REGIONAL resource:
//! it exists only in the Region whose credential scope the storing call carried, and
//! every read, list, delete and synthesis resolves names in the caller's own Region
//! alone.
//!
//! Each test checks the rule over a TABLE of Regions rather than over the one pair an
//! issue happens to quote: every Region below is both a storing and a reading Region,
//! so the assertions are about the partition, not about a particular Region name.

use service_com_amazonaws_polly_parrot_v1::ops::list_lexicons::LEXICON_PAGE_SIZE;
use service_com_amazonaws_polly_parrot_v1::ops::put_lexicon::{MAX_LEXICONS, MAX_STORED_LEXICONS};

use crate::lexicons_and_speech::{error_code, json, listed_names, pls, Polly};

/// The Regions the tables below span. Four real AWS Regions in three partitions of the
/// globe, including the emulator's own default (`us-east-1`), so a rule that happened
/// to hold only for the default Region cannot pass.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-west-2"];

/// Real AWS Region names, enough of them to fill the whole-store bound at
/// [`MAX_LEXICONS`] per Region and still have one Region left over. Every name is a
/// Region Amazon Polly publishes an endpoint in.
const MANY_REGIONS: [&str; 12] = [
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "eu-west-1",
    "eu-west-2",
    "eu-central-1",
    "ap-northeast-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "sa-east-1",
];

/// The account id every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The ARN a lexicon `name` stored in `region` carries.
fn lexicon_arn(region: &str, name: &str) -> String {
    format!("arn:aws:polly:{region}:{ACCOUNT}:lexicon/{name}")
}

/// RULE: a lexicon belongs to the Region it was stored in — `GetLexicon` and
/// `ListLexicons` report it in that Region and answer `LexiconNotFoundException` /
/// omit it in every other, the same name in two Regions is two independent lexicons,
/// and the `LexiconArn` a read returns names the Region the read was made in.
#[tokio::test]
async fn a_lexicon_is_visible_only_in_the_region_it_was_stored_in() {
    let polly = Polly::new();

    // One lexicon under the SAME name in every Region, each with a document of its
    // own, so a read that crossed the partition would be detectable by its content as
    // well as by its ARN. The language differs per row because that is a stored
    // attribute the reader reports back.
    let languages = ["en-US", "de-DE", "fr-FR", "es-ES"];
    for (index, region) in REGIONS.iter().enumerate() {
        let content = pls("ipa", languages[index], index + 1);
        let put = polly.put_lexicon_in(region, "Shared", &content).await;
        assert_eq!(
            put.status(),
            200,
            "region {region:?}: PutLexicon failed, body {:?}",
            json(&put)
        );
    }
    // A name only one Region ever stores, to pin the absent case.
    let only_content = pls("x-sampa", "en-GB", 2);
    assert_eq!(
        polly
            .put_lexicon_in("eu-west-1", "Only", &only_content)
            .await
            .status(),
        200,
        "PutLexicon of the single-region lexicon failed"
    );

    for (index, region) in REGIONS.iter().enumerate() {
        let expected_content = pls("ipa", languages[index], index + 1);

        let got = polly.get_lexicon_in(region, "Shared").await;
        assert_eq!(
            got.status(),
            200,
            "region {region:?}: GetLexicon of its own lexicon failed, body {:?}",
            json(&got)
        );
        let body = json(&got);
        assert_eq!(
            body["Lexicon"]["Content"].as_str(),
            Some(expected_content.as_str()),
            "region {region:?}: GetLexicon returned another region's document, got {:?}",
            body["Lexicon"]["Content"]
        );
        assert_eq!(
            body["LexiconAttributes"]["LanguageCode"].as_str(),
            Some(languages[index]),
            "region {region:?}: wrong LanguageCode, got {:?}",
            body["LexiconAttributes"]["LanguageCode"]
        );
        assert_eq!(
            body["LexiconAttributes"]["LexiconArn"].as_str(),
            Some(lexicon_arn(region, "Shared").as_str()),
            "region {region:?}: the ARN a read returns must name the reading region, got {:?}",
            body["LexiconAttributes"]["LexiconArn"]
        );

        // The list of this Region is this Region's lexicons and nothing else.
        let listed = polly.list_lexicons_in(region, "").await;
        assert_eq!(
            listed.status(),
            200,
            "region {region:?}: ListLexicons failed, body {:?}",
            json(&listed)
        );
        let body = json(&listed);
        let expected_names: Vec<String> = if *region == "eu-west-1" {
            vec!["Only".to_string(), "Shared".to_string()]
        } else {
            vec!["Shared".to_string()]
        };
        assert_eq!(
            listed_names(&body).into_iter().collect::<Vec<_>>(),
            expected_names,
            "region {region:?}: ListLexicons reported the wrong set"
        );
        for entry in body["Lexicons"].as_array().expect("Lexicons is a list") {
            let name = entry["Name"].as_str().unwrap_or_default();
            assert_eq!(
                entry["Attributes"]["LexiconArn"].as_str(),
                Some(lexicon_arn(region, name).as_str()),
                "region {region:?}, lexicon {name:?}: a listed ARN must name the listing region, \
                 got {:?}",
                entry["Attributes"]["LexiconArn"]
            );
        }

        // The single-region lexicon: found in its own Region, a 404 in every other.
        let cross = polly.get_lexicon_in(region, "Only").await;
        if *region == "eu-west-1" {
            assert_eq!(
                cross.status(),
                200,
                "region {region:?}: GetLexicon of its own lexicon failed, body {:?}",
                json(&cross)
            );
        } else {
            assert_eq!(
                (cross.status().as_u16(), error_code(&cross)),
                (404, "LexiconNotFoundException".to_string()),
                "region {region:?}: a lexicon of another region must not be readable, body {:?}",
                json(&cross)
            );
        }
    }

    // A Region that stored nothing has nothing, and reads 404.
    let empty = polly.list_lexicons_in("sa-east-1", "").await;
    assert!(
        listed_names(&json(&empty)).is_empty(),
        "a region that stored nothing must list nothing, got {:?}",
        json(&empty)
    );
    for name in ["Shared", "Only"] {
        let response = polly.get_lexicon_in("sa-east-1", name).await;
        assert_eq!(
            (response.status().as_u16(), error_code(&response)),
            (404, "LexiconNotFoundException".to_string()),
            "lexicon {name:?}: a region that stored nothing must answer not-found, body {:?}",
            json(&response)
        );
    }
}

/// RULE: a delete reaches only the caller's Region — a name another Region holds is
/// `LexiconNotFoundException` here and stays stored there.
#[tokio::test]
async fn a_delete_reaches_only_the_callers_region() {
    let content = pls("ipa", "en-US", 1);

    // Each row deletes in one Region and then checks every Region: only that one lost
    // its lexicon.
    for deleted_in in REGIONS {
        let polly = Polly::new();
        for region in REGIONS {
            assert_eq!(
                polly.put_lexicon_in(region, "Shared", &content).await.status(),
                200,
                "case {deleted_in:?}: PutLexicon in {region:?} failed"
            );
        }

        let deleted = polly.delete_lexicon_in(deleted_in, "Shared").await;
        assert_eq!(
            deleted.status(),
            200,
            "case {deleted_in:?}: DeleteLexicon in the storing region failed, body {:?}",
            json(&deleted)
        );

        for region in REGIONS {
            let response = polly.get_lexicon_in(region, "Shared").await;
            let expected = if region == deleted_in { 404 } else { 200 };
            assert_eq!(
                response.status().as_u16(),
                expected,
                "case {deleted_in:?}: region {region:?} answered the wrong status after the \
                 delete, body {:?}",
                json(&response)
            );
        }

        // Deleting it again, in its own Region and in a Region that never held it, is
        // the same not-found either way.
        for region in REGIONS {
            let again = polly.delete_lexicon_in(region, "Absent").await;
            assert_eq!(
                (again.status().as_u16(), error_code(&again)),
                (404, "LexiconNotFoundException".to_string()),
                "case {deleted_in:?}: deleting an absent name in {region:?} must be a 404, \
                 body {:?}",
                json(&again)
            );
        }
    }
}

/// RULE: `ListLexicons` pages one Region's lexicons in name order, and a page never
/// carries a lexicon of another Region however many pages it takes.
#[tokio::test]
async fn pagination_stays_inside_the_callers_region() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);

    // Enough in one Region to need several pages, and a handful in each other Region
    // whose names INTERLEAVE with them, so a page that leaked across the partition
    // would show up inside a page rather than only at the end.
    let paged_region = "eu-west-1";
    let total = LEXICON_PAGE_SIZE * 2 + 3;
    for index in 0..total {
        let name = format!("Lex{index:03}");
        assert_eq!(
            polly
                .put_lexicon_in(paged_region, &name, &content)
                .await
                .status(),
            200,
            "PutLexicon {name} in {paged_region} failed"
        );
    }
    for region in REGIONS.iter().filter(|region| **region != paged_region) {
        for index in [0usize, 7, total - 1] {
            let name = format!("Lex{index:03}");
            assert_eq!(
                polly.put_lexicon_in(region, &name, &content).await.status(),
                200,
                "PutLexicon {name} in {region} failed"
            );
        }
    }

    let mut seen: Vec<String> = Vec::new();
    let mut query = String::new();
    let mut pages = 0;
    loop {
        let page = json(&polly.list_lexicons_in(paged_region, &query).await);
        let names: Vec<String> = page["Lexicons"]
            .as_array()
            .expect("Lexicons is a list")
            .iter()
            .map(|entry| entry["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !names.is_empty(),
            "page {pages}: a page reached through a NextToken must not be empty"
        );
        assert!(
            names.len() <= LEXICON_PAGE_SIZE,
            "page {pages}: {} entries is over the page size",
            names.len()
        );
        seen.extend(names);
        pages += 1;
        match page["NextToken"].as_str() {
            Some(token) => query = format!("?NextToken={token}"),
            None => break,
        }
        assert!(pages < 10, "pagination did not terminate");
    }
    let expected: Vec<String> = (0..total).map(|index| format!("Lex{index:03}")).collect();
    assert!(
        pages > 1,
        "{total} lexicons must not fit in one page, got {pages}"
    );
    assert_eq!(
        seen, expected,
        "the pages together must be exactly {paged_region}'s lexicons, in name order"
    );

    // Every other Region still reports its own three, whole, in one page.
    for region in REGIONS.iter().filter(|region| **region != paged_region) {
        let body = json(&polly.list_lexicons_in(region, "").await);
        let names: Vec<String> = listed_names(&body).into_iter().collect();
        let expected: Vec<String> = [0usize, 7, total - 1]
            .iter()
            .map(|index| format!("Lex{index:03}"))
            .collect();
        assert_eq!(
            names, expected,
            "region {region:?}: must report its own lexicons only"
        );
        assert_eq!(
            body["NextToken"].as_str(),
            None,
            "region {region:?}: three lexicons fit in one page, so no NextToken is owed"
        );
    }
}

/// RULE: the per-Region lexicon count quota counts one Region's lexicons, so a Region
/// at the cap refuses a new name there and refuses nothing anywhere else.
#[tokio::test]
async fn the_count_quota_is_counted_per_region() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    let filled = "ap-southeast-2";

    for index in 0..MAX_LEXICONS {
        let name = format!("Lex{index:04}");
        assert_eq!(
            polly.put_lexicon_in(filled, &name, &content).await.status(),
            200,
            "lexicon {index} is within {filled}'s quota"
        );
    }

    let over = polly.put_lexicon_in(filled, "OneTooMany", &content).await;
    assert_eq!(
        (over.status().as_u16(), error_code(&over)),
        (400, "MaxLexiconsNumberExceededException".to_string()),
        "a new name past the region's count quota must be refused, body {:?}",
        json(&over)
    );
    // Replacing a name the Region already holds adds no entry, so the cap is silent.
    assert_eq!(
        polly.put_lexicon_in(filled, "Lex0000", &content).await.status(),
        200,
        "replacing a stored lexicon must still succeed at the count quota"
    );

    // Every other Region is unaffected: its own count is zero.
    for region in REGIONS.iter().filter(|region| **region != filled) {
        let response = polly.put_lexicon_in(region, "OneTooMany", &content).await;
        assert_eq!(
            response.status(),
            200,
            "region {region:?}: a full region must not consume another region's quota, body {:?}",
            json(&response)
        );
        assert_eq!(
            listed_names(&json(&polly.list_lexicons_in(region, "").await))
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["OneTooMany".to_string()],
            "region {region:?}: must hold only what was put in it"
        );
    }
}

/// RULE: the Region a request is signed for is the caller's own choice, so the
/// per-Region count quota alone does not bound the store — the store is also bounded
/// across every Region, and the entries it holds are what that bound counts.
#[tokio::test]
async fn the_store_is_bounded_across_every_region() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);

    let regions_to_fill = MAX_STORED_LEXICONS / MAX_LEXICONS;
    assert_eq!(
        MAX_STORED_LEXICONS % MAX_LEXICONS,
        0,
        "this test fills whole regions, so the store bound must be a multiple of the \
         per-region quota"
    );
    assert!(
        regions_to_fill < MANY_REGIONS.len(),
        "the table needs more than {regions_to_fill} regions to fill the store and one over"
    );

    for region in MANY_REGIONS.iter().take(regions_to_fill) {
        for index in 0..MAX_LEXICONS {
            let name = format!("Lex{index:04}");
            assert_eq!(
                polly.put_lexicon_in(region, &name, &content).await.status(),
                200,
                "region {region:?}, lexicon {name}: within both bounds"
            );
        }
    }

    // A Region holding nothing of its own, so only the whole-store bound can refuse it.
    let spare = MANY_REGIONS[regions_to_fill];
    let refused = polly.put_lexicon_in(spare, "Fresh", &content).await;
    assert_eq!(
        (refused.status().as_u16(), error_code(&refused)),
        (400, "MaxLexiconsNumberExceededException".to_string()),
        "signing for a fresh region must not grow the store past its bound, body {:?}",
        json(&refused)
    );
    // Replacing a name already stored adds no entry, so the bound is silent for it.
    let replaced = polly
        .put_lexicon_in(MANY_REGIONS[0], "Lex0000", &content)
        .await;
    assert_eq!(
        replaced.status(),
        200,
        "replacing a stored lexicon must still succeed at the store bound, body {:?}",
        json(&replaced)
    );
    // And a delete frees a slot, in whichever Region the next put names.
    assert_eq!(
        polly
            .delete_lexicon_in(MANY_REGIONS[0], "Lex0000")
            .await
            .status(),
        200,
        "DeleteLexicon failed"
    );
    assert_eq!(
        polly.put_lexicon_in(spare, "Fresh", &content).await.status(),
        200,
        "a freed slot must be usable by any region"
    );
}

/// RULE: `SynthesizeSpeech` resolves every name in `LexiconNames` in the caller's own
/// Region, so a lexicon of another Region is `LexiconNotFoundException`.
#[tokio::test]
async fn synthesis_resolves_lexicon_names_in_the_callers_region() {
    let polly = Polly::new();
    let content = pls("ipa", "en-US", 1);
    let stored_in = "us-west-2";
    assert_eq!(
        polly
            .put_lexicon_in(stored_in, "Speech", &content)
            .await
            .status(),
        200,
        "PutLexicon failed"
    );

    for region in REGIONS {
        let request = serde_json::json!({
            "Text": "hello there",
            "OutputFormat": "mp3",
            "VoiceId": "Joanna",
            "LexiconNames": ["Speech"],
        });
        let response = polly.synthesize_in(region, &request).await;
        if region == stored_in {
            assert_eq!(
                response.status(),
                200,
                "region {region:?}: synthesis with its own lexicon failed, body {:?}",
                String::from_utf8_lossy(response.body())
            );
        } else {
            assert_eq!(
                (response.status().as_u16(), error_code(&response)),
                (404, "LexiconNotFoundException".to_string()),
                "region {region:?}: a lexicon of another region must not resolve, body {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }
}
