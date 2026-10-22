// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `BatchGetTokenBalance`, driven over HTTP through the same
//! `MultiServiceRouter` the production front door serves (the harness in
//! [`crate::chain_query_slice`]).
//!
//! The invariant under test is the one a batch read exists for: every item of the
//! request is answered INDEPENDENTLY, exactly as the single-item `GetTokenBalance`
//! answers that same item — a balance the ledger holds lands in `tokenBalances`, an item
//! the single read refuses lands in `errors` with that refusal's code, message and
//! modelled `ErrorType`, and no item's failure costs the caller the others' balances.
//!
//! Each test is therefore a TABLE whose rows are generated from the ledger the service
//! actually serves (every network, every holding, every instant each holding changed at)
//! plus rows the ledger deliberately knows nothing about, and the rows are then grouped
//! into batches of EVERY size the model admits. Where a bound or a spelling comes from
//! the Smithy model, the comment beside it names the shape it belongs to.

use serde_json::{json, Value};

use crate::chain_query_slice::{error_code, token_json, Emulator, NetworkFixture};

/// The largest batch the model admits: `GetTokenBalanceInputList` carries
/// `@length(min: 1, max: 10)`, and `BatchGetTokenBalanceOutputList` /
/// `BatchGetTokenBalanceErrors` carry `@length(min: 0, max: 10)`.
const MAX_BATCH: usize = 10;

/// The smallest batch the model admits; see [`MAX_BATCH`].
const MIN_BATCH: usize = 1;

/// Addresses that satisfy `ChainAddress`'s `^[-A-Za-z0-9]{13,74}$` and that the fixture
/// ledger holds nothing for. The forms differ: an Ethereum-shaped address, a
/// base58-shaped one, a hyphenated label and the longest the pattern admits.
fn strangers() -> Vec<String> {
    vec![
        "0x0000000000000000000000000000000000000000".to_string(),
        "1NotASeededBitcoinAddressAtAll".to_string(),
        "not-a-seeded-owner".to_string(),
        "z".repeat(74),
    ]
}

/// One row of a table: the request item, and a label naming what makes it different.
#[derive(Debug, Clone)]
struct Row {
    label: String,
    /// A `BatchGetTokenBalanceInputItem`, which declares exactly the members of
    /// `GetTokenBalanceInput`, so the SAME JSON is a valid request to either operation.
    item: Value,
}

/// A request item for `token`/`owner`, as of `at` when the caller names an instant.
fn request_item(token: Value, owner: &str, at: Option<i64>) -> Value {
    let mut item = json!({
        "tokenIdentifier": token,
        "ownerIdentifier": {"address": owner},
    });
    if let Some(at) = at {
        item["atBlockchainInstant"] = json!({"time": at});
    }
    item
}

/// What the single-item read answers for one row — the answer the batch read has to
/// reproduce.
#[derive(Debug, Clone)]
enum Expected {
    /// HTTP 200: the balance, and the stored `lastUpdatedTime` beside it.
    Balance {
        balance: String,
        last_updated_time: Value,
    },
    /// A refusal: the wire error code and the message it carried.
    Failure { code: String, message: String },
}

/// The modelled `ErrorType` a batch item reports for the exception the single-item read
/// raises. `ErrorType` declares exactly two values, one per exception a per-holding read
/// can fail with.
fn error_type_of(code: &str) -> &'static str {
    match code {
        "ValidationException" => "VALIDATION_EXCEPTION",
        "ResourceNotFoundException" => "RESOURCE_NOT_FOUND_EXCEPTION",
        other => panic!("no ErrorType value stands for the wire code {other}"),
    }
}

/// Whole seconds of a `BlockchainInstant$time` on the wire.
///
/// `BlockchainInstant$time` is "recorded up to the second", and an epoch-seconds
/// timestamp serialises as a JSON number.
fn instant_secs(value: &Value) -> Option<i64> {
    value.get("time").and_then(Value::as_f64).map(|t| t as i64)
}

/// `GetTokenBalance` for one row: the answer every batch carrying that row must repeat.
async fn single_read(emulator: &Emulator, row: &Row) -> Expected {
    let (status, body) = emulator.post("/get-token-balance", &row.item).await;
    match status {
        200 => Expected::Balance {
            balance: body["balance"]
                .as_str()
                .unwrap_or_else(|| panic!("case {}: balance is required, got {body:?}", row.label))
                .to_string(),
            last_updated_time: body["lastUpdatedTime"].clone(),
        },
        400 | 404 => Expected::Failure {
            code: error_code(&body),
            message: body["message"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: a refusal carries a message, got {body:?}",
                        row.label
                    )
                })
                .to_string(),
        },
        other => panic!(
            "case {}: GetTokenBalance answered {other} with {body:?}",
            row.label
        ),
    }
}

