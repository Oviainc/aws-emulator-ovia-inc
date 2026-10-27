// SPDX-License-Identifier: CC0-1.0

//! How this service states a modelled constraint broken by an ELEMENT of a list.
//!
//! The RULE: a constraint declared on a list's member shape is a requirement on the LIST
//! member, so a refusal attributes it to that member — no element position, no `member`
//! step — echoes the whole list the member held, and nests the element's own requirement
//! inside `Member must satisfy constraint: [ … ]`. A constraint broken by the list ITSELF
//! (its own `@length`) is attributed to the same member and is NOT nested, and a scalar
//! member's own failure is untouched.
//!
//! The rows span what the rule generalises over rather than the one recorded request: the
//! element constraint CLASS (an enum value set, a `@pattern`), the position of the offending
//! element, how many elements of one list are refused, and whether valid elements sit beside
//! them. The recorded `StartConfigurationChecks` / `SAP_CHECK_NOT_A_REAL_ID` request is ONE
//! row.
//!
//! Two of the three scalar lists this service's inputs declare are driven here
//! (`smithy_query(query="shape", shape="ConfigurationCheckTypeList,InstanceList,TagKeyList")`):
//! `ConfigurationCheckTypeList` and `InstanceList`. The third, `TagKeyList`, declares on its
//! element only `TagKey`'s `@pattern`, whose negative lookahead the constraint walk's regex
//! engine cannot compile — such a pattern admits every value rather than refusing one — so no
//! request can make its element constraint fire; that list's wording is covered at the clause
//! level in `src/service.rs`'s own table instead.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk and this service's refusal
//! wording all run exactly as they do in the emulator binary.

use serde_json::{json, Value};

