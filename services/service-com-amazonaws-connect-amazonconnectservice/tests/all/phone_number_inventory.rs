// SPDX-License-Identifier: CC0-1.0

//! The claimable phone-number inventory — search, claim, describe, release — end to
//! end over HTTP through the router the emulator serves with.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * `SearchAvailablePhoneNumbers` REPORTS the inventory: for a target it can
//!   resolve (by `InstanceId` or by `TargetArn`, which name the same target) it
//!   answers HTTP 200 with a non-empty page of numbers, each E.164 and carrying the
//!   E.164 calling code of the requested ISO country, each echoing the requested
//!   country and type, paged by `MaxResults`/`NextToken`;
//! * every number it advertises is CLAIMABLE, and a number it does not advertise is
//!   not: `ClaimPhoneNumber` takes the first and refuses the second;
//! * claiming a number STOPS it being advertised; releasing it advertises it again;
//! * `DescribePhoneNumber` reads a claim back: the number, its country and type, the
//!   description and tags the claim carried, the target it was claimed to, and the
//!   workflow status the clock owes it (`IN_PROGRESS`, then `CLAIMED`);
//! * a target that names nothing is refused with a MODELLED error — never a 501.
//!
//! The country/type catalog cases below are exhaustive rather than sampled: every
//! ISO country `PhoneNumberCountryCode` names, crossed with every `PhoneNumberType`,
//! is checked, so no entry of the catalog is left unexercised.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::{
    PHONE_NUMBER_COUNTRY_CODE_VALUES, PHONE_NUMBER_TYPE_VALUES,
};
use service_com_amazonaws_connect_amazonconnectservice::state::{
    country_calling_code, inventory_phone_number, parse_inventory_number, InventoryNumber,
    COUNTRY_CALLING_CODES, PHONE_NUMBERS_PER_COUNTRY_AND_TYPE, PHONE_NUMBER_CLAIM_DELAY,
    PHONE_NUMBER_STATUS_CLAIMED, PHONE_NUMBER_STATUS_IN_PROGRESS,
    SEARCH_AVAILABLE_PHONE_NUMBERS_DEFAULT_PAGE_SIZE,
};

use super::harness::{connect_managed, looks_like_a_uuid, Answer, Connect};

/// `SearchAvailablePhoneNumbers`: `POST /phone-number/search-available`, every input
/// member in the body.
async fn search(connect: &Connect, body: Value) -> Answer {
    connect
        .send_answer("POST", "/phone-number/search-available", Some(body))
        .await
}

/// `ClaimPhoneNumber`: `POST /phone-number/claim`.
async fn claim(connect: &Connect, body: Value) -> Answer {
    connect
        .send_answer("POST", "/phone-number/claim", Some(body))
        .await
}

/// `DescribePhoneNumber`: `GET /phone-number/{PhoneNumberId}`.
async fn describe(connect: &Connect, phone_number_id: &str) -> Answer {
    connect
        .send_answer("GET", &format!("/phone-number/{phone_number_id}"), None)
        .await
}

/// `ReleasePhoneNumber`: `DELETE /phone-number/{PhoneNumberId}`.
async fn release(connect: &Connect, phone_number_id: &str) -> Answer {
    connect
        .send_answer("DELETE", &format!("/phone-number/{phone_number_id}"), None)
        .await
}

