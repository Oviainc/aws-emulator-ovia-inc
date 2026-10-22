// SPDX-License-Identifier: CC0-1.0

//! `GetMedia` end to end: a seeded stream's chunks are read back over HTTP, through
//! the router, for every start selector the model declares.
//!
//! The rule under test: `GetMedia` answers the contiguous run of PERSISTED fragments
//! that begins at the position its `StartSelector` names, in fragment order, each
//! chunk followed by the Matroska `Tags` element carrying that chunk's
//! `AWS_KINESISVIDEO_*` metadata and the continuation token that resumes immediately
//! after it. The tables below span the selector types, the two stream identifiers, the
//! cursor positions and the refusals — the request the issue recorded is one row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::ops::get_media::{
    ID_SIMPLE_TAG, ID_TAG, ID_TAGS, ID_TAG_NAME, ID_TAG_STRING,
};
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::{
    ComAmazonawsKinesisvideomediaAwsacuityinletserviceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

// ---------------------------------------------------------------------------
// The inverse of the chunk framing the handler writes.
// ---------------------------------------------------------------------------
//
// The emulator only EMITS this framing, so the reader lives here, in the test that
// checks what was emitted, rather than on the service crate's surface. Both halves
// follow RFC 8794 section 4: an element is its id, then its size as a variable-size
// integer, then its body, and the width of an id or a size is given by the position of
// the leading one bit of its first byte.

/// Read the first EBML element in `bytes`: its id, its body, and how many bytes the
/// whole element occupied. `None` when `bytes` does not start with a well-formed,
/// fully present element.
fn read_ebml_element(bytes: &[u8]) -> Option<(&[u8], &[u8], usize)> {
    let id_first = *bytes.first()?;
    if id_first == 0 {
        return None;
    }
    let id_len = id_first.leading_zeros() as usize + 1;
    let size_first = *bytes.get(id_len)?;
    if size_first == 0 {
        return None;
    }
    let size_len = size_first.leading_zeros() as usize + 1;
    if bytes.len() < id_len + size_len {
        return None;
    }
    // VINT_DATA is the first byte with its marker bit cleared, then the rest,
    // big-endian. Masking through `u16` keeps the 8-byte width from shifting a `u8` by
    // its own bit count.
    let mut size = u64::from(size_first & ((0xFFu16 >> size_len) as u8));
    for byte in &bytes[id_len + 1..id_len + size_len] {
        size = (size << 8) | u64::from(*byte);
    }
    let body_start = id_len + size_len;
    let end = body_start.checked_add(usize::try_from(size).ok()?)?;
    if bytes.len() < end {
        return None;
    }
    Some((&bytes[..id_len], &bytes[body_start..end], end))
}

/// The `TagName`/`TagString` pairs of the `Tags` element at the start of `bytes`, in
/// the order they were written. Empty when `bytes` does not start with a `Tags`
/// element.
fn read_kvs_tags(bytes: &[u8]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let Some((id, tags_body, _)) = read_ebml_element(bytes) else {
        return pairs;
    };
    if id != ID_TAGS {
        return pairs;
    }
    let mut tags = tags_body;
    while let Some((id, tag_body, used)) = read_ebml_element(tags) {
        if id == ID_TAG {
            let mut simple_tags = tag_body;
            while let Some((id, simple_body, used)) = read_ebml_element(simple_tags) {
                if id == ID_SIMPLE_TAG {
                    let mut name = None;
                    let mut value = None;
                    let mut fields = simple_body;
                    while let Some((id, body, used)) = read_ebml_element(fields) {
                        if id == ID_TAG_NAME {
                            name = String::from_utf8(body.to_vec()).ok();
                        } else if id == ID_TAG_STRING {
                            value = String::from_utf8(body.to_vec()).ok();
                        }
                        fields = &fields[used..];
                    }
                    if let (Some(name), Some(value)) = (name, value) {
                        pairs.push((name, value));
                    }
                }
                simple_tags = &simple_tags[used..];
            }
        }
        tags = &tags[used..];
    }
    pairs
}

/// The signing region every request in this suite uses.
const REGION: &str = "us-east-1";

/// Producer timestamp of the first seeded fragment, in epoch milliseconds. Chosen on a
/// whole-second boundary so a `StartTimestamp` sent as epoch seconds is exact.
const BASE_MILLIS: u64 = 1_700_000_000_000;

/// Milliseconds between two consecutive seeded fragments.
const FRAGMENT_SPACING_MILLIS: u64 = 2_000;

/// How far behind its producer timestamp a seeded fragment's server timestamp sits.
const SERVER_LAG_MILLIS: u64 = 500;

/// The chunk bytes seeded for the four fragments of `my-stream`. Deliberately
/// different lengths and contents, including an empty chunk, so a payload that
/// concatenated them wrongly (or padded them) cannot pass.
const CHUNKS: &[&[u8]] = &[
    b"fragment-one-mkv-bytes",
    b"2",
    b"",
    b"\x00\x01\x02\xff\xfe-fourth-chunk",
];

fn producer_millis(index: usize) -> u64 {
    BASE_MILLIS + (index as u64) * FRAGMENT_SPACING_MILLIS
}

fn server_millis(index: usize) -> u64 {
    producer_millis(index) + SERVER_LAG_MILLIS
}

fn at(millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(millis)
}

/// Epoch milliseconds as the epoch-seconds number restJson1 binds a timestamp from.
fn as_epoch_seconds(millis: u64) -> serde_json::Value {
    serde_json::json!(millis as f64 / 1000.0)
}

/// A router carrying only this service, over the shared engine the seeding helper
/// wrote to — the same single engine the production router hands every operation.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Seed `stream_name` with one fragment per entry of `chunks`, through the producer
/// entry point on this service's state, and answer the stream's ARN.
fn seed(engine: &Arc<engine::InMemoryEngine>, stream_name: &str, chunks: &[&[u8]]) -> String {
    let state = engine.state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();
    let mut store = state.lock();
    store
        .ensure_stream(stream_name, REGION, at(BASE_MILLIS))
        .expect("the seeded stream name is one the service accepts");
    for (index, chunk) in chunks.iter().enumerate() {
        store
            .put_fragment(
                stream_name,
                REGION,
                chunk.to_vec(),
                at(producer_millis(index)),
                at(server_millis(index)),
            )
            .expect("the seeded stream name is one the service accepts");
    }
    store
        .streams
        .get(stream_name)
        .map(|stream| stream.arn.clone())
        .unwrap_or_default()
}

/// A signed-looking `POST /getMedia`. The credential scope's service field is the
/// routing key the router selects this service with; no authenticator is registered,
/// so the signature itself is not checked.
fn get_media_request(body: &serde_json::Value) -> http::Request<Bytes> {
    http::Request::builder()
        .method("POST")
        .uri("/getMedia")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/kinesisvideo/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request")
}

async fn get_media(
    router: &MultiServiceRouter,
    body: &serde_json::Value,
) -> http::Response<Bytes> {
    router.dispatch(get_media_request(body)).await
}

/// A `GetMedia` body naming a stream by name, with the given selector.
fn by_name(stream_name: &str, selector: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "StreamName": stream_name, "StartSelector": selector })
}

