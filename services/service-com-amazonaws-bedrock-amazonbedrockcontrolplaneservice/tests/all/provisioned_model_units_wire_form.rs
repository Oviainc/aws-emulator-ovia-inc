// SPDX-License-Identifier: CC0-1.0

//! `CreateProvisionedModelThroughput` persists the model-unit count it bought, and every
//! surface that reports the Provisioned Throughput reports that same count.
//!
//! The rule under test: `modelUnits` is a `PositiveInteger` (`range 1..`)
//! (`smithy_query(query="operation",operation="CreateProvisionedModelThroughput")`), so the
//! count a purchase carried is reported back EXACTLY — the same integral value, in the
//! unquoted JSON integer form the shape models, by a read and by a listing alike — and a
//! read never changes it. Nothing about it is rounded, widened to a floating-point value,
//! quoted as text, or recomputed from the request a second time.
//!
//! The rows span what the rule generalises over: the unit count itself (the modelled
//! minimum, counts in between, and the bound the account's commitment quota grants), the
//! commitment term bought, the base model provisioned, and the `modelId` spelling. The
//! recorded case is ONE row of this table.
//!
//! The sibling suite `provisioned_model_throughputs` covers the resource as a whole (its
//! listing, its quota refusals, its status settle, its delete); this one pins only the
//! numeric fidelity of the model-unit count, through the same `crate::support` harness.

use serde_json::{json, Value};

use crate::support::{emulator, foundation_model_arn, label, Emulator, Reply, REGION};

/// Base models this account's published catalog offers WITH Provisioned Throughput (rows of
/// `service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::FOUNDATION_MODELS`
/// whose `inferenceTypesSupported` carries `PROVISIONED`, which `ListFoundationModels`
/// reports).
const PROVISIONABLE_MODELS: &[&str] = &[
    "amazon.titan-text-lite-v1",
    "anthropic.claude-3-haiku-20240307-v1:0",
    "cohere.command-text-v14",
    "meta.llama3-70b-instruct-v1:0",
];

/// `CommitmentDuration` is `[OneMonth, SixMonths]`
/// (`smithy_query(query="shape",shape="CommitmentDuration")`). Every row buys one of them:
/// the no-commitment model-unit quota is granted at zero, so a no-commitment purchase of
/// any count is refused and reports no count at all.
const COMMITMENTS: &[&str] = &["OneMonth", "SixMonths"];

/// The model units this account is granted for a committed Provisioned Throughput, as the
/// service states it — the largest count a row can ask for and be admitted.
const GRANTED_COMMITTED_UNITS: i32 =
    service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::MAX_COMMITTED_MODEL_UNITS;

/// The members a Provisioned Throughput reports its model-unit count under: what it has,
/// and what it is asked to have. Nothing is in flight on a Provisioned Throughput that was
/// just bought, so both carry the count the purchase asked for.
const UNIT_MEMBERS: &[&str] = &["modelUnits", "desiredModelUnits"];

async fn create(emu: &Emulator, body: Value) -> Reply {
    emu.call("POST", "/provisioned-model-throughput", Some(body))
        .await
}

async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/provisioned-model-throughput/{}", label(identifier)),
        None,
    )
    .await
}

async fn list(emu: &Emulator) -> Reply {
    emu.call("GET", "/provisioned-model-throughputs", None).await
}

/// The body member `name` as the JSON value the wire carried, so a row asserts the FORM of
/// the number (an integer, not a float and not a string) beside its value.
fn wire_member(case: &str, value: &Value, name: &str) -> Value {
    value
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the response has no member {name}: {value:?}"))
}

