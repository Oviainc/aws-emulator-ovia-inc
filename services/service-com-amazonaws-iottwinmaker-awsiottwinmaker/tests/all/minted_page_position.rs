// SPDX-License-Identifier: CC0-1.0

//! Which page POSITIONS an ordinal-resuming `nextToken` of this service resumes
//! at, end to end over HTTP.
//!
//! RULE under test: an operation whose pages resume at a row's POSITION in a
//! sequence — the rows carry no key of their own, so the position is the cursor
//! — honours a `nextToken` only where that position is one the operation can
//! have minted: spelled the one way it writes a position (a plain decimal, so
//! no sign, no leading zero and no other rendering of the same number), at
//! least 1, and naming a row the sequence the request selects really holds.
//! Every other position inside an otherwise well-formed token of that
//! operation's own envelope and scope is a cursor the service never issued, and
//! earns the same 400 `ValidationException` as a token another listing minted.
//! In particular a position at or past the END of the sequence is refused,
//! never answered HTTP 200 with no rows and no further token — that answer
//! would report the end of a walk this service never started.
//!
//! The rows span what the rule generalises over: WHICH ordinal-resuming
//! operation is asked (the graph query over entities, the tabular property
//! read, the property history read — every operation of this service that
//! resumes at a position rather than at a key), and WHICH position the
//! hand-built token carries (a leading sign, one leading zero and several, zero
//! itself, a fraction, exponent and hexadecimal spellings, surrounding
//! whitespace, digit grouping, a non-ASCII digit, no position at all, the
//! position one past the end of the sequence, one far past it, and a magnitude
//! no position counter holds).
//!
//! The other side of the rule is asserted over the SAME set, so it cannot be
//! satisfied by refusing everything: every position the operation really mints —
//! each one of them, hand-built in the canonical spelling rather than copied
//! from a response — resumes at exactly the rows from that position on.
//!
//! A case's token is derived from a token the service itself minted: it is
//! hex-decoded, the position (the digits ending its first line) is replaced, and
//! it is re-encoded. So a case varies the position and NOTHING else — the
//! envelope and every scope part stay the ones the operation wrote — and the
//! refusal cannot be the envelope's or the scope's doing.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below pages inside.
const WORKSPACE: &str = "mintedPositions";
/// The component type whose instances and whose time series carry the rows the
/// tabular and history cases page over.
const SENSOR: &str = "com.example.sensorPositions";
/// The component the seeded entities carry [`SENSOR`] under.
const COMPONENT: &str = "Sensor";

/// How many rows every sequence below holds, so the positions a case asks for
/// straddle a known end. Four rows mint the positions 1, 2 and 3.
const ROWS: usize = 4;

/// The instants the seeded time series carries a value at, one per row.
const TIMES: [&str; ROWS] = [
    "2026-05-01T00:00:00Z",
    "2026-05-01T00:01:00Z",
    "2026-05-01T00:02:00Z",
    "2026-05-01T00:03:00Z",
];

/// The ONE sentence every paginated operation of this service reports for a
/// token it did not mint. Written out here rather than read from the crate, so
/// this asserts the wording instead of restating the implementation.
const REFUSAL: &str = "nextToken is not a token this listing minted";

/// One operation of this service whose pages resume at a row's POSITION.
///
/// These are the three it has: a query's rows, a table's rows and a series'
/// samples all come out of a sequence the request itself decides, so none of
/// them has a key to resume at. Every other paginated operation of this service
/// resumes at a stored id, ARN or tag key, which is a different rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ordinal {
    Query,
    Tabular,
    History,
}

impl Ordinal {
    const ALL: [Self; 3] = [Self::Query, Self::Tabular, Self::History];

