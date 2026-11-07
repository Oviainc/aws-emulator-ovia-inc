// SPDX-License-Identifier: CC0-1.0

//! The FORM an `APIKey` value takes, end to end over the wire.
//!
//! `CreateAPIKey` "creates an API key that contains a set of token domains" and
//! answers "the generated, encrypted API key", and `GetDecryptedAPIKey` "returns
//! your API key in decrypted form" so a caller "can check the token domains that you
//! have defined for the key". The RULE asserted here is what that makes the value:
//! an opaque token that CARRIES the key's definition — not a digest of it, which
//! nothing could be read back out of. Concretely, for every definition a key can be
//! minted over:
//!
//! * the value satisfies `APIKey`'s own modelled form (`length 1..2048`,
//!   `pattern \S`) and the form this service mints a key in;
//! * it is longer for a definition that carries more, so the definition travels
//!   inside the key rather than being hashed to a fixed width;
//! * it decrypts back to exactly that definition, through `GetDecryptedAPIKey`;
//! * two different definitions are two different keys, and no two mints answer one
//!   value by accident.
//!
//! Each suite asserts that over a TABLE spanning the class: both scopes, one to the
//! documented maximum of five token domains, the shortest and the longest domain the
//! model allows, and the punctuation `TokenDomain`'s pattern permits.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    is_api_key_shaped, MAX_TOKEN_DOMAINS_PER_API_KEY, SCOPE_CLOUDFRONT, SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::Waf;

/// The bounds `APIKey` allows (`smithy_query(query="shape",shape="APIKey")` →
/// `length 1..2048`, `pattern \S`).
const MAX_API_KEY_LENGTH: usize = 2048;

/// The longest value `TokenDomain` allows
/// (`smithy_query(query="shape",shape="TokenDomain")` → `length 1..253`, pattern
/// `^[\w\.\-/]+$`).
const MAX_TOKEN_DOMAIN_LENGTH: usize = 253;

/// One definition a key is minted over.
struct Definition {
    /// What the row is called in an assertion.
    case: &'static str,
    /// The scope the key is defined in.
    scope: &'static str,
    /// The token domains the key is defined over.
    domains: Vec<String>,
}

/// The table: every way a definition can differ — how many domains it names, how
/// long each is, which punctuation they use, and which scope the key lives in.
fn definitions() -> Vec<Definition> {
    let longest = format!(
        "{}.example.com",
        "d".repeat(MAX_TOKEN_DOMAIN_LENGTH - ".example.com".len())
    );
    vec![
        Definition {
            case: "one domain, REGIONAL",
            scope: SCOPE_REGIONAL,
            domains: vec!["example.com".to_string()],
        },
        Definition {
            case: "one domain, CLOUDFRONT",
            scope: SCOPE_CLOUDFRONT,
            domains: vec!["example.com".to_string()],
        },
        Definition {
            case: "the shortest domain the model allows",
            scope: SCOPE_REGIONAL,
            domains: vec!["a".to_string()],
        },
        Definition {
            case: "the longest domain the model allows",
            scope: SCOPE_REGIONAL,
            domains: vec![longest.clone()],
        },
        Definition {
            case: "two domains",
            scope: SCOPE_REGIONAL,
            domains: vec!["one.example.com".to_string(), "two.example.com".to_string()],
        },
        Definition {
            case: "the punctuation the domain pattern permits",
            scope: SCOPE_REGIONAL,
            domains: vec!["a_b-c.example.com/path".to_string()],
        },
        Definition {
            case: "the documented maximum of five domains",
            scope: SCOPE_CLOUDFRONT,
            domains: (0..MAX_TOKEN_DOMAINS_PER_API_KEY)
                .map(|index| format!("host{index}.example.com"))
                .collect(),
        },
        Definition {
            case: "five domains, each as long as the model allows",
            scope: SCOPE_REGIONAL,
            domains: (0..MAX_TOKEN_DOMAINS_PER_API_KEY)
                .map(|index| format!("{index}{}", &longest[1..]))
                .collect(),
        },
    ]
}

/// Mint one key; the call must succeed and answer a key value.
async fn mint(waf: &Waf, definition: &Definition) -> String {
    let answered = waf
        .ok(
            "CreateAPIKey",
            json!({ "Scope": definition.scope, "TokenDomains": definition.domains }),
        )
        .await;
    answered["APIKey"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: a mint must answer an APIKey, got {answered}",
                definition.case
            )
        })
        .to_string()
}

