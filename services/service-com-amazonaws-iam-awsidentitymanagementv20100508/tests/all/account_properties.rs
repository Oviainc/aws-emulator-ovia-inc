// SPDX-License-Identifier: CC0-1.0

//! The account-level properties, end to end over the awsQuery wire:
//! `PutAccountProperties` -> `GetAccountProperties`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of the
//! properties the account PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share, so a request is built and read exactly one way.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted
//! generalises over: how many pairs one request carries, the alphabet and length of
//! the key, the form and length of the value, the namespace, and how many puts have
//! run in turn. No single request is the specification.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, error_code, router};

/// The namespace this service serves, in its published spelling.
///
/// SOURCE: the account-wide feature `GetAccountProperties`/`PutAccountProperties`
/// name — "configuration settings that control account-wide IAM features such as
/// Role Manager" — which is the namespace every property of that feature is keyed
/// under. Written out here rather than read from the crate, so the rows are what
/// AWS publishes and not what the code happens to hold.
const SERVED_NAMESPACE: &str = "RoleManager";

/// The properties an account holds before anything has been written to them: the
/// `Enabled` property of the served namespace, reported off.
///
/// SOURCE: the baseline a fresh account reports for this feature — a feature is not
/// on until an account turns it on — stated as the literal pair so the assertion is
/// falsifiable rather than a restatement of the crate's own table.
fn default_properties() -> BTreeMap<String, String> {
    [(format!("{SERVED_NAMESPACE}/Enabled"), "false".to_string())]
        .into_iter()
        .collect()
}

/// [`default_properties`] with `properties` set on top of it: what a read must
/// report after a write, since a write says nothing about the keys it does not name.
fn with_defaults(properties: &[(String, String)]) -> BTreeMap<String, String> {
    let mut expected = default_properties();
    for (key, value) in properties {
        expected.insert(key.clone(), value.clone());
    }
    expected
}

/// A key under the served namespace.
fn served_key(property_name: &str) -> String {
    format!("{SERVED_NAMESPACE}/{property_name}")
}

/// The property catalog: rows spanning the key alphabet and both published key
/// bounds, the value forms a property carries, and the pair count of one request.
/// Each row's keys all sit in the served namespace, which is what a single request
/// may carry.
///
/// Source: `accountPropertiesMapType` on
/// `smithy_query(query="shape", shape="accountPropertiesMapType")` — key
/// `accountPropertyKeyType` (length 1..50, pattern `^[A-Za-z][A-Za-z0-9/_-]*$`),
/// value `accountPropertyValueType` (length 1..1024) — and
/// `PutAccountPropertiesRequest$Properties` for the key format
/// (`Namespace/PropertyName`, "exactly one `/`").
fn property_catalog() -> Vec<Vec<(String, String)>> {
    // The published maximum key length is 50 characters, of which the served
    // namespace and its separator take their share.
    let longest_property_name = "B".repeat(50 - SERVED_NAMESPACE.len() - 1);
    vec![
        // One pair, a boolean-looking value: "boolean properties expect true or
        // false".
        vec![param(&served_key("Enabled"), "true")],
        // The same shape of property set to the other boolean spelling.
        vec![param(&served_key("Enabled"), "false")],
        // Several pairs, values spanning other forms a property value takes:
        // digits, free text, and a value needing XML escaping.
        vec![
            param(&served_key("Threshold"), "250"),
            param(&served_key("Contact"), "finance team"),
            param(&served_key("Note"), "a <b> & \"quoted\" value"),
        ],
        // The key alphabet in full: letters, digits, underscores and dashes in the
        // property name.
        vec![
            param(&served_key("Property_Name-2"), "x"),
            param(&served_key("P9"), "y"),
        ],
        // The published minimum of each: a one-character property name and a
        // one-character value.
        vec![param(&served_key("B"), "z")],
        // The published maximum key length (50 characters) and value length (1024).
        vec![param(
            &served_key(&longest_property_name),
            &"v".repeat(1024),
        )],
        // Five pairs: a request carrying more than a handful.
        vec![
            param(&served_key("One"), "1"),
            param(&served_key("Two"), "2"),
            param(&served_key("Three"), "3"),
            param(&served_key("Four"), "4"),
            param(&served_key("Five"), "5"),
        ],
    ]
}

/// The awsQuery parameters that carry `properties` as this operation's `Properties`
/// map: the wrapped `Name.entry.N.key` / `Name.entry.N.value` form a query-protocol
/// map arrives in.
fn properties_params(properties: &[(String, String)]) -> Vec<(String, String)> {
    properties
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                param(&format!("Properties.entry.{position}.key"), key),
                param(&format!("Properties.entry.{position}.value"), value),
            ]
        })
        .collect()
}