    fn name(self) -> &'static str {
        match self {
            Self::Query => "ExecuteQuery",
            Self::Tabular => "GetPropertyValue (tabular rows)",
            Self::History => "GetPropertyValueHistory",
        }
    }

    /// One page of this operation at the page size `max_results`, resumed at
    /// `next_token` where one is given.
    ///
    /// Everything that decides the sequence is held fixed across every call, so
    /// a token minted by one of them is in the scope of all of them.
    async fn page(self, emulator: &Emulator, max_results: u32, next_token: Option<&str>) -> Resp {
        let mut body = json!({"maxResults": max_results});
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        match self {
            Self::Query => {
                body["workspaceId"] = json!(WORKSPACE);
                body["queryStatement"] = json!("SELECT e.entityId FROM EntityGraph MATCH (e)");
                emulator.execute_query(body).await
            }
            Self::Tabular => {
                body["componentTypeId"] = json!(SENSOR);
                body["selectedProperties"] = json!(["Tag"]);
                body["tabularConditions"] = json!({"orderBy": [{"propertyName": "Tag"}]});
                emulator.get_property_value(WORKSPACE, body).await
            }
            Self::History => {
                body["entityId"] = json!("unit-0");
                body["componentName"] = json!(COMPONENT);
                body["selectedProperties"] = json!(["Temp"]);
                emulator.get_property_value_history(WORKSPACE, body).await
            }
        }
    }

    /// The rows a page of this operation reported, in the order it reported
    /// them.
    fn rows(self, response: &Resp) -> Vec<Value> {
        match self {
            Self::Query => response
                .member("rows")
                .as_array()
                .cloned()
                .unwrap_or_default(),
            Self::Tabular => response
                .member("tabularPropertyValues")
                .as_array()
                .and_then(|tables| tables.first())
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            Self::History => response
                .member("propertyValues")
                .as_array()
                .map(|series| {
                    series
                        .iter()
                        .flat_map(|one| one["values"].as_array().cloned().unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The workspace, component type, entities and time series every case pages
/// over: [`ROWS`] entities, each told apart by its `Tag`, and a time series
/// carrying one value per instant of [`TIMES`] — so a page that repeats or skips
/// a row is visible in the rows it reports.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Tag": {"dataType": {"type": "STRING"}},
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
                // The write connector the seeded time series values are set THROUGH.
                "functions": data_writer_functions(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {SENSOR:?}: expected 200, got {created:?}"
    );

    for index in 0..ROWS {
        let entity_id = format!("unit-{index}");
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: {
                        "componentTypeId": SENSOR,
                        "properties": {
                            "Tag": {"value": {"stringValue": format!("tag-{index}")}},
                        },
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }

    let values: Vec<Value> = TIMES
        .iter()
        .enumerate()
        .map(|(index, time)| json!({"time": time, "value": {"doubleValue": index as f64}}))
        .collect();
    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": "unit-0",
                    "componentName": COMPONENT,
                    "propertyName": "Temp",
                },
                "propertyValues": values,
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues seeding the series: expected 200, got {written:?}"
    );
}

/// A token of the SAME envelope and scope as `minted`, carrying `position`
/// where the minted one carries its own.
///
/// The minted token is hex over an envelope and a position followed by the
/// scope parts on lines of their own, so the position is the run of digits
/// ending the first line: it is cut off, `position` is written in its place, and
/// the body is re-encoded. Derived from what the service minted rather than
/// written out here, so a case varies the position alone.
fn at_position(minted: &str, position: &str) -> String {
    let body = String::from_utf8(hex::decode(minted).expect("a minted token is hex"))
        .expect("a minted token decodes to UTF-8");
    let (first_line, scope) = match body.find('\n') {
        Some(at) => body.split_at(at),
        None => (body.as_str(), ""),
    };
    let envelope = first_line.trim_end_matches(|character: char| character.is_ascii_digit());
    assert!(
        envelope.len() < first_line.len(),
        "a minted ordinal token's first line ends in the position it resumes at, got \
         {first_line:?}"
    );
    hex::encode(format!("{envelope}{position}{scope}"))
}

/// Every position no page of a sequence of [`ROWS`] rows can have been minted
/// at: a spelling of a number this service never writes, or a position outside
/// the row set.
///
/// The first group keeps the VALUE a minted position carries (1, a row the
/// sequence holds) and changes only how it is written, so each row is refused
/// for its spelling and not for where it points; the second group is spelled
/// the one way the service writes a position and is refused for pointing
/// outside the sequence.
fn unminted_positions() -> Vec<(&'static str, String)> {
    vec![
        ("aLeadingPlusSign", "+1".to_string()),
        ("oneLeadingZero", "01".to_string()),
        ("manyLeadingZeros", "0000001".to_string()),
        ("theFirstPositionAsZero", "0".to_string()),
        ("zeroWithASign", "+0".to_string()),
        ("aNegativePosition", "-1".to_string()),
        ("aFraction", "1.0".to_string()),
        ("exponentNotation", "1e0".to_string()),
        ("hexadecimal", "0x1".to_string()),
        ("leadingWhitespace", " 1".to_string()),
        ("trailingWhitespace", "1 ".to_string()),
        ("groupedDigits", "1_000".to_string()),
        // A digit of another script: a number to a reader, no position here.
        ("aNonAsciiDigit", "\u{ff11}".to_string()),
        ("noPositionAtAll", String::new()),
        // The row count itself: the first position past the end, since a page
        // resumes at a row and the rows are counted from zero.
        ("thePositionJustPastTheEnd", ROWS.to_string()),
        ("aPositionFarPastTheEnd", "99999".to_string()),
        (
            "aMagnitudeNoPositionCounterHolds",
            "18446744073709551616".to_string(),
        ),
    ]
}

/// The whole sequence an operation selects and a token it really minted, read
/// off the service itself: the first page of one row leaves the rest over, so it
/// must mint a token, and that token's envelope and scope are what every
/// hand-built one below carries.
async fn sequence_and_minted_token(
    ordinal: Ordinal,
    emulator: &Emulator,
) -> (Vec<Value>, String) {
    let whole = ordinal.page(emulator, ROWS as u32, None).await;
    assert_eq!(
        whole.status,
        200,
        "case {:?}: expected 200 for the whole sequence, got {whole:?}",
        ordinal.name()
    );
    let sequence = ordinal.rows(&whole);
    assert_eq!(
        sequence.len(),
        ROWS,
        "case {:?}: expected the {ROWS} seeded rows, got {whole:?}",
        ordinal.name()
    );

    let minted = ordinal.page(emulator, 1, None).await;
    assert_eq!(
        minted.status,
        200,
        "case {:?}: expected 200 for the minting page, got {minted:?}",
        ordinal.name()
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the minting page left rows over, so it must mint a token: {minted:?}",
                ordinal.name()
            )
        })
        .to_string();
    (sequence, token)
}

/// RULE: a position no page of this sequence was minted at is refused with a 400
/// `ValidationException` reporting the one refusal this service answers for a
/// token it did not mint, and no row at all — whichever ordinal-resuming
/// operation is asked, and whether the position is spelled a way the service
/// never writes or points outside the row set.
#[tokio::test]
async fn an_ordinal_token_carrying_a_position_nothing_minted_is_refused() {
    for ordinal in Ordinal::ALL {
        let emulator = Emulator::new();
        seed(&emulator).await;
        let (_, token) = sequence_and_minted_token(ordinal, &emulator).await;

        for (what, position) in unminted_positions() {
            let case = format!("{}/{what}", ordinal.name());
            let forged = at_position(&token, &position);
            let refused = ordinal.page(&emulator, ROWS as u32, Some(&forged)).await;

            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} (position {position:?}): expected a 400 ValidationException, got \
                 {refused:?}"
            );
            let reported = refused
                .member("message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                reported, REFUSAL,
                "case {case:?} (position {position:?}): expected the message {REFUSAL:?}, got \
                 {reported:?}"
            );
            assert!(
                ordinal.rows(&refused).is_empty(),
                "case {case:?} (position {position:?}): expected no rows at all, got {refused:?}"
            );
        }
    }
}

