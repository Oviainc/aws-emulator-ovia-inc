// SPDX-License-Identifier: CC0-1.0

//! `SetupInstanceHttps` and `GetSetupHistory` — securing a stored instance's
//! traffic, driven over the wire through the same front door a client uses.
//!
//! The rules under test:
//!
//! * `SetupInstanceHttps` resolves `instanceName` against the stored instances,
//!   records the request it accepted in that instance's setup history, and
//!   answers one non-terminal `SetupInstanceHttps` operation-log entry located
//!   where the instance is; a name this account holds no instance under is
//!   refused with the service's `DoesNotExist` envelope and records nothing.
//! * A request naming more than one domain must name at least one of at most 63
//!   characters ("If you provide more than one domain name in the request, at
//!   least one name must be less than or equal to 63 characters in length"); one
//!   that names only long ones is refused with the modelled
//!   `InvalidInputException` and records nothing.
//! * `GetSetupHistory` reports the stored records of the named resource, most
//!   recent first, five to a page ("Returns detailed information for five of the
//!   most recent `SetupInstanceHttps` requests that were ran on the target
//!   instance"), with `nextPageToken` present only while records remain.
//! * The contact address a setup carried is never reported back: the modelled
//!   `SetupRequest` "Returns information that was submitted during the
//!   `SetupInstanceHttps` request. Email information is redacted for privacy."
//!
//! Each case is a TABLE whose rows differ along what the rules generalise over:
//! how many domain names the request carries, how long each of them is, and the
//! Region, zone, blueprint and bundle of the instance the certificate is
//! installed on.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The only value of the modelled `CertificateProvider` enum, and the one every
/// accepted request here carries ("The certificate authority that issues the
/// SSL/TLS certificate").
const PROVIDER: &str = "LetsEncrypt";

/// The renewal contact every request here carries, satisfying the modelled
/// `EmailAddress` length (6..254) and pattern. It must never appear in any
/// response.
const EMAIL: &str = "certs@example.com";

/// One setup to run: the instance it is installed on, and the domain names it
/// secures.
#[derive(Debug)]
struct Row {
    case: &'static str,
    availability_zone: &'static str,
    region: &'static str,
    blueprint_id: &'static str,
    bundle_id: &'static str,
    /// Fixed domain names, used when `long_name_lengths` is empty.
    domain_names: &'static [&'static str],
    /// Lengths of generated domain names, for the rows the 63-character rule is
    /// about. Each length is a TOTAL name length, built by
    /// [`domain_name_of_length`].
    long_name_lengths: &'static [usize],
}

impl Row {
    /// The domain names this row's request carries.
    fn domain_names(&self) -> Vec<String> {
        if self.long_name_lengths.is_empty() {
            self.domain_names.iter().map(|name| (*name).to_string()).collect()
        } else {
            self.long_name_lengths
                .iter()
                .map(|length| domain_name_of_length(*length))
                .collect()
        }
    }
}

