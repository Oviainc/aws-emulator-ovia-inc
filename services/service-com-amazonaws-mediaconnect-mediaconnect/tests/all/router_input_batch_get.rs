// SPDX-License-Identifier: CC0-1.0

//! BatchGetRouterInput end to end over HTTP: CreateRouterInput -> BatchGetRouterInput,
//! cross-checked against GetRouterInput.
//!
//! The subject is BatchGetRouterInput, implemented in this change: for the ARNs one
//! request names it must report the router inputs the STORE HOLDS — each computed from the
//! stored record, each requested resource once, in the order the request named them — and
//! one `Errors` entry for every named ARN the store holds no input for, never a value fixed
//! in the handler. A batch read of an ARN therefore has to answer exactly what
//! GetRouterInput answers for that same ARN, which is what every case below checks it
//! against.
//!
//! The cases vary what those rules generalise over: which `RouterInputConfiguration` union
//! variant the input carries (all five, and two ingest protocols for a standard one, which
//! is what its `InputType` and `StreamDetails` follow from), how many ARNs the request
//! names, in what order, and whether each resolves, repeats or resolves nothing, which
//! state the clock has the input in when the read arrives, and both ends of the modelled
//! `Arns` length bound with one past each.
//!
//! The harness is the one `flow_lifecycle` owns (one router, one shared engine), so every
//! case drives the same request path a MediaConnect client reaches: routing, restJson1
//! deserialization, constraint validation, handler, serialization.

use std::time::Duration;

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_deletion::seed_router_input;
use crate::router_input_restart::{
    configuration_rows, create_body, create_router_input, get_router_input, union_member,
};

/// An ARN satisfying the modelled `RouterInputArn` pattern
/// (`smithy_query(query="shape",shape="RouterInputArn")`:
/// `^arn:(aws[a-zA-Z-]*):mediaconnect:[a-z0-9-]+:[0-9]{12}:routerInput:[a-z0-9]{12}$`) that
/// no CreateRouterInput minted, so this store holds no router input for it. It passes the
/// member's constraint, so it reaches the handler and is answered in `Errors`.
const UNKNOWN_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:ffffffffffff";
/// A second one, for the cases that name more than one absent input.
const OTHER_UNKNOWN_ARN: &str =
    "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:eeeeeeeeeeee";
/// A value that does NOT satisfy the `RouterInputArn` pattern (the id segment is not 12
/// lowercase alphanumerics), so the framework refuses the request before the handler runs.
const MALFORMED_ARN: &str = "arn:aws:mediaconnect:us-east-1:000000000000:routerInput:NOPE";

/// The modelled `@length` on `BatchGetRouterInput$Arns`
/// (`smithy_query(query="operation",operation="BatchGetRouterInput")`: `Arns` is required
/// with `length {min: 1, max: 20}`).
const MIN_ARNS: usize = 1;
/// See [`MIN_ARNS`].
const MAX_ARNS: usize = 20;

/// BatchGetRouterInput binds its `Arns` member to the `arns` query parameter, one parameter
/// per ARN (`smithy_query(query="operation",operation="BatchGetRouterInput")`:
/// `GET /v1/routerInputs`, `Arns` `@httpQuery("arns")`).
async fn batch_get_router_inputs(
    emulator: &Emulator,
    arns: &[&str],
) -> (u16, Option<String>, Value) {
    let query = arns
        .iter()
        .map(|arn| format!("arns={arn}"))
        .collect::<Vec<String>>()
        .join("&");
    let uri = if query.is_empty() {
        "/v1/routerInputs".to_string()
    } else {
        format!("/v1/routerInputs?{query}")
    };
    emulator.send("GET", &uri, None).await
}

/// Long enough for a transition the clock owes to be due: twice [`mc::TRANSITION_SPAN`], so
/// a settled state is never read early.
fn past_transition_span() -> Duration {
    mc::TRANSITION_SPAN * 2
}

/// How many router inputs the store holds.
fn stored_count(emulator: &Emulator) -> usize {
    let store = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = store.lock();
    store.router_inputs.len()
}

