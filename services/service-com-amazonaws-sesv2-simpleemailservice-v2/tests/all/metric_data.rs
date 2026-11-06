// SPDX-License-Identifier: CC0-1.0

//! `BatchGetMetricData`, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization
//! are all exercised.
//!
//! The rules these cases pin:
//!
//! * A batch is answered from the records this service keeps, one result per query in
//!   the order the queries were sent. `SEND` counts the messages of the outbound log
//!   inside the query's window, one data point per UTC day, excluding — as `Metric`'s
//!   own documentation says — mail addressed to more than one recipient and mail sent
//!   to the mailbox simulator. Every other metric counts an event nothing here
//!   observes, so it has no data points.
//! * A dimension filters the recorded activity rather than looking the resource it
//!   names up, so a value the records do not carry keeps no message and the query
//!   reports no data for it.
//! * The `VDM` namespace is served only while the account's VDM configuration is
//!   enabled, and a query window whose end precedes its start is refused.
//!
//! The tables span what the rules generalise over: the recipient count and the
//! header each recipient sits in, the day a message was accepted, where the window
//! falls around it, every metric the model publishes, every dimension name it
//! publishes with a matching and a non-matching value for each, every feature status
//! the account's VDM switch takes, and the three orderings of a window's bounds.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    FEATURE_STATUS_VALUES, METRIC_DIMENSION_NAME_VALUES, METRIC_NAMESPACE_VALUES, METRIC_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    ComAmazonawsSesv2SimpleemailserviceV2State, SentMessage, FEATURE_ENABLED,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `BatchGetMetricData`'s route, from the operation's HTTP binding
/// (`smithy_query(query="operation",operation="BatchGetMetricData")`:
/// `POST /v2/email/metrics/batch`).
const METRICS_PATH: &str = "/v2/email/metrics/batch";

/// The only namespace `MetricNamespace` publishes.
const VDM: &str = "VDM";

/// The metric the records can count, and the one every table that is not about the
/// metric itself uses.
const SEND: &str = "SEND";

/// Seconds in a day, the span one data point covers.
const DAY: i64 = 24 * 60 * 60;

/// A fixed UTC midnight the logged cases place their messages around:
/// `2026-01-01T00:00:00Z`. Expected timestamps are then written out as literal
/// instants rather than recomputed the way the handler computes them.
const DAY_ONE: i64 = 1_767_225_600;

/// The second UTC midnight after [`DAY_ONE`].
const DAY_TWO: i64 = DAY_ONE + DAY;