/// Call `PutAccountProperties` with `properties`, answering the status and body.
async fn put_properties(
    router: &MultiServiceRouter,
    properties: &[(String, String)],
) -> (u16, String) {
    let params = properties_params(properties);
    call(router, "PutAccountProperties", &borrowed(&params)).await
}

/// Call `PutAccountProperties` with `properties`, asserting the fixture put
/// succeeded.
async fn put_properties_ok(
    router: &MultiServiceRouter,
    case: &str,
    properties: &[(String, String)],
) {
    let (status, body) = put_properties(router, properties).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture PutAccountProperties must succeed, got {status}: {body}"
    );
}

/// The `Properties` map a `GetAccountProperties` response reports.
///
/// `accountPropertiesMapType` is a wrapped map, so each pair arrives as a
/// `<key>`/`<value>` element run inside `<entry>`, and that map is the only one in
/// this response shape.
fn reported_properties(body: &str) -> BTreeMap<String, String> {
    crate::user_lifecycle::elements(body, "key")
        .into_iter()
        .zip(crate::user_lifecycle::elements(body, "value"))
        .collect()
}

/// The properties a `GetAccountProperties` reports, asserting the read succeeded.
async fn current_properties(router: &MultiServiceRouter, case: &str) -> BTreeMap<String, String> {
    let (status, body) = call(router, "GetAccountProperties", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetAccountProperties to answer 200, got {status}: {body}"
    );
    reported_properties(&body)
}

/// RULE: an account-wide feature these properties gate is in its documented default
/// state from the moment the account exists, so a read before any write reports
/// that default rather than nothing — and a write of another property leaves it
/// where it is.
#[tokio::test]
async fn a_fresh_account_reports_the_default_properties() {
    let router = router();
    // Rows: the read on a fresh account, repeated, since a report changes nothing.
    for read in 0..3 {
        let reported = current_properties(&router, "a fresh account").await;
        assert_eq!(
            reported,
            default_properties(),
            "case {read}: expected a fresh account to report its defaults, got {reported:?}"
        );
    }

    // A write naming another property says nothing about the default, which
    // therefore keeps the value it holds.
    let other = vec![param(&served_key("Mode"), "auto")];
    put_properties_ok(&router, "another property", &other).await;
    let reported = current_properties(&router, "after another property").await;
    assert_eq!(
        reported,
        with_defaults(&other),
        "expected the default to survive a write naming another property, got {reported:?}"
    );

    // And every property the crate's own table carries a default for is reported,
    // so a namespace added to it is covered here too.
    for key in state::default_account_properties().keys() {
        assert!(
            reported.contains_key(key),
            "case {key:?}: expected the account to report a default for every served \
             property, got {reported:?}"
        );
    }
}

/// RULE: a namespace names an account-wide feature the service itself offers, so a
/// key under a namespace the service does not serve configures nothing: the write
/// is refused — HTTP 400 under the code IAM answers a value it cannot accept with
/// (`ValidationError`) — naming that namespace, and nothing it carried is persisted, so
/// a later read never reports a property the service does not serve.
#[tokio::test]
async fn a_write_under_a_namespace_the_service_does_not_serve_is_refused() {
    // Rows span the ways a namespace can fail to be one this service serves: a name
    // nobody publishes, a plausible feature name, a served name in another case, a
    // served name with a character added, and the shortest name the key alphabet
    // admits.
    let unserved = [
        "NotARealProperty",
        "Billing",
        "rolemanager",
        "ROLEMANAGER",
        "RoleManagers",
        "Role_Manager",
        "A",
        "My_Namespace-1",
    ];

    for namespace in unserved {
        assert!(
            !state::SERVED_ACCOUNT_PROPERTY_NAMESPACES
                .iter()
                .any(|served| served.namespace == namespace),
            "case {namespace:?}: this row must name a namespace the service does not serve"
        );
        let router = router();
        // An account already holding a property: the refusal must leave it alone.
        let held = vec![param(&served_key("Held"), "kept")];
        put_properties_ok(&router, "the fixture property", &held).await;

        // Rows also differ in the property name and in how many pairs of the
        // unserved namespace one request carries.
        for properties in [
            vec![param(&format!("{namespace}/Enabled"), "true")],
            vec![param(&format!("{namespace}/B"), "1")],
            vec![
                param(&format!("{namespace}/One"), "1"),
                param(&format!("{namespace}/Two"), "2"),
            ],
        ] {
            let case = format!("{properties:?}");
            let (status, body) = put_properties(&router, &properties).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {case:?}: expected ValidationError, got {:?}",
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                format!("Unsupported namespace: {namespace}"),
                "case {case:?}: unexpected message, got {:?}",
                element(&body, "Message")
            );
            let reported = current_properties(&router, &case).await;
            assert_eq!(
                reported,
                with_defaults(&held),
                "case {case:?}: a refused write persists nothing, got {reported:?}"
            );
        }
    }
}

