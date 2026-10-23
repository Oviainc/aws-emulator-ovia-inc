// SPDX-License-Identifier: CC0-1.0

//! The contact + contact-channel lifecycle, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1
//! request (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is
//! what a signing AWS client would see — routing, deserialization, constraint validation,
//! the handler and serialization included.
//!
//! The suites are TABLES rather than single calls: each row varies the thing the rule
//! under test generalises over (the contact type and the plan shape it carries, the
//! channel type, which optional members the request names, the page size, the filter), and
//! the case the bootstrap issue cited is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names; every minted ARN is scoped to it.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// One emulator over one shared engine, driven through the same `dispatch` the served
/// socket path uses.
///
/// `pub(crate)` because the sibling suites declared in `tests/all.rs` drive the service the
/// same way: this is the crate's ONE integration-test harness, not one per suite.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the router handed the service, kept so a test can assert a bound on
    /// the store itself rather than only on what the wire reports.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite drives unsigned requests, so signature verification is off; the
        // `Authorization` header below is still what routes the request and names the
        // region, exactly as a signed client's would.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ssmcontacts_ssmcontacts::EmulatedService::new(engine.clone()),
        );
        let emulator = Self { router, engine };
        // Amazon SSM Contacts is fronted by the account's Incident Manager replication
        // set: a contact resource lives in that set's Regions, so a write is refused
        // until the account has an ACTIVE one (`tests/all/replication_set_precondition.rs`
        // is the suite about that rule). This suite is about the contact lifecycle on an
        // ONBOARDED account, so it installs the set through the one shared fixture every
        // suite uses.
        crate::contact_updates::onboard(&emulator);
        emulator
    }

    /// How many entries the idempotency-replay ledger holds right now.
    fn idempotency_token_count(&self) -> usize {
        let state = service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state(
            &*self.engine,
        )
        .expect("the engine holds this service's state");
        let state = state.lock();
        assert_eq!(
            state.idempotency_order.len(),
            state.idempotency_tokens.len(),
            "the insertion-order index must hold exactly one key per ledger entry"
        );
        state.idempotency_tokens.len()
    }

    /// Call `operation` with `body`, returning the HTTP status and the parsed response.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SSMContacts.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}\
                     /ssm-contacts/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a contact, asserting the call succeeded, and answer its `ContactArn`.
    pub(crate) async fn create_contact(&self, request: Value, case: &str) -> String {
        let (status, body) = self.call("CreateContact", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateContact to answer 200, got {status} {body:?}"
        );
        body["ContactArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ContactArn, got {body:?}"))
            .to_string()
    }

    /// Create a contact channel, asserting the call succeeded, and answer its
    /// `ContactChannelArn`.
    async fn create_channel(&self, request: Value, case: &str) -> String {
        let (status, body) = self.call("CreateContactChannel", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
        );
        body["ContactChannelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannelArn, got {body:?}"))
            .to_string()
    }

    /// Every alias `ListContacts` reports for `request`, paging to exhaustion.
    async fn all_contact_aliases(&self, request: Value, case: &str) -> Vec<String> {
        let mut request = request;
        let mut aliases: Vec<String> = Vec::new();
        loop {
            let (status, body) = self.call("ListContacts", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListContacts to answer 200, got {status} {body:?}"
            );
            let page = body["Contacts"].as_array().cloned().unwrap_or_default();
            for contact in &page {
                let alias = contact["Alias"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: expected every contact to carry an Alias, got {contact:?}")
                });
                aliases.push(alias.to_string());
            }
            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        aliases.len() <= 64,
                        "case {case:?}: ListContacts paging did not terminate, \
                         collected {aliases:?}"
                    );
                    request["NextToken"] = json!(token);
                }
                None => return aliases,
            }
        }
    }

    /// Every `ContactChannelArn` `ListContactChannels` reports for `request`, paging to
    /// exhaustion. Also asserts no page is larger than the `MaxResults` the request names.
    async fn all_channel_arns(&self, request: Value, case: &str) -> Vec<String> {
        let mut request = request;
        let limit = request["MaxResults"].as_i64();
        let mut arns: Vec<String> = Vec::new();
        loop {
            let (status, body) = self.call("ListContactChannels", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListContactChannels to answer 200, got {status} {body:?}"
            );
            let page = body["ContactChannels"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if let Some(limit) = limit.filter(|limit| *limit > 0) {
                assert!(
                    page.len() as i64 <= limit,
                    "case {case:?}: a page held {} entries for MaxResults={limit}",
                    page.len()
                );
            }
            for channel in &page {
                let arn = channel["ContactChannelArn"].as_str().unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected every channel to carry a ContactChannelArn, \
                         got {channel:?}"
                    )
                });
                arns.push(arn.to_string());
            }
            match body["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        arns.len() <= 64,
                        "case {case:?}: ListContactChannels paging did not terminate, \
                         collected {arns:?}"
                    );
                    request["NextToken"] = json!(token);
                }
                None => return arns,
            }
        }
    }
}