/// The table for one network: every holding the ledger serves, asked for now and as of
/// every instant it changed at, plus an instant before the ledger opened, owners the
/// ledger does not know, and a `tokenIdentifier` that identifies no token.
fn rows(network: &NetworkFixture) -> Vec<Row> {
    let name = &network.network;
    let opened_at = network.opened_at();
    let mut rows = Vec::new();

    for (key, holding) in &network.holdings {
        let token = token_json(name, key);
        let resource = key.resource_id();
        rows.push(Row {
            label: format!("{name}/{resource}@now"),
            item: request_item(token.clone(), &key.owner_address, None),
        });
        // Every instant this holding's balance changed at, and one second after each:
        // the balance in force is the record at or before the instant asked for.
        for record in &holding.history {
            for at in [record.time, record.time + 1] {
                rows.push(Row {
                    label: format!("{name}/{resource}@{at}"),
                    item: request_item(token.clone(), &key.owner_address, Some(at)),
                });
            }
        }
        // One second before the ledger opened: no balance is in force yet.
        rows.push(Row {
            label: format!("{name}/{resource}@before-the-ledger-opened"),
            item: request_item(token.clone(), &key.owner_address, Some(opened_at - 1)),
        });
    }

    let first_token = network
        .holdings
        .first()
        .map(|(key, _)| token_json(name, key))
        .unwrap_or_else(|| panic!("case {name}: the seeded ledger holds no balance"));

    for stranger in strangers() {
        rows.push(Row {
            label: format!("{name}/stranger/{stranger}"),
            item: request_item(first_token.clone(), &stranger, None),
        });
    }

    // A `TokenIdentifier` naming only its required `network` member identifies no token:
    // `contractAddress` and `tokenId` are both modelled optional, so this reaches the
    // handler and the per-item rule has to refuse it.
    let owner = network.holdings[0].0.owner_address.clone();
    rows.push(Row {
        label: format!("{name}/network-only-token"),
        item: request_item(json!({"network": name}), &owner, None),
    });

    rows
}

/// POST a batch of `rows` and return the parsed 200 response.
async fn batch(emulator: &Emulator, rows: &[Row]) -> Value {
    let items: Vec<Value> = rows.iter().map(|row| row.item.clone()).collect();
    emulator
        .ok(
            "/batch-get-token-balance",
            &json!({"getTokenBalanceInputs": items}),
        )
        .await
}

/// The items of a response list member, which the output declares `@required`.
fn list<'reply>(reply: &'reply Value, member: &str, case: &str) -> &'reply Vec<Value> {
    reply[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: {member} is required, got {reply:?}"))
}

