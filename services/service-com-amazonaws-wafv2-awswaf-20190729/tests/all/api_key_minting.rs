// SPDX-License-Identifier: CC0-1.0

//! `CreateAPIKey` and `GetDecryptedAPIKey`, end to end over the wire.
//!
//! `CreateAPIKey` "creates an API key that contains a set of token domains" and
//! answers "the generated, encrypted API key"; `GetDecryptedAPIKey` "returns your API
//! key in decrypted form", so a caller can "check the token domains that you have
//! defined for the key". The RULE these suites assert is therefore one round trip: a
//! create STORES a key the service minted for the scope and token-domain SET the
//! request names, and every later read of that key — its decrypted form and its
//! listing summary — reports that set, in WAF's lexicographic order, and the instant
//! it was created, out of that stored record. A value this service did not
//! mint names no key, and a key of one scope names no key of the other. That a scope
//! holds ONE key per domain set is spanned by `crate::api_key_identity`.
//!
//! Each suite asserts that over a TABLE spanning the class rather than over one
//! request: both scopes, one to the documented maximum of five token domains, the
//! shortest and longest domain the model allows and the punctuation its pattern
//! permits, repeated mints over the same definition, a scope filled to its bound, and
//! each refusal — a malformed key, a well-formed key naming nothing stored, a key read
//! in the wrong scope, a domain list the model refuses and one longer than a single
//! key can serve.
//!
//! Every call goes through the router's front door, the same door an SDK uses; the
//! only direct call into the crate is [`is_api_key_shaped`], used to assert that a
//! case meant to be a WELL-FORMED unknown key really is one.

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::{
    is_api_key_shaped, MAX_API_KEYS_PER_SCOPE, MAX_TOKEN_DOMAINS_PER_API_KEY, SCOPE_CLOUDFRONT,
    SCOPE_REGIONAL,
};

use crate::web_acl_lifecycle::Waf;

/// The operations under test, named once.
const CREATE: &str = "CreateAPIKey";
/// See [`CREATE`].
const DECRYPT: &str = "GetDecryptedAPIKey";
/// The listing a minted key must also show up in.
const LIST: &str = "ListAPIKeys";

/// The longest value `APIKey` allows
/// (`smithy_query(query="shape",shape="APIKey")` → `length 1..2048`).
const MAX_API_KEY_LENGTH: usize = 2048;

/// The bounds `TokenDomain` allows
/// (`smithy_query(query="shape",shape="TokenDomain")` → `length 1..253`, pattern
/// `^[\w\.\-/]+$`).
const MAX_TOKEN_DOMAIN_LENGTH: usize = 253;

/// A value carrying the form a minted key takes, so it is well formed, but that no
/// mint ever answered: an envelope of the smallest size a key is rendered at, with
/// every byte of it zero, so nothing this service sealed can be behind it. Checked
/// against [`is_api_key_shaped`] wherever it is used, so the case cannot silently
/// become a malformed one.
pub(crate) fn well_formed_unknown_key() -> String {
    use base64::Engine as _;

    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0_u8; 128])
}

/// The awsJson error code of a refusal, as its `__type` carries it.
fn error_code(body: &Value) -> &str {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The message of a refusal.
fn error_message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Seconds since the Unix epoch, now: the window a creation instant must fall in.
#[allow(clippy::cast_precision_loss)]
fn now_epoch_seconds() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Mint one key for `scope` over `token_domains`; the call must succeed and answer a
/// key value.
async fn mint(waf: &Waf, scope: &str, token_domains: &[String], case: &str) -> String {
    let answered = waf
        .ok(
            CREATE,
            json!({ "Scope": scope, "TokenDomains": token_domains }),
        )
        .await;
    let api_key = answered["APIKey"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a mint must answer an APIKey, got {answered}")
        })
        .to_string();
    assert!(
        (1..=MAX_API_KEY_LENGTH).contains(&api_key.len())
            && !api_key.chars().any(char::is_whitespace),
        "case {case:?}: the key must satisfy APIKey's own length and pattern, got \
         {api_key:?}"
    );
    assert!(
        is_api_key_shaped(&api_key),
        "case {case:?}: a minted key must have the form this service mints, got \
         {api_key:?}"
    );
    api_key
}