/// RULE, the other side of it: EVERY position the operation really mints
/// resumes, at exactly the rows from that position on — so the refusal above is
/// about the position being one nothing minted, not about the token having been
/// built by hand.
#[tokio::test]
async fn an_ordinal_token_carrying_a_position_the_operation_mints_resumes_there() {
    for ordinal in Ordinal::ALL {
        let emulator = Emulator::new();
        seed(&emulator).await;
        let (sequence, token) = sequence_and_minted_token(ordinal, &emulator).await;

        for position in 1..ROWS {
            let case = format!("{}/position {position}", ordinal.name());
            let built = at_position(&token, &position.to_string());
            let resumed = ordinal.page(&emulator, ROWS as u32, Some(&built)).await;

            assert_eq!(
                resumed.status, 200,
                "case {case:?}: expected 200, got {resumed:?}"
            );
            assert_eq!(
                ordinal.rows(&resumed),
                sequence[position..].to_vec(),
                "case {case:?}: expected the rows from that position on, got {resumed:?}"
            );
            assert!(
                resumed.member("nextToken").is_null(),
                "case {case:?}: the page carried the rest of the sequence, so it leaves no token, \
                 got {resumed:?}"
            );
        }
    }
}

/// RULE: a refused token changes nothing — a read refused for its cursor
/// consumes no row, so the sequence still reads whole afterwards and the
/// operation's own token still resumes.
#[tokio::test]
async fn a_refused_position_leaves_the_sequence_whole() {
    for ordinal in Ordinal::ALL {
        let emulator = Emulator::new();
        seed(&emulator).await;
        let (sequence, token) = sequence_and_minted_token(ordinal, &emulator).await;

        for (what, position) in unminted_positions() {
            let forged = at_position(&token, &position);
            let refused = ordinal.page(&emulator, ROWS as u32, Some(&forged)).await;
            assert_eq!(
                refused.status,
                400,
                "case {}/{what}: expected 400, got {refused:?}",
                ordinal.name()
            );
        }

        let whole = ordinal.page(&emulator, ROWS as u32, None).await;
        assert_eq!(
            ordinal.rows(&whole),
            sequence,
            "case {:?}: the sequence still reports every row after the refusals, got {whole:?}",
            ordinal.name()
        );
        let resumed = ordinal.page(&emulator, ROWS as u32, Some(&token)).await;
        assert_eq!(
            resumed.status,
            200,
            "case {:?}: the operation's own token still resumes, got {resumed:?}",
            ordinal.name()
        );
        assert_eq!(
            ordinal.rows(&resumed),
            sequence[1..].to_vec(),
            "case {:?}: expected the rows the minting page left over, got {resumed:?}",
            ordinal.name()
        );
    }
}
