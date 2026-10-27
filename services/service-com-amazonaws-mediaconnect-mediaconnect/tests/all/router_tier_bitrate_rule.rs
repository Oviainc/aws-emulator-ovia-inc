// SPDX-License-Identifier: CC0-1.0

//! The tier-bounds-the-bitrate rule, driven end to end over HTTP on BOTH sides of the
//! router.
//!
//! A router tier "sets the maximum bitrate, in Mbps" for the resource it configures
//! (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")` and
//! `smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`, which
//! `UpdateRouterInput$Tier` and `UpdateRouterOutput$Tier` repeat), so the tier a resource
//! ENDS UP with resolves to a ceiling on the published Mbps ladder, and the `MaximumBitrate`
//! it is asked for must be a positive count of bits per second at most that ceiling. Router
//! inputs and router outputs name the SAME ladder, each step prefixed by the side of the
//! router it belongs to, so one rule serves all four operations that set either member —
//! CreateRouterInput, UpdateRouterInput, CreateRouterOutput and UpdateRouterOutput — and
//! this suite is what pins the two sides to it TOGETHER: every case drives the same pair
//! through both sides and asserts the verdict, the status, the error code and the refusal's
//! sentence, which may differ between them only in the tier the request named and the noun
//! for the resource.
//!
//! The rows vary what the rule generalises over: every step of the published ladder, and at
//! each step a bitrate well inside it, half of it, exactly at it, one bit per second past it,
//! ten times it, none at all and a negative one; then, for an update, which of the two
//! members the request moves — the bitrate alone, the tier alone, both, or neither. The
//! ladder below is written out by hand from the tier documentation named above rather than
//! read from the crate, so a bug in the crate's own encoding of it cannot hide here.
//!
//! Every request goes through the harness and the create and read helpers the router input
//! and router output suites own, so these cases reach the same request path a MediaConnect
//! client does: routing, restJson1 deserialization, constraint validation, handler,
//! serialization.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::Emulator;
use crate::router_input_restart::{
    create_body as router_input_create_body, create_router_input, get_router_input,
    minted_configuration_rows,
};
use crate::router_outputs::{
    batch_get_router_outputs, create_router_output, minted_network_interface, standard,
};

/// The Mbps steps a router tier names, as the tier documentation publishes them: "INPUT_20 –
/// 20 Mbps, INPUT_50 – 50 Mbps, INPUT_100 – 100 Mbps, INPUT_250 – 250 Mbps, INPUT_500 – 500
/// Mbps" (`smithy_query(query="doc",shape="CreateRouterInputRequest",member="Tier")`) and the
/// same five numbers under `OUTPUT_*`
/// (`smithy_query(query="doc",shape="CreateRouterOutputRequest",member="Tier")`), which
/// together are every value the two tier enums declare
/// (`smithy_query(query="shape",shape="RouterInputTier")` and
/// `smithy_query(query="shape",shape="RouterOutputTier")`).
const PUBLISHED_MEGABITS_PER_SECOND: [i64; 5] = [20, 50, 100, 250, 500];

/// The member name a router INPUT request and answer carries, which is also how a case tells
/// the two sides apart.
const ROUTER_INPUT_MEMBER: &str = "routerInput";

/// One side of the router as a client drives it: the two things the rule is parameterised by
/// — the prefix this side's tier names carry and the noun a refusal calls the resource — plus
/// what a request of this side needs to reach the handler.
struct Side {
    /// The prefix before the Mbps step in every tier name of this side.
    prefix: &'static str,
    /// What this side's resource is called, which is the only side-specific word in a
    /// refusal about a tier.
    noun: &'static str,
    /// The crate's catalog for this side, asserted against the published ladder.
    ladder: mc::RouterTierLadder,
    /// The response member this side's resource is reported under.
    member: &'static str,
    /// The collection URI this side's operations hang off
    /// (`smithy_query(query="operation",operation="UpdateRouterInput")`:
    /// `PUT /v1/routerInput/{Arn}`, and `/v1/routerOutput/{Arn}` for the output side).
    collection: &'static str,
    /// A create body this service accepts, whose name, tier and bitrate each case replaces.
    base_create: Value,
}