/// Read `api_key` back in decrypted form; the call must succeed.
async fn decrypt(waf: &Waf, scope: &str, api_key: &str) -> Value {
    waf.ok(DECRYPT, json!({ "Scope": scope, "APIKey": api_key }))
        .await
}

/// The `TokenDomains` of a decrypted key or a listing summary, as strings.
fn token_domains(body: &Value, case: &str) -> Vec<String> {
    body["TokenDomains"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected TokenDomains, got {body}"))
        .iter()
        .map(|domain| domain.as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every summary the scope's listing reports, in creation order.
async fn listing(waf: &Waf, scope: &str) -> Vec<Value> {
    let page = waf.ok(LIST, json!({ "Scope": scope })).await;
    page["APIKeySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("scope {scope}: a listing must carry APIKeySummaries, got {page}"))
        .clone()
}

/// The key values the scope's listing reports, in creation order.
async fn listed_keys(waf: &Waf, scope: &str) -> Vec<String> {
    listing(waf, scope)
        .await
        .iter()
        .map(|summary| summary["APIKey"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One mint: what it is called in an assertion, the scope it is minted in and the
/// token domains it defines.
struct MintCase {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The scope the key is minted in.
    scope: &'static str,
    /// The token domains the request names, in the order it names them.
    token_domains: Vec<String>,
}

/// `domains` as the SET they name: sorted lexicographically, which is the order WAF
/// normalizes a key's domains to and therefore the order every read reports.
fn as_set(domains: &[String]) -> Vec<String> {
    let mut set = domains.to_vec();
    set.sort_unstable();
    set.dedup();
    set
}

/// The table the round-trip suites span: both scopes, one domain to the documented
/// maximum of five, the shortest and the longest domain the model allows, and the
/// punctuation `TokenDomain`'s pattern permits.
fn mint_cases() -> Vec<MintCase> {
    let longest_domain = format!(
        "{}.example.com",
        "d".repeat(MAX_TOKEN_DOMAIN_LENGTH - ".example.com".len())
    );
    let five_domains: Vec<String> = (0..MAX_TOKEN_DOMAINS_PER_API_KEY)
        .map(|index| format!("app{index}.example.com"))
        .collect();
    vec![
        MintCase {
            case: "one domain, regional",
            scope: SCOPE_REGIONAL,
            token_domains: vec!["example.com".to_string()],
        },
        MintCase {
            case: "two domains, given out of order",
            scope: SCOPE_REGIONAL,
            token_domains: vec![
                "zeta.example.com".to_string(),
                "alpha.example.com".to_string(),
            ],
        },
        MintCase {
            case: "the punctuation TokenDomain's pattern allows",
            scope: SCOPE_REGIONAL,
            token_domains: vec![
                "sub_domain.example.com".to_string(),
                "hyphen-ated.example.com".to_string(),
                "path/segment".to_string(),
            ],
        },
        MintCase {
            case: "the most domains one key can be used for",
            scope: SCOPE_REGIONAL,
            token_domains: five_domains,
        },
        MintCase {
            case: "the longest domain the model allows, cloudfront",
            scope: SCOPE_CLOUDFRONT,
            token_domains: vec![longest_domain],
        },
        MintCase {
            case: "the shortest domain the model allows, cloudfront",
            scope: SCOPE_CLOUDFRONT,
            token_domains: vec!["a".to_string()],
        },
    ]
}

/// A minted key decrypts to the domain set it was defined with, in WAF's
/// lexicographic order, and to the instant it was created — and the same record is
/// what its listing summary reports.
#[tokio::test]
async fn a_minted_key_decrypts_to_the_domains_it_was_defined_with() {
    let waf = Waf::new();

    for case in mint_cases() {
        let before = now_epoch_seconds();
        let api_key = mint(&waf, case.scope, &case.token_domains, case.case).await;
        let after = now_epoch_seconds();

        let decrypted = decrypt(&waf, case.scope, &api_key).await;
        assert_eq!(
            token_domains(&decrypted, case.case),
            as_set(&case.token_domains),
            "case {:?}: the decrypted key must carry the domain set it was defined \
             with, in lexicographic order, got {decrypted}",
            case.case
        );
        let created_at = decrypted["CreationTimestamp"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {:?}: a decrypted key must carry a CreationTimestamp, got \
                 {decrypted}",
                case.case
            )
        });
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {:?}: the creation instant must be the instant the key was minted \
             ({before}..{after}), got {created_at}",
            case.case
        );

        // The same stored record, read through the listing: one summary for this key,
        // carrying the same domains and the same creation instant.
        let summary = listing(&waf, case.scope)
            .await
            .into_iter()
            .find(|summary| summary["APIKey"].as_str() == Some(api_key.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the minted key must appear in its scope's listing",
                    case.case
                )
            });
        assert_eq!(
            (
                token_domains(&summary, case.case),
                summary["CreationTimestamp"].as_f64()
            ),
            (as_set(&case.token_domains), Some(created_at)),
            "case {:?}: the listing summary and the decrypted key must report one \
             stored record, got {summary}",
            case.case
        );

        // A scope is a namespace: the other scope holds no such key, so the same
        // value decrypts to nothing there.
        let other_scope = if case.scope == SCOPE_REGIONAL {
            SCOPE_CLOUDFRONT
        } else {
            SCOPE_REGIONAL
        };
        waf.refused(
            DECRYPT,
            json!({ "Scope": other_scope, "APIKey": api_key }),
            "WAFNonexistentItemException",
            case.case,
        )
        .await;
    }
}

/// Every call for one definition answers the ONE key that definition has: a key
/// "contains a set of token domains" and is the key of that set, so repeating the
/// mint neither changes the value nor adds a summary, and a different set is a
/// different key.
#[tokio::test]
async fn every_mint_for_one_definition_answers_the_same_key() {
    let waf = Waf::new();
    let domains = vec!["one.example.com".to_string(), "two.example.com".to_string()];
    let case = "three mints over one definition";

    let mut minted: Vec<String> = Vec::new();
    for _ in 0..3 {
        minted.push(mint(&waf, SCOPE_REGIONAL, &domains, case).await);
    }

    let distinct: std::collections::BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        1,
        "case {case:?}: every mint over one definition must answer the one key of that \
         definition, got {minted:?}"
    );
    for api_key in &minted {
        let decrypted = decrypt(&waf, SCOPE_REGIONAL, api_key).await;
        assert_eq!(
            token_domains(&decrypted, case),
            as_set(&domains),
            "case {case:?}: the key must decrypt to the domain set it was defined \
             with, got {decrypted}"
        );
    }
    let listed = listed_keys(&waf, SCOPE_REGIONAL).await;
    assert_eq!(
        listed,
        minted[..1].to_vec(),
        "case {case:?}: the listing must report one summary for the definition"
    );

    // A different definition is a key of its own, listed after the first.
    let other = mint(
        &waf,
        SCOPE_REGIONAL,
        &["three.example.com".to_string()],
        "another definition",
    )
    .await;
    assert_ne!(
        other, minted[0],
        "case {:?}: a different domain set must be a different key",
        "another definition"
    );
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        vec![minted[0].clone(), other],
        "case {:?}: the listing must report one summary per domain set, in creation \
         order",
        "another definition"
    );
}

/// A request naming more token domains than one key can be used for is refused —
/// with WAF's own invalid-parameter refusal naming the token domain — and stores
/// nothing; the documented maximum itself is honoured.
#[tokio::test]
async fn a_mint_refuses_more_domains_than_one_key_can_serve() {
    let waf = Waf::new();
    let domains = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("app{index}.example.com"))
            .collect()
    };

    // Every count up to the documented maximum is honoured, and each key decrypts to
    // exactly the domains it was given.
    for count in 1..=MAX_TOKEN_DOMAINS_PER_API_KEY {
        let case = format!("{count} token domains");
        let wanted = domains(count);
        let api_key = mint(&waf, SCOPE_REGIONAL, &wanted, &case).await;
        let decrypted = decrypt(&waf, SCOPE_REGIONAL, &api_key).await;
        assert_eq!(
            token_domains(&decrypted, &case),
            wanted,
            "case {case:?}: a key on or under the documented maximum must carry every \
             domain it was given, got {decrypted}"
        );
    }

    let stored_before = listed_keys(&waf, SCOPE_REGIONAL).await;
    for count in [
        MAX_TOKEN_DOMAINS_PER_API_KEY + 1,
        MAX_TOKEN_DOMAINS_PER_API_KEY + 4,
    ] {
        let case = format!("{count} token domains");
        let wanted = domains(count);
        let body = waf
            .refused(
                CREATE,
                json!({ "Scope": SCOPE_REGIONAL, "TokenDomains": wanted.clone() }),
                "WAFInvalidParameterException",
                &case,
            )
            .await;
        assert_eq!(
            (body["Field"].as_str(), body["Parameter"].as_str()),
            (
                Some("TOKEN_DOMAIN"),
                wanted.get(MAX_TOKEN_DOMAINS_PER_API_KEY).map(String::as_str)
            ),
            "case {case:?}: the refusal must name the token domain as the field at \
             fault and the first domain past the bound as the parameter, got {body}"
        );
        assert!(
            body["APIKey"].is_null(),
            "case {case:?}: a refused mint must answer no key, got {body}"
        );
    }
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        stored_before,
        "case {:?}: a refused mint must store nothing",
        "more domains than one key can serve"
    );
}

