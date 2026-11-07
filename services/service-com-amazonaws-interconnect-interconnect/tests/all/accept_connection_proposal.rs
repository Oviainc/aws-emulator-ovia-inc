// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `AcceptConnectionProposal`, driven over the wire the way a
//! client drives it: `POST /` with `X-Amz-Target: Interconnect.<Operation>` on
//! `awsJson1_0`, through the same `MultiServiceRouter::dispatch` the socket front
//! door uses.
//!
//! The RULE under test: an `activationKey` names the proposal this store holds for it
//! — the same proposal `DescribeConnectionProposal` reads its chosen parameters out of
//! — and accepting it associates THAT proposal's connection with the Attach Point the
//! request selects, leaving the Environment and the bandwidth chosen on the partner's
//! portal untouched, and reports the connection in the state the model documents an
//! accepted proposal's connection in. A key that names no proposal, and an Attach
//! Point that resolves to nothing, are both refused with the operation's modelled 404
//! and change nothing; a proposal that has already been accepted is reported again
//! only for the same acceptance.
//!
//! Every assertion is checked over a TABLE built from the live seeded catalog, whose
//! rows differ in the environment, the provider kind, the bandwidth, the Region the
//! request is signed for, the gateway the proposal was created on, the gateway (or
//! ARN) the acceptance selects, the presence of the optional description, and the tag
//! sets the proposal and the acceptance carry.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, tags_of, text,
    Emulator,
};

/// The two Regions the rows sign their requests for, so the rule is checked across the
/// Region a proposal belongs to as well as across the proposal itself.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The gateways the rows attach their connections to: an attach point IS a Direct
/// Connect gateway of the account, so the suite creates its own and the rows differ in
/// which one the proposal was created on, which one its acceptance selects, in each
/// gateway's name and in its `amazonSideAsn`.
const GATEWAY_ROWS: [(&str, Option<i64>); 2] =
    [("ic-accept-a", None), ("ic-accept-b", Some(64_512))];

/// The `ConnectionState` the model documents for an accepted proposal's connection:
/// "pending: The connection has been accepted and is being provisioned between AWS and
/// the Partner" (`smithy_query(query="shape",shape="Connection")`, member `state`).
const ACCEPTED_STATE: &str = "pending";

/// The `ConnectionState` values a connection whose proposal has been accepted can
/// report by the time a later call reads it: the accepted state itself and the fully
/// provisioned one, which the clock hands it on to. Both are values of the modelled
/// `ConnectionState` enum (`smithy_query(query="shape",shape="ConnectionState")`).
const ACCEPTED_STATES: [&str; 2] = [ACCEPTED_STATE, "available"];

/// The terminal `ConnectionState` a completed `DeleteConnection` settles into.
const DELETED_STATE: &str = "deleted";

/// Long enough to cover a connection's tear-down span, so a record a row deleted has
/// reached its terminal state.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_millis(700);

/// One row: the proposal to create, and the acceptance to make on it.
#[derive(Debug, Clone)]
struct AcceptCase {
    environment: Value,
    bandwidth: String,
    region: &'static str,
    /// The gateway the proposal's connection was created on.
    created_on: String,
    /// The gateway the acceptance selects — the same one on some rows and the other
    /// one on the rest, so both re-association and a repeat of the proposal's own
    /// attach point are covered.
    accept_on: String,
    /// The tag set the proposal itself was created with.
    proposal_tags: BTreeMap<String, String>,
    /// The optional description the acceptance supplies.
    description: Option<String>,
    /// The tags the acceptance asks to associate with the resulting Connection.
    tags: BTreeMap<String, String>,
}

impl AcceptCase {
    fn environment_id(&self) -> String {
        text(&self.environment, "environmentId")
    }

    fn label(&self) -> String {
        format!(
            "{} / {} / {} / created on {} accepted on {}",
            self.environment_id(),
            self.bandwidth,
            self.region,
            self.created_on,
            self.accept_on
        )
    }

    /// The attach point this acceptance selects, as the union member names it.
    fn accept_attach_point(&self) -> Value {
        json!({ "directConnectGateway": self.accept_on })
    }

