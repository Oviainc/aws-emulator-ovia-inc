// SPDX-License-Identifier: CC0-1.0

//! MemoryDB `ResetParameterGroup`, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a reset takes the caller's stored overrides back OFF the group — all of them when
//!   `AllParameters` is true, otherwise exactly the parameters `ParameterNames` names —
//!   so each reset parameter reports its family's engine default and `system` as its
//!   source again, while every override the request did not name keeps the value an
//!   update set;
//! * resetting every override returns the group to EXACTLY the listing it had before any
//!   update, for any family the engine catalog publishes;
//! * the two selectors are exclusive and one of them is required: naming both, or
//!   neither, is the modelled combination refusal;
//! * a name the group's family does not carry is refused and NOTHING is written, so a
//!   batch carrying one bad name leaves the whole group as it was;
//! * a group that does not exist, and one the service itself owns, are refused with the
//!   modelled faults;
//! * every refusal states the rule it broke in the service's own sentence for that
//!   rule, the same sentence for every input that breaks it;
//! * a reset is idempotent (resetting the same names twice leaves the same listing) and
//!   is recorded as a `parameter-group` event the way every other change to a group is.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::parameters::{create_group, field, parameters, update};

/// How a reset selects what it resets, over the forms the model admits for the two
/// selectors (`ResetParameterGroup$AllParameters`, `$ParameterNames`).
#[derive(Debug, Clone, Copy)]
enum Select {
    /// `AllParameters: true`, with no `ParameterNames` member at all.
    All,
    /// `AllParameters: true` beside an EMPTY name list, which names nothing and so is
    /// the same request as [`Select::All`].
    AllWithEmptyNames,
    /// `AllParameters` left at its modelled default (false), with the names listed.
    Named(&'static [&'static str]),
    /// `AllParameters: false` spelled out beside the names.
    NamedWithFalse(&'static [&'static str]),
}

/// The `ResetParameterGroup` request body for one selector.
fn reset_body(group: &str, select: Select) -> Value {
    match select {
        Select::All => json!({ "ParameterGroupName": group, "AllParameters": true }),
        Select::AllWithEmptyNames => json!({
            "ParameterGroupName": group,
            "AllParameters": true,
            "ParameterNames": Vec::<String>::new(),
        }),
        Select::Named(names) => json!({ "ParameterGroupName": group, "ParameterNames": names }),
        Select::NamedWithFalse(names) => json!({
            "ParameterGroupName": group,
            "AllParameters": false,
            "ParameterNames": names,
        }),
    }
}

/// One `ResetParameterGroup` call.
async fn reset(router: &MultiServiceRouter, group: &str, select: Select) -> (u16, Value) {
    call(router, "ResetParameterGroup", &reset_body(group, select)).await
}

/// One row of the reset table: a group of `family` carrying the overrides `set`, reset
/// through `select`, after which exactly `cleared` must be back at the family default.
struct ResetCase {
    case: &'static str,
    /// A family the engine catalog publishes (`DescribeEngineVersions`:
    /// `ParameterGroupFamily`).
    family: &'static str,
    /// The overrides an `UpdateParameterGroup` persists before the reset.
    set: &'static [(&'static str, &'static str)],
    select: Select,
    /// The overrides the reset must take off; every other name in `set` must keep the
    /// value the update persisted for it.
    cleared: &'static [&'static str],
}