/// The `AvailableNumbersList` of a successful search, as plain numbers.
fn numbers_of(body: &Value) -> Vec<String> {
    body["AvailableNumbersList"]
        .as_array()
        .unwrap_or_else(|| panic!("a search answers with an AvailableNumbersList, got {body}"))
        .iter()
        .map(|entry| {
            entry["PhoneNumber"]
                .as_str()
                .unwrap_or_else(|| panic!("an advertised number carries a PhoneNumber, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Whether `value` is an E.164 number as the modelled `PhoneNumber`
/// (`^\+[1-9]\d{1,14}$`) writes it.
fn is_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    (2..=15).contains(&digits.len())
        && digits.bytes().all(|digit| digit.is_ascii_digit())
        && !digits.starts_with('0')
}

/// Every page of one search, walked to the end through `NextToken`.
async fn walk_pages(
    connect: &Connect,
    instance_id: &str,
    country: &str,
    number_type: &str,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 1..=100 {
        let mut body = json!({
            "InstanceId": instance_id,
            "PhoneNumberCountryCode": country,
            "PhoneNumberType": number_type,
        });
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(cursor) = token.as_deref() {
            body["NextToken"] = json!(cursor);
        }
        let answer = search(connect, body).await;
        assert_eq!(
            answer.status, 200,
            "case ({country}, {number_type}, maxResults {max_results:?}) page {page}: \
             expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        collected.extend(numbers_of(&answer.body));
        token = answer.body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case ({country}, {number_type}): the search never ran out of pages");
}

/// A Connect instance to target, and its ARN.
async fn an_instance(connect: &Connect, alias: &str) -> (String, String) {
    connect.create_instance(connect_managed(alias)).await
}

#[test]
fn the_calling_code_catalog_covers_exactly_the_modelled_countries() {
    // The catalog is keyed by the model's own `PhoneNumberCountryCode` values, so a
    // country the model names always has a calling code and the table names no
    // country the model does not.
    for country in PHONE_NUMBER_COUNTRY_CODE_VALUES {
        let code = country_calling_code(country);
        assert!(
            code.is_some(),
            "case {country:?}: expected an E.164 calling code for every modelled country, got \
             {code:?}"
        );
        let code = code.expect("checked just above");
        assert!(
            (1..=3).contains(&code.len())
                && code.bytes().all(|digit| digit.is_ascii_digit())
                && !code.starts_with('0'),
            "case {country:?}: expected a 1-3 digit E.164 calling code, got {code:?}"
        );
    }
    for (country, code) in COUNTRY_CALLING_CODES {
        assert!(
            PHONE_NUMBER_COUNTRY_CODE_VALUES.contains(country),
            "case {country:?}: the catalog names a country PhoneNumberCountryCode does not, \
             with code {code:?}"
        );
    }
    assert!(
        COUNTRY_CALLING_CODES.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "case \"catalog order\": the table is searched by ISO code, so it must be sorted and \
         free of duplicates"
    );

    // Published values, from the ITU-T E.164 assigned country-code list: rows the
    // issue did not cite, plus two countries that SHARE one calling code.
    for (country, expected) in [
        ("GB", "44"),
        ("JP", "81"),
        ("AD", "376"),
        ("KZ", "7"),
        ("US", "1"),
        ("CA", "1"),
    ] {
        assert_eq!(
            country_calling_code(country),
            Some(expected),
            "case {country:?}: expected the published E.164 calling code {expected:?}, got {:?}",
            country_calling_code(country)
        );
    }
}

#[test]
fn every_inventory_number_is_e164_in_its_country_and_round_trips() {
    // Exhaustive over the model's two catalogs: every country, every type, every
    // serial of the inventory.
    let mut seen: HashSet<String> = HashSet::new();
    for country in PHONE_NUMBER_COUNTRY_CODE_VALUES {
        let calling_code = country_calling_code(country).expect("a modelled country has a code");
        for number_type in PHONE_NUMBER_TYPE_VALUES {
            for serial in 0..PHONE_NUMBERS_PER_COUNTRY_AND_TYPE {
                let case = (country, number_type, serial);
                let number = inventory_phone_number(country, number_type, serial)
                    .unwrap_or_else(|| panic!("case {case:?}: the inventory offers no number"));
                assert!(
                    is_e164(&number),
                    "case {case:?}: expected an E.164 number, got {number:?}"
                );
                assert!(
                    number.starts_with(&format!("+{calling_code}")),
                    "case {case:?}: expected a number in country +{calling_code}, got {number:?}"
                );
                assert_eq!(
                    parse_inventory_number(&number),
                    Some(InventoryNumber {
                        country_code: (*country).to_string(),
                        number_type: (*number_type).to_string(),
                        serial,
                    }),
                    "case {case:?}: expected {number:?} to read back as the entry it was built \
                     from, got {:?}",
                    parse_inventory_number(&number)
                );
                assert!(
                    seen.insert(number.clone()),
                    "case {case:?}: {number:?} is advertised by a second country or type too"
                );
            }
            // One past the inventory's last serial is not an entry at all.
            let past = inventory_phone_number(
                country,
                number_type,
                PHONE_NUMBERS_PER_COUNTRY_AND_TYPE,
            );
            assert!(
                past.is_none(),
                "case ({country}, {number_type}, serial {PHONE_NUMBERS_PER_COUNTRY_AND_TYPE}): \
                 expected nothing past the inventory's bound, got {past:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_search_advertises_the_requested_country_and_type_for_either_target_form() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = an_instance(&connect, "search-targets").await;

    // Rows spanning the two catalogs: countries with 1-, 2- and 3-digit calling
    // codes, two that share a calling code (US and CA), and every `PhoneNumberType`
    // the model declares across them.
    let rows: Vec<(&str, &str)> = vec![
        ("US", "DID"),
        ("CA", "DID"),
        ("GB", "TOLL_FREE"),
        ("JP", "SHARED"),
        ("BR", "UIFN"),
        ("AU", "THIRD_PARTY_DID"),
        ("NZ", "THIRD_PARTY_TF"),
        ("AD", "SHORT_CODE"),
    ];

    let mut by_row: Vec<(String, Vec<String>)> = Vec::new();
    for (country, number_type) in rows {
        let case = format!("{country}/{number_type}");
        let calling_code = country_calling_code(country).expect("a modelled country");

        let by_id = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": country,
                "PhoneNumberType": number_type,
            }),
        )
        .await;
        assert_eq!(
            by_id.status, 200,
            "case {case:?}: expected HTTP 200 from SearchAvailablePhoneNumbers, got {} {}",
            by_id.status, by_id.body
        );
        let numbers = numbers_of(&by_id.body);
        assert_eq!(
            numbers.len(),
            SEARCH_AVAILABLE_PHONE_NUMBERS_DEFAULT_PAGE_SIZE,
            "case {case:?}: expected a full default page of claimable numbers, got {}",
            by_id.body
        );
        for entry in by_id.body["AvailableNumbersList"]
            .as_array()
            .expect("a list")
        {
            assert_eq!(
                entry["PhoneNumberCountryCode"].as_str(),
                Some(country),
                "case {case:?}: an advertised number echoes the requested country, got {entry}"
            );
            assert_eq!(
                entry["PhoneNumberType"].as_str(),
                Some(number_type),
                "case {case:?}: an advertised number echoes the requested type, got {entry}"
            );
            let number = entry["PhoneNumber"].as_str().expect("a number");
            assert!(
                is_e164(number) && number.starts_with(&format!("+{calling_code}")),
                "case {case:?}: expected an E.164 number in +{calling_code}, got {number:?}"
            );
        }

        // The ARN form of the same target names the same target, so it is answered
        // with the same inventory.
        let by_arn = search(
            &connect,
            json!({
                "TargetArn": instance_arn,
                "PhoneNumberCountryCode": country,
                "PhoneNumberType": number_type,
            }),
        )
        .await;
        assert_eq!(
            by_arn.status, 200,
            "case {case:?} (by TargetArn): expected HTTP 200, got {} {}",
            by_arn.status, by_arn.body
        );
        assert_eq!(
            numbers_of(&by_arn.body),
            numbers,
            "case {case:?}: InstanceId and TargetArn name one target, so the page is the same"
        );

        by_row.push((case, numbers));
    }

    // No two rows advertise the same number — not even the two countries that share
    // calling code +1.
    for (left, left_numbers) in &by_row {
        for (right, right_numbers) in &by_row {
            if left == right {
                continue;
            }
            let shared: Vec<&String> = left_numbers
                .iter()
                .filter(|number| right_numbers.contains(number))
                .collect();
            assert!(
                shared.is_empty(),
                "case {left:?} vs {right:?}: expected disjoint inventories, got {shared:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_pages_the_whole_inventory_and_its_bounds_are_enforced() {
    let connect = Connect::new();
    let (instance_id, _arn) = an_instance(&connect, "search-paging").await;
    let expected: Vec<String> = (0..PHONE_NUMBERS_PER_COUNTRY_AND_TYPE)
        .map(|serial| {
            inventory_phone_number("FR", "DID", serial).expect("an inventory entry")
        })
        .collect();

    // `MaxResults` is `MaxResult10`, range 1..10: both published bounds, a value
    // between them, and no value at all.
    for max_results in [Some(1), Some(3), Some(10), None] {
        let walked = walk_pages(&connect, &instance_id, "FR", "DID", max_results).await;
        assert_eq!(
            walked, expected,
            "case maxResults {max_results:?}: expected the whole inventory, in order, exactly \
             once"
        );
    }

    // One step past either published bound is refused by the modelled range before
    // the handler runs.
    for max_results in [0, 11, -1] {
        let answer = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": "FR",
                "PhoneNumberType": "DID",
                "MaxResults": max_results,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {max_results}: expected HTTP 400 outside MaxResult10's range, got \
             {} {}",
            answer.status, answer.body
        );
    }

    // A cursor this service did not issue is a bad parameter, not a restarted scan.
    // Rows spanning the ways a token fails to be one this service issued: not hex at
    // all, an odd number of hex digits, and hex that is not UTF-8.
    for token in ["not-a-token", "zzzz", "abc", "ff"] {
        let answer = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": "FR",
                "PhoneNumberType": "DID",
                "NextToken": token,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: expected HTTP 400 for a token this service never issued, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "InvalidParameterException",
            "case {token:?}: expected the modelled InvalidParameterException, got {:?} {}",
            answer.error_code, answer.body
        );
    }
}

#[tokio::test]
async fn claiming_a_number_takes_it_out_of_the_inventory_and_describe_reads_it_back() {
    // Rows differing in country, type, and whether the claim carries a description
    // and tags at all.
    let rows: Vec<(&str, &str, Option<&str>, Option<Value>)> = vec![
        (
            "US",
            "DID",
            Some("campaign source number"),
            Some(json!({"team": "outbound"})),
        ),
        ("DE", "TOLL_FREE", None, None),
        ("IN", "SHARED", Some(""), Some(json!({"a": "1", "b": "2"}))),
    ];

    for (country, number_type, description, tags) in rows {
        let case = format!("{country}/{number_type}");
        let connect = Connect::new();
        let (instance_id, instance_arn) = an_instance(&connect, "claim-and-describe").await;

        let before = walk_pages(&connect, &instance_id, country, number_type, Some(10)).await;
        assert_eq!(
            before.len(),
            PHONE_NUMBERS_PER_COUNTRY_AND_TYPE as usize,
            "case {case:?}: expected the whole inventory to be available first, got {before:?}"
        );
        let wanted = before[0].clone();

        let mut body = json!({"InstanceId": instance_id, "PhoneNumber": wanted});
        if let Some(description) = description {
            body["PhoneNumberDescription"] = json!(description);
        }
        if let Some(tags) = tags.clone() {
            body["Tags"] = tags;
        }
        let claimed = claim(&connect, body).await;
        assert_eq!(
            claimed.status, 200,
            "case {case:?}: expected HTTP 200 claiming the advertised number {wanted}, got {} {}",
            claimed.status, claimed.body
        );
        let id = claimed.body["PhoneNumberId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a claim answers with a PhoneNumberId, got {}", claimed.body))
            .to_string();
        assert!(
            looks_like_a_uuid(&id),
            "case {case:?}: expected a minted uuid phone number id, got {id:?}"
        );
        let arn = claimed.body["PhoneNumberArn"].as_str().unwrap_or_default();
        assert!(
            arn.ends_with(&format!(":phone-number/{id}")),
            "case {case:?}: expected the ARN to name the claimed number, got {arn:?}"
        );

        // The claim reads back: the number and its country and type, the metadata
        // the request carried, the target it resolved, and the transient status the
        // clock owes.
        let described = describe(&connect, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected HTTP 200 from DescribePhoneNumber, got {} {}",
            described.status, described.body
        );
        let summary = &described.body["ClaimedPhoneNumberSummary"];
        assert_eq!(
            summary["PhoneNumber"].as_str(),
            Some(wanted.as_str()),
            "case {case:?}: expected the claimed number, got {summary}"
        );
        assert_eq!(
            summary["PhoneNumberCountryCode"].as_str(),
            Some(country),
            "case {case:?}: expected the number's own country, got {summary}"
        );
        assert_eq!(
            summary["PhoneNumberType"].as_str(),
            Some(number_type),
            "case {case:?}: expected the number's own type, got {summary}"
        );
        assert_eq!(
            summary["PhoneNumberId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the id the claim minted, got {summary}"
        );
        assert_eq!(
            summary["InstanceId"].as_str(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the instance the number was claimed to, got {summary}"
        );
        assert_eq!(
            summary["TargetArn"].as_str(),
            Some(instance_arn.as_str()),
            "case {case:?}: expected the resolved target's ARN, got {summary}"
        );
        assert_eq!(
            summary["PhoneNumberDescription"].as_str(),
            description,
            "case {case:?}: expected the description the claim carried, got {summary}"
        );
        assert_eq!(
            summary.get("Tags").cloned().unwrap_or(Value::Null),
            tags.clone().unwrap_or(Value::Null),
            "case {case:?}: expected the tags the claim carried, got {summary}"
        );
        assert_eq!(
            summary["PhoneNumberStatus"]["Status"].as_str(),
            Some(PHONE_NUMBER_STATUS_IN_PROGRESS),
            "case {case:?}: a claim the call has only just made has not completed yet, got \
             {summary}"
        );

        // The number is no longer advertised, and nothing else left the inventory.
        let during = walk_pages(&connect, &instance_id, country, number_type, Some(10)).await;
        assert!(
            !during.contains(&wanted),
            "case {case:?}: expected the claimed number {wanted} to stop being advertised, got \
             {during:?}"
        );
        assert_eq!(
            during.len(),
            before.len() - 1,
            "case {case:?}: expected exactly one number to leave the inventory, got {during:?}"
        );

        // Past the due instant the claim has completed, and it stays completed: the
        // status is read off the stored instant, so two reads agree.
        tokio::time::sleep(PHONE_NUMBER_CLAIM_DELAY + std::time::Duration::from_millis(150)).await;
        for read in 1..=2 {
            let described = describe(&connect, &id).await;
            assert_eq!(
                described.body["ClaimedPhoneNumberSummary"]["PhoneNumberStatus"]["Status"].as_str(),
                Some(PHONE_NUMBER_STATUS_CLAIMED),
                "case {case:?} read {read}: expected the claim to have completed, got {}",
                described.body
            );
        }

        // Releasing it hands it back to the inventory, and the record is gone.
        let released = release(&connect, &id).await;
        assert_eq!(
            released.status, 200,
            "case {case:?}: expected HTTP 200 from ReleasePhoneNumber, got {} {}",
            released.status, released.body
        );
        let after = walk_pages(&connect, &instance_id, country, number_type, Some(10)).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the released number to be advertised again"
        );
        let described = describe(&connect, &id).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: expected HTTP 404 describing a released number, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code, "ResourceNotFoundException",
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?}",
            described.error_code
        );
        let released_twice = release(&connect, &id).await;
        assert_eq!(
            released_twice.status, 404,
            "case {case:?}: expected HTTP 404 releasing a number nobody holds, got {} {}",
            released_twice.status, released_twice.body
        );
    }
}

#[tokio::test]
async fn a_target_the_service_cannot_resolve_is_refused_with_a_modelled_error() {
    let connect = Connect::new();
    let (instance_id, instance_arn) = an_instance(&connect, "unresolvable-targets").await;
    let (other_id, _other_arn) = an_instance(&connect, "another-instance").await;

    // Rows spanning the ways a target fails to resolve: absent, an unknown id, an
    // ARN of an instance that does not exist, an ARN of a resource type this service
    // cannot resolve (a traffic distribution group), and two members naming
    // different targets.
    let rows: Vec<(&str, Value)> = vec![
        ("no target at all", json!({})),
        (
            "an unknown instance id",
            json!({"InstanceId": "11111111-2222-3333-4444-555555555555"}),
        ),
        (
            "an instance ARN that names no instance",
            json!({"TargetArn": format!("{instance_arn}-missing")}),
        ),
        (
            "a traffic distribution group ARN",
            json!({"TargetArn": instance_arn.replace("instance/", "traffic-distribution-group/")}),
        ),
        (
            "an InstanceId and a TargetArn naming different targets",
            json!({"InstanceId": other_id, "TargetArn": instance_arn}),
        ),
    ];

    for (case, target) in rows {
        let mut body = json!({"PhoneNumberCountryCode": "US", "PhoneNumberType": "DID"});
        for (member, value) in target.as_object().expect("an object") {
            body[member] = value.clone();
        }
        let answer = search(&connect, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 from SearchAvailablePhoneNumbers, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "InvalidParameterException",
            "case {case:?}: expected the modelled InvalidParameterException, got {:?} {}",
            answer.error_code, answer.body
        );

        // The same target, on the claim: a target that names nothing is the modelled
        // not-found, and a request that names no target at all is a bad parameter.
        let number = inventory_phone_number("US", "DID", 0).expect("an inventory entry");
        let mut body = json!({"PhoneNumber": number});
        for (member, value) in target.as_object().expect("an object") {
            body[member] = value.clone();
        }
        let answer = claim(&connect, body).await;
        let (expected_status, expected_code) = if case == "no target at all"
            || case == "an InstanceId and a TargetArn naming different targets"
        {
            (400, "InvalidParameterException")
        } else {
            (404, "ResourceNotFoundException")
        };
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from ClaimPhoneNumber, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, expected_code,
            "case {case:?}: expected {expected_code}, got {:?} {}",
            answer.error_code, answer.body
        );
    }

    // None of those refusals claimed anything: the inventory is untouched.
    let available = walk_pages(&connect, &instance_id, "US", "DID", Some(10)).await;
    assert_eq!(
        available.len(),
        PHONE_NUMBERS_PER_COUNTRY_AND_TYPE as usize,
        "case \"after the refusals\": expected the whole inventory to still be available, got \
         {available:?}"
    );
}

#[tokio::test]
async fn only_an_advertised_number_can_be_claimed() {
    let connect = Connect::new();
    let (instance_id, _arn) = an_instance(&connect, "claimable-numbers").await;
    let advertised = inventory_phone_number("US", "DID", 0).expect("an inventory entry");

    // Claim the one number that IS advertised, so the row below covering "already
    // claimed" has something to collide with.
    let first = claim(
        &connect,
        json!({"InstanceId": instance_id, "PhoneNumber": advertised}),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case \"an advertised number\": expected HTTP 200, got {} {}",
        first.status, first.body
    );

    // Rows spanning the ways an E.164 number can fail to be an available one: never
    // offered, already claimed, a serial past the inventory's bound, and a number
    // whose country slot and calling code disagree (a +1 number in Great Britain's
    // slot).
    let gb_slot_number = inventory_phone_number("GB", "DID", 0)
        .expect("an inventory entry")
        .replacen("+44", "+1", 1);
    let past_the_bound = format!(
        "{}{:04}",
        advertised
            .strip_suffix("0000")
            .expect("the serial is the last four digits"),
        PHONE_NUMBERS_PER_COUNTRY_AND_TYPE
    );
    let rows: Vec<(&str, String)> = vec![
        ("a number the inventory never offered", "+15551234567".to_string()),
        ("a number already claimed", advertised.clone()),
        ("a serial past the inventory's bound", past_the_bound),
        ("a calling code that is not that country's", gb_slot_number),
    ];

    for (case, number) in rows {
        let answer = claim(
            &connect,
            json!({"InstanceId": instance_id, "PhoneNumber": number}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({number}): expected HTTP 400, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "InvalidParameterException",
            "case {case:?} ({number}): expected the modelled InvalidParameterException, got \
             {:?} {}",
            answer.error_code, answer.body
        );
    }
}

#[tokio::test]
async fn a_repeated_claim_with_the_same_client_token_claims_once() {
    let connect = Connect::new();
    let (instance_id, _arn) = an_instance(&connect, "claim-idempotency").await;
    let token = "11111111-2222-3333-4444-555555555555";
    let first_number = inventory_phone_number("US", "DID", 0).expect("an inventory entry");
    let second_number = inventory_phone_number("US", "DID", 1).expect("an inventory entry");

    let first = claim(
        &connect,
        json!({
            "InstanceId": instance_id,
            "PhoneNumber": first_number,
            "ClientToken": token,
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case \"first claim\": expected HTTP 200, got {} {}",
        first.status, first.body
    );
    let id = first.body["PhoneNumberId"].as_str().expect("an id").to_string();

    // The same token for the same claim reports the claim it already made.
    let repeated = claim(
        &connect,
        json!({
            "InstanceId": instance_id,
            "PhoneNumber": first_number,
            "ClientToken": token,
        }),
    )
    .await;
    assert_eq!(
        repeated.status, 200,
        "case \"repeated claim\": expected HTTP 200, got {} {}",
        repeated.status, repeated.body
    );
    assert_eq!(
        repeated.body["PhoneNumberId"].as_str(),
        Some(id.as_str()),
        "case \"repeated claim\": expected the id the first claim minted, got {}",
        repeated.body
    );

    // The same token for a DIFFERENT claim is the modelled conflict, and claims
    // nothing.
    let conflicting = claim(
        &connect,
        json!({
            "InstanceId": instance_id,
            "PhoneNumber": second_number,
            "ClientToken": token,
        }),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case \"token reused for another number\": expected HTTP 409, got {} {}",
        conflicting.status, conflicting.body
    );
    assert_eq!(
        conflicting.error_code, "IdempotencyException",
        "case \"token reused for another number\": expected the modelled IdempotencyException, \
         got {:?} {}",
        conflicting.error_code, conflicting.body
    );
    let available = walk_pages(&connect, &instance_id, "US", "DID", Some(10)).await;
    assert!(
        available.contains(&second_number),
        "case \"token reused for another number\": the refused claim must leave {second_number} \
         advertised, got {available:?}"
    );
    assert_eq!(
        available.len(),
        PHONE_NUMBERS_PER_COUNTRY_AND_TYPE as usize - 1,
        "case \"token reused for another number\": expected exactly the one claimed number to \
         be gone, got {available:?}"
    );
}

#[tokio::test]
async fn a_prefix_keeps_only_the_numbers_it_starts() {
    let connect = Connect::new();
    let (instance_id, _arn) = an_instance(&connect, "search-prefix").await;
    let first = inventory_phone_number("US", "DID", 0).expect("an inventory entry");

    // `PhoneNumberPrefix` is `^\+?[0-9]{1,11}$`: the same prefix with and without
    // the leading `+` selects the same numbers, a prefix no number starts selects
    // none, and the whole country's calling code selects everything.
    let digits = first.trim_start_matches('+').to_string();
    let rows: Vec<(&str, String, bool)> = vec![
        ("the calling code, with +", "+1".to_string(), true),
        ("the calling code, without +", "1".to_string(), true),
        (
            "the whole number's first digits, with +",
            format!("+{}", &digits[..digits.len() - 1]),
            true,
        ),
        ("a prefix no number starts", "+999".to_string(), false),
    ];

    for (case, prefix, expect_matches) in rows {
        let answer = search(
            &connect,
            json!({
                "InstanceId": instance_id,
                "PhoneNumberCountryCode": "US",
                "PhoneNumberType": "DID",
                "PhoneNumberPrefix": prefix,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let numbers = numbers_of(&answer.body);
        assert_eq!(
            !numbers.is_empty(),
            expect_matches,
            "case {case:?} ({prefix}): expected matches: {expect_matches}, got {numbers:?}"
        );
        for number in &numbers {
            assert!(
                number
                    .trim_start_matches('+')
                    .starts_with(prefix.trim_start_matches('+')),
                "case {case:?}: expected every number to start with {prefix}, got {number:?}"
            );
        }
    }
}
