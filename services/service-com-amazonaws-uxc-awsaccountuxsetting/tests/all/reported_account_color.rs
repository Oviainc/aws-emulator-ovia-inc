// SPDX-License-Identifier: CC0-1.0

//! The reported-colour rule, driven through BOTH operations that render a
//! customization record.
//!
//! One stored record reports one `accountColor`, whichever call a client asks
//! through: `GetAccountCustomizations` and the `UpdateAccountCustomizations` echo
//! of the record it just stored must answer the same member with the same value —
//! the stored colour, or the documented `none` while the account never configured
//! one ("A value of none indicates that you have not set a color",
//! `smithy_query(query="doc", shape="AccountColor")`).
//!
//! Every case goes through the real wire path (`crate::wire`), so the two
//! operations are compared as a client sees them.

use crate::wire::{build_router, get_customizations, update_customizations};
use serde_json::{json, Value};

/// The ten values of `AccountColor`
/// (`smithy_query(query="shape", shape="AccountColor")`) — the catalog of colours
/// a record can hold, including entries (`pink`, `orange`, `red`, …) the issue
/// never cited.
const MODELLED_COLORS: [&str; 10] = [
    "none",
    "pink",
    "purple",
    "darkBlue",
    "lightBlue",
    "teal",
    "green",
    "yellow",
    "orange",
    "red",
];

/// The `accountColor` member of a response body, which the rule says is always
/// present: panics when it is absent or not a string.
fn reported_color(case: &str, origin: &str, body: &Value) -> String {
    body.get("accountColor")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {origin} must report a string accountColor, got {body}")
        })
        .to_string()
}

/// One row: the requests that build the stored record, and the colour that record
/// is reported with.
struct Case {
    name: String,
    requests: Vec<Value>,
    expected: String,
}

/// The table spanning the class the rule generalises over: whether a colour was
/// ever configured, which of the modelled colours it is, whether the account's
/// other settings were configured instead or as well, and whether the colour was
/// reset back to the modelled `none` after being set.
fn cases() -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "never updated at all".to_string(),
            requests: vec![],
            expected: "none".to_string(),
        },
        Case {
            name: "an update naming no setting".to_string(),
            requests: vec![json!({})],
            expected: "none".to_string(),
        },
        Case {
            name: "only visibleServices configured".to_string(),
            requests: vec![json!({"visibleServices": ["s3", "lambda"]})],
            expected: "none".to_string(),
        },
        Case {
            name: "only visibleRegions configured".to_string(),
            requests: vec![json!({"visibleRegions": ["us-east-1"]})],
            expected: "none".to_string(),
        },
        Case {
            name: "both lists configured, explicitly empty".to_string(),
            requests: vec![json!({"visibleServices": [], "visibleRegions": []})],
            expected: "none".to_string(),
        },
    ];

    // Each modelled colour, set on its own.
    for color in MODELLED_COLORS {
        cases.push(Case {
            name: format!("{color} set by a colour-only update"),
            requests: vec![json!({"accountColor": color})],
            expected: color.to_string(),
        });
    }

    // A colour that survives a later update of another setting, and one reset
    // back to the modelled `none` value.
    cases.push(Case {
        name: "a colour set, then a list-only update".to_string(),
        requests: vec![
            json!({"accountColor": "orange"}),
            json!({"visibleServices": ["ec2"]}),
        ],
        expected: "orange".to_string(),
    });
    cases.push(Case {
        name: "a colour overwritten by another colour".to_string(),
        requests: vec![
            json!({"accountColor": "pink"}),
            json!({"accountColor": "darkBlue"}),
        ],
        expected: "darkBlue".to_string(),
    });
    cases.push(Case {
        name: "a colour reset with the modelled none".to_string(),
        requests: vec![
            json!({"accountColor": "teal"}),
            json!({"accountColor": "none"}),
        ],
        expected: "none".to_string(),
    });

    cases
}

/// The rule over the whole table: the colour a stored record is reported with is
/// the same value in the `UpdateAccountCustomizations` echo and in
/// `GetAccountCustomizations`, and is the documented `none` for an account that
/// never configured one.
#[tokio::test]
async fn both_operations_report_one_stored_record_with_the_same_color() {
    for case in cases() {
        // A fresh router per row: the record is exactly what this row's requests
        // built.
        let router = build_router();
        for (n, request) in case.requests.iter().enumerate() {
            let (status, _, body) = update_customizations(&router, request.clone()).await;
            assert_eq!(
                status, 200,
                "case {:?}: request {n} expected 200, got {status} ({body})",
                case.name
            );
        }

        // The update echo of the stored record, taken through a request that
        // names no setting: it renders the record this row built without
        // changing it, so both operations describe the SAME stored record.
        let (status, _, echoed) = update_customizations(&router, json!({})).await;
        assert_eq!(
            status, 200,
            "case {:?}: the re-render update expected 200, got {status} ({echoed})",
            case.name
        );
        let echo_color = reported_color(&case.name, "the Update echo", &echoed);

        let (status, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            status, 200,
            "case {:?}: Get expected 200, got {status} ({fetched})",
            case.name
        );
        let get_color = reported_color(&case.name, "Get", &fetched);

        assert_eq!(
            get_color, case.expected,
            "case {:?}: Get must report {:?}, got {get_color:?}",
            case.name, case.expected
        );
        assert_eq!(
            echo_color, case.expected,
            "case {:?}: the Update echo must report {:?}, got {echo_color:?}",
            case.name, case.expected
        );
        assert_eq!(
            echo_color, get_color,
            "case {:?}: one stored record must report one colour, got {echo_color:?} from Update and {get_color:?} from Get",
            case.name
        );
        assert!(
            MODELLED_COLORS.contains(&get_color.as_str()),
            "case {:?}: the reported colour must be a modelled AccountColor, got {get_color:?}",
            case.name
        );
    }
}