/// Assert that `reply` answers `rows` exactly as `expected` says the single-item read
/// answers each of them, in request order.
fn assert_batch_matches(case: &str, rows: &[Row], expected: &[Expected], reply: &Value) {
    let balances = list(reply, "tokenBalances", case);
    let errors = list(reply, "errors", case);

    let wanted_balances = expected.len()
        - expected
            .iter()
            .filter(|answer| matches!(answer, Expected::Failure { .. }))
            .count();
    assert_eq!(
        balances.len() + errors.len(),
        rows.len(),
        "case {case}: every request item is answered exactly once, got {} balances and \
         {} errors for {} items",
        balances.len(),
        errors.len(),
        rows.len()
    );
    assert_eq!(
        balances.len(),
        wanted_balances,
        "case {case}: expected {wanted_balances} balances, got {balances:?}"
    );

    let mut balance_index = 0;
    let mut error_index = 0;
    for (row, answer) in rows.iter().zip(expected) {
        match answer {
            Expected::Balance {
                balance,
                last_updated_time,
            } => {
                let got = balances.get(balance_index).unwrap_or_else(|| {
                    panic!(
                        "case {case}: row {} must be the balance at index {balance_index}, got \
                         {balances:?}",
                        row.label
                    )
                });
                balance_index += 1;
                assert_eq!(
                    got["balance"].as_str(),
                    Some(balance.as_str()),
                    "case {case}: row {} must report the single read's balance {balance}, got \
                     {got:?}",
                    row.label
                );
                assert_eq!(
                    &got["lastUpdatedTime"], last_updated_time,
                    "case {case}: row {} must report the stored lastUpdatedTime \
                     {last_updated_time:?}, got {got:?}",
                    row.label
                );
                assert_eq!(
                    got["tokenIdentifier"], row.item["tokenIdentifier"],
                    "case {case}: row {} must report the token it was asked about, got {got:?}",
                    row.label
                );
                assert_eq!(
                    got["ownerIdentifier"], row.item["ownerIdentifier"],
                    "case {case}: row {} must report the owner it was asked about, got {got:?}",
                    row.label
                );
                // `BatchGetTokenBalanceOutputItem$atBlockchainInstant` is required: it is
                // the instant asked for, or the current time when the row named none (a
                // per-call clock reading, so only its presence is asserted then).
                let asked = instant_secs(&row.item["atBlockchainInstant"]);
                let reported = instant_secs(&got["atBlockchainInstant"]);
                match asked {
                    Some(asked) => assert_eq!(
                        reported,
                        Some(asked),
                        "case {case}: row {} asked for {asked}, got {got:?}",
                        row.label
                    ),
                    None => assert!(
                        reported.is_some(),
                        "case {case}: row {} must report an atBlockchainInstant, got {got:?}",
                        row.label
                    ),
                }
            }
            Expected::Failure { code, message } => {
                let got = errors.get(error_index).unwrap_or_else(|| {
                    panic!(
                        "case {case}: row {} must be the error at index {error_index}, got \
                         {errors:?}",
                        row.label
                    )
                });
                error_index += 1;
                assert_eq!(
                    got["errorCode"].as_str(),
                    Some(code.as_str()),
                    "case {case}: row {} must carry the single read's code {code}, got {got:?}",
                    row.label
                );
                assert_eq!(
                    got["errorType"].as_str(),
                    Some(error_type_of(code)),
                    "case {case}: row {} must carry the ErrorType of {code}, got {got:?}",
                    row.label
                );
                assert_eq!(
                    got["errorMessage"].as_str(),
                    Some(message.as_str()),
                    "case {case}: row {} must carry the single read's message {message:?}, got \
                     {got:?}",
                    row.label
                );
                assert_eq!(
                    got["tokenIdentifier"], row.item["tokenIdentifier"],
                    "case {case}: row {} must name the token it failed on, got {got:?}",
                    row.label
                );
                assert_eq!(
                    got["ownerIdentifier"], row.item["ownerIdentifier"],
                    "case {case}: row {} must name the owner it failed on, got {got:?}",
                    row.label
                );
                assert_eq!(
                    instant_secs(&got["atBlockchainInstant"]),
                    instant_secs(&row.item["atBlockchainInstant"]),
                    "case {case}: row {} must echo the instant it was asked for, got {got:?}",
                    row.label
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The per-item rule
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_item_of_a_batch_is_answered_as_the_single_item_read_answers_it() {
    let emulator = Emulator::new();
    let fixture = emulator.fixture();
    assert!(
        !fixture.is_empty(),
        "every QueryNetwork value must have a seeded ledger, got none"
    );

    for network in &fixture {
        let rows = rows(network);
        // What `GetTokenBalance` answers for each row, asked once.
        let mut expected = Vec::with_capacity(rows.len());
        for row in &rows {
            expected.push(single_read(&emulator, row).await);
        }
        assert!(
            expected
                .iter()
                .any(|answer| matches!(answer, Expected::Balance { .. }))
                && expected
                    .iter()
                    .any(|answer| matches!(answer, Expected::Failure { .. })),
            "case {}: the table must mix balances and refusals, got {expected:?}",
            network.network
        );

        // Every batch size the model admits, so each row is answered beside neighbours
        // of other kinds and at every position in a batch.
        for size in MIN_BATCH..=MAX_BATCH {
            for (chunk, answers) in rows.chunks(size).zip(expected.chunks(size)) {
                let case = format!("{}/batch-of-{size}", network.network);
                let reply = batch(&emulator, chunk).await;
                assert_batch_matches(&case, chunk, answers, &reply);
            }
        }
    }
}

#[tokio::test]
async fn a_batch_reports_the_balance_and_update_instant_the_ledger_stored() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        // One row per (holding, instant it changed at), asserted against the STORED
        // record rather than against the single-item read, so a regression shared by
        // both reads cannot pass.
        let mut rows: Vec<(String, Value, String, i64)> = Vec::new();
        for (key, holding) in &network.holdings {
            for record in &holding.history {
                rows.push((
                    format!("{}/{}@{}", network.network, key.resource_id(), record.time),
                    request_item(
                        token_json(&network.network, key),
                        &key.owner_address,
                        Some(record.time),
                    ),
                    record.balance.to_string(),
                    record.time,
                ));
            }
        }
        assert!(
            !rows.is_empty(),
            "case {}: the seeded ledger holds no balance record",
            network.network
        );

        for chunk in rows.chunks(MAX_BATCH) {
            let items: Vec<Value> = chunk.iter().map(|(_, item, _, _)| item.clone()).collect();
            let case = format!("{}/stored-balances", network.network);
            let reply = emulator
                .ok(
                    "/batch-get-token-balance",
                    &json!({"getTokenBalanceInputs": items}),
                )
                .await;
            let balances = list(&reply, "tokenBalances", &case);
            assert_eq!(
                balances.len(),
                chunk.len(),
                "case {case}: every stored holding resolves, got {reply:?}"
            );
            for (index, (label, _, balance, time)) in chunk.iter().enumerate() {
                let got = &balances[index];
                assert_eq!(
                    got["balance"].as_str(),
                    Some(balance.as_str()),
                    "case {case}: row {label} must report the stored balance {balance}, got {got:?}"
                );
                assert_eq!(
                    instant_secs(&got["lastUpdatedTime"]),
                    Some(*time),
                    "case {case}: row {label} must report the stored record's instant {time}, \
                     got {got:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_batch_answers_a_repeated_item_once_per_request_item() {
    let emulator = Emulator::new();

    for network in emulator.fixture() {
        let (key, holding) = &network.holdings[0];
        let stored = holding
            .history
            .last()
            .unwrap_or_else(|| panic!("case {}: the holding has no balance", network.network))
            .balance
            .to_string();
        let known = Row {
            label: format!("{}/{}", network.network, key.resource_id()),
            item: request_item(
                token_json(&network.network, key),
                &key.owner_address,
                Some(holding.history.last().expect("a balance record").time),
            ),
        };
        let unknown = Row {
            label: format!("{}/stranger", network.network),
            item: request_item(
                token_json(&network.network, key),
                &strangers()[0],
                Some(holding.history.last().expect("a balance record").time),
            ),
        };

        // Counts spanning the modelled bound: one copy, a mix, and the largest batch.
        for count in [MIN_BATCH, 3, MAX_BATCH] {
            let repeated: Vec<Row> = (0..count)
                .map(|index| {
                    if index % 2 == 0 {
                        known.clone()
                    } else {
                        unknown.clone()
                    }
                })
                .collect();
            let case = format!("{}/repeated-{count}", network.network);
            let reply = batch(&emulator, &repeated).await;

            let balances = list(&reply, "tokenBalances", &case);
            let errors = list(&reply, "errors", &case);
            let wanted_balances = count.div_ceil(2);
            assert_eq!(
                balances.len(),
                wanted_balances,
                "case {case}: expected {wanted_balances} balances, got {balances:?}"
            );
            assert_eq!(
                errors.len(),
                count - wanted_balances,
                "case {case}: expected {} errors, got {errors:?}",
                count - wanted_balances
            );
            for balance in balances {
                assert_eq!(
                    balance["balance"].as_str(),
                    Some(stored.as_str()),
                    "case {case}: every copy of {} reports {stored}, got {balance:?}",
                    known.label
                );
            }
            for error in errors {
                assert_eq!(
                    error["errorType"].as_str(),
                    Some("RESOURCE_NOT_FOUND_EXCEPTION"),
                    "case {case}: every copy of {} fails the same way, got {error:?}",
                    unknown.label
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The modelled size bounds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_batch_is_accepted_at_the_modelled_bounds_and_refused_outside_them() {
    let emulator = Emulator::new();
    let network = emulator
        .fixture()
        .into_iter()
        .next()
        .expect("a seeded network");
    let (key, _) = &network.holdings[0];
    let item = request_item(token_json(&network.network, key), &key.owner_address, None);

    // `GetTokenBalanceInputList` is `@length(min: 1, max: 10)`: both bounds are
    // accepted, and one item past either is refused.
    for size in [MIN_BATCH, MAX_BATCH] {
        let items: Vec<Value> = (0..size).map(|_| item.clone()).collect();
        let reply = emulator
            .ok(
                "/batch-get-token-balance",
                &json!({"getTokenBalanceInputs": items}),
            )
            .await;
        let case = format!("batch-of-{size}");
        assert_eq!(
            list(&reply, "tokenBalances", &case).len(),
            size,
            "case {case}: a batch of {size} known holdings reports {size} balances, got {reply:?}"
        );
    }

    for size in [MIN_BATCH - 1, MAX_BATCH + 1] {
        let items: Vec<Value> = (0..size).map(|_| item.clone()).collect();
        emulator
            .fails(
                "/batch-get-token-balance",
                &json!({"getTokenBalanceInputs": items}),
                400,
                "ValidationException",
            )
            .await;
    }
}

#[tokio::test]
async fn a_request_that_names_no_input_is_answered_with_no_balance_and_no_error() {
    let emulator = Emulator::new();
    // `BatchGetTokenBalanceInput$getTokenBalanceInputs` is NOT required, so a request
    // that names no item satisfies the model; `tokenBalances` and `errors` are both
    // required, so the answer is the two empty lists.
    let reply = emulator.ok("/batch-get-token-balance", &json!({})).await;
    assert!(
        list(&reply, "tokenBalances", "no-input").is_empty()
            && list(&reply, "errors", "no-input").is_empty(),
        "case no-input: a request for no balance is answered by two empty lists, got {reply:?}"
    );
}