/// The `routerInputs` entries of a batch read, by ARN order.
fn reported_arns(read: &Value) -> Vec<String> {
    read["routerInputs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|input| input["arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A batch read reports, for every kind of router input, the record the create stored —
/// the same projection GetRouterInput answers from, in the request's own order, and
/// following the state the clock has the input in.
#[tokio::test]
async fn batch_get_reports_the_stored_router_input_every_arn_addresses() {
    let emulator = Emulator::new();

    // One input per configuration union variant (and per ingest protocol for a standard
    // one): what the input's `InputType` and `StreamDetails` follow from, so the rule is
    // asserted over the whole class.
    let rows = configuration_rows();
    let mut created: Vec<(String, String, &'static str, &'static str, &'static str)> = Vec::new();
    for (kind, member_name, json_name, configuration, expected_input_type) in rows {
        let name = format!("batch-{kind}").replace('/', "-");
        let (status, code, body) =
            create_router_input(&emulator, create_body(&name, configuration)).await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "case {kind:?}: CreateRouterInput answers 201, got {body}"
        );
        let arn = body["routerInput"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            mc::is_mintable_router_input_arn(&arn),
            "case {kind:?}: the create mints a RouterInputArn, got {arn:?}"
        );

        // Read this input straight away, while its own create is still in flight: the
        // batch read reports the TRANSIENT state, so it is computing from the record
        // rather than answering a fixed value. One ARN, read right after its create, so
        // the window the clock owes (`mc::TRANSITION_SPAN`) is not spent on the other
        // rows' round trips.
        let (status, code, in_flight) = batch_get_router_inputs(&emulator, &[arn.as_str()]).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {kind:?}: a batch read of a stored ARN answers 200, got {status} \
             {code:?}: {in_flight}"
        );
        assert_eq!(
            in_flight["routerInputs"][0]["state"].as_str(),
            Some(mc::ROUTER_INPUT_STATE_CREATING),
            "case {kind:?}: a batch read before the clock settles the create reports \
             CREATING, got {in_flight}"
        );

        created.push((
            kind.to_string(),
            arn,
            member_name,
            json_name,
            expected_input_type,
        ));
    }

    // Once the clock has settled every create, the batch read follows it there, and each
    // entry is exactly what GetRouterInput answers for that ARN.
    let every_arn: Vec<&str> = created.iter().map(|(_, arn, ..)| arn.as_str()).collect();
    tokio::time::sleep(past_transition_span()).await;
    let (status, _, read) = batch_get_router_inputs(&emulator, &every_arn).await;
    assert_eq!(
        status, 200,
        "a settled batch read answers 200, got {status}: {read}"
    );
    assert_eq!(
        reported_arns(&read),
        every_arn
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>(),
        "RouterInputs follows the order the request named the ARNs in, got {read}"
    );
    assert!(
        read["errors"]
            .as_array()
            .is_some_and(|errors| errors.is_empty()),
        "no requested ARN is absent, so Errors is empty, got {read}"
    );

    let entries = read["routerInputs"].as_array().cloned().unwrap_or_default();
    for ((case, arn, member_name, json_name, expected_input_type), entry) in
        created.iter().zip(entries.iter())
    {
        assert_eq!(
            entry["inputType"].as_str(),
            Some(*expected_input_type),
            "case {case:?}: InputType follows from the stored configuration, got {entry}"
        );
        assert!(
            !union_member(&entry["streamDetails"], member_name, json_name).is_null(),
            "case {case:?}: StreamDetails reports the {member_name} member, got {entry}"
        );
        assert_eq!(
            entry["state"].as_str(),
            Some(mc::ROUTER_INPUT_STATE_STANDBY),
            "case {case:?}: the clock settles a created input into STANDBY, got {entry}"
        );

        let (status, _, single) = get_router_input(&emulator, arn).await;
        assert_eq!(
            (status, &single["routerInput"]),
            (200, entry),
            "case {case:?}: a batch read answers what GetRouterInput answers for the same \
             ARN, got {single}"
        );
    }

    // A read changes nothing a later caller sees.
    let (_, _, again) = batch_get_router_inputs(&emulator, &every_arn).await;
    assert_eq!(
        again, read,
        "a batch read must not change what the next one sees, got {read} then {again}"
    );
    assert_eq!(
        stored_count(&emulator),
        created.len(),
        "a read stores no router input, got {} stored for {} created",
        stored_count(&emulator),
        created.len()
    );
}

/// One slot of a request: either a stored router input, by its index in the creates the
/// case seeded, or an ARN the store holds nothing for, by its index in [`ABSENT_ARNS`].
#[derive(Clone, Copy, Debug)]
enum Slot {
    Stored(usize),
    Absent(usize),
}

/// The ARNs a [`Slot::Absent`] slot names, each satisfying the modelled pattern and held by
/// nothing: two of them, so a case can name two DIFFERENT absent ARNs or the SAME one twice.
const ABSENT_ARNS: [&str; 2] = [UNKNOWN_ARN, OTHER_UNKNOWN_ARN];

/// One request shape: what each slot names, the stored inputs expected in `RouterInputs`,
/// and how many `Errors` entries.
struct BatchCase {
    case: &'static str,
    requested: Vec<Slot>,
    expected_inputs: Vec<usize>,
    expected_errors: usize,
}

/// Every named ARN is answered exactly once — in `RouterInputs` when the store holds it, in
/// `Errors` when it does not — whatever the request's count, order and repeats.
#[tokio::test]
async fn batch_get_answers_each_requested_arn_once_and_errors_for_the_rest() {
    let emulator = Emulator::new();

    // Three stored inputs to address, created through the wire like any client's.
    let (_, _, _, configuration, _) = configuration_rows()
        .into_iter()
        .next()
        .expect("the configuration catalog has a row");
    let mut arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let (status, _, created) = create_router_input(
            &emulator,
            create_body(&format!("ri-batch-{index}"), configuration.clone()),
        )
        .await;
        assert_eq!(status, 201, "seeding create expected 201, got {created}");
        arns.push(
            created["routerInput"]["arn"]
                .as_str()
                .expect("the create answers an ARN")
                .to_string(),
        );
    }

    let cases = vec![
        BatchCase {
            case: "one stored ARN",
            requested: vec![Slot::Stored(1)],
            expected_inputs: vec![1],
            expected_errors: 0,
        },
        BatchCase {
            case: "every stored ARN, in the order the request names them",
            requested: vec![Slot::Stored(2), Slot::Stored(0), Slot::Stored(1)],
            expected_inputs: vec![2, 0, 1],
            expected_errors: 0,
        },
        BatchCase {
            case: "a stored ARN either side of one nothing holds",
            requested: vec![Slot::Stored(0), Slot::Absent(0), Slot::Stored(2)],
            expected_inputs: vec![0, 2],
            expected_errors: 1,
        },
        BatchCase {
            case: "two different ARNs nothing holds",
            requested: vec![Slot::Absent(0), Slot::Absent(1)],
            expected_inputs: vec![],
            expected_errors: 2,
        },
        BatchCase {
            case: "a repeated stored ARN is one resource, reported once",
            requested: vec![Slot::Stored(1), Slot::Stored(1), Slot::Stored(0)],
            expected_inputs: vec![1, 0],
            expected_errors: 0,
        },
        BatchCase {
            case: "the SAME absent ARN twice is one error entry",
            requested: vec![Slot::Absent(0), Slot::Absent(0), Slot::Stored(0)],
            expected_inputs: vec![0],
            expected_errors: 1,
        },
    ];

    for BatchCase {
        case,
        requested,
        expected_inputs,
        expected_errors,
    } in cases
    {
        let requested_arns: Vec<&str> = requested
            .iter()
            .map(|slot| match slot {
                Slot::Stored(index) => arns[*index].as_str(),
                Slot::Absent(index) => ABSENT_ARNS[*index],
            })
            .collect();

        let (status, code, read) = batch_get_router_inputs(&emulator, &requested_arns).await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: a batch read answers 200 whatever its ARNs resolve, got \
             {status} {code:?}: {read}"
        );

        let reported = reported_arns(&read);
        let expected: Vec<String> = expected_inputs
            .iter()
            .map(|index| arns[*index].clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: RouterInputs must report the stored inputs in request order, \
             got {read}"
        );

        let errors = read["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            expected_errors,
            "case {case:?}: expected {expected_errors} Errors entries, got {read}"
        );
        for error in &errors {
            let arn = error["arn"].as_str().unwrap_or_default().to_string();
            assert!(
                requested_arns.contains(&arn.as_str()) && !reported.contains(&arn),
                "case {case:?}: an error entry names a requested ARN that was not reported, \
                 got {error}"
            );
            assert_eq!(
                error["code"].as_str(),
                Some("NotFoundException"),
                "case {case:?}: an ARN the store holds nothing for carries the not-found \
                 code, got {error}"
            );
            assert_eq!(
                error["message"].as_str(),
                Some(mc::not_found_message(&arn).as_str()),
                "case {case:?}: the error message is this service's not-found envelope, got \
                 {error}"
            );
        }
    }

    // Every case above was a read: the store still holds exactly the three creates.
    assert_eq!(
        stored_count(&emulator),
        3,
        "a read stores no router input, got {} stored",
        stored_count(&emulator)
    );
}