/// The decoded form of one emitted chunk: the media bytes and its `AWS_KINESISVIDEO_*`
/// tags.
#[derive(Debug)]
struct EmittedChunk {
    media: Vec<u8>,
    tags: Vec<(String, String)>,
}

impl EmittedChunk {
    fn tag(&self, name: &str) -> String {
        self.tags
            .iter()
            .find(|(tag, _)| tag == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }
}

/// Split a payload into its chunks, given the media bytes each emitted fragment was
/// seeded with: every chunk is its stored bytes followed by exactly one `Tags`
/// element, and the payload ends with the last one.
fn split_payload(case: &str, payload: &[u8], expected_media: &[&[u8]]) -> Vec<EmittedChunk> {
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    for media in expected_media {
        let end = offset + media.len();
        assert!(
            payload.len() >= end && &payload[offset..end] == *media,
            "case {case:?}: expected the stored chunk {media:?} at offset {offset}, \
             got payload {:?}",
            &payload[offset.min(payload.len())..]
        );
        let (_, _, used) = read_ebml_element(&payload[end..]).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a Tags element after the chunk at offset {end}, \
                 got {:?}",
                &payload[end.min(payload.len())..]
            )
        });
        let tags = read_kvs_tags(&payload[end..]);
        assert!(
            !tags.is_empty(),
            "case {case:?}: the element after the chunk at offset {end} carried no MKV tags"
        );
        chunks.push(EmittedChunk {
            media: media.to_vec(),
            tags,
        });
        offset = end + used;
    }
    assert_eq!(
        offset,
        payload.len(),
        "case {case:?}: the payload carried {} unexpected trailing bytes ({:?})",
        payload.len() - offset,
        &payload[offset.min(payload.len())..]
    );
    chunks
}