/// The reset cases, differing along the family (so along the engine line whose settings
/// the group carries), the selector form, the NUMBER of parameters reset (none, one,
/// some, every one of them), and whether a named parameter was ever overridden at all.
///
/// The parameter names and values are settings the catalog publishes for the family of
/// the row: `lua-time-limit`/`hash-max-ziplist-entries` only exist on the Redis 6 line,
/// `busy-reply-threshold`/`hash-max-listpack-entries` only from Redis 7 on, and
/// `search-enabled` only on the Valkey line (see the catalog suite in `parameters`).
const RESET_CASES: &[ResetCase] = &[
    ResetCase {
        case: "one override of several, named",
        family: "memorydb_redis7",
        set: &[
            ("maxmemory-policy", "allkeys-lru"),
            ("timeout", "300"),
            ("busy-reply-threshold", "1000"),
        ],
        select: Select::Named(&["timeout"]),
        cleared: &["timeout"],
    },
    ResetCase {
        case: "several overrides in one request",
        family: "memorydb_valkey7",
        set: &[
            ("activerehashing", "no"),
            ("search-enabled", "yes"),
            ("active-expire-effort", "10"),
        ],
        select: Select::Named(&["activerehashing", "search-enabled"]),
        cleared: &["activerehashing", "search-enabled"],
    },
    ResetCase {
        case: "every override named, AllParameters spelled false",
        family: "memorydb_redis6",
        set: &[
            ("lua-time-limit", "1000"),
            ("hash-max-ziplist-entries", "256"),
        ],
        select: Select::NamedWithFalse(&["lua-time-limit", "hash-max-ziplist-entries"]),
        cleared: &["lua-time-limit", "hash-max-ziplist-entries"],
    },
    ResetCase {
        case: "one name repeated in one request",
        family: "memorydb_redis7",
        set: &[("timeout", "300"), ("maxmemory-policy", "volatile-lru")],
        select: Select::Named(&["timeout", "timeout"]),
        cleared: &["timeout"],
    },
    ResetCase {
        case: "a name the group never overrode",
        family: "memorydb_valkey7",
        set: &[("timeout", "300")],
        select: Select::Named(&["notify-keyspace-events"]),
        cleared: &[],
    },
    ResetCase {
        case: "AllParameters over a group with overrides",
        family: "memorydb_redis7",
        set: &[
            ("maxmemory-policy", "noeviction"),
            ("hash-max-listpack-entries", "256"),
            ("notify-keyspace-events", "KEA"),
        ],
        select: Select::All,
        cleared: &[
            "maxmemory-policy",
            "hash-max-listpack-entries",
            "notify-keyspace-events",
        ],
    },
    ResetCase {
        case: "AllParameters beside an empty name list",
        family: "memorydb_redis6",
        set: &[("activerehashing", "no"), ("timeout", "300")],
        select: Select::AllWithEmptyNames,
        cleared: &["activerehashing", "timeout"],
    },
    ResetCase {
        case: "AllParameters over a group with no overrides",
        family: "memorydb_valkey7",
        set: &[],
        select: Select::All,
        cleared: &[],
    },
];

/// The `Value` and the `Source` one group reports for one parameter.
fn reported(listing: &BTreeMap<String, Value>, name: &str, case: &str) -> (String, String) {
    let parameter = listing
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected parameter {name:?} in the listing"));
    (
        field(parameter, "Value", case),
        field(parameter, "Source", case),
    )
}

/// A reset takes exactly the overrides it names off the group, leaves every other
/// override standing, and reports each reset parameter at its family's engine default
/// with `system` as its source again.
#[tokio::test]
async fn a_reset_takes_the_named_overrides_off_the_group() {
    let router = router();

    for (index, row) in RESET_CASES.iter().enumerate() {
        let case = row.case;
        // A group name holds no underscore (the service's name rule), so the row
        // contributes its index rather than its family's spelling.
        let group = format!("pg-reset-{index}");
        create_group(&router, &group, row.family).await;

        // The listing BEFORE any update: the family's engine defaults, which is what
        // every reset parameter must report again.
        let defaults = parameters(&router, &group).await;
        assert!(
            !defaults.is_empty(),
            "case {case:?}: a published family must carry engine defaults"
        );

        if !row.set.is_empty() {
            let (status, body) = update(&router, &group, row.set).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} with {body}"
            );
            let updated = parameters(&router, &group).await;
            for (name, value) in row.set {
                let (reported_value, source) = reported(&updated, name, case);
                assert_eq!(
                    (reported_value.as_str(), source.as_str()),
                    (*value, "user"),
                    "case {case:?}: expected {name:?} to carry the value the update set, \
                     as the caller's"
                );
            }
        }

        let (status, body) = reset(&router, &group, row.select).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ResetParameterGroup to answer 200, got {status} \
             with {body}"
        );
        // The answer is the group that was reset, as the store holds it.
        let answered = body
            .get("ParameterGroup")
            .unwrap_or_else(|| panic!("case {case:?}: expected ParameterGroup in {body}"));
        assert_eq!(
            field(answered, "Name", case),
            group,
            "case {case:?}: expected the reset group to be answered, got {answered}"
        );
        assert_eq!(
            field(answered, "Family", case),
            row.family,
            "case {case:?}: expected the group's own family, got {answered}"
        );

        let after = parameters(&router, &group).await;
        for name in row.cleared {
            let (value, source) = reported(&after, name, case);
            let (default_value, _) = reported(&defaults, name, case);
            assert_eq!(
                (value.as_str(), source.as_str()),
                (default_value.as_str(), "system"),
                "case {case:?}: expected {name:?} back at its family default, as the \
                 engine's"
            );
        }
        for (name, value) in row.set {
            if row.cleared.contains(name) {
                continue;
            }
            let (reported_value, source) = reported(&after, name, case);
            assert_eq!(
                (reported_value.as_str(), source.as_str()),
                (*value, "user"),
                "case {case:?}: an override the reset did not name must stand, for \
                 {name:?}"
            );
        }

        // Resetting EVERY override returns the group to exactly the listing it had
        // before any update — nothing else in the family's catalog moved.
        let every_override_reset = row
            .set
            .iter()
            .all(|(name, _)| row.cleared.contains(name));
        if every_override_reset {
            assert_eq!(
                after, defaults,
                "case {case:?}: a group whose every override was reset must report its \
                 family's defaults again"
            );
        }

        // A reset is idempotent: asking for the same reset again changes nothing a
        // later caller sees.
        let (status, body) = reset(&router, &group, row.select).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated reset to answer 200, got {status} \
             with {body}"
        );
        let again = parameters(&router, &group).await;
        assert_eq!(
            again, after,
            "case {case:?}: a repeated reset must leave the same listing"
        );
    }
}