/// The setups every accepted case here runs. The rows differ in how many domain
/// names the request carries (one, two, and the modelled maximum of ten), in how
/// long those names are (the 63-character bound, one past it, and far past it),
/// and in where the instance lives.
const ACCEPTED: &[Row] = &[
    Row {
        case: "one domain name",
        availability_zone: "us-east-1a",
        region: "us-east-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "micro_3_0",
        domain_names: &["example.com"],
        long_name_lengths: &[],
    },
    Row {
        case: "a domain and its www subdomain",
        availability_zone: "us-west-2b",
        region: "us-west-2",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "nano_3_0",
        domain_names: &["example.com", "www.example.com"],
        long_name_lengths: &[],
    },
    Row {
        case: "the modelled maximum of ten domain names",
        availability_zone: "ap-northeast-1c",
        region: "ap-northeast-1",
        blueprint_id: "wordpress",
        bundle_id: "large_3_0",
        domain_names: &[
            "example.com",
            "www.example.com",
            "blog.example.com",
            "shop.example.com",
            "api.example.com",
            "cdn.example.com",
            "mail.example.com",
            "docs.example.com",
            "status.example.com",
            "a.co",
        ],
        long_name_lengths: &[],
    },
    Row {
        case: "one name far longer than 63 characters: the rule applies only to \
               more than one name",
        availability_zone: "eu-west-3a",
        region: "eu-west-3",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "small_3_0",
        domain_names: &[],
        long_name_lengths: &[131],
    },
    Row {
        case: "two names, the shorter one exactly at the 63-character bound",
        availability_zone: "eu-central-1a",
        region: "eu-central-1",
        blueprint_id: "amazon_linux_2023",
        bundle_id: "medium_3_0",
        domain_names: &[],
        long_name_lengths: &[63, 131],
    },
    Row {
        case: "ten names, one of them at the 63-character bound",
        availability_zone: "sa-east-1a",
        region: "sa-east-1",
        blueprint_id: "ubuntu_22_04",
        bundle_id: "micro_3_0",
        domain_names: &[],
        long_name_lengths: &[64, 65, 66, 67, 68, 69, 70, 131, 200, 63],
    },
];

/// The setups the 63-character rule refuses: more than one name, and every one of
/// them longer than 63 characters — one character past the bound, and far past
/// it.
const REFUSED_FOR_LENGTH: &[&[usize]] = &[
    &[64, 64],
    &[64, 65],
    &[131, 200],
    &[64, 65, 66, 67, 68, 69, 70, 131, 200, 253],
];

/// Names nothing created, differing along the length, the case, the digits and
/// the separators. Every one satisfies the modelled `ResourceName` pattern
/// `^\w[\w\-]*\w$`, so the centralized walk passes it through to the handler.
const MISSING_NAMES: &[&str] = &[
    "ab",
    "never-created",
    "Probe_7",
    "a-name-with-many-hyphen-separated-words-and-digits-0123456789",
];

/// A domain name of exactly `total` characters that satisfies the modelled
/// `SetupDomainName` pattern
/// `^[a-zA-Z0-9\-]{1,63}(\.[a-zA-Z0-9\-]{1,63}){0,8}(\.[a-zA-Z]{2,63})$`: labels
/// of at most 63 characters, finished with `.com`.
fn domain_name_of_length(total: usize) -> String {
    assert!(
        (5..=253).contains(&total),
        "a generated domain name must fit the modelled length, got {total}"
    );
    // What the labels before the `.com` have to add up to, their separating dots
    // included.
    let mut remaining = total - ".com".len();
    let mut labels: Vec<String> = Vec::new();
    while remaining > 0 {
        // Every label after the first costs its own separating dot.
        let dot = usize::from(!labels.is_empty());
        let available = remaining - dot;
        let take = if available > 63 && available - 63 == 1 {
            // Leaving exactly one character behind would need a label of one
            // character AND its dot, which is two — so leave two instead.
            62
        } else {
            available.min(63)
        };
        labels.push("a".repeat(take));
        remaining -= dot + take;
    }
    let name = format!("{}.com", labels.join("."));
    assert_eq!(
        name.chars().count(),
        total,
        "a generated domain name must be exactly as long as asked, got {name}"
    );
    name
}

/// Whether `value` satisfies the modelled `SetupHistoryPageToken` constraints:
/// 24..40 characters of `^[A-Za-z0-9+/=]+$`.
fn is_modelled_page_token(value: &str) -> bool {
    (24..=40).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
}

/// The `CreateInstances` body for `row`, naming one instance.
fn create_body(row: &Row, name: &str) -> Value {
    json!({
        "instanceNames": [name],
        "availabilityZone": row.availability_zone,
        "blueprintId": row.blueprint_id,
        "bundleId": row.bundle_id,
    })
}