/// One answered request: the status and the parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or `""` on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried, for an assertion that wants to show it.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Id`s of `Results`, in the order the response lists them.
    fn result_ids(&self) -> Vec<String> {
        self.body
            .get("Results")
            .and_then(Value::as_array)
            .map(|results| {
                results
                    .iter()
                    .map(|result| {
                        result
                            .get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The series of the result at `position`, as (timestamp, value) pairs.
    ///
    /// `Timestamps` and `Values` are two aligned lists, so a reader of either must
    /// see the same number of entries; a pairing is what shows that.
    fn series(&self, position: usize) -> Vec<(i64, i64)> {
        let result = self
            .body
            .get("Results")
            .and_then(Value::as_array)
            .and_then(|results| results.get(position))
            .cloned()
            .unwrap_or(Value::Null);
        let numbers = |member: &str| -> Vec<i64> {
            result
                .get(member)
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|entry| entry.as_f64().unwrap_or_default() as i64)
                        .collect()
                })
                .unwrap_or_default()
        };
        let timestamps = numbers("Timestamps");
        let values = numbers("Values");
        assert_eq!(
            timestamps.len(),
            values.len(),
            "Timestamps and Values must be aligned, got {timestamps:?} and {values:?}"
        );
        timestamps.into_iter().zip(values).collect()
    }

    /// The `Errors` list, which carries one entry per query this service could not
    /// answer at all.
    fn errors(&self) -> Vec<Value> {
        self.body
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can
/// place a message in the outbound log at the instant it is about.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// A service whose account has VDM switched on, which is what the namespace is
    /// served under.
    async fn with_vdm() -> Self {
        let ses = Self::new();
        let put = ses.put_vdm(FEATURE_ENABLED).await;
        assert_eq!(
            put.status, 200,
            "case \"setup\": expected 200 from the VDM put, got {} {:?}",
            put.status, put.body
        );
        ses
    }

    async fn put_vdm(&self, status: &str) -> Reply {
        self.call(
            "PUT",
            "/v2/email/account/vdm",
            Some(json!({"VdmAttributes": {"VdmEnabled": status}})),
        )
        .await
    }

    async fn batch(&self, queries: Vec<Value>) -> Reply {
        self.call("POST", METRICS_PATH, Some(json!({"Queries": queries})))
            .await
    }

    async fn create_identity(&self, identity: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/identities",
            Some(json!({"EmailIdentity": identity})),
        )
        .await
    }

    async fn create_configuration_set(&self, name: &str) -> Reply {
        self.call(
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": name})),
        )
        .await
    }

    async fn create_tenant(&self, name: &str) -> Reply {
        self.call("POST", "/v2/email/tenants", Some(json!({"TenantName": name})))
            .await
    }

    async fn send(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/outbound-emails", Some(body))
            .await
    }

    /// Bring every identity's verification transition into the past, so the next
    /// call that settles the store finds it due and the identity may send.
    fn verify_identities(&self) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let past = SystemTime::now()
            .checked_sub(Duration::from_secs(1))
            .expect("an instant before now");
        for record in store.identities.values_mut() {
            record.verification_due_at = past;
        }
    }

    /// Append a message to the outbound log as if it had been accepted at
    /// `at_epoch_seconds`, so a case can place one in a day of its choosing.
    fn log(&self, message: LoggedMessage) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let sent_at = SystemTime::UNIX_EPOCH + Duration::from_secs(message.at as u64);
        store.sent_messages.push(SentMessage {
            message_id: format!("logged-{}", message.at),
            from: Some(message.from.to_string()),
            to: message.to.iter().map(|to| (*to).to_string()).collect(),
            cc: message.cc.iter().map(|cc| (*cc).to_string()).collect(),
            bcc: message.bcc.iter().map(|bcc| (*bcc).to_string()).collect(),
            subject: None,
            body: None,
            body_html: None,
            configuration_set_name: message
                .configuration_set
                .map(|configuration_set| configuration_set.to_string()),
            reply_to: Vec::new(),
            feedback_forwarding: None,
            email_tags: Vec::new(),
            headers: Vec::new(),
            sent_at,
        });
    }
}

/// One message a case places in the outbound log.
struct LoggedMessage {
    from: &'static str,
    to: Vec<&'static str>,
    cc: Vec<&'static str>,
    bcc: Vec<&'static str>,
    configuration_set: Option<&'static str>,
    /// When it was accepted, in seconds since the epoch.
    at: i64,
}

impl LoggedMessage {
    /// A message from `from` to the single recipient `to`, accepted at `at`.
    fn simple(from: &'static str, to: &'static str, at: i64) -> Self {
        Self {
            from,
            to: vec![to],
            cc: Vec::new(),
            bcc: Vec::new(),
            configuration_set: None,
            at,
        }
    }
}

/// One query of a batch, as the request carries it.
fn query(id: &str, metric: &str, dimensions: Value, start: i64, end: i64) -> Value {
    let mut query = json!({
        "Id": id,
        "Namespace": VDM,
        "Metric": metric,
        "StartDate": start,
        "EndDate": end,
    });
    if let Value::Object(dimensions) = dimensions {
        if !dimensions.is_empty() {
            query["Dimensions"] = Value::Object(dimensions);
        }
    }
    query
}

/// The series of the single result of a batch of one query, asserted to have been
/// answered at all.
async fn one_series(ses: &Ses, case: &str, query: Value) -> Vec<(i64, i64)> {
    let reply = ses.batch(vec![query]).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected 200 from the batch, got {} {:?}",
        reply.status,
        reply.message()
    );
    assert_eq!(
        reply.result_ids().len(),
        1,
        "case {case:?}: expected one result, got {:?}",
        reply.body
    );
    reply.series(0)
}

