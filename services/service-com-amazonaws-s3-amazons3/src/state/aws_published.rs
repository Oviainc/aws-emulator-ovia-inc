// SPDX-License-Identifier: CC0-1.0

//! The AWS-PUBLISHED buckets this emulator's S3 holds without any caller creating them.
//!
//! Not every bucket in S3's global namespace belongs to a caller. AWS publishes a few of
//! its own, world-readable, and its service documentation points clients at them for the
//! sample data a tutorial or a getting-started flow trains and deploys from. Such a bucket
//! EXISTS before the first request this emulator serves — nobody creates it, and no
//! `CreateBucket` is what brings it into being — so a read addressed to one resolves the
//! bucket and answers from the object it holds, while a read addressed to a name nothing
//! published and nobody created still reports no such bucket.
//!
//! This catalog is what makes that true here: [`ComAmazonawsS3Amazons3State::default`]
//! records every bucket below and stores every object listed under it, so every reader of
//! this service's object store — `GetObject`, `HeadObject`, `ListObjectsV2`, `HeadBucket`
//! — serves them through exactly the path it serves a caller's own bucket with, and none
//! of them carries a case of its own for a managed name.
//!
//! [`ComAmazonawsS3Amazons3State::default`]: super::ComAmazonawsS3Amazons3State
//!
//! Two things this is NOT. It is not the account's: a bucket here is AWS's, so
//! `super::ComAmazonawsS3Amazons3State::account_buckets` leaves it out of what reports
//! "the buckets in your account", a `CreateBucket` of its name is a name that is not
//! available rather than a bucket the caller already owns, and a `DeleteBucket` of it is
//! refused — the places that already decide who a taken name belongs to. What it does NOT
//! do is refuse a caller's WRITE: this service models no cross-account access control over
//! object writes (no operation checks an owner unless the request names
//! `ExpectedBucketOwner`), so a caller that writes over a published key is served like any
//! other writer and later reads of that key answer what it wrote.
//!
//! And it is not a mirror of the published bucket's whole contents: AWS's own copy holds
//! thousands of keys and this emulator serves data out of this process's memory, so the
//! catalog carries the keys a documented sample-data path reads, each with its own small
//! copy of the published dataset in the layout that key's file has.

/// One object an AWS-published bucket holds, as this emulator publishes it.
#[derive(Debug, Clone, Copy)]
pub struct PublishedObject {
    /// The object's key, exactly as the published path names it.
    pub key: &'static str,
    /// The `Content-Type` a read of it reports.
    pub content_type: &'static str,
    /// The object's bytes. Fixed at compile time, so the bytes the store is SEEDED with
    /// are the same in this process and the next: nothing derives them per run, and no
    /// read changes them.
    pub body: &'static str,
}

/// One bucket AWS publishes, with the objects it holds.
#[derive(Debug, Clone, Copy)]
pub struct PublishedBucket {
    /// The bucket's name in S3's global namespace.
    pub name: &'static str,
    /// The Region the bucket lives in, which is what a read of it reports as its location.
    pub region: &'static str,
    /// The objects the bucket holds, by key.
    pub objects: &'static [PublishedObject],
}

/// The AWS-managed bucket SageMaker's documentation reads sample training data from.
pub const SAGEMAKER_SAMPLE_FILES: &str = "sagemaker-sample-files";

/// The `Content-Type` a comma-separated dataset is published with (RFC 4180's registered
/// media type for comma-separated values).
const CSV_CONTENT_TYPE: &str = "text/csv";

/// Every bucket AWS publishes that this emulator holds.
///
/// Walked once, by the store's `Default`, which records each bucket and stores each
/// object under it.
pub const PUBLISHED_BUCKETS: &[PublishedBucket] = &[PublishedBucket {
    name: SAGEMAKER_SAMPLE_FILES,
    // The bucket is reached at the global S3 endpoint, which is the `us-east-1` one; it is
    // the Region a bucket with no location constraint lives in here too
    // (`super::DEFAULT_BUCKET_REGION`).
    region: super::DEFAULT_BUCKET_REGION,
    objects: UCI_ABALONE_OBJECTS,
}];

