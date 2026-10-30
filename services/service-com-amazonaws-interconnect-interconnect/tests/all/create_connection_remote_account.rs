// SPDX-License-Identifier: CC0-1.0

//! What identifying information `CreateConnection` asks the caller for, driven over the
//! wire through the shared `crate::emulator` harness.
//!
//! RULE: the create is judged on the PRESENCE of `remoteAccount` and never on the text
//! of the identifier it carries — an Environment that declares a `remoteIdentifierType`
//! needs the member on every create against it (the operation's modelled
//! `InterconnectValidationException`, 400, naming the member that is missing), while
//! every `identifier` the union's single generic member admits under its own modelled
//! `@length` and `@pattern` is accepted whatever kind that Environment declares,
//! because the identity behind it is verified later at the partner portal. A value
//! outside those modelled constraints is the only identifier refused, and it is
//! refused for the constraint it breaks rather than for the kind it is not.
//!
//! The table's rows differ along what the rule generalises over — the FORM of the
//! identifier (opaque token, account number, email address, each character class the
//! pattern allows) and its LENGTH (both published bounds and one past each) — across
//! every Environment of the seeded catalog that takes connections, so each declared
//! kind and the undeclared case are covered.

use std::collections::{BTreeSet, BTreeMap};

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, text, Emulator, PARTNER_EMAIL,
};

/// The `@length` bounds the model puts on the only member the `RemoteAccountIdentifier`
/// union offers (`smithy_query(query="shape",shape="RemoteAccountIdentifier")`:
/// `identifier` -> `RemoteOwnerAccount`, length 1..255, pattern
/// `^[-a-zA-Z0-9_@\.]+$`), which are the only rules stated for what it may carry.
const IDENTIFIER_LENGTH: (usize, usize) = (1, 255);

/// How the service words a create whose Environment asks for identifying information
/// and that carries none.
const MISSING_IDENTIFIER_MESSAGE: &str =
    "Unable to process request: remoteAccount.identifier is required";