/// The `__type` discriminator an awsJson error body carries, or `""`.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A table row's JSON fragment, as a `Value`.
pub(crate) fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("a table row's JSON fragment parses")
}

// ---------------------------------------------------------------------------
// The contact catalog
// ---------------------------------------------------------------------------

/// A row of the contact catalog.
///
/// `contact_type` spans the model's whole `ContactType` value set (rendered into
/// `src/ops/types/shared_types.rs` as `CONTACT_TYPE_VALUES`), and `plan` spans the two
/// shapes the modelled `Plan` admits — a `Stages` list and a `RotationIds` list. Every
/// `DurationInMinutes` is inside `StageDurationInMins`' modelled range (0..30), and the
/// second row sits on its published maximum. The first row is the case the bootstrap issue
/// cited; the others are rows it did not, including a contact with no `DisplayName`, one
/// with tags, a multi-stage escalation plan and an on-call schedule whose plan carries
/// rotations instead of stages.
struct ContactRow {
    case: &'static str,
    alias: &'static str,
    display_name: Option<&'static str>,
    contact_type: &'static str,
    plan: &'static str,
    tags: Option<&'static str>,
}

const CONTACT_CATALOG: &[ContactRow] = &[
    ContactRow {
        case: "the issue's own case: a personal contact with one empty stage",
        alias: "oncall-primary",
        display_name: Some("On Call Primary"),
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": []}]}"#,
        tags: None,
    },
    ContactRow {
        case: "a personal contact with no DisplayName, tags, and the longest stage",
        alias: "pager_backup_2",
        display_name: None,
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 30, "Targets": []}]}"#,
        tags: Some(r#"[{"Key": "team", "Value": "sre"}]"#),
    },
    ContactRow {
        case: "an escalation plan with two stages, the second naming a contact target",
        alias: "escalation-tier-1",
        display_name: Some("Escalation Tier 1"),
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [
            {"DurationInMinutes": 10, "Targets": []},
            {"DurationInMinutes": 20, "Targets": [{"ContactTargetInfo": {"IsEssential": true}}]}
        ]}"#,
        tags: None,
    },
    ContactRow {
        case: "an on-call schedule whose plan carries rotation ids instead of stages",
        alias: "weekend-rotation",
        display_name: Some("Weekend Rotation"),
        contact_type: "ONCALL_SCHEDULE",
        // A `RotationIds` entry only has to be an `SsmContactsArn`; the slice stores it
        // verbatim and resolves no rotation.
        plan: r#"{"RotationIds": ["arn:aws:ssm-contacts:us-east-1:000000000000:rotation/weekend"]}"#,
        tags: None,
    },
];

/// The request body a catalog row creates its contact with.
fn create_request(row: &ContactRow) -> Value {
    let mut request = json!({
        "Alias": row.alias,
        "Type": row.contact_type,
        "Plan": parse(row.plan),
    });
    if let Some(display_name) = row.display_name {
        request["DisplayName"] = json!(display_name);
    }
    if let Some(tags) = row.tags {
        request["Tags"] = parse(tags);
    }
    request
}

/// A row of the channel catalog: the model's whole `ChannelType` value set
/// (`CHANNEL_TYPE_VALUES`) with the address format each one documents, and
/// `DeferActivation` absent, true and false.
struct ChannelRow {
    case: &'static str,
    name: &'static str,
    channel_type: &'static str,
    address: &'static str,
    /// `None` leaves `DeferActivation` out of the request entirely.
    defer_activation: Option<bool>,
}

const CHANNEL_CATALOG: &[ChannelRow] = &[
    ChannelRow {
        case: "the issue's own case: an email channel with DeferActivation absent",
        name: "email",
        channel_type: "EMAIL",
        address: "ops@example.com",
        defer_activation: None,
    },
    ChannelRow {
        case: "an SMS channel with activation deferred",
        name: "mobile sms",
        channel_type: "SMS",
        address: "+15550100",
        defer_activation: Some(true),
    },
    ChannelRow {
        case: "a voice channel with activation not deferred",
        name: "desk.phone",
        channel_type: "VOICE",
        address: "+15550101",
        defer_activation: Some(false),
    },
];