/// A reset the service cannot act on is refused with the modelled fault and writes
/// NOTHING: the group is left exactly as it was, down to each override's source.
#[tokio::test]
async fn a_refused_reset_leaves_the_group_exactly_as_it_was() {
    const FAMILY: &str = "memorydb_redis7";
    let router = router();
    let group = "pg-reset-refusals";
    create_group(&router, group, FAMILY).await;
    let overrides: &[(&str, &str)] = &[("timeout", "300"), ("maxmemory-policy", "allkeys-lru")];
    let (status, body) = update(&router, group, overrides).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the update to answer 200, got {status} with {body}",
        "the overrides under test"
    );
    let before = parameters(&router, group).await;

    // Each row is a reset the service refuses, with the modelled fault and status it
    // answers: a selector combination it cannot act on, a name the group's family does
    // not carry, or a group it cannot resolve.
    let refusals: Vec<(&str, Value, &str, u16)> = vec![
        (
            "both selectors at once",
            json!({
                "ParameterGroupName": group,
                "AllParameters": true,
                "ParameterNames": ["timeout"],
            }),
            "InvalidParameterCombinationException",
            400,
        ),
        (
            "neither selector",
            json!({ "ParameterGroupName": group }),
            "InvalidParameterCombinationException",
            400,
        ),
        (
            "AllParameters false with no names",
            json!({ "ParameterGroupName": group, "AllParameters": false }),
            "InvalidParameterCombinationException",
            400,
        ),
        (
            "an empty name list",
            json!({ "ParameterGroupName": group, "ParameterNames": Vec::<String>::new() }),
            "InvalidParameterCombinationException",
            400,
        ),
        (
            // A setting of ANOTHER engine line: the Redis 6 spelling of the
            // collection-encoding parameters is not carried by a Redis 7 family.
            "a name the group's family does not carry",
            json!({
                "ParameterGroupName": group,
                "ParameterNames": ["hash-max-ziplist-entries"],
            }),
            "InvalidParameterValueException",
            400,
        ),
        (
            "a name no family carries",
            json!({ "ParameterGroupName": group, "ParameterNames": ["not-a-parameter"] }),
            "InvalidParameterValueException",
            400,
        ),
        (
            // ALL OR NOTHING: one unusable name refuses the whole request, so the
            // overrides the other names would have reset are still standing below.
            "one bad name in a batch of good ones",
            json!({
                "ParameterGroupName": group,
                "ParameterNames": ["timeout", "not-a-parameter"],
            }),
            "InvalidParameterValueException",
            400,
        ),
        (
            // On this `awsJson1_1` service the request URI names the endpoint and never
            // a resource, so a fault about the request's CONTENT is a client fault and
            // answers 400 whatever `@httpError` its shape declares (the rule the
            // `fault_http_status` suite pins for every other operation).
            "a group that does not exist",
            json!({ "ParameterGroupName": "pg-reset-absent", "AllParameters": true }),
            "ParameterGroupNotFoundFault",
            400,
        ),
        (
            // The groups MemoryDB itself provides hold their family's engine defaults
            // and are not the caller's to change — and the dotted name they carry is
            // not a well-formed identifier either, which is the rule this one breaks
            // first (the wording suite below pins which sentence says so).
            "a default parameter group",
            json!({ "ParameterGroupName": "default.memorydb-redis7", "AllParameters": true }),
            "InvalidParameterValueException",
            400,
        ),
    ];

    for (case, request, expected_code, expected_status) in refusals {
        let (status, body) = call(&router, "ResetParameterGroup", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} with {body}"
        );
        assert!(
            body.get("message")
                .or_else(|| body.get("Message"))
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a refusal message, got {body}"
        );
        let after = parameters(&router, group).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused reset must leave the group exactly as it was"
        );
    }

    // A default group is refused, so it still reports its family's engine defaults.
    let default_group = parameters(&router, "default.memorydb-redis7").await;
    for (name, parameter) in &default_group {
        let case = format!("default group/{name}");
        assert_eq!(
            field(parameter, "Source", &case),
            "system",
            "case {case:?}: a default group carries nothing but engine defaults"
        );
    }
}

