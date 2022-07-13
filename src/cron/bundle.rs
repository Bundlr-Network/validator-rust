extern crate diesel;

use super::arweave::{self, ArweaveContext};
use super::error::ValidatorCronError;
use super::slasher::vote_slash;
use super::transactions::get_transactions;
use crate::bundler::Bundler;
use crate::context::{ArweaveAccess, BundlerAccess};
use crate::cron::arweave::{Arweave, Transaction as ArweaveTx};
use crate::database::models::{Block, Epoch, NewBundle, NewTransaction};
use crate::database::queries::{self, *};
use crate::key_manager::KeyManagerAccess;
use crate::types::Validator;
use crate::{http, key_manager};
use awc::Client;
use bundlr_sdk::deep_hash_sync::{deep_hash_sync, ONE_AS_BUFFER};
use bundlr_sdk::verify::types::Item;
use bundlr_sdk::{deep_hash::DeepHashChunk, verify::file::verify_file_bundle};
use data_encoding::BASE64URL_NOPAD;
use log::{error, info};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct TxReceipt {
    block: u128,
    tx_id: String,
    signature: String,
}

pub async fn validate_bundler<Context, HttpClient, KeyManager>(
    ctx: &Context,
) -> Result<(), ValidatorCronError>
where
    Context: queries::QueryContext
        + arweave::ArweaveContext<HttpClient>
        + ArweaveAccess
        + BundlerAccess
        + KeyManagerAccess<KeyManager>,
    HttpClient: http::Client<Request = reqwest::Request, Response = reqwest::Response>,
    KeyManager: key_manager::KeyManager,
{
    let arweave = ctx.arweave();
    let bundler = ctx.bundler();
    let latest_transactions_response = arweave
        .get_latest_transactions(ctx, &bundler.address, Some(50), None)
        .await;

    let latest_transactions = match latest_transactions_response {
        Err(err) => {
            error!(
                "Error occurred while getting txs from bundler address: \n {}. Error: {}",
                bundler.address, err
            );
            return Err(ValidatorCronError::TxsFromAddressNotFound);
        }
        Ok((latest_transactions, _, _)) => latest_transactions,
    };

    for bundle in latest_transactions {
        let res = validate_bundle(ctx, arweave, &bundle).await;
        if let Err(err) = res {
            match err {
                ValidatorCronError::TxNotFound => todo!(),
                ValidatorCronError::AddressNotFound => todo!(),
                ValidatorCronError::TxsFromAddressNotFound => todo!(),
                ValidatorCronError::BundleNotInsertedInDB => todo!(),
                ValidatorCronError::TxInvalid => todo!(),
                ValidatorCronError::FileError => (),
            }
        }
    }

    Ok(())
}

async fn validate_bundle<Context, HttpClient, KeyManager>(
    ctx: &Context,
    arweave: &Arweave,
    bundle: &ArweaveTx,
) -> Result<(), ValidatorCronError>
where
    Context: queries::QueryContext
        + ArweaveContext<HttpClient>
        + BundlerAccess
        + KeyManagerAccess<KeyManager>,
    HttpClient: http::Client<Request = reqwest::Request, Response = reqwest::Response>,
    KeyManager: key_manager::KeyManager,
{
    let block_ok = check_bundle_block(bundle);
    let current_block = match block_ok {
        Err(err) => return Err(err),
        Ok(None) => return Ok(()),
        Ok(Some(block)) => block,
    };

    store_bundle(ctx, bundle, current_block)?;

    let path = match arweave.get_tx_data(ctx, &bundle.id).await {
        Ok(path) => path,
        Err(err) => {
            error!("File path error {:?}", err);
            return Err(ValidatorCronError::FileError);
        }
    };

    let bundle_txs = match verify_file_bundle(path.clone()).await {
        Err(r) => {
            error!("Error verifying bundle {}:", r);
            Vec::new()
        }
        Ok(v) => v,
    };

    info!(
        "{} transactions found in bundle {}",
        &bundle_txs.len(),
        &bundle.id
    );
    for bundle_tx in bundle_txs {
        let tx_receipt = verify_bundle_tx(ctx, &bundle_tx, Some(current_block)).await;
        if let Err(err) = tx_receipt {
            info!("Error found in transaction {} : {}", &bundle_tx.tx_id, err);
            return Err(ValidatorCronError::TxInvalid);
        }
    }
    info!("All transactions ok in bundle {}", &bundle.id);

    /*
    match std::fs::remove_file(path.clone()) {
        Ok(_r) => info!("Successfully deleted {}", path),
        Err(err) => error!("Error deleting file {} : {}", path, err),
    };
    */

    Ok(())
}

