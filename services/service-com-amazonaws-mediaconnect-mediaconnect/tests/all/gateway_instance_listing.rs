// SPDX-License-Identifier: CC0-1.0

//! ListGatewayInstances, end to end over HTTP: `GET /v1/gateway-instances`.
//!
//! The subject is the rule ListGatewayInstances follows: it "displays a list of instances
//! associated with the Amazon Web Services account", a page at a time in one stable order,
//! each entry the STORED instance record projected into the modelled
//! `ListedGatewayInstance` summary (`smithy_query(query="shape",
//! shape="ListedGatewayInstance")`: `GatewayArn`, `GatewayInstanceArn`, `InstanceId`,
//! `InstanceState` and nothing else), narrowed by the optional `FilterArn` to "only the
//! instances associated with the selected Gateway ARN". Nothing is fixed in the handler:
//! every member asserted below is compared against what the store holds, and an instance
//! the store does not hold is not listed.
//!
//! The rows therefore vary what that rule generalises over: the `InstanceState` a record
//! carries (every value of the modelled enum, including the terminal `DEREGISTERED`), how
//! many instances the store holds and across how many gateways, whether `FilterArn` is
//! absent, names a gateway with instances or names one without, the page size (across the
//! modelled `MaxResults` range and its documented default), and the form of the pagination
//! token.
//!
//! An instance is never created by a client: the model declares no operation that registers
//! one, so the fixtures register through the store's own out-of-band channel, exactly as
//! `tests/all/gateway_instances.rs` and `tests/all/describe_gateway_instance.rs` do.
//!
//! Every call goes over the wire through the same router the emulator's front door uses, on
//! the harness `flow_lifecycle` owns, so what these tests assert is what a MediaConnect
//! client sees: routing, restJson1 deserialization, constraint validation, handler,
//! serialization.

use std::time::Instant;

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::gateway_instances::{
    gateway_with, instance_in_state, register_instance, PAST_THE_TRANSITION,
};

/// `GET /v1/gateway-instances` — the ListGatewayInstances binding
/// (`smithy_query(query="operation",operation="ListGatewayInstances")`).
async fn list_gateway_instances(emulator: &Emulator, query: &str) -> (u16, Option<String>, Value) {
    emulator
        .send("GET", &format!("/v1/gateway-instances{query}"), None)
        .await
}

/// The `Instances` member of a ListGatewayInstances response.
fn listed(body: &Value) -> Vec<Value> {
    body["instances"].as_array().cloned().unwrap_or_default()
}

