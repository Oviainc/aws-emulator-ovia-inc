// SPDX-License-Identifier: CC0-1.0

//! The VIRTUAL-HOSTED half of the object-scoped refusal table: a request that names its
//! bucket in the host names its object in the whole path, and the refusal's envelope names
//! that same key.
//!
//! "Both the virtual-hosted-style requests and the path-style requests are supported"
//! (`GetObject`'s own documentation): a path-style request carries the bucket as the first
//! segment of its path, and a virtual-hosted-style one carries it as the first label of its
//! host — `https://bucket-name.s3.region-code.amazonaws.com/key-name` — leaving the whole
//! path to be the key. Every SDK's URL presigner writes the second form, so a client reading
//! an object through a presigned URL addresses it that way; the `<Key>` an object-scoped 404
//! names must therefore be the same key in either form, which is what this suite drives.
//!
//! The rows, the setup and the envelope assertion are
//! [`super::object_scoped_refusal_envelope`]'s own, so the two address forms are judged by
//! one table and one rule rather than by two copies of either.

use super::bucket_lifecycle::{router, send_bytes_to_host};
use super::object_scoped_refusal_envelope::{
    assert_names_object, bucket_holding, key_spelling_rows,
};
use service_com_amazonaws_s3_amazons3::state::NO_SUCH_KEY_CODE;

#[tokio::test]
async fn a_virtual_hosted_request_is_refused_about_the_key_its_whole_path_names() {
    let router = router();
    let name = "object-envelope-host-style";
    bucket_holding(&router, name).await;

    for (case, spelled, key) in key_spelling_rows() {
        let answer = send_bytes_to_host(
            &router,
            "GET",
            &format!("{name}.s3.us-east-1.amazonaws.com"),
            &format!("/{spelled}?tagging"),
            &[],
            b"",
        )
        .await;
        assert_names_object(
            &format!("{case} addressed virtual-hosted"),
            &answer,
            NO_SUCH_KEY_CODE,
            key,
            None,
        );
    }
}