impl Side {
    /// The tier name this side gives the `megabits_per_second` step of the ladder.
    fn tier(&self, megabits_per_second: i64) -> String {
        format!("{}_{megabits_per_second}", self.prefix)
    }

    /// Send one create of this side at `tier` and `maximum_bitrate`, through the POST helper
    /// that side's own suite owns.
    async fn create(
        &self,
        emulator: &Emulator,
        name: &str,
        tier: &str,
        maximum_bitrate: i64,
    ) -> (u16, Option<String>, Value) {
        let mut body = self.base_create.clone();
        body["name"] = json!(name);
        body["tier"] = json!(tier);
        body["maximumBitrate"] = json!(maximum_bitrate);
        if self.member == ROUTER_INPUT_MEMBER {
            create_router_input(emulator, body).await
        } else {
            create_router_output(emulator, body).await
        }
    }

    /// Send one update of this side, which binds its `Arn` to the URI.
    async fn update(
        &self,
        emulator: &Emulator,
        arn: &str,
        body: Value,
    ) -> (u16, Option<String>, Value) {
        emulator
            .send("PUT", &format!("{}/{arn}", self.collection), Some(body))
            .await
    }

    /// The record the store holds for `arn`, read back over the wire through this side's
    /// reader.
    async fn read(&self, emulator: &Emulator, case: &str, arn: &str) -> Value {
        if self.member == ROUTER_INPUT_MEMBER {
            let (status, code, read) = get_router_input(emulator, arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: reading the stored resource expected 200, got {status} \
                 {code:?}: {read}"
            );
            read[ROUTER_INPUT_MEMBER].clone()
        } else {
            let (status, code, read) = batch_get_router_outputs(emulator, &[arn]).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?}: reading the stored resource expected 200, got {status} \
                 {code:?}: {read}"
            );
            read["routerOutputs"][0].clone()
        }
    }

    /// The `(tier, maximumBitrate)` pair a create or read answer reports for this side.
    fn reported_pair(&self, resource: &Value) -> (Option<String>, Option<i64>) {
        (
            resource["tier"].as_str().map(str::to_string),
            resource["maximumBitrate"].as_i64(),
        )
    }
}

/// Both sides of the router, with the resources a create of each one names minted over the
/// wire on `emulator` first: a router input ingests on a resource this account holds, and a
/// STANDARD router output egresses from a router network interface of its own.
async fn both_sides(emulator: &Emulator, prefix: &str) -> [Side; 2] {
    let (_, _, _, input_configuration, _) = minted_configuration_rows(emulator, prefix)
        .await
        .into_iter()
        .next()
        .expect("the configuration table has rows");
    let interface_arn = minted_network_interface(emulator, &format!("rni-{prefix}-out")).await;
    [
        Side {
            prefix: "INPUT",
            noun: "router input",
            ladder: mc::ROUTER_INPUT_TIERS,
            member: ROUTER_INPUT_MEMBER,
            collection: "/v1/routerInput",
            base_create: router_input_create_body("placeholder", input_configuration),
        },
        Side {
            prefix: "OUTPUT",
            noun: "router output",
            ladder: mc::ROUTER_OUTPUT_TIERS,
            member: "routerOutput",
            collection: "/v1/routerOutput",
            base_create: json!({
                "name": "placeholder",
                "configuration": standard(&interface_arn, json!({"Rtp": {
                    "destinationAddress": "198.51.100.80", "destinationPort": 5004}})),
                "maximumBitrate": 20_000_000i64,
                "routingScope": "REGIONAL",
                "tier": "OUTPUT_20",
            }),
        },
    ]
}

/// What the rule says about one `MaximumBitrate` asked for at a tier the side declares.
#[derive(Debug, Clone, Copy)]
enum Verdict {
    /// The pair is acceptable, so the request is applied.
    Accepted,
    /// A bitrate that is not a positive count of bits per second.
    NonPositive,
    /// A bitrate past the ceiling the tier allows.
    PastCeiling,
}

