// SPDX-License-Identifier: CC0-1.0

//! A CloudTrail Lake channel created through the FULLY assembled router is readable
//! back through it.
//!
//! The per-service suite
//! (`services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/tests/all/channel_lifecycle.rs`)
//! pins what `CreateChannel` and `GetChannel` do with one service mounted, and the
//! sibling router suite (`cloudtrail_channel_routing.rs`) pins that an `awsJson1_1`
//! `X-Amz-Target` naming a CloudTrail operation is claimed by exactly one service and
//! that a malformed channel ARN comes back as the MODELLED refusal. What is left to this
//! suite, and to nothing else, is the half neither can see: a channel CREATED through the
//! fully assembled router is read back through it, out of the one shared registry.
//!
//! `CreateEventDataStore` is still a scaffold stub, so the event data store a channel
//! logs to is seeded into the same registry the handler reads, through the shared
//! engine; everything else goes over the wire.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, service_state, EventDataStoreRecord, EventDataStoreState,
};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The Region the calls are signed for, which is the Region the channel is homed in.
const REGION: &str = "us-west-2";

/// The `X-Amz-Target` prefix every AWS SDK sends for this service: the fully qualified
/// Coral service name.
const TARGET_PREFIX: &str = "com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101.";

/// The event data store the channel logs to.
const STORE_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";

/// A SigV4 `Authorization` header whose credential scope names the CloudTrail signing
/// name and [`REGION`]. Request authentication is off in this router, so the signature
/// value itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR/20260101/us-west-2/cloudtrail/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

#[test]
fn a_channel_created_through_the_assembled_router_reads_back_through_it() {
    std::thread::Builder::new()
        .name("cloudtrail-channel-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(Arc::clone(&engine));
                // Signature verification is the emulator's concern, not this
                // service's: pin it off here rather than depending on the
                // process-wide default, exactly as `cloudtrail_sdk_target_routing.rs`
                // does.
                router.set_request_authentication(false);

                // The store a channel destination names, seeded the way
                // `CreateEventDataStore` will once it is implemented.
                service_state(&*engine)
                    .expect("the in-memory engine carries this service's state")
                    .lock()
                    .event_data_stores
                    .insert(
                        STORE_ID.to_string(),
                        EventDataStoreRecord {
                            id: STORE_ID.to_string(),
                            name: "router-store".to_string(),
                            region: REGION.to_string(),
                            status: EventDataStoreState::Enabled,
                            owner_account_id: None,
                            created_at: Some(
                                SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
                            ),
                            // The members only a read of the store itself reports are
                            // not what a channel destination resolves against.
                            ..EventDataStoreRecord::default()
                        },
                    );

                let call = |operation: &'static str, body: String| {
                    let router = &router;
                    async move {
                        let request = http::Request::builder()
                            .method("POST")
                            .uri("https://127.0.0.1:9297/")
                            .header("host", "127.0.0.1:9297")
                            .header("content-type", "application/x-amz-json-1.1")
                            .header("authorization", AUTHORIZATION)
                            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
                            .body(bytes::Bytes::from(body))
                            .expect("the request is well formed");
                        let response = router.dispatch(request).await;
                        let status = response.status().as_u16();
                        let body = String::from_utf8_lossy(response.body().as_ref()).to_string();
                        (status, body)
                    }
                };

                let create_body = format!(
                    r#"{{"Name":"router-channel","Source":"Custom","Destinations":[{{"Type":"EVENT_DATA_STORE","Location":"{}"}}]}}"#,
                    event_data_store_arn(REGION, STORE_ID)
                );
                let (status, body) = call("CreateChannel", create_body).await;
                for refusal in ["AmbiguousService", "UnknownService", "NotImplemented"] {
                    assert!(
                        !body.contains(refusal),
                        "CreateChannel must reach the CloudTrail handler, got {status} {body}"
                    );
                }
                assert_eq!(
                    status, 200,
                    "CreateChannel on a free name is a success, got {status} {body}"
                );
                let created: serde_json::Value =
                    serde_json::from_str(&body).expect("the response is JSON");
                let channel_arn = created["ChannelArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    channel_arn
                        .starts_with(&format!("arn:aws:cloudtrail:{REGION}:000000000000:channel/")),
                    "a created channel carries an ARN in the Region it was created in, got \
                     {channel_arn:?}"
                );

                // The channel reads back through the same router, by the ARN it minted.
                let (status, body) =
                    call("GetChannel", format!(r#"{{"Channel":"{channel_arn}"}}"#)).await;
                assert_eq!(
                    status, 200,
                    "GetChannel must report the channel just created, got {status} {body}"
                );
                let fetched: serde_json::Value =
                    serde_json::from_str(&body).expect("the response is JSON");
                assert_eq!(
                    fetched["ChannelArn"],
                    serde_json::json!(channel_arn),
                    "GetChannel must report the ARN it was asked for, got {body}"
                );
                assert_eq!(
                    fetched["Name"],
                    serde_json::json!("router-channel"),
                    "GetChannel must report the stored name, got {body}"
                );
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