/// The fragment numbers a payload delivered, read out of the chunks' own MKV tags.
fn delivered_fragment_numbers(case: &str, payload: &[u8], expected: &[usize]) -> Vec<String> {
    let media: Vec<&[u8]> = expected.iter().map(|index| CHUNKS[*index]).collect();
    split_payload(case, payload, &media)
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect()
}

fn status_of(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

fn error_code(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// EARLIEST delivers every persisted chunk, in fragment order, with the stream's
/// content type and each chunk's own metadata — all of it read back from the
/// fragments the producer entry point wrote.
#[tokio::test]
async fn earliest_returns_every_persisted_chunk_with_its_metadata() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine, "my-stream", CHUNKS);
    let router = router(&engine);

    let response = get_media(
        &router,
        &by_name("my-stream", serde_json::json!({"StartSelectorType": "EARLIEST"})),
    )
    .await;

    assert_eq!(status_of(&response), 200, "case \"EARLIEST\": expected 200");
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default(),
        "video/webm",
        "case \"EARLIEST\": expected the stream's content type on the response"
    );

    let chunks = split_payload("EARLIEST", response.body(), CHUNKS);
    assert_eq!(
        chunks.len(),
        CHUNKS.len(),
        "case \"EARLIEST\": expected {} chunks, got {}",
        CHUNKS.len(),
        chunks.len()
    );
    for (index, chunk) in chunks.iter().enumerate() {
        let case = format!("EARLIEST chunk {index}");
        assert_eq!(
            chunk.media,
            CHUNKS[index],
            "case {case:?}: expected the stored media bytes, got {:?}",
            chunk.media
        );
        assert_eq!(
            chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"),
            (index + 1).to_string(),
            "case {case:?}: wrong fragment number, got {:?}",
            chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER")
        );
        assert_eq!(
            chunk.tag("AWS_KINESISVIDEO_PRODUCER_TIMESTAMP"),
            producer_millis(index).to_string(),
            "case {case:?}: wrong producer timestamp, got {:?}",
            chunk.tag("AWS_KINESISVIDEO_PRODUCER_TIMESTAMP")
        );
        assert_eq!(
            chunk.tag("AWS_KINESISVIDEO_SERVER_TIMESTAMP"),
            server_millis(index).to_string(),
            "case {case:?}: wrong server timestamp, got {:?}",
            chunk.tag("AWS_KINESISVIDEO_SERVER_TIMESTAMP")
        );
        // How far behind the newest persisted chunk this one is.
        let behind = server_millis(CHUNKS.len() - 1) - server_millis(index);
        assert_eq!(
            chunk.tag("AWS_KINESISVIDEO_MILLIS_BEHIND_NOW"),
            behind.to_string(),
            "case {case:?}: wrong millis-behind-now, got {:?}",
            chunk.tag("AWS_KINESISVIDEO_MILLIS_BEHIND_NOW")
        );
        let token = chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN");
        assert!(
            (1..=128).contains(&token.len())
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
            "case {case:?}: the continuation token must satisfy the modelled \
             ^[a-zA-Z0-9_\\.\\-]+$ (1..128), got {token:?}"
        );
    }
}