use crate::application_lifecycle::{Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// An `ApplicationId` inside its modelled length and pattern, so a row's own list is the
/// only member that fails.
const GOOD_APPLICATION_ID: &str = "HDB-TEST";

/// An instance id inside `InstanceId`'s modelled pattern, for a row whose failing member is
/// another one — or whose list fails its own `@length`.
const GOOD_INSTANCE_ID: &str = "i-0123456789abcdef0";

/// The value set `ConfigurationCheckType` declares, as a requirement lists it. Source:
/// `smithy_query(query="shape", shape="ConfigurationCheckType")` — `[SAP_CHECK_01,
/// SAP_CHECK_02, SAP_CHECK_03]`.
const CHECK_VALUE_SET: &str = "SAP_CHECK_01, SAP_CHECK_02, SAP_CHECK_03";

/// `InstanceId`'s modelled `@pattern` as a requirement reports it: `^i-[\w\d]{8}$|^i-[\w\d]{17}$`
/// without the OUTER anchors (the inner pair belongs to the alternation's branches).
const INSTANCE_ID_PATTERN: &str = r"i-[\w\d]{8}$|^i-[\w\d]{17}";

/// A `StartConfigurationChecks` body naming `checks`, with every other member satisfied.
fn start_checks_body(checks: &[&str]) -> Value {
    json!({
        "ApplicationId": GOOD_APPLICATION_ID,
        "ConfigurationCheckIds": checks,
    })
}

/// A `RegisterApplication` body naming `instances`, with every other required member
/// satisfied.
fn registration_body(instances: &[&str]) -> Value {
    json!({
        "ApplicationId": GOOD_APPLICATION_ID,
        "ApplicationType": "HANA",
        "Instances": instances,
    })
}

/// Assert `reply` is the modelled constraint refusal — HTTP 400 `ValidationException` —
/// and answer its message.
fn refusal(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected 400, got {}: {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
        reply.code()
    );
    message(reply)
}

/// RULE: an element's broken constraint is stated at the LIST member, with the whole list
/// echoed and the element's requirement nested — over both element constraint classes, every
/// arrangement of valid and invalid elements, and a body-bound as well as a query-bound list.
#[tokio::test]
async fn an_elements_constraint_is_stated_at_its_list_member() {
    let emulator = Emulator::new();

    // (label, the whole expected message, the reply)
    let mut rows: Vec<(&str, String, Reply)> = Vec::new();

    // The enum-value-set class, on the body-bound `ConfigurationCheckIds`.
    for (label, checks, echoed) in [
        (
            "one unknown check, the only element",
            vec!["SAP_CHECK_NOT_A_REAL_ID"],
            "[SAP_CHECK_NOT_A_REAL_ID]",
        ),
        (
            "an unknown check after a known one",
            vec!["SAP_CHECK_01", "SAP_CHECK_NOT_A_REAL_ID"],
            "[SAP_CHECK_01, SAP_CHECK_NOT_A_REAL_ID]",
        ),
        (
            "two unknown checks either side of a known one",
            vec!["NOT_A_CHECK_A", "SAP_CHECK_02", "NOT_A_CHECK_B"],
            "[NOT_A_CHECK_A, SAP_CHECK_02, NOT_A_CHECK_B]",
        ),
        (
            "the same unknown check twice",
            vec!["SAP_CHECK_NOPE", "SAP_CHECK_NOPE"],
            "[SAP_CHECK_NOPE, SAP_CHECK_NOPE]",
        ),
    ] {
        let reply = emulator
            .send("/start-configuration-checks", start_checks_body(&checks))
            .await;
        rows.push((
            label,
            format!(
                "1 validation error detected: Value '{echoed}' at 'configurationCheckIds' \
                 failed to satisfy constraint: Member must satisfy constraint: [Member must \
                 satisfy enum value set: [{CHECK_VALUE_SET}]]"
            ),
            reply,
        ));
    }

    // The `@pattern` class, on the body-bound `Instances` — a list whose modelled
    // `@length` admits exactly one element, so the single bad element is the whole list.
    let reply = emulator
        .send("/register-application", registration_body(&["nope"]))
        .await;
    rows.push((
        "an instance id outside its pattern",
        format!(
            "1 validation error detected: Value '[nope]' at 'instances' failed to satisfy \
             constraint: Member must satisfy constraint: [Member must satisfy regular \
             expression pattern: {INSTANCE_ID_PATTERN}]"
        ),
        reply,
    ));

    for (label, expected, reply) in rows {
        let actual = refusal(label, &reply);
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        // The position and the `member` step are the walk's bookkeeping, not an input
        // position a client can map back onto its request.
        assert!(
            !actual.contains(".member"),
            "case {label:?}: the message names an element path, got {actual:?}"
        );
    }
}

/// RULE: a constraint the LIST itself declares is stated at the same member and is NOT
/// nested — the nesting says "an element of this member failed", so applying it to the
/// member's own rule would report a requirement the model does not state.
#[tokio::test]
async fn a_lists_own_constraint_is_not_nested() {
    let emulator = Emulator::new();
    // `InstanceList` is modelled `@length(min: 1, max: 1)`, so two well-formed instance
    // ids break the list's own maximum and no element's rule.
    let reply = emulator
        .send(
            "/register-application",
            registration_body(&[GOOD_INSTANCE_ID, "i-0fedcba9876543210"]),
        )
        .await;
    let actual = refusal("two instances in a one-element list", &reply);
    assert!(
        actual.contains(
            "at 'instances' failed to satisfy constraint: Member must have length less than \
             or equal to 1"
        ),
        "case \"two instances in a one-element list\": expected the list's own requirement \
         stated at 'instances', got {actual:?}"
    );
    assert!(
        !actual.contains("Member must satisfy constraint: ["),
        "case \"two instances in a one-element list\": the list's own rule must not be \
         nested, got {actual:?}"
    );
}

/// RULE, the other side: a list whose every element satisfies its member shape is not
/// refused for it, at every list length the model admits — and a scalar member's own
/// failure keeps its own path, unnested.
#[tokio::test]
async fn a_list_inside_its_constraints_is_not_refused_and_a_scalar_keeps_its_own_path() {
    let emulator = Emulator::new();

    // Every arrangement of the modelled value set: one value, several, the whole set, and
    // the member omitted altogether (the request then runs the service's catalogue).
    let rows: Vec<(&str, Value)> = vec![
        ("no list at all", json!({ "ApplicationId": GOOD_APPLICATION_ID })),
        ("one known check", start_checks_body(&["SAP_CHECK_01"])),
        (
            "two known checks",
            start_checks_body(&["SAP_CHECK_02", "SAP_CHECK_03"]),
        ),
        (
            "the whole value set",
            start_checks_body(&["SAP_CHECK_01", "SAP_CHECK_02", "SAP_CHECK_03"]),
        ),
    ];
    for (label, body) in rows {
        let reply = emulator.send("/start-configuration-checks", body).await;
        let stated = reply
            .member("message")
            .or_else(|| reply.member("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !stated.contains("validation error"),
            "case {label:?}: a list inside its constraints must not be refused for it, got \
             {stated:?}"
        );
    }

    // A SCALAR member's own failure: no list is involved, so nothing is collapsed and
    // nothing is nested.
    let reply = emulator
        .send(
            "/start-configuration-checks",
            json!({
                "ApplicationId": "bad app id!",
                "ConfigurationCheckIds": ["SAP_CHECK_01"],
            }),
        )
        .await;
    let actual = refusal("a scalar member outside its pattern", &reply);
    assert_eq!(
        actual,
        "1 validation error detected: Value 'bad app id!' at 'applicationId' failed to \
         satisfy constraint: Member must satisfy regular expression pattern: [\\w\\d\\.-]+",
        "case \"a scalar member outside its pattern\": got {actual:?}"
    );
}

/// RULE: a request breaking a list element's rule AND another member's is one envelope with
/// one clause each — the list's clause is still stated once, at its member.
#[tokio::test]
async fn a_list_clause_and_a_scalar_clause_are_one_envelope() {
    let emulator = Emulator::new();
    let reply = emulator
        .send(
            "/register-application",
            json!({
                "ApplicationId": GOOD_APPLICATION_ID,
                "ApplicationType": "HANA",
                "Instances": ["nope-one"],
                "Sid": "abc",
            }),
        )
        .await;
    let actual = refusal("a bad instance id and a bad SID", &reply);
    assert!(
        actual.starts_with("2 validation errors detected: "),
        "case \"a bad instance id and a bad SID\": expected the plural envelope over two \
         clauses, got {actual:?}"
    );
    for clause in [
        format!(
            "Value '[nope-one]' at 'instances' failed to satisfy constraint: Member must \
             satisfy constraint: [Member must satisfy regular expression pattern: \
             {INSTANCE_ID_PATTERN}]"
        ),
        "Value 'abc' at 'sid' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: [A-Z][A-Z0-9]{2}"
            .to_string(),
    ] {
        assert!(
            actual.contains(&clause),
            "case \"a bad instance id and a bad SID\": expected the clause {clause:?}, got \
             {actual:?}"
        );
    }
    assert_eq!(
        actual.matches("; ").count(),
        1,
        "case \"a bad instance id and a bad SID\": expected the two clauses joined with a \
         single `; `, got {actual:?}"
    );
}