#[tokio::test]
async fn the_send_metric_counts_the_messages_the_log_holds() {
    /// One row: the messages in the log, the window asked about, and the series the
    /// records yield for it.
    struct Case {
        case: &'static str,
        messages: Vec<LoggedMessage>,
        window: (i64, i64),
        expected: Vec<(i64, i64)>,
    }

    let cases = vec![
        Case {
            case: "an empty log collected nothing",
            messages: Vec::new(),
            window: (DAY_ONE, DAY_ONE + 3 * DAY),
            expected: Vec::new(),
        },
        Case {
            case: "one message, counted in the day it was accepted",
            messages: vec![LoggedMessage::simple(
                "sender@example.com",
                "reader@recipient.example",
                DAY_ONE + 3_600,
            )],
            window: (DAY_ONE, DAY_ONE + 3 * DAY),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            case: "two messages the same day are one data point",
            messages: vec![
                LoggedMessage::simple(
                    "sender@example.com",
                    "reader@recipient.example",
                    DAY_ONE + 3_600,
                ),
                LoggedMessage::simple(
                    "sender@example.com",
                    "other@recipient.example",
                    DAY_ONE + 7_200,
                ),
            ],
            window: (DAY_ONE, DAY_ONE + 3 * DAY),
            expected: vec![(DAY_ONE, 2)],
        },
        Case {
            case: "messages on two days are two data points, in time order",
            messages: vec![
                LoggedMessage::simple(
                    "sender@example.com",
                    "reader@recipient.example",
                    DAY_TWO + 60,
                ),
                LoggedMessage::simple(
                    "sender@example.com",
                    "other@recipient.example",
                    DAY_ONE + 3_600,
                ),
                LoggedMessage::simple(
                    "sender@example.com",
                    "third@recipient.example",
                    DAY_ONE + DAY - 1,
                ),
            ],
            window: (DAY_ONE, DAY_ONE + 3 * DAY),
            expected: vec![(DAY_ONE, 2), (DAY_TWO, 1)],
        },
        Case {
            case: "a window that starts after the message leaves it out",
            messages: vec![LoggedMessage::simple(
                "sender@example.com",
                "reader@recipient.example",
                DAY_ONE + 3_600,
            )],
            window: (DAY_TWO, DAY_TWO + DAY),
            expected: Vec::new(),
        },
        Case {
            case: "a window that ends before the message leaves it out",
            messages: vec![LoggedMessage::simple(
                "sender@example.com",
                "reader@recipient.example",
                DAY_TWO + 3_600,
            )],
            window: (DAY_ONE, DAY_ONE + 600),
            expected: Vec::new(),
        },
        Case {
            case: "a one-second window around the message keeps it",
            messages: vec![LoggedMessage::simple(
                "sender@example.com",
                "reader@recipient.example",
                DAY_ONE + 3_600,
            )],
            window: (DAY_ONE + 3_600, DAY_ONE + 3_600),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            case: "the single recipient may sit in Cc or Bcc",
            messages: vec![
                LoggedMessage {
                    from: "sender@example.com",
                    to: Vec::new(),
                    cc: vec!["copied@recipient.example"],
                    bcc: Vec::new(),
                    configuration_set: None,
                    at: DAY_ONE + 100,
                },
                LoggedMessage {
                    from: "sender@example.com",
                    to: Vec::new(),
                    cc: Vec::new(),
                    bcc: vec!["blind@recipient.example"],
                    configuration_set: None,
                    at: DAY_ONE + 200,
                },
            ],
            window: (DAY_ONE, DAY_ONE + DAY),
            expected: vec![(DAY_ONE, 2)],
        },
        Case {
            // `Metric`: SEND "excludes … emails addressed to more than one
            // recipient", however the recipients were addressed.
            case: "mail addressed to more than one recipient is excluded",
            messages: vec![
                LoggedMessage {
                    from: "sender@example.com",
                    to: vec!["first@recipient.example", "second@recipient.example"],
                    cc: Vec::new(),
                    bcc: Vec::new(),
                    configuration_set: None,
                    at: DAY_ONE + 100,
                },
                LoggedMessage {
                    from: "sender@example.com",
                    to: vec!["first@recipient.example"],
                    cc: vec!["copied@recipient.example"],
                    bcc: Vec::new(),
                    configuration_set: None,
                    at: DAY_ONE + 200,
                },
                LoggedMessage {
                    from: "sender@example.com",
                    to: vec!["first@recipient.example"],
                    cc: Vec::new(),
                    bcc: vec!["blind@recipient.example"],
                    configuration_set: None,
                    at: DAY_ONE + 300,
                },
                LoggedMessage {
                    from: "sender@example.com",
                    to: vec![
                        "first@recipient.example",
                        "second@recipient.example",
                        "third@recipient.example",
                    ],
                    cc: Vec::new(),
                    bcc: Vec::new(),
                    configuration_set: None,
                    at: DAY_ONE + 400,
                },
            ],
            window: (DAY_ONE, DAY_ONE + DAY),
            expected: Vec::new(),
        },
        Case {
            // `Metric`: SEND "excludes emails sent to the mailbox simulator", whose
            // addresses are the ones at `simulator.amazonses.com`.
            case: "mail to the mailbox simulator is excluded",
            messages: vec![
                LoggedMessage::simple(
                    "sender@example.com",
                    "success@simulator.amazonses.com",
                    DAY_ONE + 100,
                ),
                LoggedMessage::simple(
                    "sender@example.com",
                    "bounce@SIMULATOR.AMAZONSES.COM",
                    DAY_ONE + 200,
                ),
                LoggedMessage::simple(
                    "sender@example.com",
                    "reader@recipient.example",
                    DAY_ONE + 300,
                ),
            ],
            window: (DAY_ONE, DAY_ONE + DAY),
            expected: vec![(DAY_ONE, 1)],
        },
    ];

    for case in cases {
        // A fresh service per row, so each row is answered from the messages it
        // logged and nothing else.
        let ses = Ses::with_vdm().await;
        for message in case.messages {
            ses.log(message);
        }

        let series = one_series(
            &ses,
            case.case,
            query("q", SEND, json!({}), case.window.0, case.window.1),
        )
        .await;
        assert_eq!(
            series, case.expected,
            "case {:?}: expected the series {:?}, got {series:?}",
            case.case, case.expected
        );

        // Reading a metric does not change what a later caller reads.
        let again = one_series(
            &ses,
            case.case,
            query("q", SEND, json!({}), case.window.0, case.window.1),
        )
        .await;
        assert_eq!(
            again, case.expected,
            "case {:?}: expected a second read to report {:?}, got {again:?}",
            case.case, case.expected
        );
    }
}