/// Every start selector the model declares, resolved over one seeded stream. The
/// expectation is the list of fragment INDEXES the call must deliver.
#[tokio::test]
async fn every_start_selector_resolves_to_its_documented_position() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let arn = seed(&engine, "my-stream", CHUNKS);
    let router = router(&engine);

    // `StartSelectorType`'s own documentation is the source of each row: NOW is "the
    // latest chunk", EARLIEST "the earliest available chunk", FRAGMENT_NUMBER "the
    // chunk AFTER a specific fragment", PRODUCER_TIMESTAMP / SERVER_TIMESTAMP "the
    // chunk containing a fragment with the specified timestamp".
    let rows: Vec<(&str, serde_json::Value, Vec<usize>)> = vec![
        (
            "EARLIEST",
            serde_json::json!({"StartSelectorType": "EARLIEST"}),
            vec![0, 1, 2, 3],
        ),
        (
            "NOW",
            serde_json::json!({"StartSelectorType": "NOW"}),
            vec![3],
        ),
        (
            "FRAGMENT_NUMBER after the first",
            serde_json::json!({"StartSelectorType": "FRAGMENT_NUMBER", "AfterFragmentNumber": "1"}),
            vec![1, 2, 3],
        ),
        (
            "FRAGMENT_NUMBER after the third",
            serde_json::json!({"StartSelectorType": "FRAGMENT_NUMBER", "AfterFragmentNumber": "3"}),
            vec![3],
        ),
        (
            "FRAGMENT_NUMBER after the newest leaves nothing to send",
            serde_json::json!({"StartSelectorType": "FRAGMENT_NUMBER", "AfterFragmentNumber": "4"}),
            vec![],
        ),
        (
            "PRODUCER_TIMESTAMP exactly on the first fragment",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(producer_millis(0)),
            }),
            vec![0, 1, 2, 3],
        ),
        (
            "PRODUCER_TIMESTAMP inside the first fragment's span",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(producer_millis(0) + 1_000),
            }),
            vec![0, 1, 2, 3],
        ),
        (
            "PRODUCER_TIMESTAMP exactly on the second fragment",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(producer_millis(1)),
            }),
            vec![1, 2, 3],
        ),
        (
            "PRODUCER_TIMESTAMP older than every fragment",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(BASE_MILLIS - 60_000),
            }),
            vec![0, 1, 2, 3],
        ),
        (
            "PRODUCER_TIMESTAMP newer than every fragment",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(BASE_MILLIS + 600_000),
            }),
            vec![3],
        ),
        (
            "SERVER_TIMESTAMP exactly on the first fragment",
            serde_json::json!({
                "StartSelectorType": "SERVER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(server_millis(0)),
            }),
            vec![0, 1, 2, 3],
        ),
        (
            "SERVER_TIMESTAMP inside the third fragment's span",
            serde_json::json!({
                "StartSelectorType": "SERVER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(server_millis(2) + 100),
            }),
            vec![2, 3],
        ),
        (
            "SERVER_TIMESTAMP before the first fragment reached the service",
            serde_json::json!({
                "StartSelectorType": "SERVER_TIMESTAMP",
                "StartTimestamp": as_epoch_seconds(server_millis(0) - 100),
            }),
            vec![0, 1, 2, 3],
        ),
        (
            "PRODUCER_TIMESTAMP at the largest epoch second that is still representable in milliseconds",
            serde_json::json!({
                "StartSelectorType": "PRODUCER_TIMESTAMP",
                "StartTimestamp": 9.0e15,
            }),
            vec![3],
        ),
    ];

    for (case, selector, expected) in rows {
        let response = get_media(&router, &by_name("my-stream", selector.clone())).await;
        assert_eq!(
            status_of(&response),
            200,
            "case {case:?}: expected 200, got {} ({})",
            status_of(&response),
            error_code(&response)
        );
        let delivered = delivered_fragment_numbers(case, response.body(), &expected);
        let want: Vec<String> = expected.iter().map(|i| (i + 1).to_string()).collect();
        assert_eq!(
            delivered, want,
            "case {case:?}: expected fragments {want:?}, got {delivered:?}"
        );

        // The same selector against the SAME stream named by its ARN must resolve
        // identically: the two identifiers name one resource.
        let by_arn = serde_json::json!({ "StreamARN": arn, "StartSelector": selector });
        let response = get_media(&router, &by_arn).await;
        assert_eq!(
            status_of(&response),
            200,
            "case {case:?} by ARN: expected 200, got {} ({})",
            status_of(&response),
            error_code(&response)
        );
        let delivered = delivered_fragment_numbers(case, response.body(), &expected);
        assert_eq!(
            delivered, want,
            "case {case:?} by ARN: expected fragments {want:?}, got {delivered:?}"
        );
    }
}

