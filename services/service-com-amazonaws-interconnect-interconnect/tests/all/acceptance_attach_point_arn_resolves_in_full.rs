// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ARN an ACCEPTANCE selects its attach point by, driven over
//! the wire through the shared [`crate::emulator`] harness.
//!
//! RULE under test: an `AttachPoint$arn` identifies the attach point in FULL — partition,
//! Region, account and resource id — and is resolved against the connections the Region
//! the REQUEST addressed holds, so an ARN differing from a stored connection's own ARN in
//! any one of those fields, or naming a connection of another Region, is an ARN this
//! endpoint holds no attach point for: the acceptance is refused with the operation's
//! modelled 404, worded exactly as an id this scope never allocated is, and the proposal
//! is left as it was.
//!
//! The rows differ along what the rule generalises over: WHICH field of the ARN differs
//! from the stored one (partition, Region, account, resource id), the shape of the value
//! that field carries (two partitions, two Regions, two unallocated id prefixes), the
//! Region the acceptance is signed for, and the environment and bandwidth the proposal
//! sits on. The ARN fields themselves come from the member's own pattern
//! (`smithy_query(query="shape",shape="AttachPoint")` -> `arn`:
//! `^arn:aws[a-z-]*:interconnect:[^:]+:[0-9]{12}:connection/(mcc|lmcc)-[a-z0-9]{8}$`), so
//! every row below is a value that member accepts and the handler therefore has to judge.

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::attach_point_not_valid_message;

use crate::emulator::{
    code, message, placements, remote_account_for, text, Emulator, DEFAULT_SIGNING_REGION,
    SIGNING_NAME,
};

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The partition every ARN this emulator mints carries.
const PARTITION: &str = "aws";

/// An account id that is not this emulator's, of the twelve digits the member's pattern
/// writes — so the only thing wrong with the ARN is whose account it names.
const OTHER_ACCOUNT_ID: &str = "123456789012";

/// Partitions other than the one this emulator mints ARNs in, both of the
/// `aws[a-z-]*` shape the member's pattern allows.
const OTHER_PARTITIONS: [&str; 2] = ["aws-cn", "aws-us-gov"];

/// Resource ids no `CreateConnection` of these rows allocated, one of each prefix a
/// `ConnectionId` may carry (`(mcc|lmcc)-[a-z0-9]{8}`).
const UNALLOCATED_IDS: [&str; 2] = ["mcc-00000000", "lmcc-zzzzzzzz"];

/// A connection ARN with the fields a row wants, written the way the member's pattern
/// writes them: `arn:<partition>:interconnect:<region>:<account>:connection/<id>`.
fn connection_arn(partition: &str, region: &str, account: &str, id: &str) -> String {
    format!("arn:{partition}:{SIGNING_NAME}:{region}:{account}:connection/{id}")
}

/// `CreateConnection` on `placement` with `attach_point`, signed for `region`, as a
/// `(status, body)` pair so a refusal is readable. The `remoteAccount` is taken from the
/// Environment's own record, so a row about the attach point is not also a row about the
/// identifying information.
async fn create_in(
    emulator: &Emulator,
    region: &str,
    placement: &(String, String),
    attach_point: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(placement.0));
    request.insert("bandwidth".to_string(), json!(placement.1));
    request.insert("attachPoint".to_string(), attach_point.clone());
    if let Some(remote_account) = remote_account_for(&emulator.environment(&placement.0).await) {
        request.insert("remoteAccount".to_string(), remote_account);
    }
    emulator
        .ic_in(region, "CreateConnection", Value::Object(request))
        .await
}

/// A connection of `region`, created on one of the account's own Direct Connect
/// gateways. Fresh out of the create it is the proposal awaiting acceptance; its ARN is
/// also what the rows below mutate one field of.
async fn live_connection(emulator: &Emulator, region: &str, placement: &(String, String)) -> Value {
    let attach_point = json!({ "directConnectGateway": emulator.attach_point().await });
    let (status, body) = create_in(emulator, region, placement, &attach_point).await;
    assert_eq!(
        status, 200,
        "setup {region:?}: expected the connection to be created, got {status} with {body:?}"
    );
    body["connection"].clone()
}

/// Accept the proposal `activation_key` names with the `arn` attach point, signed for
/// `region`.
async fn accept_with_arn(
    emulator: &Emulator,
    region: &str,
    activation_key: &str,
    arn: &str,
) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            "AcceptConnectionProposal",
            json!({
                "activationKey": activation_key,
                "attachPoint": { "arn": arn },
            }),
        )
        .await
}

/// Assert `(status, body)` is the modelled 404 this service answers an attach point that
/// resolves to nothing with, worded for `arn` on `environment_id`.
fn assert_not_an_attach_point(
    case: &str,
    arn: &str,
    environment_id: &str,
    status: u16,
    body: &Value,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected the modelled 404 for an attach point that resolves to nothing, got {status} with {body:?}"
    );
    assert!(
        code(body).contains("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {:?}",
        code(body)
    );
    assert_eq!(
        message(body),
        attach_point_not_valid_message(arn, environment_id),
        "case {case:?}: expected this service's own wording for an attach point that resolves to nothing, got {:?}",
        message(body)
    );
}