#[tokio::test]
async fn only_the_send_metric_has_data_points() {
    // Every metric the model publishes, so a metric added to the value set is
    // covered rather than only the ones a caller happened to ask for
    // (`smithy_query(query="shape",shape="Metric")`).
    for metric in METRIC_VALUES {
        let ses = Ses::with_vdm().await;
        ses.log(LoggedMessage::simple(
            "sender@example.com",
            "reader@recipient.example",
            DAY_ONE + 3_600,
        ));

        let series = one_series(
            &ses,
            metric,
            query("q", metric, json!({}), DAY_ONE, DAY_ONE + DAY),
        )
        .await;
        // The log records that SES accepted a message; a delivery, a bounce, a
        // complaint, an open or a click is an event after that, and nothing here
        // observes one.
        let expected: Vec<(i64, i64)> = if *metric == SEND {
            vec![(DAY_ONE, 1)]
        } else {
            Vec::new()
        };
        assert_eq!(
            series, expected,
            "case {metric:?}: expected the series {expected:?}, got {series:?}"
        );
    }
}

#[tokio::test]
async fn a_dimension_keeps_the_activity_it_names() {
    /// One row: the dimensions a query carries and the series the logged messages
    /// yield under them.
    struct Case {
        case: &'static str,
        dimensions: Value,
        expected: Vec<(i64, i64)>,
    }

    // Three messages: one from an address identity, one from an address covered only
    // by a domain identity, and one sent with a configuration set.
    let messages = || {
        vec![
            LoggedMessage::simple(
                "sender@example.com",
                "reader@gmail.com",
                DAY_ONE + 3_600,
            ),
            LoggedMessage::simple("other@example.net", "reader@yahoo.com", DAY_ONE + 7_200),
            LoggedMessage {
                from: "sender@example.com",
                to: vec!["reader@gmail.com"],
                cc: Vec::new(),
                bcc: Vec::new(),
                configuration_set: Some("tracked"),
                at: DAY_ONE + 10_800,
            },
        ]
    };

    let cases = vec![
        Case {
            case: "no dimension keeps every message",
            dimensions: json!({}),
            expected: vec![(DAY_ONE, 3)],
        },
        Case {
            case: "the address identity that authorised the send",
            dimensions: json!({"EMAIL_IDENTITY": "sender@example.com"}),
            expected: vec![(DAY_ONE, 2)],
        },
        Case {
            case: "the domain identity that authorised the send",
            dimensions: json!({"EMAIL_IDENTITY": "example.net"}),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            // A domain is compared the way DNS compares one.
            case: "the domain identity named in another case",
            dimensions: json!({"EMAIL_IDENTITY": "EXAMPLE.NET"}),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            case: "an identity the account does not hold keeps nothing",
            dimensions: json!({"EMAIL_IDENTITY": "nobody@example.org"}),
            expected: Vec::new(),
        },
        Case {
            case: "the configuration set a message was sent with",
            dimensions: json!({"CONFIGURATION_SET": "tracked"}),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            case: "a configuration set the account does not hold keeps nothing",
            dimensions: json!({"CONFIGURATION_SET": "sim2real-absent-set"}),
            expected: Vec::new(),
        },
        Case {
            case: "the recipient's provider, by its mail domain",
            dimensions: json!({"ISP": "gmail.com"}),
            expected: vec![(DAY_ONE, 2)],
        },
        Case {
            // `MetricDimensionName` documents the ISP examples as `Gmail` and
            // `Yahoo`, the provider without its public suffix.
            case: "the recipient's provider, by its name",
            dimensions: json!({"ISP": "Yahoo"}),
            expected: vec![(DAY_ONE, 1)],
        },
        Case {
            case: "a provider nothing was sent to keeps nothing",
            dimensions: json!({"ISP": "outlook.com"}),
            expected: Vec::new(),
        },
        Case {
            // Nothing in this slice sends through a tenant, so no message is
            // attributed to one — including a tenant the account does hold.
            case: "a tenant of the account accounts for no message",
            dimensions: json!({"TENANT_NAME": "a-tenant"}),
            expected: Vec::new(),
        },
        Case {
            case: "a tenant the account does not hold keeps nothing either",
            dimensions: json!({"TENANT_NAME": "sim2real-absent-tenant"}),
            expected: Vec::new(),
        },
        Case {
            case: "two dimensions keep only what satisfies both",
            dimensions: json!({"EMAIL_IDENTITY": "sender@example.com", "ISP": "gmail.com"}),
            expected: vec![(DAY_ONE, 2)],
        },
        Case {
            case: "two dimensions that no one message satisfies keep nothing",
            dimensions: json!({"EMAIL_IDENTITY": "example.net", "ISP": "gmail.com"}),
            expected: Vec::new(),
        },
        Case {
            case: "three dimensions, the most a query may carry",
            dimensions: json!({
                "EMAIL_IDENTITY": "sender@example.com",
                "CONFIGURATION_SET": "tracked",
                "ISP": "gmail.com",
            }),
            expected: vec![(DAY_ONE, 1)],
        },
    ];

    // Every dimension name the model publishes is exercised by the rows above
    // (`smithy_query(query="shape",shape="MetricDimensionName")`).
    for name in METRIC_DIMENSION_NAME_VALUES {
        assert!(
            cases
                .iter()
                .any(|case| case.dimensions.get(name).is_some()),
            "case {name:?}: the table must cover every published dimension name"
        );
    }

    for case in cases {
        let ses = Ses::with_vdm().await;
        // The identities, configuration set and tenant the dimensions name, created
        // through their own operations so the records are the service's own.
        for identity in ["sender@example.com", "example.net"] {
            let created = ses.create_identity(identity).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected 200 creating {identity}, got {} {:?}",
                case.case,
                created.status,
                created.message()
            );
        }
        let set = ses.create_configuration_set("tracked").await;
        assert_eq!(
            set.status, 200,
            "case {:?}: expected 200 creating the configuration set, got {} {:?}",
            case.case,
            set.status,
            set.message()
        );
        let tenant = ses.create_tenant("a-tenant").await;
        assert_eq!(
            tenant.status, 200,
            "case {:?}: expected 200 creating the tenant, got {} {:?}",
            case.case,
            tenant.status,
            tenant.message()
        );
        for message in messages() {
            ses.log(message);
        }

        let series = one_series(
            &ses,
            case.case,
            query("q", SEND, case.dimensions.clone(), DAY_ONE, DAY_ONE + DAY),
        )
        .await;
        assert_eq!(
            series, case.expected,
            "case {:?}: expected the series {:?}, got {series:?}",
            case.case, case.expected
        );
    }
}