impl Verdict {
    /// The sentence a refusal must carry for `maximum_bitrate` at tier `tier`, whose ceiling
    /// is `ceiling` — or `None` for a pair this rule accepts.
    fn detail(self, tier: &str, maximum_bitrate: i64, ceiling: i64) -> Option<String> {
        match self {
            Verdict::Accepted => None,
            Verdict::NonPositive => Some(format!(
                "MaximumBitrate is a number of bits per second and must be positive; this \
                 request asks for {maximum_bitrate}."
            )),
            Verdict::PastCeiling => Some(format!(
                "MaximumBitrate {maximum_bitrate} bps exceeds the {ceiling} bps the {tier} \
                 tier allows."
            )),
        }
    }
}

/// The bitrates one tier of ceiling `ceiling` is driven over: inside the ceiling, at it, past
/// it by one bit per second and by an order of magnitude, and the two ways a bitrate names no
/// capacity at all.
fn bitrate_rows(ceiling: i64) -> [(&'static str, i64, Verdict); 8] {
    [
        ("one bit per second", 1, Verdict::Accepted),
        ("half the ceiling", ceiling / 2, Verdict::Accepted),
        ("at the published ceiling", ceiling, Verdict::Accepted),
        (
            "one bit per second past the ceiling",
            ceiling + 1,
            Verdict::PastCeiling,
        ),
        ("ten times the ceiling", ceiling * 10, Verdict::PastCeiling),
        ("no capacity at all", 0, Verdict::NonPositive),
        ("one bit per second of debt", -1, Verdict::NonPositive),
        (
            "the lowest bitrate the modelled Long carries",
            i64::MIN,
            Verdict::NonPositive,
        ),
    ]
}

/// Assert the refusal `answered` is this service's `BadRequestException` carrying exactly
/// `detail`, and answer the message so the caller can compare the two sides' wording.
fn assert_refusal(
    case: &str,
    status: u16,
    code: Option<&str>,
    answered: &Value,
    detail: &str,
) -> String {
    assert_eq!(
        (status, code),
        (400, Some("BadRequestException")),
        "case {case:?}: the request is refused with the modelled client error, got {status} \
         {code:?}: {answered}"
    );
    let message = answered["message"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        message,
        mc::validation_failed_message(detail),
        "case {case:?}: the refusal carries this service's validation envelope and the \
         sentence the rule words, got {message:?}"
    );
    message
}

/// A create is judged by the one rule on either side: the tier it names bounds the bitrate it
/// asks for, the bitrate must be positive, and the two sides refuse one pair with one
/// sentence.
#[tokio::test]
async fn a_create_is_bounded_by_its_tier_on_either_side_of_the_router() {
    let emulator = Emulator::new();
    let sides = both_sides(&emulator, "tier-rule-create").await;

    for megabits_per_second in PUBLISHED_MEGABITS_PER_SECOND {
        let ceiling = megabits_per_second * mc::BITS_PER_MEGABIT;
        for (row, maximum_bitrate, verdict) in bitrate_rows(ceiling) {
            // The sentence each side refused this pair with, for the comparison below.
            let mut refusals: Vec<String> = Vec::new();
            for side in &sides {
                let tier = side.tier(megabits_per_second);
                let case = format!("{} {tier} {row}", side.noun);
                assert_eq!(
                    side.ladder.bitrate_ceiling(&tier),
                    Some(ceiling),
                    "case {case:?}: the tier's ceiling is the published {megabits_per_second} \
                     Mbps, got {:?}",
                    side.ladder.bitrate_ceiling(&tier)
                );

                let name = format!("tr-create-{}", tier.to_ascii_lowercase());
                let (status, code, answered) = side
                    .create(&emulator, &name, &tier, maximum_bitrate)
                    .await;
                match verdict.detail(&tier, maximum_bitrate, ceiling) {
                    None => {
                        assert_eq!(
                            (status, code.as_deref()),
                            (201, None),
                            "case {case:?}: a bitrate the tier allows is accepted, got \
                             {status} {code:?}: {answered}"
                        );
                        let created = answered[side.member].clone();
                        assert_eq!(
                            side.reported_pair(&created),
                            (Some(tier.clone()), Some(maximum_bitrate)),
                            "case {case:?}: the answer reports the pair the create asked \
                             for, got {created}"
                        );
                        // The answer is the STORED record: the reader computes the same pair.
                        let arn = created["arn"].as_str().unwrap_or_default().to_string();
                        let stored = side.read(&emulator, &case, &arn).await;
                        assert_eq!(
                            side.reported_pair(&stored),
                            (Some(tier.clone()), Some(maximum_bitrate)),
                            "case {case:?}: the stored resource carries the pair the create \
                             asked for, got {stored}"
                        );
                    }
                    Some(detail) => {
                        refusals.push(assert_refusal(
                            &case,
                            status,
                            code.as_deref(),
                            &answered,
                            &detail,
                        ));
                    }
                }
            }

            // One rule, so one sentence: the only thing that may differ between the two
            // sides' refusals of this pair is the tier name the request carried.
            if let [router_input, router_output] = refusals.as_slice() {
                assert_eq!(
                    router_input.replace("INPUT_", "OUTPUT_"),
                    *router_output,
                    "case {row:?}: both sides refuse the pair with the same sentence, got \
                     {router_input:?} and {router_output:?}"
                );
            }
        }
    }
}

/// An update is judged on the pair it LEAVES BEHIND, through that same one rule: raising the
/// bitrate past the stored tier's ceiling and lowering the tier under the stored bitrate are
/// refused alike, an update that moves neither member is not judged for the pair it did not
/// introduce, and a refused update stores nothing.
#[tokio::test]
async fn an_update_is_bounded_by_the_pair_it_leaves_behind_on_either_side_of_the_router() {
    let emulator = Emulator::new();
    let sides = both_sides(&emulator, "tier-rule-update").await;

    let lowest = PUBLISHED_MEGABITS_PER_SECOND[0];
    let highest = PUBLISHED_MEGABITS_PER_SECOND[PUBLISHED_MEGABITS_PER_SECOND.len() - 1];

    for side in &sides {
        let low_tier = side.tier(lowest);
        let high_tier = side.tier(highest);
        let low_ceiling = lowest * mc::BITS_PER_MEGABIT;
        let high_ceiling = highest * mc::BITS_PER_MEGABIT;

        let (status, code, created) = side
            .create(&emulator, "tr-update", &low_tier, low_ceiling)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (201, None),
            "{}: the resource every update case starts from is created, got {status} {code:?}: \
             {created}",
            side.noun
        );
        let arn = created[side.member]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("the create answers the ARN it minted: {created}"))
            .to_string();

        // (case, request body, the sentence a refusal must carry or `None` when the update is
        // applied). The rows run in order on ONE resource, so each starts from the pair the
        // one before it left behind.
        let rows = [
            (
                "the bitrate alone, past the stored tier's ceiling",
                json!({ "maximumBitrate": low_ceiling + 1 }),
                Verdict::PastCeiling.detail(&low_tier, low_ceiling + 1, low_ceiling),
            ),
            (
                "the bitrate alone, at the stored tier's ceiling",
                json!({ "maximumBitrate": low_ceiling }),
                None,
            ),
            (
                "both members, up to the highest tier",
                json!({ "tier": high_tier, "maximumBitrate": high_ceiling }),
                None,
            ),
            (
                "the tier alone, back under the bitrate the resource now carries",
                json!({ "tier": low_tier }),
                Verdict::PastCeiling.detail(&low_tier, high_ceiling, low_ceiling),
            ),
            (
                "a bitrate that names no capacity",
                json!({ "maximumBitrate": 0 }),
                Verdict::NonPositive.detail(&high_tier, 0, high_ceiling),
            ),
            (
                "neither member, so the stored pair is not judged again",
                json!({ "name": "tr-update-renamed" }),
                None,
            ),
        ];

        // What the store holds as the rows run: the pair the create stored, then whatever an
        // applied update left behind.
        let mut stored_tier = low_tier.clone();
        let mut stored_bitrate = low_ceiling;
        for (row, body, detail) in rows {
            let case = format!("{} {row}", side.noun);
            let (status, code, answered) = side.update(&emulator, &arn, body.clone()).await;
            match detail {
                None => {
                    assert_eq!(
                        (status, code.as_deref()),
                        (202, None),
                        "case {case:?}: the update is accepted, got {status} {code:?}: \
                         {answered}"
                    );
                    if let Some(tier) = body["tier"].as_str() {
                        stored_tier = tier.to_string();
                    }
                    if let Some(maximum_bitrate) = body["maximumBitrate"].as_i64() {
                        stored_bitrate = maximum_bitrate;
                    }
                    assert_eq!(
                        side.reported_pair(&answered[side.member]),
                        (Some(stored_tier.clone()), Some(stored_bitrate)),
                        "case {case:?}: the answer reports the pair the update leaves \
                         behind, got {answered}"
                    );
                }
                Some(detail) => {
                    assert_refusal(&case, status, code.as_deref(), &answered, &detail);
                }
            }
            let stored = side.read(&emulator, &case, &arn).await;
            assert_eq!(
                side.reported_pair(&stored),
                (Some(stored_tier.clone()), Some(stored_bitrate)),
                "case {case:?}: the store holds the pair the applied updates left behind and \
                 nothing a refused one asked for, got {stored}"
            );
        }
    }
}

