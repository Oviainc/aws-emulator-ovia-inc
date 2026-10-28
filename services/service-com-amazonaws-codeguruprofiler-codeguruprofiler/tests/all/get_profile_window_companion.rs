// SPDX-License-Identifier: CC0-1.0

//! The COMPANION rule on `GetProfile`'s window members, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: each of this operation's two window BOUNDS must be
//! accompanied by one of the companions its own member doc allows — the other bound, or the
//! window's length — so a request naming one bound and neither companion names no window at
//! all and is refused with HTTP 400 `ValidationException` naming the member it carried and
//! the two members it may add, before any lookup; the only window member this operation
//! serves on its own is the length.
//!
//! SOURCE: the two bound members state the rule themselves — "If you specify `startTime`,
//! then you must also specify `period` or `endTime`, but not both"
//! (`smithy_query(query="doc", shape="GetProfileRequest", member="startTime")`), the mirror
//! sentence on `endTime`, and "To get the latest aggregated profile, specify only `period`"
//! (`member="period"`).
//!
//! Rows vary what the rule generalises over: WHICH bound is named alone (the start, the
//! end), where on the time axis it sits (before the epoch, at the epoch, a current-era
//! instant, a far-future one), whether it carries a sub-second fraction, and — for the
//! namings the rule allows — which companion completes it (the other bound, a length spelled
//! in days, in minutes, with every field), the length on its own, and nothing at all. Rows
//! refused by this operation's OTHER window rules are in the table too, so the companion
//! sentence is pinned to the companion rule and not to "the request was refused".
//!
//! The sibling rules for the same operation are [`crate::get_profile_interval`] (the window
//! must be an interval) and [`crate::get_profile_window_length`] (how long it may be).
//!
//! The emulator and the HTTP plumbing are the shared [`crate::profiling_group_lifecycle`]
//! harness, not a fork of it.

use crate::interval_validation::create_group;
use crate::profiling_group_lifecycle::{
    Harness, servable_window, servable_window_from_start, servable_window_to_end,
};

/// The group the rows run against, created through the API like any other.
const GROUP: &str = "get-profile-companion";

/// A name no group is stored under, so a row can show the refusal is answered ahead of the
/// group lookup (which would be a 404).
const MISSING_GROUP: &str = "get-profile-companion-never-created";

/// The profile every case here posts, in the representation it is posted under. It carries
/// the `start` a submitted profile must state, which is what `PostAgentProfile` parses out
/// of it.
const PROFILE: &[u8] = br#"{"start":1600000000000,"callgraph":{"C":{}},"samples":3}"#;
const JSON: &str = "application/json";