/// The request is accepted at each end of the modelled `Arns` bound and refused one past
/// each, with this service's own client-input error.
#[tokio::test]
async fn batch_get_holds_the_modelled_arns_bound() {
    let emulator = Emulator::new();

    // One more stored input than the bound allows a request to name, so every case below
    // names ARNs the store really holds and only the bound can refuse it. Seeded rather
    // than created, because the subject here is the request bound and not the create.
    let seeded: Vec<String> = (0..=MAX_ARNS)
        .map(|index| {
            seed_router_input(
                &emulator,
                &format!("ri-bound-{index}"),
                mc::ROUTER_INPUT_STATE_STANDBY,
            )
        })
        .collect();

    // (case, how many ARNs the request names, whether the bound accepts it).
    //
    // The lower end of the bound is the 1-ARN row: `Arns` is required and bound to the
    // query string, so a request carrying no `arns` parameter at all resolves no route in
    // the first place and never reaches this operation's bound.
    let count_cases = [
        ("the minimum", MIN_ARNS, true),
        ("the maximum", MAX_ARNS, true),
        ("one past the maximum", MAX_ARNS + 1, false),
    ];

    for (case, count, accepted) in count_cases {
        let requested: Vec<&str> = seeded
            .iter()
            .take(count)
            .map(String::as_str)
            .collect::<Vec<&str>>();
        let (status, code, read) = batch_get_router_inputs(&emulator, &requested).await;
        if accepted {
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: {count} ARNs are within the modelled bound, got {status} \
                 {code:?}: {read}"
            );
            assert_eq!(
                reported_arns(&read).len(),
                count,
                "case {case:?}: every one of the {count} stored ARNs is reported, got {read}"
            );
        } else {
            assert_eq!(
                (status, code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?}: {count} ARNs break the modelled bound, got {status} \
                 {code:?}: {read}"
            );
        }
    }

    // A value the `RouterInputArn` pattern refuses never reaches the handler either, so it
    // is a refusal of the request and not an `Errors` entry.
    let (status, code, read) = batch_get_router_inputs(&emulator, &[MALFORMED_ARN]).await;
    assert_eq!(
        (status, code.as_deref()),
        (400, Some("BadRequestException")),
        "an ARN outside the modelled pattern is refused, got {status} {code:?}: {read}"
    );

    assert_eq!(
        stored_count(&emulator),
        seeded.len(),
        "a read, accepted or refused, stores no router input, got {} stored for {} seeded",
        stored_count(&emulator),
        seeded.len()
    );
}