/// A chunk's continuation token resumes exactly after that chunk: for every cursor the
/// stream has, the follow-up call delivers the fragments after it and no others —
/// neither a duplicate nor a gap. And a token taken before new fragments arrive
/// delivers exactly the new ones.
#[tokio::test]
async fn a_continuation_token_resumes_exactly_after_its_own_chunk() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine, "my-stream", CHUNKS);
    let router = router(&engine);

    let first = get_media(
        &router,
        &by_name("my-stream", serde_json::json!({"StartSelectorType": "EARLIEST"})),
    )
    .await;
    assert_eq!(status_of(&first), 200, "the first read must succeed");
    let tokens: Vec<String> = split_payload("EARLIEST", first.body(), CHUNKS)
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .collect();

    for (index, token) in tokens.iter().enumerate() {
        let case = format!("resume after chunk {index}");
        let expected: Vec<usize> = (index + 1..CHUNKS.len()).collect();
        let response = get_media(
            &router,
            &by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "CONTINUATION_TOKEN",
                    "ContinuationToken": token,
                }),
            ),
        )
        .await;
        assert_eq!(
            status_of(&response),
            200,
            "case {case:?}: expected 200, got {} ({})",
            status_of(&response),
            error_code(&response)
        );
        let delivered = delivered_fragment_numbers(&case, response.body(), &expected);
        let want: Vec<String> = expected.iter().map(|i| (i + 1).to_string()).collect();
        assert_eq!(
            delivered, want,
            "case {case:?}: expected fragments {want:?}, got {delivered:?}"
        );
    }

    // A producer write after the token was handed out is what the token resumes into.
    let fresh_chunk: &[u8] = b"fifth-chunk-written-later";
    {
        let state = engine.state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();
        state
            .lock()
            .put_fragment(
                "my-stream",
                REGION,
                fresh_chunk.to_vec(),
                at(producer_millis(CHUNKS.len())),
                at(server_millis(CHUNKS.len())),
            )
            .expect("the seeded stream name is one the service accepts");
    }
    let last_token = tokens.last().expect("EARLIEST delivered chunks").clone();
    let response = get_media(
        &router,
        &by_name(
            "my-stream",
            serde_json::json!({
                "StartSelectorType": "CONTINUATION_TOKEN",
                "ContinuationToken": last_token,
            }),
        ),
    )
    .await;
    assert_eq!(
        status_of(&response),
        200,
        "case \"resume into a later write\": expected 200, got {}",
        status_of(&response)
    );
    let chunks = split_payload("resume into a later write", response.body(), &[fresh_chunk]);
    let numbers: Vec<String> = chunks
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect();
    assert_eq!(
        numbers,
        vec!["5".to_string()],
        "case \"resume into a later write\": expected only the newly written fragment, \
         got {numbers:?}"
    );
}