#[tokio::test]
async fn a_batch_answers_every_query_it_carries_in_order() {
    let ses = Ses::with_vdm().await;
    ses.log(LoggedMessage::simple(
        "sender@example.com",
        "reader@recipient.example",
        DAY_ONE + 3_600,
    ));

    // The longest batch the model admits (`BatchGetMetricDataQueries`, len 1..10),
    // each query asking something different, so the correlation by `Id` is what the
    // caller reads the answers back through.
    let ids: Vec<String> = (1..=10).map(|index| format!("query-{index}")).collect();
    let queries: Vec<Value> = ids
        .iter()
        .zip(METRIC_VALUES.iter())
        .map(|(id, metric)| query(id, metric, json!({}), DAY_ONE, DAY_ONE + DAY))
        .collect();
    let reply = ses.batch(queries).await;

    assert_eq!(
        reply.status,
        200,
        "case \"full batch\": expected 200, got {} {:?}",
        reply.status,
        reply.message()
    );
    assert_eq!(
        reply.result_ids(),
        ids,
        "case \"full batch\": expected one result per query in order, got {:?}",
        reply.body
    );
    assert!(
        reply.errors().is_empty(),
        "case \"full batch\": every query was answered, so Errors must be empty, got {:?}",
        reply.errors()
    );
    // `METRIC_VALUES` lists `SEND` among the ten metrics, and the result that asked
    // for it is the only one with a data point.
    let send_position = METRIC_VALUES
        .iter()
        .position(|metric| *metric == SEND)
        .expect("SEND is a published metric");
    for (position, metric) in METRIC_VALUES.iter().enumerate() {
        let expected: Vec<(i64, i64)> = if position == send_position {
            vec![(DAY_ONE, 1)]
        } else {
            Vec::new()
        };
        let series = reply.series(position);
        assert_eq!(
            series, expected,
            "case {metric:?}: expected the series {expected:?} at position {position}, got {series:?}"
        );
    }
}