/// RULE: an acceptance's `attachPoint.arn` resolves only to a connection the Region the
/// request addressed holds, matched field for field — so an ARN whose partition, Region,
/// account or resource id is not that connection's resolves to nothing, the acceptance is
/// refused with the same 404 and the same wording whichever field differs, and the
/// proposal keeps the attach point it was created on. The same ARN, with every field the
/// stored one's, is accepted and read back.
#[tokio::test]
async fn an_acceptance_resolves_an_attach_point_arn_field_for_field_in_the_region_it_addresses() {
    let emulator = Emulator::new();
    let places = placements(&emulator).await;

    // The rows differ in the Region the acceptance addresses and in the OTHER Region a
    // live connection sits in: the Region the rest of this crate signs for, and three
    // more on three other continents, so no row's Region is the harness default by
    // accident.
    let region_rows: [(&str, &str); 2] = [
        (DEFAULT_SIGNING_REGION, "eu-west-1"),
        ("ap-southeast-2", "sa-east-1"),
    ];

    let mut refusals: Vec<(u16, String)> = Vec::new();
    for (seed, (caller, elsewhere)) in region_rows.into_iter().enumerate() {
        let caller_placement = &places[seed % places.len()];
        let foreign_placement = &places[(seed + 1) % places.len()];

        // One live connection of the Region the acceptance addresses, whose ARN is the
        // attach point that DOES resolve, and one live connection of another Region,
        // whose ARN names a resource this endpoint holds nothing for.
        let local = live_connection(&emulator, caller, caller_placement).await;
        let local_id = text(&local, "id");
        let local_arn = text(&local, "arn");
        assert_eq!(
            local_arn,
            connection_arn(PARTITION, caller, ACCOUNT_ID, &local_id),
            "case {caller:?}: expected the stored ARN to carry this request's partition, Region and account, got {local_arn:?}"
        );
        let foreign_id = text(
            &live_connection(&emulator, elsewhere, foreign_placement).await,
            "id",
        );

        // Every row is the stored ARN with exactly ONE field moved out of the scope the
        // request addresses, so what each refusal shows is that field being judged.
        let mut rows: Vec<(String, String)> = Vec::new();
        for partition in OTHER_PARTITIONS {
            rows.push((
                format!("partition {partition}"),
                connection_arn(partition, caller, ACCOUNT_ID, &local_id),
            ));
        }
        rows.push((
            "this Region's connection id in another Region".to_string(),
            connection_arn(PARTITION, elsewhere, ACCOUNT_ID, &local_id),
        ));
        rows.push((
            "a live connection of another Region".to_string(),
            connection_arn(PARTITION, elsewhere, ACCOUNT_ID, &foreign_id),
        ));
        rows.push((
            "another Region's connection id, carried by an ARN of this Region".to_string(),
            connection_arn(PARTITION, caller, ACCOUNT_ID, &foreign_id),
        ));
        rows.push((
            format!("account {OTHER_ACCOUNT_ID}"),
            connection_arn(PARTITION, caller, OTHER_ACCOUNT_ID, &local_id),
        ));
        for unallocated in UNALLOCATED_IDS {
            rows.push((
                format!("an id this scope never allocated ({unallocated})"),
                connection_arn(PARTITION, caller, ACCOUNT_ID, unallocated),
            ));
        }

        for (field, arn) in &rows {
            // A proposal per row, accepted in the call right after the create: a
            // proposal is only awaiting acceptance until the clock hands it on.
            let proposal = live_connection(&emulator, caller, caller_placement).await;
            let case = format!("{field} offered in {caller}");
            let (status, body) = accept_with_arn(
                &emulator,
                caller,
                &text(&proposal, "activationKey"),
                arn,
            )
            .await;
            assert_not_an_attach_point(&case, arn, &caller_placement.0, status, &body);
            refusals.push((status, code(&body)));

            // The refusal associated nothing: the proposal still carries the attach
            // point it was created on.
            let unchanged = emulator
                .ic_ok_in(
                    caller,
                    "GetConnection",
                    json!({ "identifier": text(&proposal, "id") }),
                )
                .await;
            assert_eq!(
                unchanged["connection"]["attachPoint"], proposal["attachPoint"],
                "case {case:?}: expected the refused acceptance to leave the proposal's attach point alone, got {:?}",
                unchanged["connection"]["attachPoint"]
            );
        }

        // The SAME ARN with every field the stored one's is the attach point the
        // acceptance selects, and the accepted connection reports it back.
        let accepted_proposal = live_connection(&emulator, caller, caller_placement).await;
        let (status, accepted) = accept_with_arn(
            &emulator,
            caller,
            &text(&accepted_proposal, "activationKey"),
            &local_arn,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {local_arn:?} in {caller}: expected the ARN of a connection of this Region to be accepted, got {status} with {accepted:?}"
        );
        assert_eq!(
            accepted["connection"]["attachPoint"],
            json!({ "arn": local_arn }),
            "case {local_arn:?} in {caller}: expected the accepted connection to carry the ARN attach point, got {:?}",
            accepted["connection"]["attachPoint"]
        );
        let fetched = emulator
            .ic_ok_in(
                caller,
                "GetConnection",
                json!({ "identifier": text(&accepted["connection"], "id") }),
            )
            .await;
        assert_eq!(
            fetched["connection"]["attachPoint"],
            json!({ "arn": local_arn }),
            "case {local_arn:?} in {caller}: expected GetConnection to report the ARN attach point, got {:?}",
            fetched["connection"]["attachPoint"]
        );
    }

    // Whichever field put the ARN outside the scope, the answer is the same one an id
    // this scope never allocated gets, so a refusal never reports whether another scope
    // holds the resource.
    let distinct: std::collections::BTreeSet<(u16, String)> = refusals.iter().cloned().collect();
    assert_eq!(
        distinct.len(),
        1,
        "case \"indistinguishable refusals\": expected every out-of-scope ARN to be refused alike, got {distinct:?}"
    );
}