/// RULE: a key under a namespace the service DOES serve is accepted and read back —
/// over every namespace the service serves, so the served set is what decides and
/// not one name.
#[tokio::test]
async fn a_write_under_a_served_namespace_is_accepted() {
    assert!(
        !state::SERVED_ACCOUNT_PROPERTY_NAMESPACES.is_empty(),
        "the service must serve at least one account-property namespace"
    );
    for served in state::SERVED_ACCOUNT_PROPERTY_NAMESPACES {
        let router = router();
        let properties = vec![param(&format!("{}/Enabled", served.namespace), "true")];
        let case = format!("{properties:?}");
        put_properties_ok(&router, &case, &properties).await;
        let reported = current_properties(&router, &case).await;
        assert_eq!(
            reported,
            with_defaults(&properties),
            "case {case:?}: expected the written property to be read back, got {reported:?}"
        );
    }
}

/// RULE: a key-value property store reports exactly the pairs a write persisted —
/// every key the write named, each under the value it set — on top of the defaults
/// the account already held, rather than a value recomputed from the reading
/// request.
#[tokio::test]
async fn get_account_properties_reports_the_pairs_the_account_persisted() {
    for properties in property_catalog() {
        let case = format!("{properties:?}");
        // A fresh account per row: properties are account-wide, so each row starts
        // from an account holding only its defaults.
        let router = router();
        assert_eq!(
            current_properties(&router, &case).await,
            default_properties(),
            "case {case:?}: an account that set no property holds its defaults"
        );

        put_properties_ok(&router, &case, &properties).await;
        let reported = current_properties(&router, &case).await;
        assert_eq!(
            reported,
            with_defaults(&properties),
            "case {case:?}: expected the stored properties, got {reported:?}"
        );
    }
}

