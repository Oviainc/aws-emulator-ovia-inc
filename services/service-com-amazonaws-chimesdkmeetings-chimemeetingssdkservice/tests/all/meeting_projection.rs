// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the `Meeting` projection every create mints and every
//! reader returns.
//!
//! Each case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `chime` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal return value.
//!
//! Two rules are under test, both about the projection rather than about one request:
//!
//! 1. Each `MediaPlacement` endpoint is the service's whole URL template, query string
//!    included, with only the meeting's id and its media Region substituted in. For the
//!    screen-viewing endpoint that means the complete viewer connect query string — the
//!    two fixed viewer parameters followed by the meeting's call id — so a client that
//!    parses the URL finds every parameter the service always supplies.
//! 2. A list member of the projection is always present: `TenantIds` reports the ids the
//!    create named, and the EMPTY list when it named none, so a client that iterates the
//!    member never meets an absent one.
//!
//! The table spans the class the rules generalise over: the media Region (which scopes
//! every endpoint), how many tenant ids the create carries (none, one, three, and the
//! modelled maximum of five), and which of the two operations that mint a meeting was
//! used — since both must project the same record, and every row is read back through
//! `GetMeeting`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service; the signature itself is never checked here, since
/// the harness turns request authentication off.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The modelled maximum length of `TenantIdList`
/// (`smithy_query(query="shape",shape="TenantIdList",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`
/// -> `@length(min: 1, max: 5)`).
const MAX_TENANT_IDS: usize = 5;

/// The fixed viewer parameters the screen-viewing connect URL carries ahead of the
/// meeting's call id, as `name=value` pairs in the order the service emits them.
const VIEWER_PARAMETERS: &[(&str, &str)] = &[("passcode", "null"), ("viewer_uuid", "null")];

/// The query parameter the screen-viewing connect URL carries the meeting's own id in.
const CALL_ID_PARAMETER: &str = "X-BitHub-Call-Id";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A named member of a success body.
    fn member(&self, name: &str) -> Value {
        self.body
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("response has no `{name}` member: {self:?}"))
    }
}

/// The emulator under test: ONE shared engine with this service mounted on it, reached
/// the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://meetings-chime.us-east-1.amazonaws.com{path}"))
            .header("authorization", SIGNED_FOR_CHIME)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create_meeting(&self, request: &Value) -> Reply {
        self.call("POST", "/meetings", Some(request)).await
    }

    async fn create_meeting_with_attendees(&self, request: &Value) -> Reply {
        self.call("POST", "/meetings?operation=create-attendees", Some(request))
            .await
    }

    async fn get_meeting(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}"), None)
            .await
    }
}

/// Which of the two operations that mint a meeting a row goes through.
#[derive(Debug, Clone, Copy)]
enum Minter {
    CreateMeeting,
    CreateMeetingWithAttendees,
}

/// One row: how the meeting is created, in which Region, and the tenant ids the request
/// carries (`None` = the member is absent from the request).
#[derive(Debug)]
struct Case {
    name: &'static str,
    minter: Minter,
    region: &'static str,
    tenant_ids: Option<Vec<&'static str>>,
}

fn string_member(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member `{member}` is not a string in {record:?}"))
        .to_string()
}

/// The query string of `url`, split into its `name=value` pairs in wire order.
fn query_pairs(url: &str) -> Vec<(String, String)> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