/// How a request names the window it asks about.
#[derive(Debug)]
enum Named {
    /// One bound and no companion — the naming the rule refuses.
    BoundAlone {
        member: &'static str,
        at: &'static str,
    },
    /// Both bounds.
    BothBounds {
        start: &'static str,
        end: &'static str,
    },
    /// One bound and the window's length.
    BoundAndPeriod {
        member: &'static str,
        at: &'static str,
        period: &'static str,
    },
    /// The length on its own, which the `period` doc allows.
    PeriodAlone(&'static str),
    /// No window member at all.
    Nothing,
    /// All three members, which the operation's doc refuses for its own reason.
    AllThree {
        start: &'static str,
        end: &'static str,
        period: &'static str,
    },
}

impl Named {
    /// The query string this window is asked for with.
    fn query(&self) -> String {
        match self {
            Named::BoundAlone { member, at } => format!("?{member}={at}"),
            Named::BothBounds { start, end } => format!("?startTime={start}&endTime={end}"),
            Named::BoundAndPeriod { member, at, period } => {
                format!("?{member}={at}&period={period}")
            }
            Named::PeriodAlone(period) => format!("?period={period}"),
            Named::Nothing => String::new(),
            Named::AllThree { start, end, period } => {
                format!("?startTime={start}&endTime={end}&period={period}")
            }
        }
    }
}

/// What the companion rule says about the naming a row carries.
#[derive(Debug)]
enum Expected {
    /// Refused by this rule: the request named `named` and neither of its companions.
    MissingCompanion {
        named: &'static str,
        companion: &'static str,
    },
    /// A naming the operation's doc allows: the companion rule has nothing to refuse, so
    /// the answer is about the DATA (served, or the no-data 404) and never a 400.
    Allowed,
    /// Refused, but by one of the operation's other window rules — so the answer must not
    /// carry the companion sentence.
    OtherRefusal,
}

#[derive(Debug)]
struct Row {
    label: &'static str,
    named: Named,
    expected: Expected,
}

/// The sentence a bound without a companion is refused with, built from the row's own
/// members rather than written out per row. Written out here rather than imported, so this
/// suite pins the WIRE text.
fn expected_refusal(named: &str, companion: &str) -> String {
    format!(
        "If you specify {named}, then you must also specify period or {companion}, but not both"
    )
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        // ---- ONE BOUND, NO COMPANION ----
        Row {
            label: "a start alone, before the epoch",
            named: Named::BoundAlone {
                member: "startTime",
                at: "1960-06-15T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "startTime",
                companion: "endTime",
            },
        },
        Row {
            label: "a start alone, at the epoch",
            named: Named::BoundAlone {
                member: "startTime",
                at: "1970-01-01T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "startTime",
                companion: "endTime",
            },
        },
        Row {
            label: "a start alone, carrying the member doc's millisecond fraction",
            named: Named::BoundAlone {
                member: "startTime",
                at: "2020-06-01T13:15:02.001Z",
            },
            expected: Expected::MissingCompanion {
                named: "startTime",
                companion: "endTime",
            },
        },
        Row {
            label: "a start alone, far in the future",
            named: Named::BoundAlone {
                member: "startTime",
                at: "2099-01-01T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "startTime",
                companion: "endTime",
            },
        },
        Row {
            label: "an end alone, before the epoch",
            named: Named::BoundAlone {
                member: "endTime",
                at: "1960-06-15T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "endTime",
                companion: "startTime",
            },
        },
        Row {
            label: "an end alone, at the epoch",
            named: Named::BoundAlone {
                member: "endTime",
                at: "1970-01-01T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "endTime",
                companion: "startTime",
            },
        },
        Row {
            label: "an end alone, carrying a sub-millisecond fraction",
            named: Named::BoundAlone {
                member: "endTime",
                at: "2026-03-04T05:06:07.123456Z",
            },
            expected: Expected::MissingCompanion {
                named: "endTime",
                companion: "startTime",
            },
        },
        Row {
            label: "an end alone, far in the future",
            named: Named::BoundAlone {
                member: "endTime",
                at: "2099-01-01T00:00:00Z",
            },
            expected: Expected::MissingCompanion {
                named: "endTime",
                companion: "startTime",
            },
        },
        // ---- A BOUND WITH ONE OF ITS COMPANIONS ----
        Row {
            label: "both bounds, a day apart",
            named: Named::BothBounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-02T00:00:00Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "both bounds, a millisecond apart before the epoch",
            named: Named::BothBounds {
                start: "1960-06-15T00:00:00.001Z",
                end: "1960-06-15T00:00:00.002Z",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a start and a day-long length",
            named: Named::BoundAndPeriod {
                member: "startTime",
                at: "1960-06-15T00:00:00Z",
                period: "P1D",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "a start and a length spelled with every field",
            named: Named::BoundAndPeriod {
                member: "startTime",
                at: "2026-01-01T00:00:00Z",
                period: "P1DT1H1M1S",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "an end and a length spelled in minutes",
            named: Named::BoundAndPeriod {
                member: "endTime",
                at: "2099-01-01T00:00:00Z",
                period: "PT5M",
            },
            expected: Expected::Allowed,
        },
        Row {
            label: "an end at the epoch and a length spelled in hours",
            named: Named::BoundAndPeriod {
                member: "endTime",
                at: "1970-01-01T00:00:00Z",
                period: "PT168H",
            },
            expected: Expected::Allowed,
        },
        // ---- THE ONLY MEMBER THIS OPERATION SERVES ON ITS OWN, AND NO MEMBER AT ALL ----
        Row {
            label: "a length alone, an hour long",
            named: Named::PeriodAlone("PT1H"),
            expected: Expected::Allowed,
        },
        Row {
            label: "a length alone, five minutes long",
            named: Named::PeriodAlone("PT5M"),
            expected: Expected::Allowed,
        },
        Row {
            label: "no window member at all",
            named: Named::Nothing,
            expected: Expected::Allowed,
        },
        // ---- REFUSED BY THE OPERATION'S OTHER WINDOW RULES ----
        // "If you specify all 3 parameters, an exception is thrown" — a different sentence
        // from this rule's, because a different rule refused it.
        Row {
            label: "all three members",
            named: Named::AllThree {
                start: "2026-01-01T00:00:00Z",
                end: "2026-01-02T00:00:00Z",
                period: "P1D",
            },
            expected: Expected::OtherRefusal,
        },
        // "The maximum time range allowed is 7 days": a PAIR wider than that is refused by
        // the length rule, which a lone bound used to escape altogether.
        Row {
            label: "both bounds, two months apart",
            named: Named::BothBounds {
                start: "2026-01-01T00:00:00Z",
                end: "2026-03-01T00:00:00Z",
            },
            expected: Expected::OtherRefusal,
        },
        Row {
            label: "a start and a length longer than the maximum",
            named: Named::BoundAndPeriod {
                member: "startTime",
                at: "2026-01-01T00:00:00Z",
                period: "P30D",
            },
            expected: Expected::OtherRefusal,
        },
        // A companion that is named but names no span is still a NAMED companion, so the
        // refusal is the one about the length's spelling.
        Row {
            label: "a start and a length that is no duration",
            named: Named::BoundAndPeriod {
                member: "startTime",
                at: "2026-01-01T00:00:00Z",
                period: "yesterday",
            },
            expected: Expected::OtherRefusal,
        },
        // A zero-length companion completes a window that is no interval, which the
        // interval rule refuses.
        Row {
            label: "an end and a zero-length companion",
            named: Named::BoundAndPeriod {
                member: "endTime",
                at: "2026-01-01T00:00:00Z",
                period: "PT0S",
            },
            expected: Expected::OtherRefusal,
        },
    ]
}

/// Every companion sentence this rule can answer with, for the rows that must NOT carry
/// one.
fn every_companion_refusal() -> [String; 2] {
    [
        expected_refusal("startTime", "endTime"),
        expected_refusal("endTime", "startTime"),
    ]
}

/// A group holding one aggregated profile, so every row reaches an operation that has
/// something to serve rather than only an absence.
async fn group_holding_a_profile(harness: &Harness) {
    create_group(harness, GROUP).await;
    let posted = harness
        .post_agent_profile(GROUP, JSON, None, PROFILE.to_vec())
        .await;
    assert_eq!(
        posted.status,
        204,
        "posting the profile must succeed, got {} {}",
        posted.status,
        String::from_utf8_lossy(&posted.body)
    );
    // The aggregation the posted profile is owed has to have fallen due, or every read
    // below would answer the pending-aggregation 404 for its own reason.
    harness.complete_aggregation(GROUP);
}

#[tokio::test]
async fn a_window_bound_without_a_companion_is_refused_whichever_bound_it_is() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    for row in rows() {
        let query = row.named.query();
        let case = format!("{} ({query})", row.label);
        let answer = harness.get_profile(GROUP, &query).await;

        match row.expected {
            Expected::MissingCompanion { named, companion } => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case}: expected HTTP 400, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case}: expected ValidationException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                let expected = expected_refusal(named, companion);
                assert_eq!(
                    answer.error_message(),
                    expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    answer.error_message()
                );
            }
            Expected::Allowed => {
                assert_ne!(
                    answer.status,
                    400,
                    "case {case}: a naming the operation's doc allows must not be refused, \
                     got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
            }
            Expected::OtherRefusal => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case}: expected HTTP 400 from another window rule, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let message = answer.error_message();
                for companion_sentence in every_companion_refusal() {
                    assert_ne!(
                        message, companion_sentence,
                        "case {case}: a request that named a companion must not be refused \
                         for a missing one, got {message:?}"
                    );
                }
            }
        }
    }

    // Each naming the doc allows really does SERVE the stored profile when it covers the
    // profile's receipt, so the non-refusals above are not a store that answers nothing
    // whatever it is asked — and that is the round trip for the two single-bound namings
    // this rule leaves standing.
    for (label, window) in [
        ("both bounds", servable_window()),
        ("a start and a length", servable_window_from_start()),
        ("an end and a length", servable_window_to_end()),
        ("a length alone", "?period=PT5M".to_string()),
    ] {
        let served = harness.get_profile(GROUP, &window).await;
        assert_eq!(
            served.status,
            200,
            "case {label:?} ({window}): a window around the stored profile must serve it, \
             got {} {}",
            served.status,
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.body,
            PROFILE.to_vec(),
            "case {label:?} ({window}): that window must serve the posted profile, got {:?}",
            String::from_utf8_lossy(&served.body)
        );
    }
}