/// `CreateConnection` with the members a row varies, as a `(status, body)` pair so a
/// refusal is readable.
async fn create(
    emulator: &Emulator,
    environment_id: &str,
    bandwidth: &str,
    attach_point: &str,
    remote_account: Option<Value>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    request.insert("bandwidth".to_string(), json!(bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": attach_point }),
    );
    if let Some(remote_account) = remote_account {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic("CreateConnection", Value::Object(request))
        .await
}

/// Every connection the store holds, walked page by page, so a refused create can be
/// shown to have provisioned nothing.
async fn stored_ids(emulator: &Emulator) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok("ListConnections", Value::Object(request))
            .await;
        for summary in listed["connections"].as_array().cloned().unwrap_or_default() {
            all.insert(text(&summary, "id"));
        }
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// The identifiers the member admits, each labelled by the form it takes.
///
/// The forms span what identifying information can be rather than one shape: an opaque
/// partner-issued token, account numbers, email addresses, and strings that are neither
/// (digit counts either side of an account number's width, `@`-shapes no address
/// grammar would allow) — the member is documented as "a generic bit of identifying
/// information", so each is a value it may carry. The length rows sit on both published
/// bounds of [`IDENTIFIER_LENGTH`], and the alphabet row uses every character class the
/// member's pattern allows.
fn identifiers_the_member_admits() -> Vec<(String, String)> {
    let rows: Vec<(&str, String)> = vec![
        (
            "an opaque partner-issued identity",
            "partner-issued-identity".to_string(),
        ),
        (
            "the account number of this account",
            engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        (
            "another twelve-digit account number",
            "123456789012".to_string(),
        ),
        ("an email address", PARTNER_EMAIL.to_string()),
        ("an address at a bare host name", "partner@localhost".to_string()),
        (
            "one character, the shortest the member allows",
            "a".repeat(IDENTIFIER_LENGTH.0),
        ),
        (
            "the longest the member allows",
            "a".repeat(IDENTIFIER_LENGTH.1),
        ),
        (
            "every character class the pattern allows",
            "aZ9-_@.".to_string(),
        ),
        (
            "one digit short of an account number",
            "1".repeat(11),
        ),
        ("one digit past an account number", "1".repeat(13)),
        ("two at signs", "partner@example@test".to_string()),
        ("no local part", "@example.test".to_string()),
        ("no domain", "partner@".to_string()),
    ];
    rows.into_iter()
        .map(|(form, value)| (form.to_string(), value))
        .collect()
}

/// The identifiers the member's own modelled constraints exclude: one character either
/// side of [`IDENTIFIER_LENGTH`], and characters outside the pattern's alphabet, each
/// labelled by the constraint it breaks.
fn identifiers_outside_the_member() -> Vec<(String, String)> {
    let rows: Vec<(&str, String)> = vec![
        (
            "the empty string, one short of the shortest allowed",
            String::new(),
        ),
        (
            "one character past the longest allowed",
            "a".repeat(IDENTIFIER_LENGTH.1 + 1),
        ),
        (
            "a space, outside the pattern's alphabet",
            "partner identity".to_string(),
        ),
        (
            "a slash, outside the pattern's alphabet",
            "partner/identity".to_string(),
        ),
    ];
    rows.into_iter()
        .map(|(form, value)| (form.to_string(), value))
        .collect()
}

/// RULE: `remoteAccount` is judged on presence alone — required by an Environment that
/// declares a `remoteIdentifierType`, and then satisfied by any identifier the member's
/// modelled length and pattern admit, whichever kind that Environment declares.
#[tokio::test]
async fn a_create_is_judged_on_the_presence_of_remote_account_not_on_the_identifier_text() {
    let emulator = Emulator::new();
    // An attach point is a Direct Connect gateway of the account, so the suite attaches
    // its connections to one the account really holds.
    let attach_point = emulator.attach_point().await;
    let catalog = emulator.environments().await;
    let environments = creatable_environments(&catalog);
    assert!(
        !environments.is_empty(),
        "case \"catalog\": expected an environment that takes connections, got {}",
        environments.len()
    );

    let mut declared_kinds: BTreeMap<String, usize> = BTreeMap::new();
    for (index, environment) in environments.iter().enumerate() {
        let environment_id = text(environment, "environmentId");
        let bandwidths = available_bandwidths(environment);
        let bandwidth = bandwidths[index % bandwidths.len()].clone();
        let declared = environment["remoteIdentifierType"]
            .as_str()
            .map(str::to_string);
        let kind = declared.clone().unwrap_or_else(|| "none".to_string());
        *declared_kinds.entry(kind.clone()).or_default() += 1;

        // ABSENCE is the one thing a declaring Environment refuses; one that declares
        // nothing asks for nothing and takes the create as it stands.
        let case = format!("{environment_id} / declares {kind} / absent");
        let (status, body) = create(
            &emulator,
            &environment_id,
            &bandwidth,
            &attach_point,
            None,
        )
        .await;
        match &declared {
            Some(_) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a create with no remoteAccount to be refused, got {status} with {body:?}"
                );
                assert!(
                    code(&body).contains("InterconnectValidationException"),
                    "case {case:?}: expected InterconnectValidationException, got {:?}",
                    code(&body)
                );
                assert_eq!(
                    message(&body),
                    MISSING_IDENTIFIER_MESSAGE,
                    "case {case:?}: expected the refusal to name the missing member, got {:?}",
                    message(&body)
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected an environment that declares no kind to ask for nothing, got {status} with {body:?}"
            ),
        }

        // Every identifier the member admits is accepted, whatever kind this
        // Environment declares, and the connection it mints is readable back.
        for (form, identifier) in identifiers_the_member_admits() {
            let case = format!("{environment_id} / declares {kind} / {form}");
            let (status, body) = create(
                &emulator,
                &environment_id,
                &bandwidth,
                &attach_point,
                Some(json!({ "identifier": identifier })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected identifier {identifier:?} to be accepted, got {status} with {body:?}"
            );
            assert_eq!(
                text(&body["connection"], "state"),
                "requested",
                "case {case:?}: expected the new connection to be returned in its initial state, got {:?}",
                body["connection"]["state"]
            );
            let id = text(&body["connection"], "id");
            let (status, fetched) = emulator.get_connection(&id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the created connection to read back, got {status} with {fetched:?}"
            );
            assert_eq!(
                fetched["connection"]["environmentId"],
                json!(environment_id),
                "case {case:?}: expected the stored connection to name the environment it was created across, got {:?}",
                fetched["connection"]["environmentId"]
            );
        }

        // A value outside the member's own modelled constraints is refused, and
        // provisions nothing.
        let before = stored_ids(&emulator).await;
        for (form, identifier) in identifiers_outside_the_member() {
            let case = format!("{environment_id} / declares {kind} / {form}");
            let (status, body) = create(
                &emulator,
                &environment_id,
                &bandwidth,
                &attach_point,
                Some(json!({ "identifier": identifier })),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected identifier {identifier:?} to be refused as outside the member's constraints, got {status} with {body:?}"
            );
            assert!(
                message(&body).contains("remoteAccount.identifier"),
                "case {case:?}: expected the refusal to name the member it judged, got {:?}",
                message(&body)
            );
        }
        let after = stored_ids(&emulator).await;
        assert_eq!(
            after, before,
            "case {environment_id:?}: expected every refused create to have provisioned nothing, got {after:?} for {before:?} before"
        );
    }

    // The rows really did span every identifier kind the catalog declares. Which kinds
    // it declares is the SERVICE's to publish — the model binds no `CreateEnvironment` —
    // so the coverage required here is the catalog's own set rather than a fixed one,
    // and the branches above state the rule for each kind a catalog can declare.
    let published: BTreeMap<String, usize> = environments
        .iter()
        .map(|environment| {
            (
                environment["remoteIdentifierType"]
                    .as_str()
                    .unwrap_or("none")
                    .to_string(),
                0,
            )
        })
        .collect();
    for expected in published.keys() {
        assert!(
            declared_kinds.contains_key(expected),
            "case \"coverage\": expected the rows to span the environment declaring {expected:?}, got {declared_kinds:?}"
        );
    }
}