/// A cursor past the newest fragment, and a stream with no fragments at all, are an
/// empty payload and not a failure.
#[tokio::test]
async fn a_stream_with_nothing_left_to_send_answers_an_empty_payload() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine, "my-stream", CHUNKS);
    {
        // A stream that exists but was never written to.
        let state = engine.state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();
        state
            .lock()
            .ensure_stream("silent-stream", REGION, at(BASE_MILLIS))
            .expect("the seeded stream name is one the service accepts");
    }
    let router = router(&engine);

    let rows: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "an empty stream read from the earliest chunk",
            "silent-stream",
            serde_json::json!({"StartSelectorType": "EARLIEST"}),
        ),
        (
            "an empty stream read from its latest chunk",
            "silent-stream",
            serde_json::json!({"StartSelectorType": "NOW"}),
        ),
        (
            "a cursor past the newest fragment",
            "my-stream",
            serde_json::json!({"StartSelectorType": "FRAGMENT_NUMBER", "AfterFragmentNumber": "4"}),
        ),
    ];

    for (case, stream, selector) in rows {
        let response = get_media(&router, &by_name(stream, selector)).await;
        assert_eq!(
            status_of(&response),
            200,
            "case {case:?}: expected 200, got {} ({})",
            status_of(&response),
            error_code(&response)
        );
        assert!(
            response.body().is_empty(),
            "case {case:?}: expected an empty payload, got {:?}",
            response.body()
        );
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default(),
            "video/webm",
            "case {case:?}: an empty payload still reports the stream's content type"
        );
    }
}

/// The refusals: a request that names no single stream, a selector missing the
/// companion member its type requires, a position that is not on the stream, and an
/// identifier that resolves to no stream at all.
#[tokio::test]
async fn malformed_requests_are_refused_with_the_modelled_errors() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let arn = seed(&engine, "my-stream", CHUNKS);
    seed(&engine, "other-stream", &[b"other-chunk"]);
    let router = router(&engine);

    // A token minted for another stream, to prove a cursor is stream-scoped.
    let other = get_media(
        &router,
        &by_name(
            "other-stream",
            serde_json::json!({"StartSelectorType": "EARLIEST"}),
        ),
    )
    .await;
    let other_token = split_payload("other-stream EARLIEST", other.body(), &[b"other-chunk"])
        .first()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .unwrap_or_default();

    let unknown_arn = arn.replace("my-stream", "absent-stream");
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "neither StreamName nor StreamARN",
            serde_json::json!({"StartSelector": {"StartSelectorType": "NOW"}}),
            400,
            "InvalidArgumentException",
        ),
        (
            "both StreamName and StreamARN",
            serde_json::json!({
                "StreamName": "my-stream",
                "StreamARN": arn,
                "StartSelector": {"StartSelectorType": "NOW"},
            }),
            400,
            "InvalidArgumentException",
        ),
        (
            "FRAGMENT_NUMBER without AfterFragmentNumber",
            by_name("my-stream", serde_json::json!({"StartSelectorType": "FRAGMENT_NUMBER"})),
            400,
            "InvalidArgumentException",
        ),
        (
            "PRODUCER_TIMESTAMP without StartTimestamp",
            by_name(
                "my-stream",
                serde_json::json!({"StartSelectorType": "PRODUCER_TIMESTAMP"}),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "SERVER_TIMESTAMP without StartTimestamp",
            by_name(
                "my-stream",
                serde_json::json!({"StartSelectorType": "SERVER_TIMESTAMP"}),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "CONTINUATION_TOKEN without ContinuationToken",
            by_name(
                "my-stream",
                serde_json::json!({"StartSelectorType": "CONTINUATION_TOKEN"}),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a continuation token this service never minted",
            by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "CONTINUATION_TOKEN",
                    "ContinuationToken": "not-a-token-we-minted",
                }),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a continuation token minted for another stream",
            by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "CONTINUATION_TOKEN",
                    "ContinuationToken": other_token,
                }),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a fragment number that is not on the stream",
            by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "FRAGMENT_NUMBER",
                    "AfterFragmentNumber": "987654321",
                }),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a StartTimestamp too large to be an epoch millisecond",
            by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "PRODUCER_TIMESTAMP",
                    "StartTimestamp": 1.0e16,
                }),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a StartTimestamp too far before the epoch to be an epoch millisecond",
            by_name(
                "my-stream",
                serde_json::json!({
                    "StartSelectorType": "SERVER_TIMESTAMP",
                    "StartTimestamp": -1.0e16,
                }),
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            // `StartSelectorType` is a modelled enum, so a value outside
            // `START_SELECTOR_TYPE_VALUES` is refused by the framework's schema
            // validation BEFORE the handler runs — the handler's own catch-all for an
            // unmodelled type is a defensive arm no request reaches.
            "a StartSelectorType outside the modelled enum",
            by_name(
                "my-stream",
                serde_json::json!({"StartSelectorType": "SOMETIME_SOON"}),
            ),
            400,
            "ValidationException",
        ),
        (
            "an unknown stream name",
            by_name("no-such-stream", serde_json::json!({"StartSelectorType": "NOW"})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an unknown stream ARN",
            serde_json::json!({
                "StreamARN": unknown_arn,
                "StartSelector": {"StartSelectorType": "EARLIEST"},
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, body, status, code) in rows {
        let response = get_media(&router, &body).await;
        assert_eq!(
            status_of(&response),
            status,
            "case {case:?}: expected HTTP {status}, got {}",
            status_of(&response)
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {case:?}: expected {code}, got {:?}",
            error_code(&response)
        );
    }
}

/// Reading a stream does not change what the next caller sees: the same request
/// answers the same bytes, and the fragment log is untouched by a read.
#[tokio::test]
async fn reading_a_stream_twice_answers_the_same_bytes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine, "my-stream", CHUNKS);
    let router = router(&engine);

    let body = by_name("my-stream", serde_json::json!({"StartSelectorType": "EARLIEST"}));
    let first = get_media(&router, &body).await;
    let second = get_media(&router, &body).await;
    assert_eq!(
        first.body(),
        second.body(),
        "case \"repeated EARLIEST\": a read must not move the stream's cursor"
    );

    let state = engine.state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();
    let store = state.lock();
    let stream = store
        .streams
        .get("my-stream")
        .expect("the seeded stream is still there");
    assert_eq!(
        stream.fragments.len(),
        CHUNKS.len(),
        "case \"repeated EARLIEST\": a read must not add or drop fragments, got {}",
        stream.fragments.len()
    );
}