/// The instance ARNs of the `Instances` member, in the order the response carries them.
fn listed_arns(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|instance| {
            instance["gatewayInstanceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A 200 listing, asserted to be answered at all.
async fn page(emulator: &Emulator, case: &str, query: &str) -> Value {
    let (status, code, body) = list_gateway_instances(emulator, query).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: listing {query:?} expected 200, got {status} {code:?}: {body}"
    );
    body
}

/// Every instance ARN the store holds, in the order the store holds them — the order a
/// listing must report, derived from the store rather than written out here.
fn stored_arns(emulator: &Emulator) -> Vec<String> {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    store.gateway_instances.keys().cloned().collect()
}

/// The whole gateway-instance map as the store holds it, for comparing a before and an
/// after: a listing must leave every record exactly as it was.
fn stored_instances(emulator: &Emulator) -> String {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    format!("{:?}", store.gateway_instances)
}

/// The `(GatewayArn, InstanceId, InstanceState)` the store holds for `arn`, so the response
/// is compared against the PERSISTED record rather than against a literal.
fn stored_record(emulator: &Emulator, arn: &str) -> (String, String, String) {
    let state = emulator
        .engine
        .state::<mc::ComAmazonawsMediaconnectMediaconnectState>();
    let store = state.lock();
    let instance = store
        .gateway_instances
        .get(arn)
        .unwrap_or_else(|| panic!("the fixture instance {arn} is stored"));
    (
        instance.gateway_arn.clone(),
        instance.instance_id.clone(),
        instance.instance_state.clone(),
    )
}

/// Every value of the modelled `InstanceState`
/// (`smithy_query(query="shape",shape="InstanceState")`: REGISTERING | ACTIVE |
/// DEREGISTERING | DEREGISTERED | REGISTRATION_ERROR | DEREGISTRATION_ERROR). A listing
/// reports each one as the record carries it, the terminal `DEREGISTERED` included: a
/// deregistered instance stays a readable resource.
const INSTANCE_STATES: &[&str] = &[
    mc::INSTANCE_STATE_REGISTERING,
    mc::INSTANCE_STATE_ACTIVE,
    mc::INSTANCE_STATE_DEREGISTERING,
    mc::INSTANCE_STATE_DEREGISTERED,
    mc::INSTANCE_STATE_REGISTRATION_ERROR,
    mc::INSTANCE_STATE_DEREGISTRATION_ERROR,
];

/// The listing reports every stored instance, projected from its own record, whatever state
/// that record is in — and it stores nothing.
#[tokio::test]
async fn every_stored_instance_is_listed_from_its_own_record() {
    let emulator = Emulator::new();
    // Two gateways, so a summary that reported the wrong gateway's ARN would fail here
    // rather than pass unnoticed.
    let first_gateway = gateway_with(&emulator, "gw-listing-one").await;
    let second_gateway = gateway_with(&emulator, "gw-listing-two").await;

    for instance_state in INSTANCE_STATES {
        instance_in_state(&emulator, &first_gateway, instance_state);
    }
    instance_in_state(&emulator, &second_gateway, mc::INSTANCE_STATE_ACTIVE);

    let expected = stored_arns(&emulator);
    assert_eq!(
        expected.len(),
        INSTANCE_STATES.len() + 1,
        "the fixtures must all be stored, got {expected:?}"
    );
    let before = stored_instances(&emulator);

    let body = page(&emulator, "every state at once", "?maxResults=1000").await;
    assert_eq!(
        listed_arns(&body),
        expected,
        "every stored instance is listed, in the store's own order, got {body}"
    );

    for (index, arn) in expected.iter().enumerate() {
        let case = format!("instance {index} ({arn})");
        let (gateway_arn, instance_id, instance_state) = stored_record(&emulator, arn);
        let summary = listed(&body)
            .into_iter()
            .find(|summary| summary["gatewayInstanceArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the stored instance is listed: {body}"));
        assert_eq!(
            (
                summary["gatewayArn"].as_str(),
                summary["instanceId"].as_str(),
                summary["instanceState"].as_str(),
            ),
            (
                Some(gateway_arn.as_str()),
                Some(instance_id.as_str()),
                Some(instance_state.as_str()),
            ),
            "case {case:?}: the summary is the stored record, got {summary}"
        );
        assert!(
            mc::is_mintable_gateway_instance_arn(arn),
            "case {case:?}: a listed instance ARN must be one this service could mint, got \
             {arn:?}"
        );
        // A `ListedGatewayInstance` carries exactly four members, so the summary is not the
        // whole `GatewayInstance`: `BridgePlacement`, `ConnectionStatus`,
        // `InstanceMessages` and `RunningBridgeCount` stay on DescribeGatewayInstance's
        // projection.
        assert_eq!(
            summary.as_object().map(|members| members.len()),
            Some(4),
            "case {case:?}: a summary carries only the modelled members, got {summary}"
        );
    }

    // Every value of the enum was reported, so no row of the table went unasserted.
    let mut reported_states: Vec<String> = listed(&body)
        .iter()
        .map(|summary| {
            summary["instanceState"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    reported_states.sort();
    reported_states.dedup();
    let mut expected_states: Vec<String> =
        INSTANCE_STATES.iter().map(ToString::to_string).collect();
    expected_states.sort();
    expected_states.dedup();
    assert_eq!(
        reported_states, expected_states,
        "every value of the modelled InstanceState is listed as stored, got {body}"
    );

    // A read does not change what a later caller sees: the second listing is the same
    // document and the store is untouched.
    let again = page(&emulator, "repeated listing", "?maxResults=1000").await;
    assert_eq!(
        again, body,
        "listing twice reports the same page, got {again}"
    );
    let after = stored_instances(&emulator);
    assert_eq!(
        after, before,
        "listing instances must store nothing; expected {before}, got {after}"
    );
}

/// One `FilterArn` case: how many instances each gateway holds, and what the filter must
/// report.
#[derive(Debug)]
struct FilterRow {
    case: &'static str,
    /// How many instances each gateway of the fixture store is given, in order.
    instances_per_gateway: &'static [usize],
}

/// `FilterArn` narrows the whole-account listing to the instances registered with the
/// gateway it names, and an absent filter reports them all. The rows vary how the instances
/// are spread over the gateways, including a gateway with none.
#[tokio::test]
async fn filter_arn_reports_only_the_named_gateways_instances() {
    const ROWS: &[FilterRow] = &[
        FilterRow {
            case: "one gateway holding one instance",
            instances_per_gateway: &[1],
        },
        FilterRow {
            case: "two gateways, one of them with no instance",
            instances_per_gateway: &[2, 0],
        },
        FilterRow {
            case: "three gateways holding different counts",
            instances_per_gateway: &[3, 1, 4],
        },
    ];

    for row in ROWS {
        let emulator = Emulator::new();
        let case = row.case;
        let mut gateways: Vec<(String, Vec<String>)> = Vec::new();
        for (index, count) in row.instances_per_gateway.iter().enumerate() {
            let gateway_arn = gateway_with(&emulator, &format!("gw-filter-{index}")).await;
            let instances: Vec<String> = (0..*count)
                .map(|_| register_instance(&emulator, &gateway_arn))
                .collect();
            gateways.push((gateway_arn, instances));
        }

        let unfiltered = page(&emulator, case, "?maxResults=1000").await;
        assert_eq!(
            listed_arns(&unfiltered),
            stored_arns(&emulator),
            "case {case:?}: with no filter every instance of the account is listed, got \
             {unfiltered}"
        );

        for (gateway_arn, instances) in &gateways {
            let body = page(
                &emulator,
                case,
                &format!("?maxResults=1000&filterArn={gateway_arn}"),
            )
            .await;
            assert_eq!(
                listed_arns(&body),
                instances.clone(),
                "case {case:?}: the filter reports only {gateway_arn}'s instances, got {body}"
            );
            for summary in listed(&body) {
                assert_eq!(
                    summary["gatewayArn"].as_str(),
                    Some(gateway_arn.as_str()),
                    "case {case:?}: every filtered summary names the selected gateway, got \
                     {summary}"
                );
            }
        }

        // A gateway ARN no instance is registered with matches nothing: the filter narrows
        // a listing, so it is an empty page and not a refusal or a not-found.
        let unknown = "arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-deadbeef:missing";
        let body = page(&emulator, case, &format!("?filterArn={unknown}")).await;
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: a gateway with no instances lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );
    }
}

/// The listing pages every instance exactly once, whatever `MaxResults` is, and the token is
/// the ARN the next page resumes at. With no `MaxResults` the page holds the documented
/// default of ten. The same holds under a `FilterArn`.
#[tokio::test]
async fn pages_partition_every_instance_exactly_once() {
    let emulator = Emulator::new();
    // More than the documented default page size, so the default is observable.
    let total = 13_usize;
    let paged_gateway = gateway_with(&emulator, "gw-paging").await;
    let other_gateway = gateway_with(&emulator, "gw-paging-other").await;
    let paged: Vec<String> = (0..total)
        .map(|_| register_instance(&emulator, &paged_gateway))
        .collect();
    // Instances of another gateway, so a filtered paging walk that leaked them would fail.
    let others: Vec<String> = (0..4)
        .map(|_| register_instance(&emulator, &other_gateway))
        .collect();

    let all = stored_arns(&emulator);
    assert_eq!(
        all.len(),
        paged.len() + others.len(),
        "every fixture instance is stored, got {all:?}"
    );

    // "If MaxResults is not included in the request, the service defaults to pagination
    // with a maximum of 10 results per page"
    // (`smithy_query(query="doc",shape="ListGatewayInstancesRequest",member="MaxResults")`).
    let default_page = page(&emulator, "the default page size", "").await;
    assert_eq!(
        listed_arns(&default_page),
        all[..10].to_vec(),
        "the default page holds the first ten instances, got {default_page}"
    );
    assert_eq!(
        default_page["nextToken"].as_str(),
        Some(all[10].as_str()),
        "the default page carries the ARN the next page resumes at, got {default_page}"
    );

    // The page size spans the modelled `MaxResults` range (1..1000): its lower bound, sizes
    // smaller than the set, one that equals it, and the upper bound. Each walk is run with
    // no filter and under one, because the filter and the paging compose.
    for page_size in [1_usize, 2, 7, all.len(), 1000] {
        for (filter, expected) in [
            (None, all.clone()),
            (Some(paged_gateway.as_str()), paged.clone()),
            (Some(other_gateway.as_str()), others.clone()),
        ] {
            let case = format!("page size {page_size}, filter {filter:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut query = format!("?maxResults={page_size}");
                if let Some(filter) = filter {
                    query.push_str(&format!("&filterArn={filter}"));
                }
                if let Some(token) = &token {
                    query.push_str(&format!("&nextToken={token}"));
                }
                let body = page(&emulator, &case, &query).await;
                let listed_page = listed_arns(&body);
                token = body["nextToken"].as_str().map(ToString::to_string);
                if token.is_some() {
                    assert_eq!(
                        listed_page.len(),
                        page_size,
                        "case {case:?}: a page that carries a token is full, got {body}"
                    );
                }
                seen.extend(listed_page);
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: the pages must partition the matching instances once, in \
                 order"
            );
        }
    }

    // The token is the ARN the listing RESUMES AT, so a page resumed from it starts there.
    let first = page(&emulator, "resuming", "?maxResults=3").await;
    let token = first["nextToken"]
        .as_str()
        .expect("a page smaller than the set carries a token")
        .to_string();
    let resumed = page(
        &emulator,
        "resuming",
        &format!("?maxResults=1000&nextToken={token}"),
    )
    .await;
    assert_eq!(
        listed_arns(&resumed),
        all[3..].to_vec(),
        "the listing resumes at the key the token names, got {resumed}"
    );
}

/// An account that holds no instance is listed empty — not a 404 and not a page of invented
/// instances. A store with no gateway at all, and one whose gateway has no instance, are
/// both empty listings.
#[tokio::test]
async fn an_account_that_holds_no_instance_is_listed_empty() {
    let emulator = Emulator::new();
    for case in ["an empty store", "a gateway with no instance"] {
        let body = page(&emulator, case, "").await;
        assert_eq!(
            listed(&body),
            Vec::<Value>::new(),
            "case {case:?}: an account holding no instance lists none, got {body}"
        );
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: the last page carries no token, got {body}"
        );

        // Set the second case up: a gateway, still with no instance registered.
        gateway_with(&emulator, "gw-listing-empty").await;
    }
}

/// The listed `InstanceState` is the CURRENT one: a listing materialises only the state
/// change the clock already made due, so an instance whose registration is still in flight
/// is listed `REGISTERING` and only the transition span settles it into `ACTIVE`.
#[tokio::test]
async fn the_listed_state_is_the_one_the_clock_has_made_due() {
    let emulator = Emulator::new();
    let gateway_arn = gateway_with(&emulator, "gw-listing-clock").await;
    let registered_at = Instant::now();
    let instance_arn = register_instance(&emulator, &gateway_arn);

    let body = page(&emulator, "inside the transition span", "").await;
    let elapsed = registered_at.elapsed();
    assert!(
        elapsed < mc::TRANSITION_SPAN,
        "the listing must happen inside the {:?} transition span, {elapsed:?} elapsed",
        mc::TRANSITION_SPAN
    );
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["instanceState"].as_str()),
        Some(mc::INSTANCE_STATE_REGISTERING),
        "inside the span the registration is still in flight, got {body}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let body = page(&emulator, "past the transition span", "").await;
    assert_eq!(
        listed(&body)
            .first()
            .and_then(|summary| summary["instanceState"].as_str()),
        Some(mc::INSTANCE_STATE_ACTIVE),
        "once the span has passed the registration is settled, got {body}"
    );
    assert_eq!(
        listed_arns(&body),
        vec![instance_arn],
        "the settled instance is still the one listed, got {body}"
    );
}

/// One refused ListGatewayInstances request.
struct RefusalCase {
    case: &'static str,
    query: &'static str,
}

/// ListGatewayInstances refuses a page size outside the modelled range and a pagination
/// token it could not have issued — with the modelled `BadRequestException` (400), never
/// with a 501 and never with a success.
#[tokio::test]
async fn refusals_name_the_modelled_error() {
    let emulator = Emulator::new();

    // The modelled `MaxResults` range is 1..1000, so each bound is usable (the paging suite
    // above lists with 1 and with 1000) and each value past it is refused. A `NextToken` is
    // the instance ARN the listing resumes at, so a token the service could not have minted
    // is refused; the rows are the ways a token can fail that form.
    const CASES: &[RefusalCase] = &[
        RefusalCase {
            case: "MaxResults below the modelled range",
            query: "?maxResults=0",
        },
        RefusalCase {
            case: "MaxResults above the modelled range",
            query: "?maxResults=1001",
        },
        RefusalCase {
            case: "a nextToken that is not an ARN",
            query: "?nextToken=not-a-token",
        },
        RefusalCase {
            case: "a nextToken naming no instance segment",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-abcdef12:g",
        },
        RefusalCase {
            case: "a nextToken from another service",
            query: "?nextToken=arn:aws:medialive:us-east-1:000000000000:gateway:1-abcdef12:g:instance:mi-00000000000000001",
        },
        RefusalCase {
            case: "a nextToken whose account id is too short",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:00000000000:gateway:1-abcdef12:g:instance:mi-00000000000000001",
        },
        RefusalCase {
            case: "a nextToken naming another resource type",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:flow:1-abcdef12:f:instance:mi-00000000000000001",
        },
        RefusalCase {
            case: "a nextToken whose instance id lacks the modelled mi- prefix",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-abcdef12:g:instance:i-00000000000000001",
        },
        RefusalCase {
            case: "a nextToken whose instance id is empty",
            query: "?nextToken=arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-abcdef12:g:instance:",
        },
    ];

    for RefusalCase { case, query } in CASES {
        let (status, code, body) = list_gateway_instances(&emulator, query).await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "case {case:?}: expected 400 BadRequestException, got {status} {code:?}: {body}"
        );
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "case {case:?}: the refusal carries a message, got {body}"
        );
    }

    // A token the service COULD have minted is accepted even when no such instance is
    // stored: paging must survive a deregistration between pages.
    let unknown = format!(
        "arn:aws:mediaconnect:us-east-1:000000000000:gateway:1-deadbeef:missing:instance:{}",
        mc::gateway_instance_id(4_095)
    );
    let body = page(
        &emulator,
        "a well-formed token for an unknown instance",
        &format!("?nextToken={unknown}"),
    )
    .await;
    assert_eq!(
        listed(&body),
        Vec::<Value>::new(),
        "nothing is stored past that key, got {body}"
    );
}
