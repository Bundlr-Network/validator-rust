use std::fs;

use bundlr_sdk::{
    deep_hash::{DeepHashChunk, ONE_AS_BUFFER},
    deep_hash_sync::deep_hash_sync,
};
use clap::Parser;
use data_encoding::BASE64URL_NOPAD;
use jsonwebkey::JsonWebKey;
use openssl::{hash::MessageDigest, rsa::Padding, sign};

use validator::consts::BUNDLR_AS_BUFFER;
use validator::key_manager;

#[derive(Clone, Debug, Parser)]
struct Args {
    /// Path to JWK file containing Arweaver wallet
    #[clap(short = 'w', long)]
    wallet: String,

    /// Bundlr transaction ID
    #[clap(short = 't', long)]
    tx: String,

    #[clap(short = 's', long)]
    size: usize,

    #[clap(short = 'f', long)]
    fee: u128,

    #[clap(short = 'c', long)]
    currency: String,

    #[clap(short = 'b', long)]
    promised_block: u128,

    #[clap(short = 'v', long)]
    validator: String,
}

fn main() {
    let args = Args::parse();

    let (private_key, _, _) = {
        let wallet = fs::read_to_string(&args.wallet).expect("Failed to find wallet file");
        let jwk: JsonWebKey = wallet.parse().expect("Failed to parse wallet file");
        key_manager::split_jwk(&jwk)
    };

    let tx = args.tx;
    let size = args.size;
    let fee = args.fee;
    let currency = args.currency;
    let block = args.promised_block;
    let validator = args.validator;
    let signature_data = deep_hash_sync(DeepHashChunk::Chunks(vec![
        DeepHashChunk::Chunk(BUNDLR_AS_BUFFER.into()),
        DeepHashChunk::Chunk(ONE_AS_BUFFER.into()),
        DeepHashChunk::Chunk(tx.as_bytes().to_owned().into()),
        DeepHashChunk::Chunk(size.to_string().as_bytes().to_owned().into()),
        DeepHashChunk::Chunk(fee.to_string().as_bytes().to_owned().into()),
        DeepHashChunk::Chunk(currency.as_bytes().to_owned().into()),
        DeepHashChunk::Chunk(block.to_string().as_bytes().to_owned().into()),
        DeepHashChunk::Chunk(validator.as_bytes().to_owned().into()),
    ]))
    .unwrap();

    let (buf, len) = {
        let mut signer = sign::Signer::new(MessageDigest::sha256(), &private_key).unwrap();
        signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
        signer.update(&signature_data).unwrap();
        let mut buf = vec![0; 512];
        let len = signer.sign(&mut buf).unwrap();
        (buf, len)
    };

    let sig = BASE64URL_NOPAD.encode(&buf[0..len]);

    println!(
        r#"{{"id":"{}","size":{},"fee":"{}","currency":"{}","block":"{}","validator":"{}","signature":"{}"}}"#,
        &tx, size, fee, currency, &block, &validator, sig,
    );
}