/// The `SetupInstanceHttps` body for `instance` and `domain_names`.
fn setup_body(instance: &str, domain_names: &[String]) -> Value {
    json!({
        "instanceName": instance,
        "emailAddress": EMAIL,
        "domainNames": domain_names,
        "certificateProvider": PROVIDER,
    })
}

/// Assert `operations` is exactly one `SetupInstanceHttps` log entry against
/// `name`, answering its id.
fn assert_setup_operation(case: &str, operations: &Value, name: &str, region: &str) -> String {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations list, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: a setup acts on one instance, so it logs one operation, got \
         {operations:?}"
    );
    let operation = &entries[0];
    assert_eq!(
        (
            operation["operationType"].as_str(),
            operation["resourceName"].as_str(),
            operation["resourceType"].as_str(),
            operation["location"]["regionName"].as_str(),
        ),
        (
            Some("SetupInstanceHttps"),
            Some(name),
            Some("Instance"),
            Some(region)
        ),
        "case {case}: expected a SetupInstanceHttps entry against the instance where it \
         lives, got {operation:?}"
    );
    // The certificate is not in place when the call returns, so the entry is
    // answered non-terminal and the clock owes it its `Succeeded` status.
    assert_eq!(
        (
            operation["status"].as_str(),
            operation["isTerminal"].as_bool()
        ),
        (Some("Started"), Some(false)),
        "case {case}: expected a non-terminal Started entry, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_else(|| {
        panic!("case {case}: expected a minted operation id, got {operation:?}")
    });
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id.to_string()
}