/// The `message` of a refusal body.
fn message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

/// The sentence a reset answers a request naming BOTH selectors with: the flag is named
/// by the public name the service words this refusal with, and nothing is interpolated.
const BOTH_SELECTORS: &str = "May not specify both ResetAllParameters and a list of \
                              parameters to reset.";

/// The sentence a reset answers a request naming NEITHER selector with — the other half
/// of the same exclusivity rule, equally fixed.
const NO_SELECTOR: &str = "Must specify either ResetAllParameters or a list of \
                           parameters to reset.";

/// The sentence a reset answers a `ParameterGroupName` that is not a well-formed
/// MemoryDB identifier with: the request MEMBER at fault, then the rule for identifiers
/// (`crate::parameter_groups` pins the same sentence for the create).
const NOT_AN_IDENTIFIER: &str = "The parameter ParameterGroupName is not a valid \
                                 identifier. Identifiers must begin with a letter; must \
                                 contain only ASCII letters, digits, and hyphens; and \
                                 must not end with a hyphen or contain two consecutive \
                                 hyphens.";

/// The sentence a reset answers a group this store holds none of with: fixed text, which
/// reports back no name.
const NOT_FOUND: &str = "Parameter Group not found.";

/// A name one character past the service's 40-character identifier bound.
const OVERLONG_NAME: &str = "pg-reset-name-that-is-one-character-too-l";

/// A name list of `repeats` copies of `repeated` followed by `last`, for the rows about
/// how LONG a list the service takes rather than what the names say.
fn names_repeating(repeated: &str, repeats: usize, last: &str) -> Vec<String> {
    let mut names: Vec<String> = vec![repeated.to_string(); repeats];
    names.push(last.to_string());
    names
}

/// One row of the refusal-wording table: the request, and the error class and the
/// EXACT sentence the service answers it with.
struct RefusalCase {
    case: &'static str,
    request: fn(&str) -> Value,
    code: &'static str,
    message: &'static str,
}

