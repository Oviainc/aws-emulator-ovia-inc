// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRandomPassword`, driven over the wire through the
//! router.
//!
//! The rules under test:
//!
//! * a password is drawn from the character types the request leaves INCLUDED —
//!   uppercase and lowercase letters, numbers and punctuation unless the matching
//!   `Exclude…` switch turns one off, plus the space only when `IncludeSpace` asks
//!   for it — minus every character `ExcludeCharacters` names, and it is exactly
//!   `PasswordLength` characters long (32 when the request names no length);
//! * a request that requires each included type gets at least one character of
//!   every type it includes, and a request that switches that requirement off is
//!   bound only by the available characters;
//! * a request that needs a character of a type it has left without any is refused
//!   with `InvalidParameterException`, as is one whose length falls short of the
//!   number of types it requires, while a request left with no character at all to
//!   select from is refused with `InvalidRequestException`;
//! * a password is generated, never stored: generating one changes nothing a later
//!   caller reads, and two calls with one request answer two different passwords.
//!
//! Every test is a TABLE over the class its rule generalises: the length (absent,
//! each published bound, one past each bound, and lengths in between), which types
//! the request excludes and includes, the characters `ExcludeCharacters` names, and
//! whether each included type is required. Each row is drawn repeatedly, so an
//! assertion holds over a set of passwords rather than over one draw.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::ops::get_random_password::{
    DEFAULT_PASSWORD_LENGTH, EXCLUDED_DESIRED_TYPE_MESSAGE, NO_CHARACTERS_LEFT_MESSAGE,
    TOO_SHORT_FOR_REQUIRED_TYPES_MESSAGE,
};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager` in `us-west-2`; the router routes on the
/// credential scope and the emulator verifies signatures only when an authenticator
/// is registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The character types a password is drawn from, written here from the operation's
/// own documentation: "Secrets Manager uses uppercase and lowercase letters,
/// numbers, and the following characters in passwords: !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"
/// (`GetRandomPassword`, this service's Smithy model), plus the space, which
/// `GetRandomPasswordRequest$IncludeSpace` adds.
const UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWERCASE: &str = "abcdefghijklmnopqrstuvwxyz";
const NUMBERS: &str = "0123456789";
const PUNCTUATION: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const SPACE: &str = " ";

/// How many passwords each row of a table is drawn, so an assertion holds over a set
/// of draws rather than over one. Bounded, and the longest row is 4096 characters.
const DRAWS_PER_ROW: usize = 16;

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request and return its status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// The wire error code and message of a response that must be a 400 client error.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> (String, String) {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a 400 client error from {operation}, got {status} with body \
         {parsed}"
    );
    (error_code(&parsed, case), error_message(&parsed, case))
}

/// The modelled error code a JSON error body carries.
fn error_code(parsed: &Value, case: &str) -> String {
    let raw = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no __type in error body {parsed}"));
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// The message a JSON error body carries.
fn error_message(parsed: &Value, case: &str) -> String {
    parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in error body {parsed}"))
        .to_string()
}

/// One generated password.
async fn password(router: &MultiServiceRouter, request: &Value, case: &str) -> String {
    let response = ok(router, "GetRandomPassword", request.clone(), case).await;
    response
        .get("RandomPassword")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no RandomPassword in {response}"))
        .to_string()
}

/// One row of the success table: the request, the length it asks for, the character
/// types it leaves included, the characters it excludes, and whether every included
/// type has to appear.
struct Row {
    case: &'static str,
    request: Value,
    length: usize,
    included: Vec<&'static str>,
    excluded: &'static str,
    require_each: bool,
}

/// The characters of `characters` that `excluded` does not name.
fn available(characters: &str, excluded: &str) -> BTreeSet<char> {
    characters
        .chars()
        .filter(|candidate| !excluded.contains(*candidate))
        .collect()
}

/// A password carries exactly the length the request asks for, draws every character
/// from the types the request leaves included minus the ones it excludes, and carries
/// one of every included type when the request requires each of them.
#[tokio::test]
async fn a_password_satisfies_every_constraint_its_request_states() {
    let rows = vec![
        Row {
            case: "no-parameters-at-all",
            request: json!({}),
            length: DEFAULT_PASSWORD_LENGTH,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "length-of-twelve",
            request: json!({"PasswordLength": 12}),
            length: 12,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: true,
        },
        Row {
            // The upper bound `PasswordLengthType` publishes (range 1..4096).
            case: "the-longest-password-the-model-allows",
            request: json!({"PasswordLength": 4096}),
            length: 4096,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "space-included",
            request: json!({"PasswordLength": 40, "IncludeSpace": true}),
            length: 40,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION, SPACE],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "punctuation-excluded",
            request: json!({"PasswordLength": 20, "ExcludePunctuation": true}),
            length: 20,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "letters-excluded",
            request: json!({
                "PasswordLength": 8,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
            }),
            length: 8,
            included: vec![NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "numbers-only",
            request: json!({
                "PasswordLength": 6,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludePunctuation": true,
            }),
            length: 6,
            included: vec![NUMBERS],
            excluded: "",
            require_each: true,
        },
        Row {
            // The lower bound `PasswordLengthType` publishes, with one type included
            // so a single character can carry it.
            case: "one-punctuation-character",
            request: json!({
                "PasswordLength": 1,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
            }),
            length: 1,
            included: vec![PUNCTUATION],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "space-is-the-only-included-type",
            request: json!({
                "PasswordLength": 5,
                "IncludeSpace": true,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
            }),
            length: 5,
            included: vec![SPACE],
            excluded: "",
            require_each: true,
        },
        Row {
            case: "some-characters-of-every-type-excluded",
            request: json!({
                "PasswordLength": 30,
                "ExcludeCharacters": "abcABC012!\"#",
            }),
            length: 30,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "abcABC012!\"#",
            require_each: true,
        },
        Row {
            case: "every-included-type-required-switched-off",
            request: json!({"PasswordLength": 4, "RequireEachIncludedType": false}),
            length: 4,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: false,
        },
        Row {
            case: "one-character-with-the-requirement-switched-off",
            request: json!({"PasswordLength": 1, "RequireEachIncludedType": false}),
            length: 1,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: "",
            require_each: false,
        },
        Row {
            // With the requirement off, a type left without a character is simply
            // one the password cannot draw from.
            case: "a-whole-type-excluded-with-the-requirement-switched-off",
            request: json!({
                "PasswordLength": 16,
                "RequireEachIncludedType": false,
                "ExcludeCharacters": NUMBERS,
            }),
            length: 16,
            included: vec![UPPERCASE, LOWERCASE, NUMBERS, PUNCTUATION],
            excluded: NUMBERS,
            require_each: false,
        },
    ];

    let router = emulator();

    for row in &rows {
        let allowed: BTreeSet<char> = row
            .included
            .iter()
            .flat_map(|characters| available(characters, row.excluded))
            .collect();
        let mut drawn: BTreeSet<String> = BTreeSet::new();

        for draw in 0..DRAWS_PER_ROW {
            let case = format!("{}/draw-{draw}", row.case);
            let generated = password(&router, &row.request, &case).await;

            assert_eq!(
                generated.chars().count(),
                row.length,
                "case {case:?}: expected a password of {} characters, got {} in a password of {} \
                 characters",
                row.length,
                generated.chars().count(),
                generated.chars().count()
            );

            let unexpected: Vec<char> = generated
                .chars()
                .filter(|character| !allowed.contains(character))
                .collect();
            assert!(
                unexpected.is_empty(),
                "case {case:?}: expected every character to be one of {allowed:?}, got the \
                 unexpected characters {unexpected:?}"
            );

            if row.require_each {
                for characters in &row.included {
                    let pool = available(characters, row.excluded);
                    let carried = generated.chars().filter(|c| pool.contains(c)).count();
                    assert!(
                        carried > 0,
                        "case {case:?}: expected at least one character of the type {pool:?}, got \
                         none in a password of {} characters",
                        generated.chars().count()
                    );
                }
            }

            drawn.insert(generated);
        }

        // A password is drawn, not derived from the request: over a set of characters
        // this large, repeating one draw of eight or more characters is beyond
        // chance, so equal draws mean a memorised answer.
        if row.length >= 8 && allowed.len() >= 10 {
            assert_eq!(
                drawn.len(),
                DRAWS_PER_ROW,
                "case {:?}: expected {DRAWS_PER_ROW} different passwords from {DRAWS_PER_ROW} \
                 calls, got {} different ones",
                row.case,
                drawn.len()
            );
        }
    }
}

/// A request no password can satisfy is refused: one that needs a character of a
/// type it has left without any is refused with `InvalidParameterException`, and one
/// left with no character at all to select from with `InvalidRequestException`.
///
/// The rows span the way the characters run out (every type switched off, every
/// character of the types left on named in `ExcludeCharacters`, or both), the type
/// whose characters are all excluded, whether each included type is required, and the
/// number of required types a length falls short of.
#[tokio::test]
async fn a_request_no_password_can_satisfy_is_refused() {
    let every_character = format!("{UPPERCASE}{LOWERCASE}{NUMBERS}{PUNCTUATION}");
    let rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "every-character-type-switched-off",
            json!({
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
            }),
            "InvalidRequestException",
            NO_CHARACTERS_LEFT_MESSAGE,
        ),
        (
            "every-character-type-switched-off-and-no-type-required",
            json!({
                "PasswordLength": 10,
                "RequireEachIncludedType": false,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
            }),
            "InvalidRequestException",
            NO_CHARACTERS_LEFT_MESSAGE,
        ),
        (
            "every-character-type-switched-off-and-the-space-excluded-by-name",
            json!({
                "IncludeSpace": true,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
                "ExcludeCharacters": SPACE,
            }),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "every-letter-type-switched-off-and-the-rest-excluded-by-name",
            json!({
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeCharacters": format!("{NUMBERS}{PUNCTUATION}"),
            }),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "every-letter-type-switched-off-and-the-rest-excluded-with-no-type-required",
            json!({
                "RequireEachIncludedType": false,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeCharacters": format!("{NUMBERS}{PUNCTUATION}"),
            }),
            "InvalidRequestException",
            NO_CHARACTERS_LEFT_MESSAGE,
        ),
        (
            "exclude-characters-names-every-character",
            json!({"ExcludeCharacters": &every_character}),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "exclude-characters-names-every-character-including-the-space",
            json!({
                "PasswordLength": 12,
                "IncludeSpace": true,
                "ExcludeCharacters": format!("{every_character}{SPACE}"),
            }),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "exclude-characters-names-every-character-with-no-type-required",
            json!({
                "RequireEachIncludedType": false,
                "ExcludeCharacters": &every_character,
            }),
            "InvalidRequestException",
            NO_CHARACTERS_LEFT_MESSAGE,
        ),
        (
            "every-number-excluded-while-numbers-are-required",
            json!({"ExcludeCharacters": NUMBERS}),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "every-uppercase-letter-excluded-while-uppercase-is-required",
            json!({"PasswordLength": 24, "ExcludeCharacters": UPPERCASE}),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "every-punctuation-character-excluded-while-punctuation-is-required",
            json!({"ExcludeCharacters": PUNCTUATION, "RequireEachIncludedType": true}),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "the-space-excluded-while-it-is-included-and-required",
            json!({"IncludeSpace": true, "ExcludeCharacters": SPACE}),
            "InvalidParameterException",
            EXCLUDED_DESIRED_TYPE_MESSAGE,
        ),
        (
            "one-character-for-four-required-types",
            json!({"PasswordLength": 1}),
            "InvalidParameterException",
            TOO_SHORT_FOR_REQUIRED_TYPES_MESSAGE,
        ),
        (
            "three-characters-for-four-required-types",
            json!({"PasswordLength": 3}),
            "InvalidParameterException",
            TOO_SHORT_FOR_REQUIRED_TYPES_MESSAGE,
        ),
        (
            "four-characters-for-five-required-types",
            json!({"PasswordLength": 4, "IncludeSpace": true}),
            "InvalidParameterException",
            TOO_SHORT_FOR_REQUIRED_TYPES_MESSAGE,
        ),
        (
            "one-character-for-two-required-types",
            json!({
                "PasswordLength": 1,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
            }),
            "InvalidParameterException",
            TOO_SHORT_FOR_REQUIRED_TYPES_MESSAGE,
        ),
    ];

    let router = emulator();

    for (case, request, expected_code, expected_message) in rows {
        let (code, message) = err(&router, "GetRandomPassword", request, case).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} with {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected {expected_message:?}, got {message:?}"
        );
    }
}

/// A length outside the range the model publishes for `PasswordLength` (1..4096) is
/// refused, and one on either bound is served.
#[tokio::test]
async fn a_length_outside_the_modelled_range_is_refused() {
    let router = emulator();

    for (case, length) in [("zero", 0), ("one-past-the-maximum", 4097)] {
        let (status, parsed) = call(
            &router,
            "GetRandomPassword",
            &json!({"PasswordLength": length}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 client error for the length {length}, got {status} \
             with body {parsed}"
        );
        let code = error_code(&parsed, case);
        assert!(
            matches!(
                code.as_str(),
                "ValidationException" | "InvalidParameterException"
            ),
            "case {case:?}: expected the length {length} to be refused as an invalid value, got \
             {code} with {:?}",
            error_message(&parsed, case)
        );
    }

    // Both published bounds are served: the shortest password carries the one type
    // left included, and the longest is the length the model's maximum names.
    for (case, request, expected_length) in [
        (
            "the-minimum",
            json!({
                "PasswordLength": 1,
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludePunctuation": true,
            }),
            1_usize,
        ),
        ("the-maximum", json!({"PasswordLength": 4096}), 4096_usize),
    ] {
        let generated = password(&router, &request, case).await;
        assert_eq!(
            generated.chars().count(),
            expected_length,
            "case {case:?}: expected a password of {expected_length} characters, got {}",
            generated.chars().count()
        );
    }
}

/// Generating a password stores nothing: no secret appears, and the value of a secret
/// that already exists is untouched, however many passwords are generated.
#[tokio::test]
async fn generating_a_password_changes_nothing_a_later_caller_reads() {
    let router = emulator();
    let name = "random-password-neighbour";
    ok(
        &router,
        "CreateSecret",
        json!({"Name": name, "SecretString": "s3cret"}),
        "setup",
    )
    .await;

    // The value is read first, so the `LastAccessedDate` that read records is already
    // in the snapshot the list below is compared against.
    let read_before = ok(
        &router,
        "GetSecretValue",
        json!({"SecretId": name}),
        "before",
    )
    .await;
    let listed_before = ok(&router, "ListSecrets", json!({}), "before").await;

    // The rows span the kinds of request this operation serves and refuses, so a
    // write on either path would show up below.
    for (case, request) in [
        ("default", json!({})),
        ("explicit-length", json!({"PasswordLength": 64})),
        (
            "refused-for-having-no-character-left",
            json!({
                "ExcludeUppercase": true,
                "ExcludeLowercase": true,
                "ExcludeNumbers": true,
                "ExcludePunctuation": true,
            }),
        ),
        (
            "refused-for-being-too-short",
            json!({"PasswordLength": 2}),
        ),
    ] {
        let (status, parsed) = call(&router, "GetRandomPassword", &request).await;
        assert!(
            status == 200 || status == 400,
            "case {case:?}: expected the request to be served or refused, got {status} with body \
             {parsed}"
        );
    }

    let listed_after = ok(&router, "ListSecrets", json!({}), "after").await;
    let read_after = ok(&router, "GetSecretValue", json!({"SecretId": name}), "after").await;
    assert_eq!(
        listed_after, listed_before,
        "generating passwords must leave the secrets untouched: {listed_before} became \
         {listed_after}"
    );
    assert_eq!(
        read_after, read_before,
        "generating passwords must leave a secret's value untouched: {read_before} became \
         {read_after}"
    );
}