/// The chunk framing itself: an element written with any body length is read back
/// with the same id and body. The rows walk every EBML variable-size-integer width
/// boundary the writer can choose between, and one past each.
#[tokio::test]
async fn the_chunk_framing_round_trips_at_every_size_boundary() {
    use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::ops::get_media::{
        push_ebml_element, ID_TAG_STRING,
    };

    // A VINT of width n holds lengths up to 2^(7n) - 2, because all-ones VINT_DATA is
    // reserved for "unknown size" (RFC 8794 section 4.4). These are the widths a
    // response-sized element can reach, plus the first length past each.
    let sizes: Vec<usize> = vec![0, 1, 125, 126, 127, 128, 16_382, 16_383, 16_384, 70_000];

    for size in sizes {
        let case = format!("element body of {size} bytes");
        let body: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let mut encoded = Vec::new();
        push_ebml_element(&mut encoded, ID_TAG_STRING, &body);
        let (id, decoded, used) = read_ebml_element(&encoded)
            .unwrap_or_else(|| panic!("case {case:?}: the element did not read back"));
        assert_eq!(id, ID_TAG_STRING, "case {case:?}: wrong element id");
        assert_eq!(
            decoded, &body[..],
            "case {case:?}: the body did not round-trip"
        );
        assert_eq!(
            used,
            encoded.len(),
            "case {case:?}: the element did not consume its own bytes, got {used}"
        );
    }
}

