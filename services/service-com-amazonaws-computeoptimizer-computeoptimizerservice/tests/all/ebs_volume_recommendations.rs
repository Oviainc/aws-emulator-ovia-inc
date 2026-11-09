// SPDX-License-Identifier: CC0-1.0

//! `GetEBSVolumeRecommendations` over the wire: the recommendation this service derives
//! from each observed Amazon EBS volume, and how a request narrows, pages and refuses.
//!
//! Every assertion here is on the RULE, checked over a set of volumes that differ in
//! volume type, size, demand shape and how long they have been observed — the response
//! itself carries the analysed peaks, so an option can be checked against the demand it
//! has to carry without this suite restating the handler's arithmetic.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, emulator, error_code, opt_in, put_preference};

/// Opt the account in and let the analysis its enrollment starts come due — the gate
/// every recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// The seeded volumes, by the last four digits of their volume id.
const GP2_WITH_HEADROOM: &str = "0001";
const GP3_OUTRUNNING_ITS_IOPS: &str = "0002";
const IO1_OVER_PROVISIONED: &str = "0003";
const GP3_ALREADY_RIGHT_SIZED: &str = "0004";

/// gp3's included performance, which no option this service builds goes below (the EBS
/// volume types documentation: 3,000 IOPS and 125 MiB/s included at every size).
const GP3_INCLUDED_IOPS: f64 = 3_000.0;
const GP3_INCLUDED_THROUGHPUT_MIB: f64 = 125.0;

/// Bytes in a MiB: the `VolumeRead/WriteBytesPerSecond` metrics are bytes per second and
/// a volume's throughput is MiB per second.
const BYTES_PER_MIB: f64 = 1_048_576.0;

/// One seeded volume's ARN.
fn volume_arn(suffix: &str) -> String {
    format!(
        "arn:aws:ec2:us-east-1:{}:volume/vol-0a1b2c3d4e5f6{suffix}",
        ::engine::emulator_account_id()
    )
}

/// Read volume recommendations, asserting the call was served.
async fn recommendations(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "GetEBSVolumeRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {request}: expected 200, got {status}: {body}"
    );
    body
}

