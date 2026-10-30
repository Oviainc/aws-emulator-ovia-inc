// SPDX-License-Identifier: CC0-1.0

//! The value rule a configuration `Name` carries, driven over the wire.
//!
//! RULE: a `CreateConfiguration` whose `Name` falls outside the character set or the
//! length the member documents is refused on the input alone — HTTP 400,
//! `x-amzn-errortype: BadRequestException`, `errorAttribute` naming `name` — and
//! nothing is created; a `Name` inside both is not refused, whatever the rest of the
//! operation can do yet. `Name` is the only caller-chosen configuration name this
//! service's model carries (`UpdateConfiguration` takes `ConfigurationId`, `Data` and
//! `Description` only), so this is the whole surface the rule covers.
//!
//! The documented constraint is the member's own: "This value can contain only
//! alphanumeric characters, dashes, periods, underscores, and tildes (- . _ ~). This
//! value must be 1-150 characters long"
//! (`smithy_query(query="doc", shape="CreateConfigurationRequest", member="Name")`),
//! and the bounds the rows use are the crate's constants for that sentence rather than
//! numbers written out here.
//!
//! The tables span what the rule generalises over: for the character set, one name per
//! character class the sentence allows and names carrying a character from outside it
//! (punctuation, a path separator, a colon, a quote, whitespace in several places and a
//! letter outside ASCII); for the length, each published bound and one past each.

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::ENGINE_TYPE_VALUES;
use service_com_amazonaws_mq_mq::state::{
    MAX_CONFIGURATION_NAME_LEN, MIN_CONFIGURATION_NAME_LEN,
};

use crate::broker_lifecycle::{string_at, Mq};

/// Amazon MQ's whole sentence for a configuration name outside the allowed character
/// set: fixed text that interpolates no value from the request.
const INVALID_CHARACTERS: &str = "Make sure your Configuration name only contains valid characters.";

/// A `CreateConfiguration` body carrying `name`: the two members the model requires.
pub(crate) fn configuration_body(name: &str) -> Value {
    json!({"name": name, "engineType": ENGINE_TYPE_VALUES[0]})
}

impl Mq {
    pub(crate) async fn create_configuration(&self, name: &str) -> (u16, String, Value) {
        self.call_with_code("POST", "/v1/configurations", Some(configuration_body(name)))
            .await
    }

    pub(crate) async fn describe_configuration(&self, id: &str) -> (u16, String, Value) {
        self.call_with_code("GET", &format!("/v1/configurations/{id}"), None)
            .await
    }
}

/// Characters from OUTSIDE the documented set, one per form a caller plausibly sends.
const CHARACTERS_OUTSIDE_THE_SET: &[(&str, char)] = &[
    ("space", ' '),
    ("exclamation mark", '!'),
    ("path separator", '/'),
    ("colon", ':'),
    ("comma", ','),
    ("plus", '+'),
    ("at sign", '@'),
    ("asterisk", '*'),
    ("parenthesis", '('),
    ("quote", '"'),
    ("tab", '\t'),
    ("non-ASCII letter", 'é'),
];

/// RULE, refusal half (character set): a name carrying any character outside the
/// documented set is refused naming the member, wherever that character sits.
#[tokio::test]
async fn a_configuration_name_outside_the_allowed_characters_is_refused() {
    let mq = Mq::new("us-east-1");
    let mut rows = 0usize;

    for (what, character) in CHARACTERS_OUTSIDE_THE_SET {
        // The same character leading, trailing and inside an otherwise legal name:
        // the rule is about the character, not where it sits.
        for name in [
            format!("{character}leading"),
            format!("trailing{character}"),
            format!("in{character}side"),
        ] {
            let case = format!("{what} in {name:?}");
            let (status, code, answer) = mq.create_configuration(&name).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {answer:?}"
            );
            assert_eq!(
                code, "BadRequestException",
                "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
            );
            assert_eq!(
                answer.pointer("/errorAttribute").and_then(Value::as_str),
                Some("name"),
                "case {case:?}: expected errorAttribute \"name\", got {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/message"),
                INVALID_CHARACTERS,
                "case {case:?}: expected Amazon MQ's sentence for this failure, got {answer:?}"
            );
            rows += 1;
        }
    }

    assert_eq!(
        rows,
        3 * CHARACTERS_OUTSIDE_THE_SET.len(),
        "every character outside the set must be exercised in every position, covered {rows}"
    );
}