    /// The tag set the accepted Connection must hold: the proposal's own tags with the
    /// acceptance's merged over them, because a tag write merges rather than replaces.
    fn merged_tags(&self) -> BTreeMap<String, String> {
        let mut merged = self.proposal_tags.clone();
        merged.extend(self.tags.clone());
        merged
    }
}

/// The acceptance table: every bandwidth of every environment that can take a
/// connection, alternating the Region, the gateway the proposal is created on, the
/// gateway its acceptance selects, the presence of the optional description and the
/// two tag sets.
///
/// Each row carries the Environment ITS OWN Region's endpoint offers for that partner
/// offering: an Environment belongs to the Region whose connections it produces, so a
/// create signed for a Region names that Region's own `environmentId`.
async fn accept_table(emulator: &Emulator, gateways: &[String]) -> Vec<AcceptCase> {
    let catalog = emulator.environments().await;
    let mut rows = Vec::new();
    let mut seed = 0usize;
    for environment in creatable_environments(&catalog) {
        for bandwidth in available_bandwidths(&environment) {
            seed += 1;
            let created_on = gateways[seed % gateways.len()].clone();
            // Every third row re-selects the attach point the proposal already
            // carries; the rest select the other gateway of the account.
            let accept_on = if seed.is_multiple_of(3) {
                created_on.clone()
            } else {
                gateways[(seed + 1) % gateways.len()].clone()
            };
            rows.push(AcceptCase {
                environment: emulator
                    .counterpart_in(REGIONS[seed % REGIONS.len()], &environment)
                    .await,
                bandwidth,
                region: REGIONS[seed % REGIONS.len()],
                created_on,
                accept_on,
                proposal_tags: if seed.is_multiple_of(2) {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([("stage".to_string(), "proposal".to_string())])
                },
                description: if seed.is_multiple_of(3) {
                    None
                } else {
                    Some(format!("accepted proposal {seed}"))
                },
                tags: match seed % 3 {
                    0 => BTreeMap::new(),
                    1 => BTreeMap::from([("owner".to_string(), format!("team-{seed}"))]),
                    _ => BTreeMap::from([
                        ("owner".to_string(), format!("team-{seed}")),
                        ("stage".to_string(), "accepted".to_string()),
                    ]),
                },
            });
        }
    }
    rows
}

/// `CreateConnection` for one row, answering the stored `Connection` — the proposal
/// whose activation key the acceptance below is made with.
async fn create_proposal(emulator: &Emulator, case: &AcceptCase) -> Value {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(case.environment_id()));
    request.insert("bandwidth".to_string(), json!(case.bandwidth));
    request.insert(
        "attachPoint".to_string(),
        json!({ "directConnectGateway": case.created_on }),
    );
    request.insert(
        "description".to_string(),
        json!(format!("proposal on {}", case.created_on)),
    );
    // The identifying information this Environment asks every create on it for.
    if let Some(remote_account) = remote_account_for(&case.environment) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    if !case.proposal_tags.is_empty() {
        request.insert("tags".to_string(), json!(case.proposal_tags));
    }
    let created = emulator
        .ic_ok_in(case.region, "CreateConnection", Value::Object(request))
        .await;
    created["connection"].clone()
}

/// An `AcceptConnectionProposal` request body.
fn accept_request(
    activation_key: &str,
    attach_point: &Value,
    description: Option<&String>,
    tags: &BTreeMap<String, String>,
) -> Value {
    let mut request = Map::new();
    request.insert("activationKey".to_string(), json!(activation_key));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(description) = description {
        request.insert("description".to_string(), json!(description));
    }
    if !tags.is_empty() {
        request.insert("tags".to_string(), json!(tags));
    }
    Value::Object(request)
}