/// The token domains `GetDecryptedAPIKey` reports for `api_key`.
async fn decrypted_domains(waf: &Waf, scope: &str, api_key: &str) -> Vec<String> {
    let decrypted: Value = waf
        .ok(
            "GetDecryptedAPIKey",
            json!({ "Scope": scope, "APIKey": api_key }),
        )
        .await;
    decrypted["TokenDomains"]
        .as_array()
        .expect("GetDecryptedAPIKey must report TokenDomains")
        .iter()
        .map(|domain| domain.as_str().unwrap_or_default().to_string())
        .collect()
}

/// How many bytes a key's value carries, decoded: the size of the token itself
/// rather than of its rendering.
fn carried_bytes(api_key: &str) -> usize {
    use base64::Engine as _;

    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(api_key)
        .unwrap_or_else(|error| {
            panic!("a key must be a decodable token, got {api_key:?} ({error})")
        })
        .len()
}

/// Every minted key satisfies `APIKey`'s modelled form and the form this service
/// mints a key in, whatever definition it was minted over.
#[tokio::test]
async fn a_minted_key_has_the_modelled_form() {
    let waf = Waf::new();
    for definition in definitions() {
        let case = definition.case;
        let api_key = mint(&waf, &definition).await;
        assert!(
            (1..=MAX_API_KEY_LENGTH).contains(&api_key.len()),
            "case {case:?}: the key must satisfy APIKey's length 1..{MAX_API_KEY_LENGTH}, \
             got {} characters",
            api_key.len()
        );
        assert!(
            !api_key.chars().any(char::is_whitespace),
            "case {case:?}: the key must satisfy APIKey's pattern, got {api_key:?}"
        );
        assert!(
            is_api_key_shaped(&api_key),
            "case {case:?}: the key must carry the form this service mints, got \
             {api_key:?}"
        );
    }
}

/// The key CARRIES its definition: a definition with more in it is a longer key, by
/// exactly as much as the definition grew, so the domains travel inside the token
/// instead of being hashed to a fixed width.
///
/// A digest would answer one width for every definition; this asserts the opposite —
/// the difference between two keys' carried sizes is the difference between the
/// definitions they carry.
#[tokio::test]
async fn a_key_grows_with_the_definition_it_carries() {
    let waf = Waf::new();
    // One domain at a time, each longer than the last: the growth of the key must
    // track the growth of the definition exactly.
    let mut previous: Option<(usize, usize)> = None;
    for width in [1_usize, 2, 17, 64, MAX_TOKEN_DOMAIN_LENGTH] {
        let case = format!("a single domain of {width} characters");
        let domain = "d".repeat(width);
        let api_key = mint(
            &waf,
            &Definition {
                case: "growth",
                scope: SCOPE_REGIONAL,
                domains: vec![domain],
            },
        )
        .await;
        let carried = carried_bytes(&api_key);
        if let Some((previous_width, previous_carried)) = previous {
            assert_eq!(
                carried - previous_carried,
                width - previous_width,
                "case {case:?}: the key must grow by exactly what the definition \
                 grew by, got {carried} bytes against {previous_carried}"
            );
        }
        previous = Some((width, carried));
    }

    // And one more domain is one more domain's worth of key, not a constant.
    let mut previous: Option<(usize, usize)> = None;
    for count in 1..=MAX_TOKEN_DOMAINS_PER_API_KEY {
        let case = format!("{count} domains");
        let domains: Vec<String> = (0..count)
            .map(|index| format!("host{index}.example.com"))
            .collect();
        let spelled: usize = domains.iter().map(String::len).sum();
        let api_key = mint(
            &waf,
            &Definition {
                case: "growth",
                scope: SCOPE_REGIONAL,
                domains,
            },
        )
        .await;
        let carried = carried_bytes(&api_key);
        if let Some((previous_spelled, previous_carried)) = previous {
            assert!(
                carried - previous_carried >= spelled - previous_spelled,
                "case {case:?}: a key over more domains must carry at least those \
                 domains' own bytes more, got {carried} bytes against \
                 {previous_carried}"
            );
        }
        previous = Some((spelled, carried));
    }
}

/// A key decrypts back to exactly the definition it was minted over — the set of
/// token domains, in the one order a key stores them in — and no two definitions
/// share a key.
#[tokio::test]
async fn a_key_decrypts_back_to_its_own_definition() {
    let waf = Waf::new();
    let mut minted: Vec<String> = Vec::new();
    for definition in definitions() {
        let case = definition.case;
        let api_key = mint(&waf, &definition).await;
        let mut expected = definition.domains.clone();
        expected.sort();
        expected.dedup();
        let reported = decrypted_domains(&waf, definition.scope, &api_key).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the key must decrypt to the domains it was defined \
             over, got {reported:?}"
        );
        assert!(
            !minted.contains(&api_key),
            "case {case:?}: two definitions must not share a key, got {api_key:?}"
        );
        minted.push(api_key);
    }
}