/// The published `uci_abalone` dataset splits, under the prefix the sample-data path names
/// them by: a `train_csv/`, a `validation_csv/` and a `test_csv/` directory, each holding
/// the split's own file.
///
/// The three differ in exactly what a split differs in — how many records it carries, and
/// which — so each key answers with its own body, its own length and its own entity tag.
const UCI_ABALONE_OBJECTS: &[PublishedObject] = &[
    PublishedObject {
        key: "datasets/tabular/uci_abalone/train_csv/abalone_dataset1_train.csv",
        content_type: CSV_CONTENT_TYPE,
        body: ABALONE_TRAIN_CSV,
    },
    PublishedObject {
        key: "datasets/tabular/uci_abalone/validation_csv/abalone_dataset1_validation.csv",
        content_type: CSV_CONTENT_TYPE,
        body: ABALONE_VALIDATION_CSV,
    },
    PublishedObject {
        key: "datasets/tabular/uci_abalone/test_csv/abalone_dataset1_test.csv",
        content_type: CSV_CONTENT_TYPE,
        body: ABALONE_TEST_CSV,
    },
];

// The three bodies below are the public UCI Abalone records (the dataset the published
// `uci_abalone` files carry), written in the layout a split's CSV file has: no header row,
// the LABEL first — the measured number of rings — then the features, the three-way sex
// indicator one-hot encoded in the order the dataset lists its values (F, I, M) and then
// the seven body measurements (length, diameter, height, whole weight, shucked weight,
// viscera weight, shell weight). Label first is what lets a reader split a row into its
// label and its feature vector by taking the first field, which is what a training job
// configured for CSV input expects.

/// The `train_csv` split: the records a training job reads.
const ABALONE_TRAIN_CSV: &str = "\
15,0,0,1,0.455,0.365,0.095,0.514,0.2245,0.101,0.15\n\
7,0,0,1,0.35,0.265,0.09,0.2255,0.0995,0.0485,0.07\n\
9,1,0,0,0.53,0.42,0.135,0.677,0.2565,0.1415,0.21\n\
10,0,0,1,0.44,0.365,0.125,0.516,0.2155,0.114,0.155\n\
7,0,1,0,0.33,0.255,0.08,0.205,0.0895,0.0395,0.055\n\
8,0,1,0,0.425,0.3,0.095,0.3515,0.141,0.0775,0.12\n\
20,1,0,0,0.53,0.415,0.15,0.7775,0.237,0.1415,0.33\n\
16,1,0,0,0.545,0.425,0.125,0.768,0.294,0.1495,0.26\n\
9,0,0,1,0.475,0.37,0.125,0.5095,0.2165,0.1125,0.165\n\
19,1,0,0,0.55,0.44,0.15,0.8945,0.3145,0.151,0.32\n\
14,1,0,0,0.525,0.38,0.14,0.6065,0.194,0.1475,0.21\n\
10,0,0,1,0.43,0.35,0.11,0.406,0.1675,0.081,0.135\n\
11,0,0,1,0.49,0.38,0.135,0.5415,0.2175,0.095,0.19\n\
10,1,0,0,0.535,0.405,0.145,0.6845,0.2725,0.171,0.205\n";

/// The `validation_csv` split: the records a training job validates against.
const ABALONE_VALIDATION_CSV: &str = "\
10,1,0,0,0.47,0.355,0.1,0.4755,0.1675,0.0805,0.185\n\
12,0,0,1,0.5,0.4,0.13,0.6645,0.258,0.133,0.24\n\
7,0,1,0,0.355,0.28,0.085,0.2905,0.095,0.0395,0.115\n";

/// The `test_csv` split: the records a deployed endpoint is invoked with.
const ABALONE_TEST_CSV: &str = "\
10,1,0,0,0.44,0.34,0.1,0.451,0.188,0.087,0.13\n\
7,0,0,1,0.365,0.295,0.08,0.2555,0.097,0.043,0.1\n\
9,0,0,1,0.45,0.32,0.1,0.381,0.1705,0.075,0.115\n";