/// A `TokenDomains` list the model refuses is refused by the constraint walk, worded
/// against the list member, and stores nothing.
#[tokio::test]
async fn a_mint_refuses_a_domain_list_the_model_refuses() {
    let waf = Waf::new();
    let stored_before = listed_keys(&waf, SCOPE_REGIONAL).await;

    // Each row fails one of `APIKeyTokenDomains`' constraints: the list's own
    // `@length(min 1)`, then `TokenDomain`'s `@length(1..253)` and its
    // `^[\w\.\-/]+$` pattern.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("no domains at all", vec![]),
        (
            "an empty domain",
            vec!["example.com".to_string(), String::new()],
        ),
        (
            "one character past the longest domain",
            vec!["d".repeat(MAX_TOKEN_DOMAIN_LENGTH + 1)],
        ),
        (
            "a character the pattern excludes",
            vec!["bad:example.com".to_string()],
        ),
        (
            "a domain carrying a space",
            vec!["two words.example.com".to_string()],
        ),
    ];

    for (case, domains) in cases {
        let (status, body) = waf
            .call(
                CREATE,
                json!({ "Scope": SCOPE_REGIONAL, "TokenDomains": domains }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a domain list the model refuses must be refused, got \
             status {status} and body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: a member-constraint failure is the constraint walk's own \
             refusal, got {body}"
        );
        assert!(
            error_message(&body).contains("at 'tokenDomains' failed to satisfy constraint:"),
            "case {case:?}: the clause must name tokenDomains, got {body}"
        );
        assert!(
            body["APIKey"].is_null(),
            "case {case:?}: a refused mint must answer no key, got {body}"
        );
    }

    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        stored_before,
        "case {:?}: a refused mint must store nothing",
        "a domain list the model refuses"
    );
}