/// A tier that is not on a side's own ladder has no ceiling, and the sentence refusing it
/// names THAT side's resource — the one place the two sides' wording differs.
///
/// `Tier` is a modelled enum on all four operations, so the framework's constraint walk
/// refuses a value outside it before any handler runs (which the wire case below pins); the
/// rule's own unknown-tier branch is therefore pinned directly, over the other side's tier
/// names and over strings that merely look like a tier.
#[tokio::test]
async fn a_tier_outside_a_sides_ladder_is_refused_in_that_sides_words() {
    let emulator = Emulator::new();
    let sides = both_sides(&emulator, "tier-rule-unknown").await;

    for side in &sides {
        let other = sides
            .iter()
            .find(|candidate| candidate.member != side.member)
            .expect("the router has two sides");

        // Tier names this side does NOT declare: the other side's five, a step that is not on
        // the ladder, a step written with a leading zero, the prefix with no step at all, the
        // step with no prefix, the lowercase spelling and the empty string.
        let mut unknown: Vec<String> = PUBLISHED_MEGABITS_PER_SECOND
            .iter()
            .map(|megabits_per_second| other.tier(*megabits_per_second))
            .collect();
        unknown.extend([
            side.tier(30),
            side.tier(1_000),
            format!("{}_020", side.prefix),
            side.prefix.to_string(),
            "20".to_string(),
            side.tier(20).to_ascii_lowercase(),
            String::new(),
        ]);

        for tier in unknown {
            let case = format!("{} {tier:?}", side.noun);
            assert_eq!(
                side.ladder.bitrate_ceiling(&tier),
                None,
                "case {case:?}: a tier this side does not declare has no ceiling, got {:?}",
                side.ladder.bitrate_ceiling(&tier)
            );
            let noun = side.noun;
            assert_eq!(
                side.ladder.bitrate_rejection(&tier, 1),
                Some(format!(
                    "Tier {tier:?} is not a {noun} tier this service offers."
                )),
                "case {case:?}: the refusal names the tier and this side's resource, got {:?}",
                side.ladder.bitrate_rejection(&tier, 1)
            );
            // Where the other side does not declare that tier either, the two sides word the
            // refusal identically apart from that noun.
            if other.ladder.bitrate_ceiling(&tier).is_none() {
                assert_eq!(
                    side.ladder
                        .bitrate_rejection(&tier, 1)
                        .unwrap_or_default()
                        .replace(side.noun, other.noun),
                    other.ladder.bitrate_rejection(&tier, 1).unwrap_or_default(),
                    "case {case:?}: both sides word an unknown tier the same way but for the \
                     noun"
                );
            }
        }

        // Over the wire, a tier outside the modelled enum never reaches the handler: the
        // framework refuses it in this service's validation envelope.
        let foreign_tier = other.tier(lowest_step());
        let (status, code, answered) = side
            .create(
                &emulator,
                "tr-foreign-tier",
                &foreign_tier,
                20 * mc::BITS_PER_MEGABIT,
            )
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (400, Some("BadRequestException")),
            "{}: a create naming the other side's tier {foreign_tier:?} is refused, got \
             {status} {code:?}: {answered}",
            side.noun
        );
        assert_eq!(
            answered["message"].as_str(),
            Some(mc::validation_failed_message("Invalid Tier").as_str()),
            "{}: the refusal is the framework's constraint envelope for Tier, got {answered}",
            side.noun
        );
    }
}

/// The lowest step of the published ladder, which the cases that need any valid step use.
fn lowest_step() -> i64 {
    PUBLISHED_MEGABITS_PER_SECOND[0]
}