/// The ARN of every recommendation in a response, in the order it reports them.
fn reported_arns(body: &Value) -> Vec<String> {
    body["volumeRecommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|recommendation| recommendation["volumeArn"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The recommendation a response carries for one seeded volume.
fn recommendation_for(body: &Value, suffix: &str) -> Value {
    let wanted = volume_arn(suffix);
    body["volumeRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {suffix}: volumeRecommendations must be a list: {body}"))
        .iter()
        .find(|recommendation| recommendation["volumeArn"] == json!(wanted))
        .cloned()
        .unwrap_or_else(|| panic!("case {suffix}: no recommendation for {wanted}: {body}"))
}

/// A number the wire carries, or `0` for one the protocol omitted because it equals the
/// member's modelled default.
fn number(value: &Value, member: &str) -> f64 {
    value[member].as_f64().unwrap_or(0.0)
}

/// The value of one `utilizationMetrics` entry.
fn metric(recommendation: &Value, name: &str) -> f64 {
    recommendation["utilizationMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("utilizationMetrics must be a list: {recommendation}"))
        .iter()
        .find(|entry| entry["name"] == json!(name))
        .map(|entry| number(entry, "value"))
        .unwrap_or_else(|| panic!("no {name} metric: {recommendation}"))
}

/// The peak IOPS the analysed period demanded, from the reported metrics.
fn peak_iops(recommendation: &Value) -> f64 {
    metric(recommendation, "VolumeReadOpsPerSecond") + metric(recommendation, "VolumeWriteOpsPerSecond")
}

/// The peak throughput in MiB/s the analysed period demanded, from the reported metrics.
fn peak_throughput_mib(recommendation: &Value) -> f64 {
    (metric(recommendation, "VolumeReadBytesPerSecond")
        + metric(recommendation, "VolumeWriteBytesPerSecond"))
        / BYTES_PER_MIB
}

/// The single recommendation option of a recommendation.
fn only_option(recommendation: &Value) -> Value {
    let options = recommendation["volumeRecommendationOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("volumeRecommendationOptions must be a list: {recommendation}"));
    assert_eq!(
        options.len(),
        1,
        "case {}: expected one recommendation option, got {}: {recommendation}",
        recommendation["volumeArn"],
        options.len(),
    );
    options[0].clone()
}

/// What the request a recommendation answers claimed, plus the invariants every
/// recommendation has to satisfy whatever volume it is for.
fn assert_recommendation_invariants(recommendation: &Value, look_back_period: &str) {
    let arn = recommendation["volumeArn"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        recommendation["accountId"],
        json!(::engine::emulator_account_id()),
        "case {arn}: a recommendation names the account that owns the volume: {recommendation}"
    );
    assert!(
        recommendation["lastRefreshTimestamp"].is_number(),
        "case {arn}: a recommendation carries the instant it was generated: {recommendation}"
    );
    assert!(
        ["Optimized", "NotOptimized"].contains(&recommendation["finding"].as_str().unwrap_or("")),
        "case {arn}: finding must be an EBSFinding, got {}",
        recommendation["finding"]
    );
    assert!(
        ["VeryLow", "Low", "Medium", "High"]
            .contains(&recommendation["currentPerformanceRisk"].as_str().unwrap_or("")),
        "case {arn}: currentPerformanceRisk must be a CurrentPerformanceRisk, got {}",
        recommendation["currentPerformanceRisk"]
    );

    // Every `EBSMetricName` the model declares, each the peak of the analysed samples.
    let metrics = recommendation["utilizationMetrics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {arn}: utilizationMetrics must be a list"));
    for name in [
        "VolumeReadOpsPerSecond",
        "VolumeWriteOpsPerSecond",
        "VolumeReadBytesPerSecond",
        "VolumeWriteBytesPerSecond",
        "VolumeIOPSExceeded",
        "VolumeThroughputExceeded",
    ] {
        let reported = metrics.iter().find(|entry| entry["name"] == json!(name));
        let reported =
            reported.unwrap_or_else(|| panic!("case {arn}: no {name} metric: {recommendation}"));
        assert_eq!(
            reported["statistic"],
            json!("Maximum"),
            "case {arn}/{name}: every reported metric is the analysed peak: {reported}"
        );
        assert!(
            number(reported, "value") >= 0.0,
            "case {arn}/{name}: a metric value is not negative, got {}",
            number(reported, "value")
        );
    }

    let effective = &recommendation["effectiveRecommendationPreferences"];
    assert_eq!(
        effective["lookBackPeriod"],
        json!(look_back_period),
        "case {arn}: the recommendation reports the look-back period in force: {effective}"
    );
    assert_eq!(
        effective["savingsEstimationMode"]["source"],
        json!("PublicPricing"),
        "case {arn}: the savings figures come from the public price table: {effective}"
    );

    // The option: gp3 at the same size, provisioned to carry the analysed peak, inside
    // gp3's documented floor of 3,000 IOPS and 125 MiB/s.
    let option = only_option(recommendation);
    let configuration = &option["configuration"];
    assert_eq!(
        configuration["volumeType"],
        json!("gp3"),
        "case {arn}: the option is a gp3 volume: {option}"
    );
    assert_eq!(
        number(configuration, "volumeSize"),
        number(&recommendation["currentConfiguration"], "volumeSize"),
        "case {arn}: a recommendation changes a volume's type and performance, not its \
         size: {option}"
    );
    let option_iops = number(configuration, "volumeBaselineIOPS");
    assert!(
        option_iops >= peak_iops(recommendation).max(GP3_INCLUDED_IOPS),
        "case {arn}: the option has to carry the analysed peak of {} IOPS and gp3's \
         included {GP3_INCLUDED_IOPS}, got {option_iops}: {option}",
        peak_iops(recommendation)
    );
    let option_throughput = number(configuration, "volumeBaselineThroughput");
    assert!(
        option_throughput >= peak_throughput_mib(recommendation).max(GP3_INCLUDED_THROUGHPUT_MIB),
        "case {arn}: the option has to carry the analysed peak of {} MiB/s and gp3's \
         included {GP3_INCLUDED_THROUGHPUT_MIB}, got {option_throughput}: {option}",
        peak_throughput_mib(recommendation)
    );
    assert_eq!(
        number(&option, "rank"),
        1.0,
        "case {arn}: the only option is the first-ranked one: {option}"
    );
    let risk = number(&option, "performanceRisk");
    assert!(
        (0.0..=4.0).contains(&risk),
        "case {arn}: PerformanceRisk is modelled 0..4, got {risk}: {option}"
    );
    for (member, savings) in [
        ("savingsOpportunity", &option["savingsOpportunity"]),
        (
            "savingsOpportunityAfterDiscounts",
            &option["savingsOpportunityAfterDiscounts"],
        ),
    ] {
        let percentage = number(savings, "savingsOpportunityPercentage");
        assert!(
            (0.0..=100.0).contains(&percentage),
            "case {arn}/{member}: a savings percentage is a percentage, got {percentage}: \
             {savings}"
        );
        assert_eq!(
            savings["estimatedMonthlySavings"]["currency"],
            json!("USD"),
            "case {arn}/{member}: every money figure is in USD: {savings}"
        );
        assert!(
            number(&savings["estimatedMonthlySavings"], "value") >= 0.0,
            "case {arn}/{member}: a saving is not negative: {savings}"
        );
    }
}

#[tokio::test]
async fn volume_recommendations_are_refused_until_the_account_opts_in() {
    let router = emulator();

    let (status, body) = call(&router, "GetEBSVolumeRecommendations", json!({})).await;
    assert_eq!(
        status, 403,
        "an account that never opted in cannot read recommendations: {body}"
    );
    assert_eq!(
        error_code(&body),
        "OptInRequiredException",
        "expected OptInRequiredException, got {body}"
    );

    opt_in(&router).await;
    let fresh = recommendations(&router, json!({})).await;
    assert_eq!(
        reported_arns(&fresh),
        Vec::<String>::new(),
        "just opted in: the analysis is not due yet, so nothing is analysed: {fresh}"
    );

    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let body = recommendations(&router, json!({})).await;
    assert!(
        !reported_arns(&body).is_empty(),
        "an opted-in account whose analysis is due reads the recommendations of its \
         volumes: {body}"
    );
}

#[tokio::test]
async fn every_observed_volume_gets_the_recommendation_its_samples_ask_for() {
    let router = emulator();
    analysed(&router).await;
    let body = recommendations(&router, json!({})).await;

    // One row per class of volume the analysis distinguishes: a previous-generation
    // volume whose demand fits its capacity, a current-generation one whose demand
    // outruns its provisioned IOPS, a heavily over-provisioned Provisioned IOPS volume,
    // and one already provisioned for its workload. `look_back_days` is the period
    // analysed: the preferred 14 days, or the volume's shorter observation window.
    let cases: [(&str, &str, bool, f64); 4] = [
        (GP2_WITH_HEADROOM, "NotOptimized", true, 14.0),
        (GP3_OUTRUNNING_ITS_IOPS, "NotOptimized", false, 14.0),
        (IO1_OVER_PROVISIONED, "NotOptimized", true, 14.0),
        (GP3_ALREADY_RIGHT_SIZED, "Optimized", false, 7.0),
    ];
    assert_eq!(
        reported_arns(&body).len(),
        cases.len(),
        "the account's whole inventory is reported: {body}"
    );

    for (suffix, finding, saves_money, look_back_days) in cases {
        let recommendation = recommendation_for(&body, suffix);
        assert_recommendation_invariants(&recommendation, "DAYS_14");
        assert_eq!(
            recommendation["finding"],
            json!(finding),
            "case {suffix}: expected finding {finding}, got {recommendation}"
        );
        assert_eq!(
            number(&recommendation, "lookBackPeriodInDays"),
            look_back_days,
            "case {suffix}: expected {look_back_days} days analysed, got {}",
            number(&recommendation, "lookBackPeriodInDays")
        );
        let option = only_option(&recommendation);
        let saved = number(
            &option["savingsOpportunity"]["estimatedMonthlySavings"],
            "value",
        );
        assert_eq!(
            saved > 0.0,
            saves_money,
            "case {suffix}: expected a monthly saving of {}, got {saved}: {option}",
            if saves_money { "more than zero" } else { "zero" }
        );
        let percentage = number(&option["savingsOpportunity"], "savingsOpportunityPercentage");
        assert_eq!(
            percentage > 0.0,
            saves_money,
            "case {suffix}: the saving and its percentage have to agree, got {percentage}% \
             of a {saved} saving"
        );
    }

    // An over-provisioned volume is cheaper on gp3 even though it is at no performance
    // risk, and an under-provisioned one is still NotOptimized though its option costs
    // more: the finding follows performance OR cost, not cost alone.
    let under_provisioned = recommendation_for(&body, GP3_OUTRUNNING_ITS_IOPS);
    assert_eq!(
        under_provisioned["currentPerformanceRisk"],
        json!("High"),
        "a volume whose peak demand is 160% of its baseline is at high risk: \
         {under_provisioned}"
    );
    let over_provisioned = recommendation_for(&body, IO1_OVER_PROVISIONED);
    assert_eq!(
        over_provisioned["currentPerformanceRisk"],
        json!("VeryLow"),
        "a volume with six times the IOPS its workload asks for is at very low risk: \
         {over_provisioned}"
    );
}

#[tokio::test]
async fn a_longer_look_back_preference_analyses_more_of_the_observed_period() {
    let router = emulator();
    analysed(&router).await;

    let default_period = recommendations(&router, json!({})).await;
    let before = recommendation_for(&default_period, IO1_OVER_PROVISIONED);

    put_preference(
        &router,
        &json!({"resourceType": "EbsVolume", "lookBackPeriod": "DAYS_93"}),
    )
    .await;

    let longer_period = recommendations(&router, json!({})).await;
    let after = recommendation_for(&longer_period, IO1_OVER_PROVISIONED);
    assert_recommendation_invariants(&after, "DAYS_93");
    assert_eq!(
        number(&after, "lookBackPeriodInDays"),
        93.0,
        "the preferred look-back period is the period analysed: {after}"
    );
    assert!(
        peak_iops(&after) > peak_iops(&before),
        "a longer look-back period analyses samples the shorter one left out: {} IOPS \
         over 93 days vs {} over 14: {after}",
        peak_iops(&after),
        peak_iops(&before)
    );
    assert!(
        number(&only_option(&after)["configuration"], "volumeBaselineIOPS")
            > number(&only_option(&before)["configuration"], "volumeBaselineIOPS"),
        "the option has to carry the higher peak the longer period reveals: {after}"
    );

    // A volume observed for less than the preferred period is analysed over what was
    // observed: the preference cannot conjure samples that were never taken.
    let short_history = recommendation_for(&longer_period, GP3_ALREADY_RIGHT_SIZED);
    assert_eq!(
        number(&short_history, "lookBackPeriodInDays"),
        7.0,
        "a volume observed for a week is analysed over that week: {short_history}"
    );
}

#[tokio::test]
async fn filters_narrow_the_answer_by_finding() {
    let router = emulator();
    analysed(&router).await;

    // `EBSFilterName` admits one name, `Finding`, so each row is a set of `EBSFinding`
    // values with the volumes whose DERIVED finding is one of them.
    let cases: [(Value, &[&str]); 4] = [
        (
            json!([{"name": "Finding", "values": ["Optimized"]}]),
            &[GP3_ALREADY_RIGHT_SIZED],
        ),
        (
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
            &[
                GP2_WITH_HEADROOM,
                GP3_OUTRUNNING_ITS_IOPS,
                IO1_OVER_PROVISIONED,
            ],
        ),
        (
            json!([{"name": "Finding", "values": ["Optimized", "NotOptimized"]}]),
            &[
                GP2_WITH_HEADROOM,
                GP3_OUTRUNNING_ITS_IOPS,
                IO1_OVER_PROVISIONED,
                GP3_ALREADY_RIGHT_SIZED,
            ],
        ),
        // A filter carrying no value selects nothing: no finding is one of none.
        (json!([{"name": "Finding", "values": []}]), &[]),
    ];

    for (filters, expected) in cases {
        let body = recommendations(&router, json!({"filters": filters})).await;
        let mut reported = reported_arns(&body);
        reported.sort();
        let mut wanted: Vec<String> = expected.iter().map(|suffix| volume_arn(suffix)).collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {filters}: expected {wanted:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_filter_the_model_does_not_declare_is_refused() {
    let router = emulator();
    analysed(&router).await;

    // A `Finding` value that is no `EBSFinding` is refused by this operation.
    let (status, body) = call(
        &router,
        "GetEBSVolumeRecommendations",
        json!({"filters": [{"name": "Finding", "values": ["Underprovisioned"]}]}),
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "InvalidParameterValueException",
        "expected InvalidParameterValueException, got {body}"
    );

    // A filter NAME outside `EBSFilterName` is refused at the request boundary against
    // the model's closed enum, before the operation sees it — including the `tag:key` and
    // `tag-key` names `EBSFilter$name`'s prose mentions, which that enum does not admit.
    for name in ["tag:Owner", "tag-key", "VolumeType"] {
        let (status, body) = call(
            &router,
            "GetEBSVolumeRecommendations",
            json!({"filters": [{"name": name, "values": ["whatever"]}]}),
        )
        .await;
        assert_eq!(status, 400, "case {name}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {name}: expected the modelled bad-input refusal, got {body}"
        );
    }
}

#[tokio::test]
async fn volume_arns_select_the_volumes_asked_about_and_nothing_else() {
    let router = emulator();
    analysed(&router).await;

    let known = volume_arn(GP2_WITH_HEADROOM);
    // A well-formed volume ARN of this account that the service holds no recommendation
    // for: absent from the answer, and NOT reported as an error — `errors` carries the
    // volumes an analysis rejected, not the ones it has never seen.
    let unknown = format!(
        "arn:aws:ec2:us-west-2:{}:volume/vol-0000000000000001c",
        ::engine::emulator_account_id()
    );

    let body = recommendations(&router, json!({"volumeArns": [known.clone()]})).await;
    assert_eq!(
        reported_arns(&body),
        vec![known.clone()],
        "a request naming one volume is answered for that volume: {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "a volume the service holds a recommendation for is no error: {body}"
    );

    for request in [
        json!({"volumeArns": [unknown.clone()]}),
        json!({"volumeArns": [unknown.clone(), known.clone()]}),
        json!({"volumeArns": [known.clone(), known.clone()]}),
    ] {
        let body = recommendations(&router, request.clone()).await;
        assert_eq!(
            reported_arns(&body),
            if request["volumeArns"]
                .as_array()
                .is_some_and(|arns| arns.iter().any(|arn| arn == &json!(known)))
            {
                vec![known.clone()]
            } else {
                Vec::new()
            },
            "case {request}: only the volumes with a recommendation are reported, once \
             each: {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {request}: a well-formed volume ARN of this account is never reported \
             as an error: {body}"
        );
    }

    // An ARN that is not an EBS volume ARN is refused, whatever else it names.
    for not_a_volume in [
        format!(
            "arn:aws:ec2:us-east-1:{}:instance/i-0123456789abcdef0",
            ::engine::emulator_account_id()
        ),
        format!(
            "arn:aws:ec2:us-east-1:{}:volume/",
            ::engine::emulator_account_id()
        ),
        "vol-0a1b2c3d4e5f60001".to_string(),
        "not-an-arn".to_string(),
    ] {
        let (status, body) = call(
            &router,
            "GetEBSVolumeRecommendations",
            json!({"volumeArns": [not_a_volume.clone()]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {not_a_volume}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {not_a_volume}: expected InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!("Invalid volume ARN."),
            "case {not_a_volume}: the refusal names the parameter's subject and is the \
             same for every malformed ARN, got {body}"
        );
    }
}

#[tokio::test]
async fn account_ids_select_one_account_and_refuse_anything_else() {
    let router = emulator();
    analysed(&router).await;

    let own = ::engine::emulator_account_id().to_string();
    let body = recommendations(&router, json!({"accountIds": [own.clone()]})).await;
    assert_eq!(
        reported_arns(&body).len(),
        4,
        "the account's own volumes are reported when it names itself: {body}"
    );

    // "Only one account ID can be specified per request", and an account id is 12 digits.
    for account_ids in [json!([own.clone(), "111122223333"]), json!(["12345"])] {
        let (status, body) = call(
            &router,
            "GetEBSVolumeRecommendations",
            json!({"accountIds": account_ids}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {account_ids}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {account_ids}: expected InvalidParameterValueException, got {body}"
        );
    }

    // Another account's volumes are readable only by the management account of its
    // organization, and this emulator records no organization — so an account other than
    // its own is not a member account it can be asked about, named in `accountIds` or in
    // a `volumeArns` entry.
    for request in [
        json!({"accountIds": ["111122223333"]}),
        json!({"volumeArns": ["arn:aws:ec2:us-east-1:111122223333:volume/vol-0123456789"]}),
        json!({
            "accountIds": [own.clone()],
            "volumeArns": ["arn:aws:ec2:us-east-1:111122223333:volume/vol-0123456789"],
        }),
    ] {
        let (status, body) = call(&router, "GetEBSVolumeRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {request}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {request}: expected InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(
                "Invalid permission to access another account. If this is an organization \
                 account, request the management account to enable service access."
            ),
            "case {request}: the refusal states the missing permission, the same way for \
             every member that named the other account, got {body}"
        );
    }
}

#[tokio::test]
async fn a_page_walks_the_inventory_once_and_a_forged_token_is_refused() {
    let router = emulator();
    analysed(&router).await;

    // Each row is a page size, with how many pages the inventory takes at that size.
    for (max_results, expected_pages) in [(1_usize, 4_usize), (2, 2), (3, 2), (4, 1), (1_000, 1)] {
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"maxResults": max_results});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let body = recommendations(&router, request.clone()).await;
            let page = reported_arns(&body);
            assert!(
                page.len() <= max_results,
                "case {max_results}: a page carries at most what was asked for, got {}: {body}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= 4,
                "case {max_results}: paging must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            pages, expected_pages,
            "case {max_results}: expected {expected_pages} page(s), got {pages}"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            4,
            "case {max_results}: paging reports every volume exactly once, got {seen:?}"
        );
    }

    // A request naming no page size gets one bounded page of the whole inventory.
    let body = recommendations(&router, json!({"maxResults": 0})).await;
    assert_eq!(
        reported_arns(&body).len(),
        4,
        "the default page size carries the whole inventory: {body}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "a final page hands out no token: {body}"
    );

    // A token this reader did not issue names no page boundary — neither a string that
    // is not one of its tokens nor one stamped with another page kind.
    for token in [
        "not-a-token".to_string(),
        ::hex::encode(json!(["some-other-reader", volume_arn(GP2_WITH_HEADROOM)]).to_string()),
    ] {
        let (status, body) = call(
            &router,
            "GetEBSVolumeRecommendations",
            json!({"nextToken": token}),
        )
        .await;
        assert_eq!(status, 400, "case {token}: expected 400, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {token}: expected InvalidParameterValueException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_recommendation_names_its_volume_in_the_region_the_request_was_signed_for() {
    let router = emulator();
    analysed(&router).await;

    // Compute Optimizer is a regional service: each row is a Region a caller addresses,
    // and every ARN the answer carries is in that Region, over the same volume ids.
    let mut ids_per_region: Vec<Vec<String>> = Vec::new();
    for region in ["us-east-1", "us-west-2", "eu-central-1"] {
        let (status, body) =
            call_in_region(&router, region, "GetEBSVolumeRecommendations", json!({})).await;
        assert_eq!(
            status, 200,
            "case {region}: expected 200, got {status}: {body}"
        );
        let prefix = format!("arn:aws:ec2:{region}:{}:volume/", ::engine::emulator_account_id());
        let mut ids = Vec::new();
        for arn in reported_arns(&body) {
            let id = arn.strip_prefix(&prefix).map(str::to_string);
            let id = id.unwrap_or_else(|| {
                panic!("case {region}: expected an ARN under {prefix}, got {arn}")
            });
            ids.push(id);
        }
        assert!(
            !ids.is_empty(),
            "case {region}: the Region's inventory is reported: {body}"
        );
        ids_per_region.push(ids);
    }
    for ids in &ids_per_region {
        assert_eq!(
            ids, &ids_per_region[0],
            "the same volumes are reported in every Region, each under its own ARN"
        );
    }

    // A volume ARN of another Region names nothing this endpoint observes, so it selects
    // nothing — and is no error, exactly like an unknown volume of this Region.
    let (status, body) = call_in_region(
        &router,
        "us-west-2",
        "GetEBSVolumeRecommendations",
        json!({"volumeArns": [volume_arn(GP2_WITH_HEADROOM)]}),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported_arns(&body),
        Vec::<String>::new(),
        "a us-east-1 volume ARN selects nothing in us-west-2: {body}"
    );
    assert_eq!(
        body["errors"],
        json!([]),
        "a volume of another Region is no error: {body}"
    );
}

/// ONE observation of one volume, read two ways: the account's EBS volume SUMMARY counts
/// exactly the volumes this operation reports, at the findings it reports for them, and
/// totals the saving of each volume's recommendation — because the summary's rows are
/// projected from the same observation rather than stated a second time.
#[tokio::test]
async fn the_ebs_volume_summary_agrees_with_the_recommendations() {
    let router = emulator();
    analysed(&router).await;
    let body = recommendations(&router, json!({})).await;
    let reported = body["volumeRecommendations"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    // What the recommendations say: how many volumes carry each finding, and what
    // adopting each one's option would save per month.
    let mut expected_counts: std::collections::BTreeMap<String, f64> =
        std::collections::BTreeMap::new();
    let mut expected_savings = 0.0;
    for recommendation in &reported {
        let finding = recommendation["finding"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        *expected_counts.entry(finding).or_default() += 1.0;
        expected_savings += number(
            &only_option(recommendation)["savingsOpportunity"]["estimatedMonthlySavings"],
            "value",
        );
    }

    let (status, summaries) = call(
        &router,
        "GetRecommendationSummaries",
        json!({"accountIds": [::engine::emulator_account_id()]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "summaries: expected 200, got {status}: {summaries}"
    );
    let summary = summaries["recommendationSummaries"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["recommendationResourceType"] == json!("EbsVolume"))
        })
        .cloned()
        .unwrap_or_else(|| panic!("no EBS volume summary was reported: {summaries}"));

    // The summary reports a count at every finding of the resource type, including the
    // ones no volume carries, so a finding the recommendations do not show counts zero.
    for finding in summary["summaries"].as_array().cloned().unwrap_or_default() {
        let name = finding["name"].as_str().unwrap_or_default().to_string();
        let expected = expected_counts.get(&name).copied().unwrap_or(0.0);
        assert!(
            (number(&finding, "value") - expected).abs() < 0.01,
            "case {name:?}: the summary must count the volumes this operation reports \
             ({expected}), got {finding}"
        );
    }
    let counted: f64 = summary["summaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|finding| number(finding, "value"))
        .sum();
    assert!(
        (counted - reported.len() as f64).abs() < 0.01,
        "the summary must count every reported volume exactly once ({}), got {summary}",
        reported.len()
    );
    assert!(
        (number(
            &summary["savingsOpportunity"]["estimatedMonthlySavings"],
            "value"
        ) - expected_savings)
            .abs()
            < 0.01,
        "the summary must total what the volume recommendations save ({expected_savings}), \
         got {summary}"
    );
}

#[tokio::test]
async fn reading_recommendations_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    analysed(&router).await;

    let first = recommendations(&router, json!({})).await;
    let paged = recommendations(&router, json!({"maxResults": 1})).await;
    let second = recommendations(&router, json!({})).await;
    assert_eq!(
        first, second,
        "GetEBSVolumeRecommendations reports; reading it twice, with a page in between \
         ({paged}), answers the same thing"
    );
}