#[tokio::test]
async fn the_namespace_is_served_only_while_the_account_vdm_is_enabled() {
    let ses = Ses::new();
    ses.log(LoggedMessage::simple(
        "sender@example.com",
        "reader@recipient.example",
        DAY_ONE + 3_600,
    ));

    // What the account's VDM switch is set to for a leg, where `None` is an account
    // nothing has configured.
    let legs: Vec<(&str, Option<&str>, bool)> = vec![
        ("an account nothing has configured", None, false),
        ("VDM switched on", Some("ENABLED"), true),
        ("VDM switched off again", Some("DISABLED"), false),
        ("VDM switched back on", Some("ENABLED"), true),
    ];
    // Every feature status the model publishes is exercised above
    // (`smithy_query(query="shape",shape="FeatureStatus")`).
    for status in FEATURE_STATUS_VALUES {
        assert!(
            legs.iter().any(|(_, set, _)| *set == Some(*status)),
            "case {status:?}: the legs must cover every published feature status"
        );
    }

    for (case, status, served) in legs {
        if let Some(status) = status {
            let put = ses.put_vdm(status).await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: expected 200 from the VDM put, got {} {:?}",
                put.status,
                put.message()
            );
        }

        let reply = ses
            .batch(vec![query("q", SEND, json!({}), DAY_ONE, DAY_ONE + DAY)])
            .await;
        if served {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected the metric to be served, got {} {:?}",
                reply.status,
                reply.message()
            );
            assert_eq!(
                reply.series(0),
                vec![(DAY_ONE, 1)],
                "case {case:?}: expected the logged message to be counted, got {:?}",
                reply.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {} {:?}",
                reply.status,
                reply.error_code()
            );
            assert!(
                !reply.message().is_empty(),
                "case {case:?}: expected the refusal to say what is not enabled, got an empty message"
            );
        }
    }
}