/// Every class of reset refusal, each over several inputs that break the SAME rule, with
/// the one sentence the service states that rule in.
///
/// Each row's request is built from the name of a group that exists, carries overrides
/// and is of family `memorydb_redis7` — except the rows that are about a name, which name
/// their own.
const REFUSAL_WORDING: &[RefusalCase] = &[
    // The two selectors are mutually exclusive: whatever the list says, naming it beside
    // the reset-everything flag is the one refusal.
    RefusalCase {
        case: "both selectors, one name",
        request: |group| json!({
            "ParameterGroupName": group,
            "AllParameters": true,
            "ParameterNames": ["timeout"],
        }),
        code: "InvalidParameterCombinationException",
        message: BOTH_SELECTORS,
    },
    RefusalCase {
        case: "both selectors, several names",
        request: |group| json!({
            "ParameterGroupName": group,
            "AllParameters": true,
            "ParameterNames": ["timeout", "maxmemory-policy", "activerehashing"],
        }),
        code: "InvalidParameterCombinationException",
        message: BOTH_SELECTORS,
    },
    RefusalCase {
        case: "both selectors, one name repeated",
        request: |group| json!({
            "ParameterGroupName": group,
            "AllParameters": true,
            "ParameterNames": ["timeout", "timeout"],
        }),
        code: "InvalidParameterCombinationException",
        message: BOTH_SELECTORS,
    },
    // One of them is required: an absent list and an empty one name nothing, and the
    // flag left at its modelled default (false) asks for nothing either.
    RefusalCase {
        case: "neither selector",
        request: |group| json!({ "ParameterGroupName": group }),
        code: "InvalidParameterCombinationException",
        message: NO_SELECTOR,
    },
    RefusalCase {
        case: "the flag spelled false, no list",
        request: |group| json!({ "ParameterGroupName": group, "AllParameters": false }),
        code: "InvalidParameterCombinationException",
        message: NO_SELECTOR,
    },
    RefusalCase {
        case: "the flag spelled false beside an empty list",
        request: |group| json!({
            "ParameterGroupName": group,
            "AllParameters": false,
            "ParameterNames": Vec::<String>::new(),
        }),
        code: "InvalidParameterCombinationException",
        message: NO_SELECTOR,
    },
    RefusalCase {
        case: "an empty list alone",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": Vec::<String>::new(),
        }),
        code: "InvalidParameterCombinationException",
        message: NO_SELECTOR,
    },
    // A name the group's FAMILY does not define: the refusal lists the names the request
    // carried that the family has no parameter for, in request order.
    RefusalCase {
        case: "a name no family carries",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": ["not-a-parameter"],
        }),
        code: "InvalidParameterValueException",
        message: "The following parameters are not defined for the specified group: \
                  not-a-parameter",
    },
    RefusalCase {
        // The Redis 6 spelling of the collection-encoding parameters, which the Redis 7
        // line replaced: a real MemoryDB parameter, defined for another family.
        case: "a name of another engine line",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": ["hash-max-ziplist-entries"],
        }),
        code: "InvalidParameterValueException",
        message: "The following parameters are not defined for the specified group: \
                  hash-max-ziplist-entries",
    },
    RefusalCase {
        case: "two undefined names among defined ones",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": ["timeout", "not-a-parameter", "hash-max-ziplist-entries"],
        }),
        code: "InvalidParameterValueException",
        message: "The following parameters are not defined for the specified group: \
                  not-a-parameter,hash-max-ziplist-entries",
    },
    RefusalCase {
        case: "one undefined name repeated",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": ["not-a-parameter", "not-a-parameter"],
        }),
        code: "InvalidParameterValueException",
        message: "The following parameters are not defined for the specified group: \
                  not-a-parameter",
    },
    // The per-request parameter limit: a list AT the bound is examined (and refused here
    // on the name the family does not define), one past it is refused as too long.
    RefusalCase {
        case: "a list at the per-request bound",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": names_repeating("timeout", 19, "not-a-parameter"),
        }),
        code: "InvalidParameterValueException",
        message: "The following parameters are not defined for the specified group: \
                  not-a-parameter",
    },
    RefusalCase {
        case: "a list one past the per-request bound",
        request: |group| json!({
            "ParameterGroupName": group,
            "ParameterNames": names_repeating("timeout", 20, "not-a-parameter"),
        }),
        code: "InvalidParameterCombinationException",
        message: "Cannot reset more than 20 parameters in a single request",
    },
    // A name that is not a well-formed identifier names no group at all, so it is
    // refused on the rule it broke — before the named group is resolved, and before
    // whose group it would be is read.
    RefusalCase {
        case: "a dotted name, which is what the service's own default groups are called",
        request: |_| json!({
            "ParameterGroupName": "default.memorydb-redis7",
            "AllParameters": true,
        }),
        code: "InvalidParameterValueException",
        message: NOT_AN_IDENTIFIER,
    },
    RefusalCase {
        case: "a name beginning with a digit",
        request: |_| json!({ "ParameterGroupName": "9pg-reset", "AllParameters": true }),
        code: "InvalidParameterValueException",
        message: NOT_AN_IDENTIFIER,
    },
    RefusalCase {
        case: "a name ending with a hyphen",
        request: |_| json!({ "ParameterGroupName": "pg-reset-", "AllParameters": true }),
        code: "InvalidParameterValueException",
        message: NOT_AN_IDENTIFIER,
    },
    RefusalCase {
        case: "a name with two consecutive hyphens",
        request: |_| json!({ "ParameterGroupName": "pg--reset", "AllParameters": true }),
        code: "InvalidParameterValueException",
        message: NOT_AN_IDENTIFIER,
    },
    RefusalCase {
        case: "a name carrying a character outside the identifier set",
        request: |_| json!({ "ParameterGroupName": "pg_reset", "AllParameters": true }),
        code: "InvalidParameterValueException",
        message: NOT_AN_IDENTIFIER,
    },
    RefusalCase {
        // ONE character past the bound, so the length rule is what it breaks rather than
        // the spelling rule — the 40-character name below is refused as absent instead.
        case: "a name one character too long",
        request: |_| json!({ "ParameterGroupName": OVERLONG_NAME, "AllParameters": true }),
        code: "InvalidParameterValueException",
        message: "Parameter group name length must be less than 40.",
    },
    // A well-formed name this store holds no group for, whatever the request asks of it.
    RefusalCase {
        case: "a group that does not exist",
        request: |_| json!({ "ParameterGroupName": "pg-reset-absent", "AllParameters": true }),
        code: "ParameterGroupNotFoundFault",
        message: NOT_FOUND,
    },
    RefusalCase {
        case: "a group that does not exist, with names",
        request: |_| json!({
            "ParameterGroupName": "pg-reset-absent",
            "ParameterNames": ["timeout"],
        }),
        code: "ParameterGroupNotFoundFault",
        message: NOT_FOUND,
    },
    RefusalCase {
        // The longest name the service admits: still absent, so still the not-found
        // sentence rather than a length refusal.
        case: "an absent group named at the length bound",
        request: |_| json!({
            "ParameterGroupName": "pg-reset-name-that-is-exactly-forty-char",
            "AllParameters": true,
        }),
        code: "ParameterGroupNotFoundFault",
        message: NOT_FOUND,
    },
];