/// A value this service did not mint names no key: one that does not decode to a key
/// is the invalid parameter, and a well-formed one the scope does not hold is the
/// nonexistent item.
#[tokio::test]
async fn decrypting_a_key_the_service_did_not_mint_is_refused() {
    let waf = Waf::new();
    // One stored key, so a refusal is about the value asked for and not about an
    // empty store.
    let stored = mint(
        &waf,
        SCOPE_REGIONAL,
        &["stored.example.com".to_string()],
        "a stored key",
    )
    .await;

    // Not the form a minted key takes — the wrong alphabet, a length no envelope is
    // rendered at, an envelope too small to carry a key, or such a value as long as
    // the model allows a key to be — so no key WAF could decode.
    for value in [
        "not-a-key",
        "k",
        "AAAA",
        "abcdef0123456789",
        "key_with_underscores",
        &"0".repeat(63),
        &"k".repeat(MAX_API_KEY_LENGTH - 3),
    ] {
        let case = format!("APIKey={value:.32?}");
        assert!(
            !is_api_key_shaped(value),
            "case {case:?}: this case must be a MALFORMED key, got one with the \
             minted form"
        );
        let body = waf
            .refused(
                DECRYPT,
                json!({ "Scope": SCOPE_REGIONAL, "APIKey": value }),
                "WAFInvalidParameterException",
                &case,
            )
            .await;
        assert_eq!(
            (
                body["Parameter"].as_str(),
                body["Field"].as_str(),
                body["Reason"].as_str()
            ),
            (
                Some(value),
                None,
                Some("The parameter contains formatting that is not valid.")
            ),
            "case {case:?}: the refusal must name the value it could not decode and \
             no field, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Error reason: The parameter contains formatting that is not valid., \
                 field: null, parameter: {value}"
            ),
            "case {case:?}: the refusal must read the way WAF words it, got {body}"
        );
        assert!(
            body["TokenDomains"].is_null(),
            "case {case:?}: a refusal must carry no decrypted key, got {body}"
        );
    }

    // Well formed, and no key: the store holds nothing under it, in either scope.
    let unknown = well_formed_unknown_key();
    assert!(
        is_api_key_shaped(&unknown),
        "the unknown-key case must be WELL FORMED, got {unknown:?}"
    );
    for scope in [SCOPE_REGIONAL, SCOPE_CLOUDFRONT] {
        let case = format!("a well-formed unknown key in {scope}");
        let body = waf
            .refused(
                DECRYPT,
                json!({ "Scope": scope, "APIKey": unknown }),
                "WAFNonexistentItemException",
                &case,
            )
            .await;
        assert!(
            body["TokenDomains"].is_null(),
            "case {case:?}: a refusal must carry no decrypted key, got {body}"
        );
    }

    // The stored key still decrypts: no refusal above touched it.
    let decrypted = decrypt(&waf, SCOPE_REGIONAL, &stored).await;
    assert_eq!(
        token_domains(&decrypted, "a stored key"),
        ["stored.example.com".to_string()],
        "case {:?}: a refused read must leave the stored key alone, got {decrypted}",
        "a stored key"
    );
}