/// Poll `GetSetupHistory` until the newest record of `instance` reports
/// `succeeded`, answering that response.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until_succeeded(emulator: &Emulator, instance: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let body = emulator
            .ok("GetSetupHistory", json!({"resourceName": instance}))
            .await;
        if body["setupHistory"][0]["status"].as_str() == Some("succeeded") {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for the setup of {instance}: the clock never settled it, last \
             {body:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The setup itself
// ---------------------------------------------------------------------------

/// RULE: a setup records the request it accepted against the instance it names,
/// and `GetSetupHistory` reports that record back — the request as it was
/// submitted, the instance as the store holds it, and the status the clock has
/// reached — while the call answers one non-terminal `SetupInstanceHttps` log
/// entry.
#[tokio::test]
async fn a_setup_is_recorded_and_reported_back_for_the_instance_it_names() {
    for row in ACCEPTED {
        let emulator = Emulator::new();
        let name = "secured";
        emulator.ok("CreateInstances", create_body(row, name)).await;
        let instance = emulator.ok("GetInstance", json!({"instanceName": name})).await
            ["instance"]
            .clone();

        let domain_names = row.domain_names();
        let answered = emulator
            .ok("SetupInstanceHttps", setup_body(name, &domain_names))
            .await;
        let operation_id = assert_setup_operation(row.case, &answered["operations"], name, row.region);

        let history = emulator
            .ok("GetSetupHistory", json!({"resourceName": name}))
            .await;
        let records = history["setupHistory"]
            .as_array()
            .unwrap_or_else(|| panic!("case {}: expected a setupHistory list, got {history:?}", row.case));
        assert_eq!(
            records.len(),
            1,
            "case {}: one setup makes one record, got {history:?}",
            row.case
        );
        let record = &records[0];
        assert_eq!(
            record["operationId"].as_str(),
            Some(operation_id.as_str()),
            "case {}: the record is identified by the operation the setup logged, got \
             {record:?}",
            row.case
        );
        // The request, as it was submitted.
        assert_eq!(
            record["request"]["instanceName"].as_str(),
            Some(name),
            "case {}: expected the request's instanceName, got {record:?}",
            row.case
        );
        assert_eq!(
            record["request"]["domainNames"],
            json!(domain_names),
            "case {}: expected the request's domainNames in the order submitted, got {:?}",
            row.case,
            record["request"]["domainNames"]
        );
        assert_eq!(
            record["request"]["certificateProvider"].as_str(),
            Some(PROVIDER),
            "case {}: expected the request's certificateProvider, got {record:?}",
            row.case
        );
        // The resource, as the store holds it.
        for (member, expected) in [
            ("name", instance["name"].clone()),
            ("arn", instance["arn"].clone()),
            ("createdAt", instance["createdAt"].clone()),
            ("location", instance["location"].clone()),
            ("resourceType", instance["resourceType"].clone()),
        ] {
            assert_eq!(
                record["resource"][member], expected,
                "case {}: the record's resource.{member} must be the stored instance's, \
                 expected {expected:?}, got {:?}",
                row.case, record["resource"][member]
            );
        }
        // Nothing ran a command on the instance, so there is no command output to
        // report — and the list is reported rather than left out.
        assert_eq!(
            record["executionDetails"],
            json!([]),
            "case {}: expected an empty executionDetails list, got {:?}",
            row.case,
            record["executionDetails"]
        );
        // The certificate is not in place yet: the caller reading straight after
        // the call sees the setup still running.
        assert_eq!(
            record["status"].as_str(),
            Some("inProgress"),
            "case {}: a setup is inProgress until the clock settles it, got {record:?}",
            row.case
        );

        // The clock settles the record into `succeeded`, and the log entry it
        // answered into `Succeeded`, without anyone polling for either.
        let settled = wait_until_succeeded(&emulator, name).await;
        assert_eq!(
            settled["setupHistory"][0]["operationId"].as_str(),
            Some(operation_id.as_str()),
            "case {}: the record's id must not change as it settles, got {settled:?}",
            row.case
        );
        let entry = emulator
            .ok("GetOperation", json!({"operationId": operation_id}))
            .await;
        assert_eq!(
            (
                entry["operation"]["status"].as_str(),
                entry["operation"]["isTerminal"].as_bool()
            ),
            (Some("Succeeded"), Some(true)),
            "case {}: the log entry settles with the setup, got {entry:?}",
            row.case
        );

        // Reading the history twice in a row reports the same thing: a read moves
        // nothing.
        let again = emulator
            .ok("GetSetupHistory", json!({"resourceName": name}))
            .await;
        assert_eq!(
            again, settled,
            "case {}: a second read must report the same history, got {again:?}",
            row.case
        );
    }
}

/// RULE: the contact address a setup carried is never reported back — the
/// modelled `SetupRequest` has no member for it, because "Email information is
/// redacted for privacy".
#[tokio::test]
async fn the_renewal_contact_is_never_reported_back() {
    let emulator = Emulator::new();
    let name = "redacted";
    emulator
        .ok("CreateInstances", create_body(&ACCEPTED[1], name))
        .await;
    let domain_names = ACCEPTED[1].domain_names();
    let answered = emulator
        .ok("SetupInstanceHttps", setup_body(name, &domain_names))
        .await;
    let history = emulator
        .ok("GetSetupHistory", json!({"resourceName": name}))
        .await;

    for (case, body) in [("the setup's own answer", &answered), ("the history", &history)] {
        let rendered = body.to_string();
        assert!(
            !rendered.contains(EMAIL),
            "case {case}: the contact address must be redacted, got {rendered}"
        );
        assert!(
            body["setupHistory"][0]["request"]["emailAddress"].is_null(),
            "case {case}: SetupRequest has no emailAddress member, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The history: order and paging
// ---------------------------------------------------------------------------

/// RULE: each setup makes its own record, and `GetSetupHistory` reports an
/// instance's records most recent first, at most five to a page, with
/// `nextPageToken` present only while records remain — and the token it reports
/// satisfies the modelled `SetupHistoryPageToken` constraints, so it can be fed
/// straight back.
#[tokio::test]
async fn the_history_reports_the_most_recent_setups_first_five_to_a_page() {
    // Enough setups for three pages, with the last page short: 5 + 5 + 2.
    for total in [1_usize, 5, 6, 12] {
        let emulator = Emulator::new();
        let name = "repeatedly-secured";
        emulator
            .ok("CreateInstances", create_body(&ACCEPTED[0], name))
            .await;

        // Each setup names its own domain, so a record can be told from the one
        // before it by more than its id.
        let mut submitted: Vec<(String, String)> = Vec::new();
        for index in 0..total {
            let domain = format!("host{index}.example.com");
            let answered = emulator
                .ok(
                    "SetupInstanceHttps",
                    setup_body(name, std::slice::from_ref(&domain)),
                )
                .await;
            let id = assert_setup_operation(
                &format!("setup {index} of {total}"),
                &answered["operations"],
                name,
                ACCEPTED[0].region,
            );
            submitted.push((id, domain));
        }

        // Newest first is the reverse of the order they were submitted in.
        let expected: Vec<(String, String)> = submitted.iter().rev().cloned().collect();

        let mut walked: Vec<(String, String)> = Vec::new();
        let mut page_sizes: Vec<usize> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"resourceName": name});
            if let Some(token) = &token {
                request["pageToken"] = json!(token);
            }
            let page = emulator.ok("GetSetupHistory", request).await;
            let records = page["setupHistory"].as_array().unwrap_or_else(|| {
                panic!("case {total} setups: expected a setupHistory list, got {page:?}")
            });
            page_sizes.push(records.len());
            for record in records {
                walked.push((
                    record["operationId"].as_str().unwrap_or_default().to_string(),
                    record["request"]["domainNames"][0]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ));
            }
            match page["nextPageToken"].as_str() {
                None => break,
                Some(next) => {
                    assert!(
                        is_modelled_page_token(next),
                        "case {total} setups: a next page token must satisfy the modelled \
                         SetupHistoryPageToken constraints, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
            }
        }

        assert_eq!(
            walked, expected,
            "case {total} setups: expected every record exactly once, most recent first, \
             got {walked:?}"
        );
        let mut want_sizes: Vec<usize> = Vec::new();
        let mut left = total;
        while left > 5 {
            want_sizes.push(5);
            left -= 5;
        }
        want_sizes.push(left);
        assert_eq!(
            page_sizes, want_sizes,
            "case {total} setups: expected pages of five until the last, got {page_sizes:?}"
        );
    }
}

/// RULE: the history is per resource — a setup recorded against one instance is
/// reported for that instance alone, and an instance nothing has been set up on
/// reports an empty history with no next page token.
#[tokio::test]
async fn the_history_is_reported_per_resource() {
    let emulator = Emulator::new();
    for (index, row) in ACCEPTED.iter().take(3).enumerate() {
        let name = format!("host-{index}");
        emulator.ok("CreateInstances", create_body(row, &name)).await;
    }
    // Only the first instance has a setup.
    let secured = "host-0";
    let domain_names = ACCEPTED[0].domain_names();
    emulator
        .ok("SetupInstanceHttps", setup_body(secured, &domain_names))
        .await;

    for name in ["host-1", "host-2"] {
        let history = emulator
            .ok("GetSetupHistory", json!({"resourceName": name}))
            .await;
        assert_eq!(
            history["setupHistory"],
            json!([]),
            "case {name}: an instance nothing was set up on has an empty history, got \
             {history:?}"
        );
        assert!(
            history["nextPageToken"].is_null(),
            "case {name}: an empty history reports no next page token, got {history:?}"
        );
    }

    let history = emulator
        .ok("GetSetupHistory", json!({"resourceName": secured}))
        .await;
    assert_eq!(
        history["setupHistory"].as_array().map(Vec::len),
        Some(1),
        "case {secured}: its own setup is reported, got {history:?}"
    );
}

/// RULE: a page token names a record of the named resource's own history, so a
/// token nothing reported — including one from another instance's history — is
/// refused with the modelled `InvalidInputException` naming the member.
#[tokio::test]
async fn a_page_token_naming_no_record_of_this_resource_is_refused() {
    let emulator = Emulator::new();
    for (index, row) in ACCEPTED.iter().take(2).enumerate() {
        let name = format!("tokened-{index}");
        emulator.ok("CreateInstances", create_body(row, &name)).await;
    }
    // Six setups on the first instance, so its first page reports a token.
    for index in 0..6 {
        emulator
            .ok(
                "SetupInstanceHttps",
                setup_body("tokened-0", &[format!("host{index}.example.com")]),
            )
            .await;
    }
    let first = emulator
        .ok("GetSetupHistory", json!({"resourceName": "tokened-0"}))
        .await;
    let borrowed = first["nextPageToken"]
        .as_str()
        .expect("six setups page, so a token is reported")
        .to_string();

    // Tokens nothing reported for `tokened-1`: 32 hex digits nothing minted, a
    // base64-ish token of another form, and the one the OTHER instance's history
    // reported.
    for (case, token) in [
        ("a token nothing minted", "00000000000000000000000000000000".to_string()),
        ("a token of another form", "AAAAbbbbCCCCddddEEEEffff".to_string()),
        ("a token from another instance's history", borrowed),
    ] {
        let (status, body) = emulator
            .call(
                "GetSetupHistory",
                json!({"resourceName": "tokened-1", "pageToken": token}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException".to_string()),
            "case {case}: expected the modelled InvalidInputException, got {status} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("pageToken"),
            "case {case}: the refusal must name the member that is wrong, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: a request naming more than one domain must name at least one of at most
/// 63 characters; one whose every name is longer is refused with the modelled
/// `InvalidInputException` (HTTP 400) and records nothing.
#[tokio::test]
async fn more_than_one_domain_name_needs_one_of_at_most_sixty_three_characters() {
    let emulator = Emulator::new();
    let name = "long-names";
    emulator
        .ok("CreateInstances", create_body(&ACCEPTED[0], name))
        .await;

    for lengths in REFUSED_FOR_LENGTH {
        let domain_names: Vec<String> = lengths
            .iter()
            .map(|length| domain_name_of_length(*length))
            .collect();
        let (status, body) = emulator
            .call("SetupInstanceHttps", setup_body(name, &domain_names))
            .await;
        assert_eq!(
            status, 400,
            "case {lengths:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {lengths:?}: expected the modelled InvalidInputException, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("domainNames") && message.contains("63"),
            "case {lengths:?}: the refusal must name the member and the constraint, got \
             {body:?}"
        );
    }

    // A refused request records nothing.
    let history = emulator
        .ok("GetSetupHistory", json!({"resourceName": name}))
        .await;
    assert_eq!(
        history["setupHistory"],
        json!([]),
        "case after every refused setup: nothing may be recorded, got {history:?}"
    );

    // One name is accepted however long it is, and more than one is accepted as
    // soon as one of them is at the bound.
    for (case, lengths) in [
        ("a single name one past the bound", vec![64]),
        ("a single name far past the bound", vec![253]),
        ("two names, one exactly at the bound", vec![63, 64]),
        ("two names, one well inside the bound", vec![10, 253]),
    ] {
        let domain_names: Vec<String> = lengths
            .iter()
            .map(|length| domain_name_of_length(*length))
            .collect();
        let answered = emulator
            .ok("SetupInstanceHttps", setup_body(name, &domain_names))
            .await;
        assert_setup_operation(case, &answered["operations"], name, ACCEPTED[0].region);
    }
}

/// RULE: a value outside a modelled constraint is refused by the boundary walk
/// with its own 400 `ValidationException`, before the handler runs — the
/// `CertificateProvider` enum, the `domainNames` list's 1..10 length, each name's
/// own 4..253 length and the contact address's pattern.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_by_the_boundary_walk() {
    let emulator = Emulator::new();
    let name = "boundary";
    emulator
        .ok("CreateInstances", create_body(&ACCEPTED[0], name))
        .await;

    let eleven: Vec<String> = (0..11).map(|i| format!("host{i}.example.com")).collect();
    let rows: &[(&str, Value)] = &[
        (
            "a certificate provider outside the modelled enum",
            json!({
                "instanceName": name,
                "emailAddress": EMAIL,
                "domainNames": ["example.com"],
                "certificateProvider": "DigiCert",
            }),
        ),
        (
            "no domain name at all, one below the list's minimum",
            json!({
                "instanceName": name,
                "emailAddress": EMAIL,
                "domainNames": [],
                "certificateProvider": PROVIDER,
            }),
        ),
        (
            "eleven domain names, one past the list's maximum",
            json!({
                "instanceName": name,
                "emailAddress": EMAIL,
                "domainNames": eleven,
                "certificateProvider": PROVIDER,
            }),
        ),
        (
            "a domain name one character below the modelled minimum",
            json!({
                "instanceName": name,
                "emailAddress": EMAIL,
                "domainNames": ["a.c"],
                "certificateProvider": PROVIDER,
            }),
        ),
        (
            "a contact address that is not an address at all",
            json!({
                "instanceName": name,
                "emailAddress": "not-an-address",
                "domainNames": ["example.com"],
                "certificateProvider": PROVIDER,
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, answered) = emulator.call("SetupInstanceHttps", body.clone()).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "ValidationException".to_string()),
            "case {case}: expected the boundary walk's ValidationException, got {status} \
             {answered:?}"
        );
    }

    // None of them recorded anything.
    let history = emulator
        .ok("GetSetupHistory", json!({"resourceName": name}))
        .await;
    assert_eq!(
        history["setupHistory"],
        json!([]),
        "case after every boundary refusal: nothing may be recorded, got {history:?}"
    );
}

/// RULE: both operations resolve their resource name against the store, so a name
/// that was never created — or has since been deleted — is refused with the
/// service's `DoesNotExist` envelope and nothing is recorded.
#[tokio::test]
async fn an_unknown_resource_answers_the_does_not_exist_envelope() {
    let emulator = Emulator::new();
    let domain_names = ACCEPTED[0].domain_names();

    for name in MISSING_NAMES {
        for (operation, body) in [
            ("SetupInstanceHttps", setup_body(name, &domain_names)),
            ("GetSetupHistory", json!({"resourceName": name})),
        ] {
            let (status, answered) = emulator.call(operation, body).await;
            assert_eq!(
                (
                    status,
                    error_code(&answered),
                    answered["code"].as_str(),
                    answered["message"].as_str(),
                ),
                (
                    400,
                    "NotFoundException".to_string(),
                    Some("DoesNotExist"),
                    Some(format!("The Instance does not exist: {name}").as_str()),
                ),
                "case {operation} on {name:?}: expected the DoesNotExist envelope naming \
                 the resource type and the requested name, got {status} {answered:?}"
            );
        }
    }

    // A deleted instance is refused exactly like one that never existed, even
    // though it had a setup recorded while it existed.
    let name = "short-lived";
    emulator
        .ok("CreateInstances", create_body(&ACCEPTED[0], name))
        .await;
    emulator
        .ok("SetupInstanceHttps", setup_body(name, &domain_names))
        .await;
    emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;
    for operation in ["SetupInstanceHttps", "GetSetupHistory"] {
        let body = if operation == "SetupInstanceHttps" {
            setup_body(name, &domain_names)
        } else {
            json!({"resourceName": name})
        };
        let (status, answered) = emulator.call(operation, body).await;
        assert_eq!(
            (status, answered["code"].as_str()),
            (400, Some("DoesNotExist")),
            "case {operation} after the delete: expected the DoesNotExist envelope, got \
             {status} {answered:?}"
        );
    }
}