#[tokio::test]
async fn a_window_whose_end_precedes_its_start_is_refused() {
    let cases: Vec<(&str, i64, i64, bool)> = vec![
        ("a window of three days", DAY_ONE, DAY_ONE + 3 * DAY, true),
        ("a window of one second", DAY_ONE, DAY_ONE + 1, true),
        ("a window of no time at all", DAY_ONE, DAY_ONE, true),
        ("an end one second before the start", DAY_ONE, DAY_ONE - 1, false),
        ("an end a whole day before the start", DAY_TWO, DAY_ONE, false),
    ];

    for (case, start, end, served) in cases {
        let ses = Ses::with_vdm().await;
        let reply = ses
            .batch(vec![query("q", SEND, json!({}), start, end)])
            .await;
        if served {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected the window to be served, got {} {:?}",
                reply.status,
                reply.message()
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {} {:?}",
                reply.status,
                reply.error_code()
            );
            assert!(
                reply.message().contains("must be before"),
                "case {case:?}: expected the refusal to name both bounds, got {:?}",
                reply.message()
            );
        }
    }
}

#[tokio::test]
async fn a_value_outside_a_published_value_set_is_refused() {
    /// One row: the query sent, and what the refusal must say about it.
    struct Case {
        case: &'static str,
        query: Value,
        /// The member path the refusal names, when the constraint is one stated at a
        /// member of the query.
        member_path: Option<&'static str>,
    }

    let cases = vec![
        Case {
            case: "a namespace the service does not publish",
            query: json!({"Id": "q", "Namespace": "NOT-A-NAMESPACE", "Metric": SEND,
                   "StartDate": DAY_ONE, "EndDate": DAY_ONE + DAY}),
            member_path: Some("queries.1.member.namespace"),
        },
        Case {
            case: "the published namespace in another case",
            query: json!({"Id": "q", "Namespace": "vdm", "Metric": SEND,
                   "StartDate": DAY_ONE, "EndDate": DAY_ONE + DAY}),
            member_path: Some("queries.1.member.namespace"),
        },
        Case {
            case: "a namespace that differs by surrounding space",
            query: json!({"Id": "q", "Namespace": " VDM", "Metric": SEND,
                   "StartDate": DAY_ONE, "EndDate": DAY_ONE + DAY}),
            member_path: Some("queries.1.member.namespace"),
        },
        Case {
            case: "a metric the service does not publish",
            query: json!({"Id": "q", "Namespace": VDM, "Metric": "SENDS",
                   "StartDate": DAY_ONE, "EndDate": DAY_ONE + DAY}),
            member_path: Some("queries.1.member.metric"),
        },
        Case {
            case: "a dimension name the service does not publish",
            query: json!({"Id": "q", "Namespace": VDM, "Metric": SEND,
                   "Dimensions": {"MAILBOX": "reader@recipient.example"},
                   "StartDate": DAY_ONE, "EndDate": DAY_ONE + DAY}),
            member_path: None,
        },
    ];

    for case in cases {
        let ses = Ses::with_vdm().await;
        let reply = ses.batch(vec![case.query]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {:?}: expected 400 BadRequestException, got {} {:?}",
            case.case,
            reply.status,
            reply.message()
        );
        let message = reply.message();
        assert!(
            !message.is_empty(),
            "case {:?}: expected the refusal to say what was invalid, got an empty message",
            case.case
        );
        if let Some(member_path) = case.member_path {
            // The clause names the member and the value set it had to satisfy, and
            // withholds the value that violated it.
            let subject = format!(
                "1 validation error detected: Value at '{member_path}' failed to satisfy \
                 constraint: Member must satisfy enum value set: ["
            );
            assert!(
                message.starts_with(&subject),
                "case {:?}: expected the refusal to open {subject:?}, got {message:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn the_published_namespace_is_the_only_one_the_refusal_names() {
    // The one published namespace (`smithy_query(query="shape",shape="MetricNamespace")`),
    // which is the whole value set the refusal states.
    let ses = Ses::with_vdm().await;
    let reply = ses
        .batch(vec![json!({
            "Id": "q",
            "Namespace": "NOT-A-NAMESPACE",
            "Metric": SEND,
            "StartDate": DAY_ONE,
            "EndDate": DAY_ONE + DAY,
        })])
        .await;
    let expected = format!(
        "1 validation error detected: Value at 'queries.1.member.namespace' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        METRIC_NAMESPACE_VALUES.join(", ")
    );
    assert_eq!(
        reply.message(),
        expected,
        "case \"unpublished namespace\": expected {expected:?}, got {:?}",
        reply.message()
    );
}

#[tokio::test]
async fn a_message_sent_over_the_wire_is_counted_by_the_metric() {
    let ses = Ses::with_vdm().await;
    let created = ses.create_identity("sender@example.com").await;
    assert_eq!(
        created.status,
        200,
        "case \"round trip\": expected 200 creating the identity, got {} {:?}",
        created.status,
        created.message()
    );
    // The identity's verification transition is due before the send, which is what
    // lets it send at all.
    ses.verify_identities();

    let sent = ses
        .send(json!({
            "FromEmailAddress": "sender@example.com",
            "Destination": {"ToAddresses": ["reader@recipient.example"]},
            "Content": {"Simple": {
                "Subject": {"Data": "hello"},
                "Body": {"Text": {"Data": "a message"}},
            }},
        }))
        .await;
    assert_eq!(
        sent.status,
        200,
        "case \"round trip\": expected 200 from the send, got {} {:?}",
        sent.status,
        sent.message()
    );

    // The window brackets now, so the message just accepted falls inside it whenever
    // the case runs.
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now is after the epoch")
        .as_secs() as i64;
    let series = one_series(
        &ses,
        "round trip",
        query("q", SEND, json!({}), now - DAY, now + DAY),
    )
    .await;
    assert_eq!(
        series.len(),
        1,
        "case \"round trip\": expected the one accepted message to be the one data point, got {series:?}"
    );
    let (at, value) = series[0];
    assert_eq!(
        value, 1,
        "case \"round trip\": expected the data point to count one message, got {value}"
    );
    assert_eq!(
        at.rem_euclid(DAY),
        0,
        "case \"round trip\": expected the data point to be stamped at a UTC day boundary, got {at}"
    );
    assert!(
        at <= now && at > now - DAY,
        "case \"round trip\": expected the data point inside the window, got {at} for a window ending {}",
        now + DAY
    );

    // The identity the message was sent under keeps the count; another identity does
    // not.
    let under_identity = one_series(
        &ses,
        "round trip, by identity",
        query(
            "q",
            SEND,
            json!({"EMAIL_IDENTITY": "sender@example.com"}),
            now - DAY,
            now + DAY,
        ),
    )
    .await;
    assert_eq!(
        under_identity, series,
        "case \"round trip, by identity\": expected the sending identity to keep the message, got {under_identity:?}"
    );
    let under_other = one_series(
        &ses,
        "round trip, by another identity",
        query(
            "q",
            SEND,
            json!({"EMAIL_IDENTITY": "someone@example.org"}),
            now - DAY,
            now + DAY,
        ),
    )
    .await;
    assert!(
        under_other.is_empty(),
        "case \"round trip, by another identity\": expected no data point, got {under_other:?}"
    );
}