fn check_bundle_block(bundle: &ArweaveTx) -> Result<Option<u128>, ValidatorCronError> {
    let current_block = match bundle.block {
        Some(ref block) => block.height,
        None => {
            info!("Bundle {} not included in any block", &bundle.id);
            return Ok(None);
        }
    };

    info!("Bundle {} included in block {}", &bundle.id, current_block);
    Ok(Some(current_block))
}

fn store_bundle<Context>(
    ctx: &Context,
    bundle: &ArweaveTx,
    current_block: u128,
) -> Result<(), ValidatorCronError>
where
    Context: queries::QueryContext + BundlerAccess,
{
    let is_bundle_present = get_bundle(ctx, &bundle.id).is_ok();
    if !is_bundle_present {
        return match insert_bundle_in_db(
            ctx,
            NewBundle {
                id: bundle.id.clone(),
                owner_address: ctx.bundler().address.clone(),
                block_height: Block(current_block),
            },
        ) {
            Ok(()) => {
                info!("Bundle {} successfully stored", &bundle.id);
                Ok(())
            }
            Err(err) => {
                error!("Error when storing bundle {} : {}", &bundle.id, err);
                Err(ValidatorCronError::BundleNotInsertedInDB)
            }
        };
    }

    Ok(())
}

async fn verify_bundle_tx<Context, KeyManager>(
    ctx: &Context,
    bundle_tx: &Item,
    current_block: Option<u128>,
) -> Result<(), ValidatorCronError>
where
    Context: queries::QueryContext + KeyManagerAccess<KeyManager>,
    KeyManager: key_manager::KeyManager,
{
    // TODO: this code needs review, especially error handling for get_tx looks suspicious
    let tx = get_tx(ctx, &bundle_tx.tx_id).await;
    let mut tx_receipt: Option<TxReceipt> = None;
    if tx.is_ok() {
        let tx = tx.unwrap();
        tx_receipt = Some(TxReceipt {
            block: tx.block_promised.into(),
            tx_id: tx.id,
            signature: match std::str::from_utf8(&tx.signature.to_vec()) {
                Ok(v) => v.to_string(),
                Err(e) => panic!("Invalid UTF-8 seq: {}", e),
            },
        });
    } else {
        let peer_tx = tx_exists_on_peers(&bundle_tx.tx_id).await;
        if peer_tx.is_ok() {
            tx_receipt = Some(peer_tx.unwrap());
        }
    }

    match tx_receipt {
        Some(receipt) => {
            let tx_is_ok = verify_tx_receipt(ctx.get_key_manager(), &receipt).unwrap();
            // FIXME: don't use unwrap
            if tx_is_ok && receipt.block <= current_block.unwrap() {
                let tx = NewTransaction {
                    id: receipt.tx_id,
                    epoch: Epoch(0),
                    block_promised: receipt.block.into(),
                    block_actual: current_block.map(Block),
                    signature: receipt.signature.as_bytes().to_vec(),
                    validated: true,
                    bundle_id: Some(bundle_tx.tx_id.clone()),
                };
                if let Err(err) = insert_tx_in_db(ctx, &tx) {
                    error!("Error inserting new tx {}, Error: {}", tx.id, err);
                    // TODO: is it enough to log this error?
                }
            } else {
                // TODO: vote slash
            }
        }
        None => {
            // TODO: handle unfound txreceipt
        }
    }

    Ok(())
}

async fn tx_exists_on_peers(tx_id: &str) -> Result<TxReceipt, ValidatorCronError> {
    let client = Client::default();
    let validator_peers = Vec::<Validator>::new();
    for peer in validator_peers {
        let response = client
            .get(format!("{}/tx/{}", peer.url, tx_id))
            .send()
            .await;

        if let Err(r) = response {
            error!("Error occurred while getting tx from peer - {}", r);
            continue;
        }

        let mut response = response.unwrap();

        if response.status().is_success() {
            return Ok(response.json().await.unwrap());
        }
    }

    Err(ValidatorCronError::TxNotFound)
}

fn verify_tx_receipt<KeyManager>(
    key_manager: &KeyManager,
    tx_receipt: &TxReceipt,
) -> std::io::Result<bool>
where
    KeyManager: key_manager::KeyManager,
{
    pub const BUNDLR_AS_BUFFER: &[u8] = "Bundlr".as_bytes();

    let block = tx_receipt.block.to_string().as_bytes().to_vec();

    let tx_id = tx_receipt.tx_id.as_bytes().to_vec();

    let message = deep_hash_sync(DeepHashChunk::Chunks(vec![
        DeepHashChunk::Chunk(BUNDLR_AS_BUFFER.into()),
        DeepHashChunk::Chunk(ONE_AS_BUFFER.into()),
        DeepHashChunk::Chunk(tx_id.into()),
        DeepHashChunk::Chunk(block.into()),
    ]))
    .unwrap();

    let sig = BASE64URL_NOPAD
        .decode(tx_receipt.signature.as_bytes())
        .unwrap();

    Ok(key_manager.verify_bundler_signature(&message, &sig))
}