/// RULE, refusal half (length): a name shorter or longer than the published bounds is
/// refused naming the member, with the sentence this service states a length refusal
/// with.
#[tokio::test]
async fn a_configuration_name_outside_the_published_length_is_refused() {
    let mq = Mq::new("us-east-1");

    // One past each published bound, and further outside each.
    for length in [
        MIN_CONFIGURATION_NAME_LEN - 1,
        MAX_CONFIGURATION_NAME_LEN + 1,
        MAX_CONFIGURATION_NAME_LEN * 2,
    ] {
        let case = format!("a name of {length} characters");
        let name = "n".repeat(length);
        let (status, code, answer) = mq.create_configuration(&name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("name"),
            "case {case:?}: expected errorAttribute \"name\", got {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            format!(
                "name must be {MIN_CONFIGURATION_NAME_LEN}-{MAX_CONFIGURATION_NAME_LEN} \
                 characters long, got {length} characters"
            ),
            "case {case:?}: the sentence must state the bounds and the length sent, \
             got {answer:?}"
        );
    }
}

/// RULE, accepted half: a name inside the character set and the length bounds is served
/// and the configuration it names reads back carrying that exact name.
#[tokio::test]
async fn a_configuration_name_inside_the_rule_is_served_and_reads_back() {
    let mq = Mq::new("us-east-1");

    // One name per character class the documented sentence allows, plus both
    // published length bounds.
    let names = vec![
        "a".to_string(),
        "Alphanumeric123".to_string(),
        "with-dash".to_string(),
        "with.period".to_string(),
        "with_underscore".to_string(),
        "with~tilde".to_string(),
        "aA0-._~".to_string(),
        "n".repeat(MIN_CONFIGURATION_NAME_LEN),
        "n".repeat(MAX_CONFIGURATION_NAME_LEN),
    ];

    for name in names {
        let case = format!("name {name:?}");
        let (status, code, created) = mq.create_configuration(&name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a name inside the rule must be served, got {status} {code:?} \
             {created:?}"
        );
        assert_eq!(
            string_at(&created, "/name"),
            name,
            "case {case:?}: expected the name sent back, got {created:?}"
        );

        let id = string_at(&created, "/id");
        let (status, code, described) = mq.describe_configuration(&id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the configuration to read back, got {status} {code:?} \
             {described:?}"
        );
        assert_eq!(
            string_at(&described, "/name"),
            name,
            "case {case:?}: the stored name must be the one sent, got {described:?}"
        );
    }
}

/// A refused name stores nothing: the configuration a rejected request named appears in
/// no listing afterwards, while the one a served request named does.
#[tokio::test]
async fn a_refused_name_leaves_no_configuration_behind() {
    let mq = Mq::new("us-east-1");

    let refused = "refused name!";
    let (status, code, answer) = mq.create_configuration(refused).await;
    assert_eq!(status, 400, "expected 400, got {status} {code:?} {answer:?}");

    let served = "served-name";
    let (status, code, created) = mq.create_configuration(served).await;
    assert_eq!(status, 200, "expected 200, got {status} {code:?} {created:?}");

    let (status, _code, listed) = mq
        .call_with_code("GET", "/v1/configurations?maxResults=100", None)
        .await;
    assert_eq!(status, 200, "expected 200, got {status} {listed:?}");
    let names: Vec<String> = listed
        .pointer("/configurations")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| string_at(entry, "/name"))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![served.to_string()],
        "only the served name may appear in the listing, got {listed:?}"
    );
}