/// The request body a channel row creates its channel with, under `contact_arn`.
fn channel_request(contact_arn: &str, row: &ChannelRow) -> Value {
    let mut request = json!({
        "ContactId": contact_arn,
        "Name": row.name,
        "Type": row.channel_type,
        "DeliveryAddress": {"SimpleAddress": row.address},
    });
    if let Some(defer) = row.defer_activation {
        request["DeferActivation"] = json!(defer);
    }
    request
}

// ---------------------------------------------------------------------------
// Create -> read -> delete, over the whole catalog
// ---------------------------------------------------------------------------

/// A create followed by a read returns WHAT WAS CREATED, and a delete is observable by the
/// next read — for every contact type and plan shape the model admits.
#[tokio::test]
async fn a_created_contact_is_read_back_and_a_deleted_one_is_gone() {
    for row in CONTACT_CATALOG {
        let emulator = Emulator::new();
        let case = row.case;
        let request = create_request(row);
        let arn = emulator.create_contact(request.clone(), case).await;

        // The ARN is the one the model's pattern describes, scoped to the caller's
        // credential region and the emulator account, and naming the alias.
        assert_eq!(
            arn,
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/{}",
                row.alias
            ),
            "case {case:?}: unexpected ContactArn, got {arn:?}"
        );

        // GetContact reports the STORED contact, member for member.
        let (status, body) = emulator.call("GetContact", json!({"ContactId": arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetContact to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected GetContact to report the created ARN, got {body:?}"
        );
        assert_eq!(
            body["Alias"].as_str(),
            Some(row.alias),
            "case {case:?}: expected the created Alias, got {body:?}"
        );
        assert_eq!(
            body["Type"].as_str(),
            Some(row.contact_type),
            "case {case:?}: expected the created Type, got {body:?}"
        );
        assert_eq!(
            body["DisplayName"].as_str(),
            row.display_name,
            "case {case:?}: expected the created DisplayName, got {body:?}"
        );
        assert_eq!(
            body["Plan"],
            parse(row.plan),
            "case {case:?}: expected the created Plan, got {:?}",
            body["Plan"]
        );

        // ListContacts reports it too.
        let aliases = emulator.all_contact_aliases(json!({}), case).await;
        assert_eq!(
            aliases,
            vec![row.alias.to_string()],
            "case {case:?}: expected ListContacts to report exactly the created contact"
        );

        // A second create under the same alias is a conflict: the alias "must be unique
        // and identifiable".
        let (status, body) = emulator.call("CreateContact", request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a duplicate alias to answer 409, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ConflictException"),
            "case {case:?}: expected a ConflictException, got {body:?}"
        );

        // The delete is observable: GetContact 404s and ListContacts no longer has it.
        let (status, body) = emulator
            .call("DeleteContact", json!({"ContactId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteContact to answer 200, got {status} {body:?}"
        );
        let (status, body) = emulator.call("GetContact", json!({"ContactId": arn})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetContact on a deleted contact to answer 404, \
             got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
        let aliases = emulator.all_contact_aliases(json!({}), case).await;
        assert!(
            aliases.is_empty(),
            "case {case:?}: expected ListContacts to report nothing after the delete, \
             got {aliases:?}"
        );
    }
}

/// A channel is stored under the contact it names, starts `NOT_ACTIVATED`, and is reported
/// by both `ListContactChannels` and `GetContactChannel` — for every channel type.
#[tokio::test]
async fn created_channels_are_stored_under_their_contact_and_start_unactivated() {
    let emulator = Emulator::new();
    let case = "one contact carrying one channel of every modelled type";
    let contact_arn = emulator
        .create_contact(create_request(&CONTACT_CATALOG[0]), case)
        .await;

    let mut created: Vec<String> = Vec::new();
    for row in CHANNEL_CATALOG {
        let arn = emulator
            .create_channel(channel_request(&contact_arn, row), row.case)
            .await;
        assert!(
            arn.starts_with(&format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/{}/",
                CONTACT_CATALOG[0].alias
            )),
            "case {:?}: unexpected ContactChannelArn, got {arn:?}",
            row.case
        );
        assert!(
            !created.contains(&arn),
            "case {:?}: expected a distinct ARN per channel, got {arn:?} twice",
            row.case
        );

        // GetContactChannel round-trips exactly what the create stored.
        let (status, body) = emulator
            .call("GetContactChannel", json!({"ContactChannelId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetContactChannel to answer 200, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(contact_arn.as_str()),
            "case {:?}: expected the channel to name its contact, got {body:?}",
            row.case
        );
        assert_eq!(
            body["Name"].as_str(),
            Some(row.name),
            "case {:?}: expected the created Name, got {body:?}",
            row.case
        );
        assert_eq!(
            body["Type"].as_str(),
            Some(row.channel_type),
            "case {:?}: expected the created Type, got {body:?}",
            row.case
        );
        assert_eq!(
            body["DeliveryAddress"]["SimpleAddress"].as_str(),
            Some(row.address),
            "case {:?}: expected the created DeliveryAddress, got {body:?}",
            row.case
        );
        assert_eq!(
            body["ActivationStatus"].as_str(),
            Some("NOT_ACTIVATED"),
            "case {:?}: a new channel is NOT_ACTIVATED whatever DeferActivation says, \
             got {body:?}",
            row.case
        );

        created.push(arn);
    }

    // ListContactChannels reports exactly the channels created for that contact, in
    // creation order, each with every member the modelled `ContactChannel` carries.
    let (status, body) = emulator
        .call("ListContactChannels", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListContactChannels to answer 200, got {status} {body:?}"
    );
    let channels = body["ContactChannels"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannels list, got {body:?}"));
    assert_eq!(
        channels.len(),
        CHANNEL_CATALOG.len(),
        "case {case:?}: expected one entry per created channel, got {channels:?}"
    );
    for (index, (channel, row)) in channels.iter().zip(CHANNEL_CATALOG).enumerate() {
        assert_eq!(
            channel["ContactChannelArn"].as_str(),
            Some(created[index].as_str()),
            "case {:?}: expected creation order at index {index}, got {channels:?}",
            row.case
        );
        assert_eq!(
            channel["ContactArn"].as_str(),
            Some(contact_arn.as_str()),
            "case {:?}: expected the channel to name its contact, got {channel:?}",
            row.case
        );
        assert_eq!(
            channel["Name"].as_str(),
            Some(row.name),
            "case {:?}: expected the created Name, got {channel:?}",
            row.case
        );
        assert_eq!(
            channel["Type"].as_str(),
            Some(row.channel_type),
            "case {:?}: expected the created Type, got {channel:?}",
            row.case
        );
        assert_eq!(
            channel["DeliveryAddress"]["SimpleAddress"].as_str(),
            Some(row.address),
            "case {:?}: expected the created DeliveryAddress, got {channel:?}",
            row.case
        );
        assert_eq!(
            channel["ActivationStatus"].as_str(),
            Some("NOT_ACTIVATED"),
            "case {:?}: expected NOT_ACTIVATED, got {channel:?}",
            row.case
        );
    }

    // A second contact sees none of them: a channel belongs to ONE contact.
    let other_arn = emulator
        .create_contact(create_request(&CONTACT_CATALOG[1]), case)
        .await;
    let other_channels = emulator
        .all_channel_arns(json!({"ContactId": other_arn}), case)
        .await;
    assert!(
        other_channels.is_empty(),
        "case {case:?}: expected a contact with no channels to report none, \
         got {other_channels:?}"
    );
}

/// A contact that still has channels cannot be deleted; deleting its channels first lets
/// the delete through. Asserted at every channel count the catalog spans.
#[tokio::test]
async fn a_contact_with_channels_refuses_deletion_until_the_channels_are_gone() {
    for channel_count in 1..=CHANNEL_CATALOG.len() {
        let emulator = Emulator::new();
        let case = format!("a contact carrying {channel_count} channel(s)");
        let contact_arn = emulator
            .create_contact(create_request(&CONTACT_CATALOG[0]), &case)
            .await;

        let mut channel_arns: Vec<String> = Vec::new();
        for row in &CHANNEL_CATALOG[..channel_count] {
            channel_arns.push(
                emulator
                    .create_channel(channel_request(&contact_arn, row), &case)
                    .await,
            );
        }

        // Every channel but the last is removed; the contact is STILL undeletable.
        for (removed, channel_arn) in channel_arns.iter().enumerate() {
            let (status, body) = emulator
                .call("DeleteContact", json!({"ContactId": contact_arn}))
                .await;
            assert_eq!(
                status, 409,
                "case {case:?}: with {} channel(s) left, expected DeleteContact to answer 409, \
                 got {status} {body:?}",
                channel_count - removed
            );
            assert!(
                error_code(&body).contains("ConflictException"),
                "case {case:?}: expected a ConflictException, got {body:?}"
            );

            let (status, body) = emulator
                .call(
                    "DeleteContactChannel",
                    json!({"ContactChannelId": channel_arn}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteContactChannel to answer 200, got {status} {body:?}"
            );
            // The channel delete is observable.
            let (status, body) = emulator
                .call(
                    "GetContactChannel",
                    json!({"ContactChannelId": channel_arn}),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected a deleted channel to answer 404, got {status} {body:?}"
            );
        }

        // With the last channel gone the contact deletes.
        let (status, body) = emulator
            .call("DeleteContact", json!({"ContactId": contact_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteContact to answer 200 once the channels are gone, \
             got {status} {body:?}"
        );
        let (status, body) = emulator
            .call("ListContactChannels", json!({"ContactId": contact_arn}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected ListContactChannels on a deleted contact to answer 404, \
             got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListContacts: filtering and pagination
// ---------------------------------------------------------------------------

/// `ListContacts` reports every persisted contact, narrowed by `AliasPrefix` and `Type`
/// when the request names them.
#[tokio::test]
async fn list_contacts_filters_by_alias_prefix_and_type() {
    let emulator = Emulator::new();
    for row in CONTACT_CATALOG {
        emulator.create_contact(create_request(row), row.case).await;
    }

    // Each row: the filter, and the aliases it must report. The expected order is ARN
    // order, which is alias order because a contact's ARN ends in its alias.
    let filters: &[(&str, Value, &[&str])] = &[
        (
            "no filter at all reports every persisted contact",
            json!({}),
            &[
                "escalation-tier-1",
                "oncall-primary",
                "pager_backup_2",
                "weekend-rotation",
            ],
        ),
        (
            "the issue's own prefix",
            json!({"AliasPrefix": "oncall"}),
            &["oncall-primary"],
        ),
        (
            "a prefix no alias starts with",
            json!({"AliasPrefix": "zzz"}),
            &[],
        ),
        (
            "a prefix that is a substring of an alias but not its start",
            json!({"AliasPrefix": "rotation"}),
            &[],
        ),
        (
            "a single-character prefix",
            json!({"AliasPrefix": "e"}),
            &["escalation-tier-1"],
        ),
        (
            "an alias in full",
            json!({"AliasPrefix": "pager_backup_2"}),
            &["pager_backup_2"],
        ),
        (
            "Type alone: the two personal contacts",
            json!({"Type": "PERSONAL"}),
            &["oncall-primary", "pager_backup_2"],
        ),
        (
            "Type alone: the escalation plan",
            json!({"Type": "ESCALATION"}),
            &["escalation-tier-1"],
        ),
        (
            "Type alone: the on-call schedule",
            json!({"Type": "ONCALL_SCHEDULE"}),
            &["weekend-rotation"],
        ),
        (
            "prefix and Type together, both satisfied",
            json!({"AliasPrefix": "oncall", "Type": "PERSONAL"}),
            &["oncall-primary"],
        ),
        (
            "prefix and Type together, the Type excluding the prefix match",
            json!({"AliasPrefix": "oncall", "Type": "ESCALATION"}),
            &[],
        ),
    ];

    for (case, request, expected) in filters {
        let aliases = emulator.all_contact_aliases(request.clone(), case).await;
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        assert_eq!(
            aliases, expected,
            "case {case:?}: expected {expected:?}, got {aliases:?}"
        );
    }
}

/// Paging a `List*` with any page size yields every item exactly once, in the same order,
/// and the last page carries no `NextToken`.
#[tokio::test]
async fn both_lists_paginate_over_every_page_size() {
    let emulator = Emulator::new();
    let case = "the whole catalog, paged";
    let mut contact_arns: Vec<String> = Vec::new();
    for row in CONTACT_CATALOG {
        contact_arns.push(emulator.create_contact(create_request(row), row.case).await);
    }
    let all_aliases = emulator.all_contact_aliases(json!({}), case).await;
    assert_eq!(
        all_aliases.len(),
        CONTACT_CATALOG.len(),
        "case {case:?}: expected every created contact, got {all_aliases:?}"
    );

    // `MaxResults` is modelled with the range 0..1024 but no default, so: every page size
    // from 1 to one past the item count, the published maximum, and the member absent —
    // all must enumerate the same list. 0 names no page size at all, so it reads as
    // absent.
    for page_size in [1_i64, 2, 3, 4, 5, 0, 1024] {
        let aliases = emulator
            .all_contact_aliases(json!({"MaxResults": page_size}), case)
            .await;
        assert_eq!(
            aliases, all_aliases,
            "case {case:?}: MaxResults={page_size} must enumerate the same list, got {aliases:?}"
        );
    }

    // One past the modelled maximum is refused.
    let (status, body) = emulator
        .call("ListContacts", json!({"MaxResults": 1025}))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: MaxResults=1025 is past the modelled maximum, got {status} {body:?}"
    );

    // The same, for ListContactChannels: one contact carrying the whole channel catalog.
    let contact_arn = contact_arns[0].clone();
    let mut expected_channels: Vec<String> = Vec::new();
    for row in CHANNEL_CATALOG {
        expected_channels.push(
            emulator
                .create_channel(channel_request(&contact_arn, row), row.case)
                .await,
        );
    }

    for page_size in [1_i64, 2, 3, 4, 0, 1024] {
        let seen = emulator
            .all_channel_arns(
                json!({"ContactId": contact_arn, "MaxResults": page_size}),
                case,
            )
            .await;
        assert_eq!(
            seen, expected_channels,
            "case {case:?}: MaxResults={page_size} must enumerate every channel once in \
             creation order, got {seen:?}"
        );
    }

    // A `NextToken` this service never issued is refused rather than silently ignored.
    for (token_case, token) in [
        ("not hex at all", "zzzz"),
        ("hex that decodes to printable text", "6e6f7065"),
        ("hex that decodes to an ARN but carries no list scope", &hex_of(&contact_arn)),
    ] {
        for (operation, request) in [
            ("ListContacts", json!({"NextToken": token})),
            (
                "ListContactChannels",
                json!({"ContactId": contact_arn, "NextToken": token}),
            ),
        ] {
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 400,
                "case {token_case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {token_case:?} on {operation}: expected a ValidationException, got {body:?}"
            );
        }
    }

    // A token one list issued is not a token the OTHER list accepts.
    let (status, body) = emulator
        .call("ListContacts", json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a real ListContacts token\": expected 200, got {status} {body:?}"
    );
    let contacts_token = body["NextToken"]
        .as_str()
        .expect("case \"a real ListContacts token\": expected a NextToken")
        .to_string();
    let (status, body) = emulator
        .call(
            "ListContactChannels",
            json!({"ContactId": contact_arn, "NextToken": contacts_token}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a ListContacts token handed to ListContactChannels\": expected HTTP 400, \
         got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"a ListContacts token handed to ListContactChannels\": expected a \
         ValidationException, got {body:?}"
    );
}

/// `text` as lowercase hex, for a `NextToken` whose payload this service never minted.
fn hex_of(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every ARN-addressed operation answers 404 `ResourceNotFoundException` for an ARN that
/// names no stored resource — including the create that must resolve its `ContactId`.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_answers_404() {
    let emulator = Emulator::new();
    // One live contact, so "absent" cannot be confused with "the store is empty".
    let live = emulator
        .create_contact(create_request(&CONTACT_CATALOG[0]), "a live contact")
        .await;

    let missing_contact = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created");
    let missing_channel = format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/\
         00000000000000000000000000000000"
    );

    let cases: &[(&str, &str, Value)] = &[
        (
            "GetContact on an unknown contact",
            "GetContact",
            json!({"ContactId": missing_contact}),
        ),
        (
            "DeleteContact on an unknown contact",
            "DeleteContact",
            json!({"ContactId": missing_contact}),
        ),
        (
            "ListContactChannels on an unknown contact",
            "ListContactChannels",
            json!({"ContactId": missing_contact}),
        ),
        (
            "CreateContactChannel under an unknown contact",
            "CreateContactChannel",
            json!({
                "ContactId": missing_contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        ),
        (
            "GetContactChannel on an unknown channel",
            "GetContactChannel",
            json!({"ContactChannelId": missing_channel}),
        ),
        (
            "DeleteContactChannel on an unknown channel",
            "DeleteContactChannel",
            json!({"ContactChannelId": missing_channel}),
        ),
        (
            "GetContactChannel on a CONTACT arn, which names no channel",
            "GetContactChannel",
            json!({"ContactChannelId": live}),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 404,
            "case {case:?} on {operation}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?} on {operation}: expected a ResourceNotFoundException, got {body:?}"
        );
    }
}

/// A member whose target is an `enum` shape is refused outside the model's value set, and
/// a member the model constrains by length or pattern is refused outside that constraint.
#[tokio::test]
async fn members_outside_their_modelled_constraint_are_refused() {
    let emulator = Emulator::new();
    let contact_arn = emulator
        .create_contact(create_request(&CONTACT_CATALOG[0]), "a live contact")
        .await;

    // Enum value sets. The permitted values come from the model's own `ContactType` and
    // `ChannelType` enums; each row names a value just outside one.
    let refusals: &[(&str, &str, Value)] = &[
        (
            "CreateContact Type outside ContactType",
            "CreateContact",
            json!({"Alias": "bad-type", "Type": "GROUP", "Plan": {"Stages": []}}),
        ),
        (
            "CreateContact Type in the wrong case",
            "CreateContact",
            json!({"Alias": "bad-type-case", "Type": "personal", "Plan": {"Stages": []}}),
        ),
        (
            "ListContacts Type outside ContactType",
            "ListContacts",
            json!({"Type": "GROUP"}),
        ),
        (
            "CreateContactChannel Type outside ChannelType",
            "CreateContactChannel",
            json!({
                "ContactId": contact_arn,
                "Name": "pager",
                "Type": "PAGER",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        ),
        (
            "CreateContactChannel DeliveryAddress naming no SimpleAddress",
            "CreateContactChannel",
            json!({
                "ContactId": contact_arn,
                "Name": "nowhere",
                "Type": "EMAIL",
                "DeliveryAddress": {},
            }),
        ),
    ];

    for (case, operation, request) in refusals {
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?} on {operation}: expected a ValidationException, got {body:?}"
        );
    }

    // A refused create wrote nothing: only the one live contact remains.
    let aliases = emulator
        .all_contact_aliases(json!({}), "after the refusals")
        .await;
    assert_eq!(
        aliases,
        vec![CONTACT_CATALOG[0].alias.to_string()],
        "case \"after the refusals\": a refused create must persist nothing"
    );

    // `ContactAlias` is `len 1..255, pattern ^[a-z0-9_\-]*$`: both published length bounds
    // are accepted and one past the maximum is refused, as is a value outside the pattern.
    let at_max = "a".repeat(255);
    let past_max = "a".repeat(256);
    let alias_cases: &[(&str, &str, u16)] = &[
        ("the minimum length", "b", 200),
        ("the maximum length", at_max.as_str(), 200),
        ("one character past the maximum", past_max.as_str(), 400),
        ("an uppercase letter, outside the pattern", "Primary", 400),
        ("a space, outside the pattern", "on call", 400),
    ];
    for (case, alias, expected_status) in alias_cases {
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({"Alias": alias, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            )
            .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
        if *expected_status == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }
}

/// A create repeated with the same `IdempotencyToken` is the SAME call: it answers with
/// the ARN the first one minted instead of minting a second resource.
#[tokio::test]
async fn a_repeated_create_with_the_same_idempotency_token_mints_nothing_new() {
    let emulator = Emulator::new();
    let case = "a retried CreateContact and CreateContactChannel";

    let contact_request = json!({
        "Alias": "oncall-primary",
        "Type": "PERSONAL",
        "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
        "IdempotencyToken": "token-contact-1",
    });
    let first = emulator.create_contact(contact_request.clone(), case).await;
    let again = emulator.create_contact(contact_request, case).await;
    assert_eq!(
        first, again,
        "case {case:?}: a retried CreateContact must answer the first ARN"
    );
    let aliases = emulator.all_contact_aliases(json!({}), case).await;
    assert_eq!(
        aliases,
        vec!["oncall-primary".to_string()],
        "case {case:?}: a retried CreateContact must not mint a second contact"
    );

    let channel_request = json!({
        "ContactId": first,
        "Name": "email",
        "Type": "EMAIL",
        "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
        "IdempotencyToken": "token-channel-1",
    });
    let channel = emulator.create_channel(channel_request.clone(), case).await;
    let channel_again = emulator.create_channel(channel_request, case).await;
    assert_eq!(
        channel, channel_again,
        "case {case:?}: a retried CreateContactChannel must answer the first ARN"
    );

    // Without a token the same channel request names a SECOND channel: nothing in the
    // model makes a channel's name unique within its contact.
    let untokened = json!({
        "ContactId": first,
        "Name": "email",
        "Type": "EMAIL",
        "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
    });
    let one = emulator.create_channel(untokened.clone(), case).await;
    let two = emulator.create_channel(untokened, case).await;
    assert_ne!(
        one, two,
        "case {case:?}: two untokened creates are two channels"
    );

    let seen = emulator
        .all_channel_arns(json!({"ContactId": first}), case)
        .await;
    assert_eq!(
        seen,
        vec![channel, one, two],
        "case {case:?}: expected the tokened channel once and the untokened ones twice"
    );

    // A token does not excuse a request that names something else. Reused against an
    // ABSENT contact it is still a 404, and reused for a different (free) alias or a
    // different contact it creates rather than replaying.
    let case = "a token reused for a different request";
    let (status, body) = emulator
        .call(
            "CreateContactChannel",
            json!({
                "ContactId": format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/absent"),
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
                "IdempotencyToken": "token-channel-1",
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case {case:?}: a replay token must not resolve an absent ContactId, \
         got {status} {body:?}"
    );

    let second_contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-secondary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "IdempotencyToken": "token-contact-1",
            }),
            case,
        )
        .await;
    assert_ne!(
        second_contact, first,
        "case {case:?}: a token reused for another alias must create, not replay"
    );

    let other_channel = emulator
        .create_channel(
            json!({
                "ContactId": second_contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
                "IdempotencyToken": "token-channel-1",
            }),
            case,
        )
        .await;
    let other_channels = emulator
        .all_channel_arns(json!({"ContactId": second_contact}), case)
        .await;
    assert_eq!(
        other_channels,
        vec![other_channel],
        "case {case:?}: a token reused under another contact must create there, not replay"
    );
}

/// The idempotency-replay ledger is keyed by caller input, so it is BOUNDED: driving more
/// distinct tokens than the cap leaves it at the cap, and a delete drops the entries that
/// named the resource it removed.
#[tokio::test]
async fn the_idempotency_ledger_stays_within_its_cap() {
    use service_com_amazonaws_ssmcontacts_ssmcontacts::state::MAX_IDEMPOTENCY_TOKENS;

    let emulator = Emulator::new();

    // A delete drops the entries naming what it removed, so the ledger shrinks rather
    // than keeping one dead entry per (create, delete) pair.
    let case = "create then delete, with a token";
    let arn = emulator
        .create_contact(
            json!({
                "Alias": "token-then-deleted",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "IdempotencyToken": "token-to-forget",
            }),
            case,
        )
        .await;
    assert_eq!(
        emulator.idempotency_token_count(),
        1,
        "case {case:?}: expected the create to record one ledger entry"
    );
    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContact to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        emulator.idempotency_token_count(),
        0,
        "case {case:?}: expected the delete to drop the entry naming the deleted contact"
    );

    // More distinct tokens than the cap: the ledger stops at the cap, and every create
    // still succeeds.
    let case = format!("{} distinct tokens, cap {MAX_IDEMPOTENCY_TOKENS}", MAX_IDEMPOTENCY_TOKENS + 50);
    for index in 0..MAX_IDEMPOTENCY_TOKENS + 50 {
        emulator
            .create_contact(
                json!({
                    "Alias": format!("bulk-{index}"),
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                    "IdempotencyToken": format!("bulk-token-{index}"),
                }),
                &case,
            )
            .await;
        let held = emulator.idempotency_token_count();
        assert!(
            held <= MAX_IDEMPOTENCY_TOKENS,
            "case {case:?}: after {} creates the ledger held {held} entries, past the cap \
             of {MAX_IDEMPOTENCY_TOKENS}",
            index + 1
        );
    }
    assert_eq!(
        emulator.idempotency_token_count(),
        MAX_IDEMPOTENCY_TOKENS,
        "case {case:?}: expected the ledger to sit exactly at its cap"
    );

    // The cap evicts the OLDEST entry, so the newest token still replays.
    let newest = format!("bulk-token-{}", MAX_IDEMPOTENCY_TOKENS + 49);
    let (status, body) = emulator
        .call(
            "CreateContact",
            json!({
                "Alias": format!("bulk-{}", MAX_IDEMPOTENCY_TOKENS + 49),
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "IdempotencyToken": newest,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the newest token to still replay, got {status} {body:?}"
    );
}

/// A contact deleted and created again under the same alias is a FRESH contact, not a
/// resurrection: the alias is free, and the new contact carries the new request's members.
#[tokio::test]
async fn an_alias_freed_by_a_delete_can_be_created_again() {
    let emulator = Emulator::new();
    let case = "delete then re-create under the same alias";
    let first = emulator
        .create_contact(create_request(&CONTACT_CATALOG[0]), case)
        .await;
    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": first}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContact to answer 200, got {status} {body:?}"
    );

    // The same alias, a different type and plan, and no DisplayName.
    let second = emulator
        .create_contact(
            json!({
                "Alias": CONTACT_CATALOG[0].alias,
                "Type": "ESCALATION",
                "Plan": {"Stages": [{"DurationInMinutes": 20, "Targets": []}]},
            }),
            case,
        )
        .await;
    let (status, body) = emulator
        .call("GetContact", json!({"ContactId": second}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Type"].as_str(),
        Some("ESCALATION"),
        "case {case:?}: expected the re-created contact's own Type, got {body:?}"
    );
    assert_eq!(
        body["DisplayName"],
        Value::Null,
        "case {case:?}: the re-create named no DisplayName, got {body:?}"
    );
    assert_eq!(
        body["Plan"]["Stages"][0]["DurationInMinutes"].as_i64(),
        Some(20),
        "case {case:?}: expected the re-created contact's own Plan, got {body:?}"
    );
}