/// Decrypting a key REPORTS: repeated reads answer the same record, and the store
/// holds exactly what it held before.
#[tokio::test]
async fn decrypting_a_key_changes_nothing_a_later_caller_sees() {
    let waf = Waf::new();
    let case = "repeated decryption";
    let api_key = mint(
        &waf,
        SCOPE_REGIONAL,
        &["read.example.com".to_string(), "again.example.com".to_string()],
        case,
    )
    .await;
    let before = listing(&waf, SCOPE_REGIONAL).await;

    let first = decrypt(&waf, SCOPE_REGIONAL, &api_key).await;
    // A refused read in between, which must not consume the key either.
    waf.refused(
        DECRYPT,
        json!({ "Scope": SCOPE_CLOUDFRONT, "APIKey": api_key }),
        "WAFNonexistentItemException",
        case,
    )
    .await;
    let second = decrypt(&waf, SCOPE_REGIONAL, &api_key).await;

    assert_eq!(
        first, second,
        "case {case:?}: a decryption must answer the same record every time"
    );
    assert_eq!(
        listing(&waf, SCOPE_REGIONAL).await,
        before,
        "case {case:?}: a decryption must not change the stored keys"
    );
}

/// A scope holds at most the keys its bound allows: the mint that would pass the
/// bound is refused as the quota failure, every key already minted still decrypts,
/// and the other scope is a namespace of its own.
#[tokio::test]
async fn a_mint_past_the_per_scope_bound_is_refused() {
    let waf = Waf::new();
    let case = "a scope filled to its bound";
    // One domain set per key, because a scope holds ONE key per set: filling the
    // bound takes that many distinct definitions.
    let definition = |index: usize| vec![format!("bounded{index}.example.com")];

    let mut minted: Vec<String> = Vec::new();
    for index in 0..MAX_API_KEYS_PER_SCOPE {
        minted.push(mint(&waf, SCOPE_REGIONAL, &definition(index), case).await);
    }

    let past_the_bound = definition(MAX_API_KEYS_PER_SCOPE);
    let body = waf
        .refused(
            CREATE,
            json!({ "Scope": SCOPE_REGIONAL, "TokenDomains": &past_the_bound }),
            "WAFLimitsExceededException",
            case,
        )
        .await;
    assert!(
        body["APIKey"].is_null(),
        "case {case:?}: a refused mint must answer no key, got {body}"
    );
    assert_eq!(
        listed_keys(&waf, SCOPE_REGIONAL).await,
        minted,
        "case {case:?}: the scope must hold exactly the {MAX_API_KEYS_PER_SCOPE} keys \
         it minted"
    );
    for (index, api_key) in minted.iter().enumerate() {
        let decrypted = decrypt(&waf, SCOPE_REGIONAL, api_key).await;
        assert_eq!(
            token_domains(&decrypted, case),
            definition(index),
            "case {case:?}: every key the scope holds must still decrypt, got \
             {decrypted}"
        );
    }
    // A mint for a set the scope ALREADY holds a key for is not a new key, so it is
    // not refused by the bound either.
    let held = mint(&waf, SCOPE_REGIONAL, &definition(0), case).await;
    assert_eq!(
        held, minted[0],
        "case {case:?}: a mint for a held definition must answer that key even at the \
         bound"
    );

    // The other scope still mints: the bound is per scope, because a scope is a
    // namespace.
    let elsewhere = mint(&waf, SCOPE_CLOUDFRONT, &past_the_bound, "the other scope").await;
    assert_eq!(
        listed_keys(&waf, SCOPE_CLOUDFRONT).await,
        [elsewhere],
        "case {:?}: a scope filled to its bound must not bound the other scope",
        "the other scope"
    );
}