#[tokio::test]
async fn the_companion_rule_is_applied_before_the_group_is_looked_up() {
    let harness = Harness::new();

    for row in rows() {
        let Expected::MissingCompanion { named, companion } = row.expected else {
            continue;
        };
        let query = row.named.query();
        let case = format!("{} ({query})", row.label);
        // No group of this name was ever created, so a lookup would answer 404: the 400
        // shows the members are checked first.
        let answer = harness.get_profile(MISSING_GROUP, &query).await;
        assert_eq!(
            answer.status,
            400,
            "case {case}: a bound without a companion must be refused before the group \
             lookup, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected = expected_refusal(named, companion);
        assert_eq!(
            answer.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.error_message()
        );
    }
}

#[tokio::test]
async fn a_bound_without_a_companion_names_no_window_at_all() {
    let harness = Harness::new();
    group_holding_a_profile(&harness).await;

    // The knock-on the rule carries: a lone bound is not a half-open window, so no request
    // can select over a range the operation's documented maximum ("The maximum time range
    // allowed is 7 days") does not bound. The same bound is refused alone, and refused
    // again once a companion stretches it past that maximum — the second refusal being the
    // length rule's, which only a PAIR of bounds can reach.
    for (member, companion, at, over_long) in [
        ("startTime", "endTime", "2000-01-01T00:00:00Z", "P3650D"),
        ("endTime", "startTime", "2099-01-01T00:00:00Z", "P30D"),
    ] {
        let alone = harness.get_profile(GROUP, &format!("?{member}={at}")).await;
        let expected = expected_refusal(member, companion);
        assert_eq!(
            alone.error_message(),
            expected,
            "case {member} alone: expected {expected:?}, got {:?}",
            alone.error_message()
        );

        let stretched = harness
            .get_profile(GROUP, &format!("?{member}={at}&period={over_long}"))
            .await;
        assert_eq!(
            stretched.status,
            400,
            "case {member} with {over_long}: a range past the documented maximum must be \
             refused, got {} {}",
            stretched.status,
            String::from_utf8_lossy(&stretched.body)
        );
        assert!(
            stretched
                .error_message()
                .contains("must be at most P7D long"),
            "case {member} with {over_long}: expected the length refusal, got {:?}",
            stretched.error_message()
        );
    }

    // And a refused request changed nothing a later caller sees: the profile still reads
    // back through a naming the doc allows.
    let served = harness.get_profile(GROUP, &servable_window()).await;
    assert_eq!(
        served.status,
        200,
        "the stored profile must still be servable, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );
    assert_eq!(
        served.body,
        PROFILE.to_vec(),
        "the stored profile must still read back byte for byte, got {:?}",
        String::from_utf8_lossy(&served.body)
    );
}