/// Every reset the service refuses states the rule it broke in the service's own
/// sentence for that rule — the same sentence for every input that breaks it, with
/// nothing interpolated but the names the request itself carried.
#[tokio::test]
async fn a_refused_reset_states_the_rule_it_broke() {
    const FAMILY: &str = "memorydb_redis7";
    let router = router();
    let group = "pg-reset-wording";
    create_group(&router, group, FAMILY).await;
    let (status, body) = update(&router, group, &[("timeout", "300")]).await;
    assert_eq!(
        status, 200,
        "case {group:?}: expected the update to answer 200, got {status} with {body}"
    );
    let before = parameters(&router, group).await;
    assert_eq!(
        OVERLONG_NAME.chars().count(),
        41,
        "case {:?}: the overlong row must sit one character past the bound, it is {}",
        "a name one character too long",
        OVERLONG_NAME.chars().count()
    );

    for row in REFUSAL_WORDING {
        let case = row.case;
        let request = (row.request)(group);
        let (status, body) = call(&router, "ResetParameterGroup", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.code,
            "case {case:?}: expected {}, got {code} with {body}",
            row.code
        );
        let answered = message(&body, case);
        assert_eq!(
            answered, row.message,
            "case {case:?}: expected the message {:?}, got {answered:?}",
            row.message
        );
        let after = parameters(&router, group).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused reset must leave the group exactly as it was"
        );
    }
}

/// An applied reset is recorded against the group it changed, so `DescribeEvents`
/// reports it the way it reports every other change to a parameter group.
#[tokio::test]
async fn an_applied_reset_is_recorded_as_an_event() {
    let router = router();
    let group = "pg-reset-events";
    create_group(&router, group, "memorydb_redis7").await;
    let (status, body) = update(&router, group, &[("timeout", "300")]).await;
    assert_eq!(
        status, 200,
        "case {group:?}: expected the update to answer 200, got {status} with {body}"
    );
    let (status, body) = reset(&router, group, Select::All).await;
    assert_eq!(
        status, 200,
        "case {group:?}: expected the reset to answer 200, got {status} with {body}"
    );

    let body = ok(
        &router,
        "DescribeEvents",
        &json!({ "SourceName": group, "SourceType": "parameter-group" }),
        group,
    )
    .await;
    let messages: Vec<String> = body["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {group:?}: expected Events in {body}"))
        .iter()
        .map(|event| field(event, "Message", group))
        .collect();
    let modify = format!("Modify parameter group {group} operation completed.");
    assert_eq!(
        messages.iter().filter(|message| **message == modify).count(),
        2,
        "case {group:?}: the update and the reset each record one change, got {messages:?}"
    );
}