/// A `Scope` that is not one of the two the model declares is refused by the request
/// validator on both operations, and nothing comes back with it.
#[tokio::test]
async fn an_unmodelled_scope_is_refused_on_both_operations() {
    let waf = Waf::new();
    let api_key = mint(
        &waf,
        SCOPE_REGIONAL,
        &["scoped.example.com".to_string()],
        "a stored key",
    )
    .await;

    // `Scope` is `CLOUDFRONT | REGIONAL`
    // (`smithy_query(query="shape",shape="Scope")`); everything else names no
    // namespace, including the right value in the wrong case.
    for scope in ["", "regional", "GLOBAL", "CLOUDFRONT REGIONAL"] {
        for (operation, request) in [
            (
                CREATE,
                json!({ "Scope": scope, "TokenDomains": ["example.com"] }),
            ),
            (DECRYPT, json!({ "Scope": scope, "APIKey": api_key })),
        ] {
            let case = format!("{operation} with Scope={scope:?}");
            let (status, body) = waf.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: an unmodelled scope must be refused, got status \
                 {status} and body {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: a member-constraint failure is the request validator's \
                 own refusal, got {body}"
            );
            assert!(
                error_message(&body).contains("at 'scope' failed to satisfy constraint:"),
                "case {case:?}: the clause must name scope, got {body}"
            );
            assert!(
                body["APIKey"].is_null() && body["TokenDomains"].is_null(),
                "case {case:?}: a refusal must carry neither a key nor its domains, \
                 got {body}"
            );
        }
    }
}