/// The producer entry point accepts exactly the stream names the model's `StreamName`
/// allows (len 1..256, `^[a-zA-Z0-9_.-]+$`) and only a region the modelled
/// `ResourceARN` can carry (`[a-z0-9-]+`), so no stream can carry an ARN the model
/// would reject. Rows walk the charset and both length bounds, plus one past each.
#[tokio::test]
async fn only_modelled_stream_names_and_regions_are_accepted_by_the_producer_path() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let state = engine.state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();

    let longest = "a".repeat(256);
    let too_long = "a".repeat(257);
    let rows: Vec<(&str, bool)> = vec![
        ("a", true),
        ("my-stream", true),
        ("Stream_1.name-2", true),
        (longest.as_str(), true),
        ("", false),
        (too_long.as_str(), false),
        ("has/slash", false),
        ("has space", false),
        ("has:colon", false),
        ("hås-unicode", false),
    ];

    for (name, accepted) in rows {
        let case = format!("stream name of {} byte(s)", name.len());
        let written = state.lock().put_fragment(
            name,
            REGION,
            b"chunk".to_vec(),
            at(BASE_MILLIS),
            at(BASE_MILLIS),
        );
        assert_eq!(
            written.is_some(),
            accepted,
            "case {case:?}: name {name:?} should be {}, got {written:?}",
            if accepted { "accepted" } else { "refused" }
        );
        let stored = state.lock().stream_by_name(name).map(|stream| stream.arn.clone());
        assert_eq!(
            stored.is_some(),
            accepted,
            "case {case:?}: name {name:?} left the wrong state behind, got {stored:?}"
        );
        if let Some(arn) = stored {
            assert!(
                arn.starts_with("arn:aws:kinesisvideo:")
                    && arn.contains(&format!(":stream/{name}/")),
                "case {case:?}: expected a stream ARN naming {name:?}, got {arn:?}"
            );
        }
    }

    // The region segment of the modelled `ResourceARN` is `[a-z0-9-]+`, so a signing
    // region outside it creates no stream rather than an ARN AWS would never mint.
    let regions: Vec<(&str, bool)> = vec![
        ("us-east-1", true),
        ("ap-southeast-4", true),
        ("", false),
        ("US-EAST-1", false),
        ("us_east_1", false),
        ("us/east/1", false),
    ];
    for (region, accepted) in regions {
        let case = format!("region {region:?}");
        let stream = format!("stream-for-{}", region.len());
        let created = state
            .lock()
            .ensure_stream(&stream, region, at(BASE_MILLIS))
            .map(|stream| stream.arn.clone());
        assert_eq!(
            created.is_some(),
            accepted,
            "case {case:?}: should be {}, got {created:?}",
            if accepted { "accepted" } else { "refused" }
        );
        if let Some(arn) = created {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {case:?}: expected the region in the ARN, got {arn:?}"
            );
        }
    }
}

/// One session carries a bounded amount of media and then stops, with the last chunk's
/// continuation token resuming exactly there — no fragment delivered twice and none
/// skipped across the two calls.
#[tokio::test]
async fn a_session_is_bounded_and_resumes_where_it_stopped() {
    // Three fragments of 10 MB: the session budget (25 MB) fits two of them, so the
    // third must wait for the follow-up call.
    let big: Vec<u8> = vec![7u8; 10_000_000];
    let chunks: Vec<&[u8]> = vec![&big, &big, &big];

    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    seed(&engine, "big-stream", &chunks);
    let router = router(&engine);

    let first = get_media(
        &router,
        &by_name("big-stream", serde_json::json!({"StartSelectorType": "EARLIEST"})),
    )
    .await;
    assert_eq!(
        status_of(&first),
        200,
        "case \"bounded session\": expected 200, got {}",
        status_of(&first)
    );
    let delivered = split_payload("bounded session", first.body(), &chunks[..2]);
    let numbers: Vec<String> = delivered
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect();
    assert_eq!(
        numbers,
        vec!["1".to_string(), "2".to_string()],
        "case \"bounded session\": expected the session to stop at the budget, got {numbers:?}"
    );

    let token = delivered
        .last()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .unwrap_or_default();
    let second = get_media(
        &router,
        &by_name(
            "big-stream",
            serde_json::json!({
                "StartSelectorType": "CONTINUATION_TOKEN",
                "ContinuationToken": token,
            }),
        ),
    )
    .await;
    assert_eq!(
        status_of(&second),
        200,
        "case \"bounded session resumed\": expected 200, got {}",
        status_of(&second)
    );
    let resumed = split_payload("bounded session resumed", second.body(), &chunks[2..]);
    let numbers: Vec<String> = resumed
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect();
    assert_eq!(
        numbers,
        vec!["3".to_string()],
        "case \"bounded session resumed\": expected only the fragment the first session \
         stopped before, got {numbers:?}"
    );
}