/// Every connection `region`'s endpoint holds, walked page by page so the count is the
/// whole store however it is windowed.
async fn stored_connections(emulator: &Emulator, region: &str) -> Vec<Value> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok_in(region, "ListConnections", Value::Object(request))
            .await;
        all.extend(
            listed["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// A `Connection` with its `state` projected out, so two readings of one record can be
/// compared on everything the clock does not move on its own.
fn without_state(connection: &Value) -> Value {
    let mut copy = connection.clone();
    if let Some(map) = copy.as_object_mut() {
        map.remove("state");
    }
    copy
}

/// `GetConnection` in `region`, answering the stored `Connection`.
async fn read_back(emulator: &Emulator, region: &str, identifier: &str) -> Value {
    let fetched = emulator
        .ic_ok_in(region, "GetConnection", json!({ "identifier": identifier }))
        .await;
    fetched["connection"].clone()
}

// ---------------------------------------------------------------------------
// Accepting a proposal
// ---------------------------------------------------------------------------

/// RULE: accepting the proposal an `activationKey` names associates that proposal's
/// connection with the Attach Point the request selects — and with the description and
/// tags it supplies — while the Environment and bandwidth chosen on the partner's
/// portal are left exactly as the proposal holds them, the connection reports the
/// documented accepted state, and no second connection is made.
#[tokio::test]
async fn accepting_a_proposal_makes_the_connection_its_activation_key_names() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = accept_table(&emulator, &gateways).await;
    assert!(
        table.len() >= 4,
        "the acceptance table must span several environments and bandwidths, got {} rows",
        table.len()
    );

    for case in &table {
        let label = case.label();
        let before = stored_connections(&emulator, case.region).await.len();

        // The proposal, and then the acceptance of it: the acceptance is the caller's
        // to make whatever the clock has done in between, so no row has to beat it.
        let proposal = create_proposal(&emulator, case).await;
        let activation_key = text(&proposal, "activationKey");
        let (status, body) = emulator
            .ic_in(
                case.region,
                "AcceptConnectionProposal",
                accept_request(
                    &activation_key,
                    &case.accept_attach_point(),
                    case.description.as_ref(),
                    &case.tags,
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from AcceptConnectionProposal, got {status} with {body:?}"
        );
        let accepted = body["connection"].clone();

        // One record, accepted: every server-owned member of the proposal is the
        // accepted connection's, so the acceptance made the connection the key names
        // rather than some other one.
        for member in [
            "id",
            "arn",
            "sharedId",
            "activationKey",
            "ownerAccount",
            "provider",
            "location",
            "type",
        ] {
            assert_eq!(
                accepted[member], proposal[member],
                "case {label:?}: expected the accepted connection's {member} to be the proposal's, got {:?} vs {:?}",
                accepted[member], proposal[member]
            );
        }

        // "The proposal contains the Environment and bandwidth that were chosen on the
        // partner's portal and cannot be modified."
        for member in ["environmentId", "bandwidth"] {
            assert_eq!(
                accepted[member], proposal[member],
                "case {label:?}: expected the acceptance to leave {member} as the proposal holds it, got {:?} vs {:?}",
                accepted[member], proposal[member]
            );
        }

        // What the acceptance DOES set: the Attach Point it selected, the description
        // it supplied (the proposal's own when it supplied none), and its tags merged
        // onto the proposal's.
        assert_eq!(
            accepted["attachPoint"],
            case.accept_attach_point(),
            "case {label:?}: expected the connection to carry the attach point the acceptance selected, got {:?}",
            accepted["attachPoint"]
        );
        let expected_description = case
            .description
            .clone()
            .unwrap_or_else(|| text(&proposal, "description"));
        assert_eq!(
            text(&accepted, "description"),
            expected_description,
            "case {label:?}: expected the description {expected_description:?}, got {:?}",
            accepted["description"]
        );
        assert_eq!(
            tags_of(&accepted),
            case.merged_tags(),
            "case {label:?}: expected the acceptance's tags merged onto the proposal's ({:?}), got {:?}",
            case.merged_tags(),
            tags_of(&accepted)
        );
        assert_eq!(
            text(&accepted, "state"),
            ACCEPTED_STATE,
            "case {label:?}: expected an accepted proposal's connection to be {ACCEPTED_STATE:?}, got {:?}",
            accepted["state"]
        );

        // Round trip: the acceptance was PERSISTED, so the implemented reader reports
        // the same record — in a state the clock may since have handed on to the fully
        // provisioned one.
        let identifier = text(&accepted, "id");
        let fetched = read_back(&emulator, case.region, &identifier).await;
        assert_eq!(
            without_state(&fetched),
            without_state(&accepted),
            "case {label:?}: expected GetConnection to report the accepted record, got {fetched:?}"
        );
        let read_state = text(&fetched, "state");
        assert!(
            ACCEPTED_STATES.contains(&read_state.as_str()),
            "case {label:?}: expected a read-back state among {ACCEPTED_STATES:?}, got {read_state:?}"
        );

        // The acceptance made no SECOND connection: the store grew by the proposal
        // alone.
        let after = stored_connections(&emulator, case.region).await.len();
        assert_eq!(
            after,
            before + 1,
            "case {label:?}: expected the acceptance to make no second connection, got {after} connections for {before} before the proposal"
        );

        // And the key still names that proposal, whose chosen parameters the
        // acceptance could not modify.
        let described = emulator
            .ic_ok_in(
                case.region,
                "DescribeConnectionProposal",
                json!({ "activationKey": activation_key }),
            )
            .await;
        for member in ["bandwidth", "environmentId"] {
            assert_eq!(
                described[member], proposal[member],
                "case {label:?}: expected the proposal's {member} to survive the acceptance, got {:?}",
                described[member]
            );
        }
    }
}

/// RULE: the `arn` member names an attach point by full ARN, so a Connection the
/// account holds is one an acceptance may select.
#[tokio::test]
async fn an_acceptance_may_select_an_attach_point_by_arn() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = accept_table(&emulator, &gateways).await;

    // Rows differ in the Region the ARN is minted in and addressed from, and in the
    // environment and bandwidth the proposal was made on.
    for case in table.iter().take(REGIONS.len() * 2) {
        let label = case.label();
        // The connection whose ARN the acceptance selects, in the same Region: an ARN
        // identifies its resource in full, so only this Region's own resolves here.
        let base = create_proposal(&emulator, case).await;
        // A connection is the connectivity an attach point names only once it is fully
        // provisioned, so its ARN is offered after the clock has carried it there.
        let base = emulator
            .provisioned_connection(case.region, &text(&base, "id"))
            .await;
        let attach_point = json!({ "arn": text(&base, "arn") });

        let proposal = create_proposal(&emulator, case).await;
        let (status, body) = emulator
            .ic_in(
                case.region,
                "AcceptConnectionProposal",
                accept_request(
                    &text(&proposal, "activationKey"),
                    &attach_point,
                    None,
                    &BTreeMap::new(),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected an ARN naming a connection of the account to be accepted, got {status} with {body:?}"
        );
        assert_eq!(
            body["connection"]["attachPoint"], attach_point,
            "case {label:?}: expected the connection to carry the ARN attach point, got {:?}",
            body["connection"]["attachPoint"]
        );

        let fetched = read_back(&emulator, case.region, &text(&proposal, "id")).await;
        assert_eq!(
            fetched["attachPoint"], attach_point,
            "case {label:?}: expected GetConnection to report the ARN attach point, got {:?}",
            fetched["attachPoint"]
        );
    }
}

// ---------------------------------------------------------------------------
// A key that names no proposal
// ---------------------------------------------------------------------------

/// RULE: an `activationKey` that is base64 text but resolves to no proposal of the
/// Region the request addresses is refused with the operation's modelled
/// `ResourceNotFoundException` (404), in the service's own wording, which never echoes
/// the `@sensitive` key — and the refusal makes nothing. (Text that is not base64 at all
/// is a fault in the REQUEST, which [`crate::activation_key_is_base64`] covers.)
#[tokio::test]
async fn accepting_a_key_that_names_no_proposal_is_refused_and_makes_nothing() {
    use service_com_amazonaws_interconnect_interconnect::state::proposal_not_retrievable_message;

    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let case = accept_table(&emulator, &gateways)
        .await
        .into_iter()
        .next()
        .expect("the acceptance table has rows");

    // One real proposal, so what is refused below is the key and not a store holding
    // no proposal at all.
    let proposal = create_proposal(&emulator, &case).await;
    let known = text(&proposal, "activationKey");

    // Rows differ in the SHAPE of the unresolvable key, every one of them base64 text so
    // that what they test is the resolution and not the encoding: unrelated, a prefix of
    // a real one, a real one with a suffix, a case variant, the shortest text the
    // encoding admits and the published `ActivationKey` maximum (`@length(max: 2048)`).
    // A key whose text is not base64 at all is a fault in the REQUEST, which
    // [`crate::activation_key_is_base64`] covers at every length including the published
    // minimum.
    let mut absent: Vec<String> = vec![
        "bm90QW5BY3RpdmF0aW9uS2V5".to_string(),
        known[..known.len() - 1].to_string(),
        format!("{known}MORE"),
        known.to_ascii_lowercase(),
        "AA".to_string(),
        "A".repeat(2048),
    ];
    absent.retain(|candidate| *candidate != known);

    let before = stored_connections(&emulator, case.region).await;
    for key in &absent {
        let label = format!("absent key of {} characters", key.len());
        let (status, body) = emulator
            .ic_in(
                case.region,
                "AcceptConnectionProposal",
                accept_request(
                    key,
                    &case.accept_attach_point(),
                    None,
                    &BTreeMap::new(),
                ),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the modelled 404 for a key that names no proposal, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            proposal_not_retrievable_message(),
            "case {label:?}: expected the service's own refusal wording, got {:?}",
            message(&body)
        );
        assert!(
            !message(&body).contains(key.as_str()),
            "case {label:?}: expected the sensitive activationKey not to be echoed, got {:?}",
            message(&body)
        );
    }

    // A proposal belongs to the Region it was made in, so its own key names no
    // proposal at any other Region's endpoint — the same answer an invented key gets.
    for region in REGIONS.iter().filter(|region| **region != case.region) {
        let label = format!("a key of {} accepted in {region}", case.region);
        let (status, body) = emulator
            .ic_in(
                region,
                "AcceptConnectionProposal",
                accept_request(
                    &known,
                    &case.accept_attach_point(),
                    None,
                    &BTreeMap::new(),
                ),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a key of another Region to name no proposal, got {status} with {body:?}"
        );
        assert_eq!(
            message(&body),
            proposal_not_retrievable_message(),
            "case {label:?}: expected the service's own refusal wording, got {:?}",
            message(&body)
        );
    }

    // Not one of those refusals made anything, and the proposal they were refused
    // beside is untouched but for the state the clock owes it.
    let after = stored_connections(&emulator, case.region).await;
    assert_eq!(
        after.len(),
        before.len(),
        "case \"refused keys\": expected the refusals to have made nothing, got {} connections for {} before",
        after.len(),
        before.len()
    );

    // A key outside the modelled length, and a request missing a required member, are
    // refused at the boundary as malformed requests — a 400, never the 404 above and
    // never a 501.
    for (label, body) in [
        (
            "one past the 2048-character maximum",
            accept_request(
                &"A".repeat(2049),
                &case.accept_attach_point(),
                None,
                &BTreeMap::new(),
            ),
        ),
        (
            "one short of the 1-character minimum",
            accept_request("", &case.accept_attach_point(), None, &BTreeMap::new()),
        ),
        ("no activationKey at all", json!({ "attachPoint": case.accept_attach_point() })),
        ("no attachPoint at all", json!({ "activationKey": known })),
    ] {
        let (status, answered) = emulator
            .ic_in(case.region, "AcceptConnectionProposal", body)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a malformed request, got {status} with {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An attach point that resolves to nothing
// ---------------------------------------------------------------------------

/// RULE: an `attachPoint` an acceptance selects has to identify an attach point that
/// EXISTS and is valid for the Environment the proposal is placed upon — the set
/// `ListAttachPoints` reports for it for the `directConnectGateway` member, a
/// Connection of the account for the `arn` one — and one that resolves to nothing is
/// refused with the modelled 404, leaving the proposal's own attach point in place.
#[tokio::test]
async fn an_attach_point_that_resolves_to_nothing_is_refused_and_accepts_nothing() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = accept_table(&emulator, &gateways).await;
    let case = table.first().expect("the acceptance table has rows").clone();
    let account = engine::EMULATOR_ACCOUNT_ID;
    let region = case.region;

    // Rows differ in the attach-point MEMBER and in the shape of the identifier that
    // resolves to nothing: a gateway id of each UUID flavour the member's pattern
    // allows, a case variant of a gateway the account does hold, and connection ARNs
    // that name no connection — one in the Region the request addresses, one in
    // another Region, one in another account.
    let unresolvable: Vec<(String, Value)> = vec![
        (
            "a v4 gateway id the account does not hold".to_string(),
            json!({ "directConnectGateway": "00000000-0000-4000-8000-000000000000" }),
        ),
        (
            "a gateway id of all digits".to_string(),
            json!({ "directConnectGateway": "12345678-1234-1234-1234-123456789012" }),
        ),
        (
            "a gateway id of all hex letters".to_string(),
            json!({ "directConnectGateway": "ffffffff-ffff-ffff-ffff-ffffffffffff" }),
        ),
        (
            "an upper-cased variant of a gateway the account holds".to_string(),
            json!({ "directConnectGateway": case.accept_on.to_ascii_uppercase() }),
        ),
        (
            "an ARN naming no connection of this Region".to_string(),
            json!({ "arn": format!("arn:aws:interconnect:{region}:{account}:connection/mcc-00000000") }),
        ),
        (
            "an ARN naming no connection of another Region".to_string(),
            json!({ "arn": format!("arn:aws:interconnect:ap-south-1:{account}:connection/lmcc-zzzzzzzz") }),
        ),
        (
            "an ARN naming a connection of another account".to_string(),
            json!({ "arn": format!("arn:aws:interconnect:{region}:123456789012:connection/mcc-0a1b2c3d") }),
        ),
    ];

    for (shape, attach_point) in &unresolvable {
        let label = format!("{} / {shape}", case.label());
        // A proposal of its own per row, so each row is refused for its attach point
        // and not for a proposal an earlier row has already accepted.
        let proposal = create_proposal(&emulator, &case).await;
        let (status, body) = emulator
            .ic_in(
                case.region,
                "AcceptConnectionProposal",
                accept_request(
                    &text(&proposal, "activationKey"),
                    attach_point,
                    None,
                    &BTreeMap::new(),
                ),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the modelled 404 for an attach point that resolves to nothing, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            code(&body)
        );
        let identifier = attach_point
            .as_object()
            .and_then(|members| members.values().next())
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            message(&body).contains(&identifier)
                && message(&body).contains(&case.environment_id()),
            "case {label:?}: expected the refusal to name the attach point and the environment, got {:?}",
            message(&body)
        );

        // The refused acceptance wrote nothing: the proposal still carries the attach
        // point it was made on.
        let fetched = read_back(&emulator, case.region, &text(&proposal, "id")).await;
        assert_eq!(
            fetched["attachPoint"],
            json!({ "directConnectGateway": case.created_on }),
            "case {label:?}: expected the refused acceptance to leave the attach point alone, got {:?}",
            fetched["attachPoint"]
        );
    }
}

// ---------------------------------------------------------------------------
// A proposal that has already been accepted
// ---------------------------------------------------------------------------

/// RULE: a proposal is accepted once. The same acceptance repeated reports the same
/// connection and changes nothing — the operation is `@idempotent` — while one asking
/// for anything that connection does not hold is refused as this service's own client
/// fault, and a proposal whose connection has been deleted can no longer be accepted
/// at all. No refusal echoes the `@sensitive` key.
#[tokio::test]
async fn an_accepted_proposal_is_accepted_again_only_as_the_same_acceptance() {
    let emulator = Emulator::new();
    let gateways = emulator.ready_gateways(&GATEWAY_ROWS).await;
    let table = accept_table(&emulator, &gateways).await;

    for case in table.iter().take(REGIONS.len() * 2) {
        let label = case.label();
        let proposal = create_proposal(&emulator, case).await;
        let activation_key = text(&proposal, "activationKey");
        let identifier = text(&proposal, "id");
        let request = accept_request(
            &activation_key,
            &case.accept_attach_point(),
            case.description.as_ref(),
            &case.tags,
        );

        let accepted = emulator
            .ic_ok_in(case.region, "AcceptConnectionProposal", request.clone())
            .await["connection"]
            .clone();

        // The SAME acceptance, repeated: the same connection, and nothing about it
        // moved but the state the clock owes it.
        let again = emulator
            .ic_ok_in(case.region, "AcceptConnectionProposal", request.clone())
            .await["connection"]
            .clone();
        assert_eq!(
            without_state(&again),
            without_state(&accepted),
            "case {label:?}: expected a repeated acceptance to report the same connection, got {again:?}"
        );
        assert!(
            ACCEPTED_STATES.contains(&text(&again, "state").as_str()),
            "case {label:?}: expected a repeated acceptance to report an accepted state, got {:?}",
            again["state"]
        );

        // Rows differ in WHAT the second acceptance asks for that the connection does
        // not hold: another attach point, another description, another tag value.
        let other_gateway = gateways
            .iter()
            .find(|gateway| **gateway != case.accept_on)
            .expect("the account holds more than one gateway")
            .clone();
        let differing: Vec<(&str, Value)> = vec![
            (
                "another attach point",
                accept_request(
                    &activation_key,
                    &json!({ "directConnectGateway": other_gateway }),
                    case.description.as_ref(),
                    &case.tags,
                ),
            ),
            (
                "another description",
                accept_request(
                    &activation_key,
                    &case.accept_attach_point(),
                    Some(&"a different description".to_string()),
                    &case.tags,
                ),
            ),
            (
                "another tag value",
                accept_request(
                    &activation_key,
                    &case.accept_attach_point(),
                    case.description.as_ref(),
                    &BTreeMap::from([("owner".to_string(), "someone-else".to_string())]),
                ),
            ),
        ];
        for (shape, body) in differing {
            let row = format!("{label} / {shape}");
            let (status, answered) = emulator
                .ic_in(case.region, "AcceptConnectionProposal", body)
                .await;
            assert_eq!(
                status, 400,
                "case {row:?}: expected an accepted proposal to refuse a different acceptance, got {status} with {answered:?}"
            );
            assert!(
                code(&answered).contains("InterconnectClientException"),
                "case {row:?}: expected InterconnectClientException, got {:?}",
                code(&answered)
            );
            assert!(
                message(&answered).contains(&identifier)
                    && !message(&answered).contains(&activation_key),
                "case {row:?}: expected the refusal to name the connection and not the sensitive key, got {:?}",
                message(&answered)
            );
            // And it changed nothing.
            let fetched = read_back(&emulator, case.region, &identifier).await;
            assert_eq!(
                without_state(&fetched),
                without_state(&accepted),
                "case {row:?}: expected the refused acceptance to change nothing, got {fetched:?}"
            );
        }

        // A proposal whose connection is gone has no acceptance left to make: the
        // refusal names the terminal state the connection reached.
        emulator
            .ic_ok_in(
                case.region,
                "DeleteConnection",
                json!({ "identifier": identifier }),
            )
            .await;
        tokio::time::sleep(SETTLE_WAIT).await;
        let (status, answered) = emulator
            .ic_in(case.region, "AcceptConnectionProposal", request)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected a deleted connection's proposal to be unacceptable, got {status} with {answered:?}"
        );
        assert!(
            code(&answered).contains("InterconnectClientException"),
            "case {label:?}: expected InterconnectClientException, got {:?}",
            code(&answered)
        );
        assert!(
            message(&answered).contains(&identifier)
                && message(&answered).contains(DELETED_STATE)
                && !message(&answered).contains(&activation_key),
            "case {label:?}: expected the refusal to name the connection and its state, got {:?}",
            message(&answered)
        );
    }
}