pub async fn validate_transactions(bundler: &Bundler) -> Result<(), ValidatorCronError> {
    let res = get_transactions(bundler, Some(100), None).await;
    let txs = match res {
        Ok(r) => r.0,
        Err(_) => Vec::new(),
    };

    for tx in txs {
        // TODO: validate transacitons
        let block_ok = tx.current_block < tx.expected_block;

        if block_ok {
            let _res = vote_slash(bundler);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::utils::get_file_as_byte_vector;
    use crate::{
        context::test_utils::test_context_with_http_client, http::reqwest::mock::MockHttpClient,
        key_manager::test_utils::test_keys,
    };
    use http::Method;
    use reqwest::{Request, Response};

    use super::validate_bundler;

    #[actix_rt::test]
    async fn validate_bundler_should_abort_due_no_block() {
        let client = MockHttpClient::new(|a: &Request, b: &Request| a.url() == b.url())
            .when(|req: &Request| {
                let url = "http://example.com/graphql?query=query($owners:%20[String!],%20$first:%20Int)%20{%20transactions(owners:%20$owners,%20first:%20$first)%20{%20pageInfo%20{%20hasNextPage%20}%20edges%20{%20cursor%20node%20{%20id%20owner%20{%20address%20}%20signature%20recipient%20tags%20{%20name%20value%20}%20block%20{%20height%20id%20timestamp%20}%20}%20}%20}%20}";
                req.method() == Method::POST && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let data = "{\"data\": {\"transactions\": {\"pageInfo\": {\"hasNextPage\": true },\"edges\": [{\"cursor\": \"cursor\", \"node\": { \"id\": \"tx_id\",\"owner\": {\"address\": \"address\"}, \"signature\": \"signature\",\"recipient\": \"\", \"tags\": [], \"block\": null } } ] } } }";
                let response = http::response::Builder::new()
                    .status(200)
                    .body(data)
                    .unwrap();
                Response::from(response)
            })
            .when(|req: &Request| {
                let url = "http://example.com/tx_id";
                req.method() == Method::GET && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let data = "";
                let response = http::response::Builder::new()
                    .status(200)
                    .body(data)
                    .unwrap();
                Response::from(response)
            });

        let (key_manager, _bundle_pvk) = test_keys();
        let ctx = test_context_with_http_client(key_manager, client);
        let res = validate_bundler(&ctx).await;
        assert!(res.is_ok())
    }

    #[actix_rt::test]
    async fn validate_bundler_should_return_ok() {
        let client = MockHttpClient::new(|a: &Request, b: &Request| a.url() == b.url())
            .when(|req: &Request| {
                let url = "http://example.com/graphql?query=query($owners:%20[String!],%20$first:%20Int)%20{%20transactions(owners:%20$owners,%20first:%20$first)%20{%20pageInfo%20{%20hasNextPage%20}%20edges%20{%20cursor%20node%20{%20id%20owner%20{%20address%20}%20signature%20recipient%20tags%20{%20name%20value%20}%20block%20{%20height%20id%20timestamp%20}%20}%20}%20}%20}";
                req.method() == Method::POST && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let data = "{\"data\": {\"transactions\": {\"pageInfo\": {\"hasNextPage\": true },\"edges\": [{\"cursor\": \"cursor\", \"node\": { \"id\": \"tx_id\",\"owner\": {\"address\": \"address\"}, \"signature\": \"signature\", \"recipient\": \"\", \"tags\": [], \"block\": { \"id\": \"id\", \"timestamp\": 10, \"height\": 10 } } } ] } } }";
                let response = http::response::Builder::new()
                    .status(200)
                    .body(data)
                    .unwrap();
                Response::from(response)
            })
            .when(|req: &Request| {
                let url = "http://example.com/tx_id";
                req.method() == Method::GET && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let buffer = get_file_as_byte_vector("./bundles/test_bundle").unwrap();
                let response = http::response::Builder::new()
                    .status(200)
                    .body(buffer)
                    .unwrap();
                Response::from(response)
            });

        let (key_manager, _bundle_pvk) = test_keys();
        let ctx = test_context_with_http_client(key_manager, client);
        let res = validate_bundler(&ctx).await;
        assert!(res.is_ok())
    }
}