#[tokio::test]
async fn a_purchase_reports_back_the_model_unit_count_it_bought() {
    let emu = emulator();

    // (case, name, base model, modelId spelled as its ARN, units, commitment)
    let rows: Vec<(&str, &str, &str, bool, i32, &str)> = vec![
        (
            "the modelled minimum count",
            "pt-units-min",
            PROVISIONABLE_MODELS[0],
            false,
            1,
            COMMITMENTS[0],
        ),
        (
            "two units, the model named by its ARN",
            "pt-units-two",
            PROVISIONABLE_MODELS[1],
            true,
            2,
            COMMITMENTS[1],
        ),
        (
            "an odd count no power of two rounds to",
            "pt_units_seven",
            PROVISIONABLE_MODELS[2],
            false,
            7,
            COMMITMENTS[0],
        ),
        (
            "a two-digit count, the model named by its ARN",
            "pt-units-13",
            PROVISIONABLE_MODELS[3],
            true,
            13,
            COMMITMENTS[1],
        ),
        (
            "one unit short of the granted units",
            "pt-units-granted-less-one",
            PROVISIONABLE_MODELS[0],
            false,
            GRANTED_COMMITTED_UNITS - 1,
            COMMITMENTS[1],
        ),
        (
            "every unit the commitment quota grants",
            "pt-units-granted",
            PROVISIONABLE_MODELS[1],
            true,
            GRANTED_COMMITTED_UNITS,
            COMMITMENTS[0],
        ),
    ];

    let mut bought: Vec<(&str, String, i32)> = Vec::new();

    for (case, name, model_id, by_arn, units, commitment) in rows {
        let spelled = if by_arn {
            foundation_model_arn(REGION, model_id)
        } else {
            model_id.to_string()
        };
        let reply = create(
            &emu,
            json!({
                "provisionedModelName": name,
                "modelId": spelled,
                "modelUnits": units,
                "commitmentDuration": commitment,
            }),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {case}: the create must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        let arn = reply.text("provisionedModelArn");

        // A read reports the count the purchase carried, as the integer the shape models.
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: a read of {arn} must answer 200, got {} {:?}",
            read.status, read.body
        );
        for member in UNIT_MEMBERS {
            let reported = wire_member(case, &read.body, member);
            assert_eq!(
                reported,
                json!(units),
                "case {case}: {member} must be the integer {units} the purchase bought, got \
                 {reported:?}"
            );
            assert!(
                reported.as_i64().is_some(),
                "case {case}: {member} is a PositiveInteger, so the wire form is an integer, \
                 got {reported:?}"
            );
        }

        // Reading it again reports the same count: a read is not a purchase.
        let again = get(&emu, &arn).await;
        for member in UNIT_MEMBERS {
            assert_eq!(
                wire_member(case, &again.body, member),
                json!(units),
                "case {case}: a second read must report the same {member}, got {:?}",
                again.body
            );
        }

        bought.push((case, arn, units));
    }

    // Every purchase stands on its own: a listing reports each one's count, and the counts
    // the rows asked for are all still there side by side.
    let listing = list(&emu).await;
    assert_eq!(
        listing.status, 200,
        "the listing must answer 200, got {} {:?}",
        listing.status, listing.body
    );
    let summaries = listing
        .get("provisionedModelSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "provisionedModelSummaries must be a present array, got {:?}",
                listing.body
            )
        });
    assert_eq!(
        summaries.len(),
        bought.len(),
        "the listing must report every Provisioned Throughput bought, got {summaries:?}"
    );
    for (case, arn, units) in &bought {
        let summary = summaries
            .iter()
            .find(|row| row.get("provisionedModelArn") == Some(&json!(arn)))
            .unwrap_or_else(|| {
                panic!("case {case}: the listing must report {arn}, got {summaries:?}")
            });
        for member in UNIT_MEMBERS {
            let reported = wire_member(case, summary, member);
            assert_eq!(
                reported,
                json!(units),
                "case {case}: the listed {member} must agree with the count bought, got \
                 {reported:?}"
            );
        }
    }
}

/// The count survives the clock: the move out of the non-terminal status the create stored
/// changes the status and nothing about the model units.
#[tokio::test]
async fn the_model_unit_count_survives_the_status_settle() {
    let emu = emulator();

    let settle_after =
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::PROVISIONED_MODEL_SETTLE_AFTER;

    for (case, units, commitment) in [
        ("the minimum count", 1, COMMITMENTS[0]),
        ("an odd count", 9, COMMITMENTS[1]),
        (
            "every granted unit",
            GRANTED_COMMITTED_UNITS,
            COMMITMENTS[0],
        ),
    ] {
        let name = format!("pt-settle-{units}");
        let reply = create(
            &emu,
            json!({
                "provisionedModelName": &name,
                "modelId": PROVISIONABLE_MODELS[0],
                "modelUnits": units,
                "commitmentDuration": commitment,
            }),
        )
        .await;
        assert_eq!(
            reply.status, 201,
            "case {case}: the create must answer 201, got {} {:?}",
            reply.status, reply.body
        );

        tokio::time::sleep(settle_after + std::time::Duration::from_millis(50)).await;

        let read = get(&emu, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case}: a read must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("status"),
            "InService",
            "case {case}: the settle the create owed must have been materialised, got {:?}",
            read.body
        );
        for member in UNIT_MEMBERS {
            assert_eq!(
                wire_member(case, &read.body, member),
                json!(units),
                "case {case}: the settle must leave {member} at the count bought, got {:?}",
                read.body
            );
        }
    }
}