/// RULE: a write SETS the keys it names — a key the store already holds takes the
/// new value, a key it does not hold is added — and says nothing about any key it
/// does not name, which therefore keeps the value it holds.
#[tokio::test]
async fn setting_a_property_replaces_its_value_and_leaves_the_others_alone() {
    // Rows: a value toggled back and forth, a second property added, a third added
    // beside it, and keys repeated across rows.
    let sequence: &[&[(&str, &str)]] = &[
        &[("Enabled", "true")],
        &[("Enabled", "false")],
        &[("Enabled", "true"), ("Mode", "auto")],
        &[("Contact", "finance")],
        &[("Mode", "manual")],
        &[("Contact", "ops"), ("Threshold", "250")],
    ];

    let router = router();
    let mut expected = default_properties();
    for (position, step) in sequence.iter().enumerate() {
        let properties: Vec<(String, String)> = step
            .iter()
            .map(|(property_name, value)| param(&served_key(property_name), value))
            .collect();
        let case = format!("step {position}: {step:?}");
        put_properties_ok(&router, &case, &properties).await;
        for (key, value) in &properties {
            expected.insert(key.clone(), value.clone());
        }
        let reported = current_properties(&router, &case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE: "all keys must belong to the same namespace", so a write whose keys span
/// more than one namespace is refused as a whole — HTTP 400 under the code IAM answers
/// a value it cannot accept with (`ValidationError`) — and persists
/// none of its pairs. The request's shape is taken before the namespace it names is
/// looked up, so a spanning request is refused for spanning whichever namespaces it
/// spans.
#[tokio::test]
async fn a_write_whose_keys_span_two_namespaces_is_refused() {
    // Rows differ in how many namespaces are spanned, in the pair count, and in
    // where in the request the foreign namespace sits.
    let spanning: &[&[(&str, &str)]] = &[
        &[("RoleManager/Enabled", "true"), ("Billing/Contact", "ops")],
        &[("Billing/Contact", "ops"), ("RoleManager/Enabled", "true")],
        &[
            ("A/One", "1"),
            ("A/Two", "2"),
            ("B/Three", "3"),
            ("A/Four", "4"),
        ],
        &[("A/One", "1"), ("B/Two", "2"), ("C/Three", "3")],
    ];

    for step in spanning {
        let router = router();
        // An account already holding a property: the refusal must leave it alone.
        let held = vec![param(&served_key("Held"), "kept")];
        put_properties_ok(&router, "the fixture property", &held).await;

        let properties: Vec<(String, String)> = step
            .iter()
            .map(|(key, value)| param(key, value))
            .collect();
        let case = format!("{step:?}");
        let (status, body) = put_properties(&router, &properties).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
        let reported = current_properties(&router, &case).await;
        assert_eq!(
            reported,
            with_defaults(&held),
            "case {case:?}: a refused write persists nothing, got {reported:?}"
        );
    }
}

/// RULE: a key is `Namespace/PropertyName` — "exactly one `/` separating the
/// namespace from the property name", neither part empty — inside the published
/// length and alphabet of its modelled type, so a key outside that shape names no
/// property and the write carrying it is refused, persisting nothing.
#[tokio::test]
async fn a_key_outside_the_published_key_shape_is_refused() {
    // Rows: no separator, a trailing separator, two separators, a leading
    // separator, a character outside the published alphabet, a non-letter first
    // character, and one character past the published 50-character bound.
    let malformed: &[&str] = &[
        "NoSeparator",
        "RoleManager/",
        "RoleManager/Nested/Property",
        "/RoleManager",
        "Role Manager/Enabled",
        "9RoleManager/Enabled",
    ];
    let over_long = served_key(&"B".repeat(50 - SERVED_NAMESPACE.len()));
    assert_eq!(
        over_long.chars().count(),
        51,
        "the over-long row must be one past the published 50"
    );

    for key in malformed
        .iter()
        .map(|key| (*key).to_string())
        .chain([over_long])
    {
        let router = router();
        let held = vec![param(&served_key("Held"), "kept")];
        put_properties_ok(&router, "the fixture property", &held).await;

        // Rows also differ in whether the malformed key travels alone or beside a
        // well-formed one.
        for properties in [
            vec![param(&key, "value")],
            vec![
                param(&key, "value"),
                param(&served_key("Held"), "changed"),
            ],
        ] {
            let case = format!("{properties:?}");
            let (status, body) = put_properties(&router, &properties).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {case:?}: expected ValidationError, got {:?}",
                error_code(&body)
            );
            let reported = current_properties(&router, &case).await;
            assert_eq!(
                reported,
                with_defaults(&held),
                "case {case:?}: a refused write persists nothing, got {reported:?}"
            );
        }
    }

    // The published key bound itself is accepted, and so is the whole alphabet.
    let router = router();
    let at_bound = served_key(&"B".repeat(50 - SERVED_NAMESPACE.len() - 1));
    assert_eq!(
        at_bound.chars().count(),
        50,
        "the row at the bound must be exactly the published 50"
    );
    for key in [at_bound, served_key("B"), served_key("b9_-")] {
        let properties = vec![param(&key, "accepted")];
        put_properties_ok(&router, &key, &properties).await;
        let reported = current_properties(&router, &key).await;
        assert_eq!(
            reported.get(&key).map(String::as_str),
            Some("accepted"),
            "case {key:?}: expected the key at the bound to be stored, got {reported:?}"
        );
    }
}

/// RULE: a value outside the published length of its modelled type is refused
/// before the operation runs, so a value the model forbids never reaches the store,
/// while each bound itself is accepted.
#[tokio::test]
async fn a_value_outside_the_published_length_is_refused() {
    let router = router();
    // `accountPropertyValueType` is length 1..1024: one past each bound.
    for out_of_range in ["".to_string(), "v".repeat(1025)] {
        let case = format!("{} characters", out_of_range.chars().count());
        let properties = vec![param(&served_key("Value"), &out_of_range)];
        let (status, body) = put_properties(&router, &properties).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the modelled value length to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }
    assert_eq!(
        current_properties(&router, "after the refusals").await,
        default_properties(),
        "a refused write persists nothing"
    );

    // Each published bound itself is accepted and round-trips unchanged.
    for in_range in ["v".to_string(), "v".repeat(1024)] {
        let case = format!("{} characters", in_range.chars().count());
        let properties = vec![param(&served_key("Value"), &in_range)];
        put_properties_ok(&router, &case, &properties).await;
        let reported = current_properties(&router, &case).await;
        assert_eq!(
            reported.get(&served_key("Value")).map(String::as_str),
            Some(in_range.as_str()),
            "case {case:?}: expected the value at the bound to be stored, got {reported:?}"
        );
    }
}

/// RULE: `Properties` is a required member of this write, so a request that names
/// no property at all is refused rather than treated as a write of nothing.
#[tokio::test]
async fn a_write_naming_no_property_is_refused() {
    let router = router();
    let held = vec![param(&served_key("Held"), "kept")];
    put_properties_ok(&router, "the fixture property", &held).await;

    let (status, body) = put_properties(&router, &[]).await;
    assert_eq!(
        status, 400,
        "expected a request carrying no Properties to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "MalformedInput",
        "expected the missing required member to be refused as malformed, got {:?}",
        error_code(&body)
    );
    let reported = current_properties(&router, "after the refusal").await;
    assert_eq!(
        reported,
        with_defaults(&held),
        "a refused write persists nothing, got {reported:?}"
    );
}

/// RULE: the set of properties one account holds is bounded, so a write that would
/// carry the account past the bound is refused as a whole and stores nothing, while
/// a write that only re-sets properties the account already holds is accepted
/// however full the account is.
#[tokio::test]
async fn a_write_past_the_bound_on_the_held_properties_is_refused() {
    // The bound itself, and the rows around it: the account filled exactly to it,
    // one fresh key past it, a whole request past it, and a re-set at the bound.
    let bound = state::MAX_ACCOUNT_PROPERTIES_PER_ACCOUNT;
    let filled = router();

    // The account already holds its defaults, so what fills it to the bound is the
    // bound less those.
    let fresh_to_bound = bound - default_properties().len();
    let at_bound: Vec<(String, String)> = (1..=fresh_to_bound)
        .map(|position| param(&served_key(&format!("P{position}")), "v"))
        .collect();
    put_properties_ok(&filled, "the account filled to the bound", &at_bound).await;
    assert_eq!(
        current_properties(&filled, "at the bound").await.len(),
        bound,
        "expected the account to hold the bound"
    );

    // One fresh key past the bound, and a request of several fresh keys: each is
    // refused and leaves the held set exactly as it was.
    for fresh in [
        vec![param(&served_key("Past"), "v")],
        vec![
            param(&served_key("PastOne"), "v"),
            param(&served_key("PastTwo"), "v"),
        ],
    ] {
        let case = format!("{fresh:?}");
        let (status, body) = put_properties(&filled, &fresh).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
        let held = current_properties(&filled, &case).await;
        assert_eq!(
            held.len(),
            bound,
            "case {case:?}: a refused write stores nothing, got {} properties",
            held.len()
        );
    }

    // Re-setting held keys is not growth, so it is accepted at the bound.
    let reset = vec![param(&served_key("P1"), "changed")];
    put_properties_ok(&filled, "a re-set at the bound", &reset).await;
    let held = current_properties(&filled, "after the re-set").await;
    assert_eq!(
        held.get(&served_key("P1")).map(String::as_str),
        Some("changed"),
        "expected the re-set value to be stored, got {:?}",
        held.get(&served_key("P1"))
    );
    assert_eq!(
        held.len(),
        bound,
        "a re-set adds nothing, got {} properties",
        held.len()
    );

    // And a single request carrying more pairs than the bound is refused on a fresh
    // account, so nothing past the bound is ever stored.
    let fresh_router = router();
    let past_bound: Vec<(String, String)> = (1..=bound + 1)
        .map(|position| param(&served_key(&format!("P{position}")), "v"))
        .collect();
    let (status, body) = put_properties(&fresh_router, &past_bound).await;
    assert_eq!(
        status, 400,
        "expected a request of {} pairs to be refused, got {status}: {body}",
        past_bound.len()
    );
    assert_eq!(
        current_properties(&fresh_router, "after the over-large request").await,
        default_properties(),
        "a refused write stores nothing"
    );
}

/// RULE: a reporting operation changes nothing a later caller sees, however many
/// times it is called — the properties a read reports are the ones the next read
/// reports.
#[tokio::test]
async fn reading_the_account_properties_changes_nothing_a_later_caller_sees() {
    let router = router();
    // Rows: an account holding only its defaults, then one holding each catalog row
    // in turn, so the invariant is asserted over a growing store.
    let mut expected = default_properties();
    for properties in [Vec::new()].into_iter().chain(property_catalog()) {
        let case = format!("{properties:?}");
        if !properties.is_empty() {
            put_properties_ok(&router, &case, &properties).await;
            for (key, value) in &properties {
                expected.insert(key.clone(), value.clone());
            }
        }
        for read in 0..3 {
            let reported = current_properties(&router, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: read {read} must report {expected:?}, got {reported:?}"
            );
        }
    }
}