/// Both projection rules hold for every meeting this service mints, whichever operation
/// minted it, and the record a reader returns is the record the create returned.
#[tokio::test]
async fn a_minted_meeting_projects_a_full_connect_url_and_a_tenant_id_list() {
    let five: Vec<&str> = vec![
        "tenant-one",
        "tenant-two",
        "tenant-three",
        "tenant-four",
        "tenant-five",
    ];
    assert_eq!(
        five.len(),
        MAX_TENANT_IDS,
        "the widest row must carry the modelled maximum of {MAX_TENANT_IDS} tenant ids"
    );

    // Regions drawn from `MediaRegion`'s own documented list, including a GovCloud one,
    // since the Region is what fills the host's region label.
    let cases: Vec<Case> = vec![
        Case {
            name: "no tenant ids at all",
            minter: Minter::CreateMeeting,
            region: "us-west-2",
            tenant_ids: None,
        },
        Case {
            name: "no tenant ids, in another Region",
            minter: Minter::CreateMeeting,
            region: "eu-central-1",
            tenant_ids: None,
        },
        Case {
            name: "one tenant id",
            minter: Minter::CreateMeeting,
            region: "ap-northeast-1",
            tenant_ids: Some(vec!["tenant-only"]),
        },
        Case {
            name: "three tenant ids",
            minter: Minter::CreateMeeting,
            region: "sa-east-1",
            tenant_ids: Some(vec!["tenant-a", "tenant-b", "tenant-c"]),
        },
        Case {
            name: "the modelled maximum of five tenant ids",
            minter: Minter::CreateMeeting,
            region: "us-gov-west-1",
            tenant_ids: Some(five.clone()),
        },
        Case {
            name: "no tenant ids, minted with attendees",
            minter: Minter::CreateMeetingWithAttendees,
            region: "ca-central-1",
            tenant_ids: None,
        },
        Case {
            name: "two tenant ids, minted with attendees",
            minter: Minter::CreateMeetingWithAttendees,
            region: "eu-west-3",
            tenant_ids: Some(vec!["tenant-x", "tenant-y"]),
        },
    ];

    let emulator = Emulator::new();

    for (index, case) in cases.iter().enumerate() {
        let name = case.name;
        let mut request = json!({
            "ClientRequestToken": format!("tok-projection-{index}"),
            "MediaRegion": case.region,
            "ExternalMeetingId": format!("ext-projection-{index}"),
        });
        if let Some(ids) = &case.tenant_ids {
            request["TenantIds"] = json!(ids);
        }
        let reply = match case.minter {
            Minter::CreateMeeting => emulator.create_meeting(&request).await,
            Minter::CreateMeetingWithAttendees => {
                request["Attendees"] = json!([{ "ExternalUserId": format!("user-{index}") }]);
                emulator.create_meeting_with_attendees(&request).await
            }
        };
        assert_eq!(
            reply.status, 200,
            "case {name:?}: expected HTTP 200, got {} with {:?}",
            reply.status, reply.body
        );

        let meeting = reply.member("Meeting");
        let meeting_id = string_member(&meeting, "MeetingId");

        // Rule 2: the member is always there — the ids asked for, or the empty list.
        let expected_tenant_ids: Value = case
            .tenant_ids
            .as_ref()
            .map_or_else(|| json!([]), |ids| json!(ids));
        assert_eq!(
            meeting.get("TenantIds"),
            Some(&expected_tenant_ids),
            "case {name:?}: expected TenantIds {expected_tenant_ids:?}, got {:?}",
            meeting.get("TenantIds")
        );

        // Rule 1: the screen-viewing endpoint is the whole connect URL — the fixed viewer
        // parameters, then this meeting's call id — on a host scoped to its Region.
        let placement = meeting
            .get("MediaPlacement")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: no MediaPlacement in {meeting:?}"));
        let screen_viewing_url = string_member(&placement, "ScreenViewingUrl");
        let (endpoint, _) = screen_viewing_url
            .split_once('?')
            .unwrap_or_else(|| panic!("case {name:?}: no query string in {screen_viewing_url:?}"));
        assert!(
            endpoint.ends_with("/ws/connect") && endpoint.contains(case.region),
            "case {name:?}: expected a connect endpoint scoped to {}, got {endpoint:?}",
            case.region
        );
        let mut expected_pairs: Vec<(String, String)> = VIEWER_PARAMETERS
            .iter()
            .map(|(parameter, value)| ((*parameter).to_string(), (*value).to_string()))
            .collect();
        expected_pairs.push((CALL_ID_PARAMETER.to_string(), meeting_id.clone()));
        assert_eq!(
            query_pairs(&screen_viewing_url),
            expected_pairs,
            "case {name:?}: expected the full viewer connect query string, got \
             {screen_viewing_url:?}"
        );

        // The round trip: a reader returns the record the create minted, so both rules
        // hold for every later read of the same meeting.
        let read = emulator.get_meeting(&meeting_id).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: GetMeeting expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.member("Meeting"),
            meeting,
            "case {name:?}: GetMeeting must report what the create returned, got {:?}",
            read.member("Meeting")
        );
    }
}

/// `TenantIds` is serialized as a list on the WIRE — the body a client parses carries
/// `[]`, not a null and not a missing member — for a create that named no tenant ids, and
/// for the read that follows it.
#[tokio::test]
async fn an_absent_tenant_id_list_reaches_the_wire_as_an_empty_array() {
    let emulator = Emulator::new();
    let request = json!({
        "ClientRequestToken": "tok-wire-tenants",
        "MediaRegion": "us-east-1",
        "ExternalMeetingId": "ext-wire-tenants",
    });

    let created = emulator.create_meeting(&request).await;
    assert_eq!(
        created.status, 200,
        "CreateMeeting expected 200, got {} with {:?}",
        created.status, created.body
    );
    let meeting_id = string_member(&created.member("Meeting"), "MeetingId");
    let read = emulator.get_meeting(&meeting_id).await;

    for (case, body) in [("CreateMeeting", &created.body), ("GetMeeting", &read.body)] {
        let rendered = serde_json::to_string(body).expect("a response body serializes");
        assert!(
            rendered.contains("\"TenantIds\":[]"),
            "case {case:?}: expected the body to carry TenantIds as an empty array, got \
             {rendered:?}"
        );
    }
}
