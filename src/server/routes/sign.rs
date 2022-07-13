use actix_web::{
    web::{Data, Json},
    HttpResponse,
};
use bundlr_sdk::deep_hash::{deep_hash, DeepHashChunk, ONE_AS_BUFFER};

use data_encoding::BASE64URL_NOPAD;
use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use log::error;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    consts::{BUNDLR_AS_BUFFER, VALIDATOR_AS_BUFFER},
    database::{
        models::{Epoch, NewTransaction},
        schema::transactions::dsl::*,
    },
    key_manager,
    server::{error::ValidatorServerError, RuntimeContext},
    state::{ValidatorRole, ValidatorStateAccess},
};

pub trait Config<KeyManager>: ValidatorStateAccess
where
    KeyManager: key_manager::KeyManager,
{
    fn bundler_address(&self) -> &str;
    fn validator_address(&self) -> &str;
    fn key_manager(&self) -> &KeyManager;
    fn current_epoch(&self) -> u128;
    fn current_block(&self) -> u128;
}

/// Deserializer from string to u128
fn de_u128<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
    let s: &str = de::Deserialize::deserialize(deserializer)?;
    s.parse().map_err(de::Error::custom)
}

/// Serialize as string
fn ser_as_string<S, T>(val: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: ToString,
{
    serializer.serialize_str(&val.to_string())
}

#[derive(Deserialize, Serialize)]
pub struct SignRequest {
    id: String,
    size: usize,
    #[serde(deserialize_with = "de_u128", serialize_with = "ser_as_string")]
    fee: u128,
    currency: String,
    #[serde(deserialize_with = "de_u128", serialize_with = "ser_as_string")]
    block: u128,
    validator: String,
    signature: String,
}

impl SignRequest {
    // FIXME: needs proper error type
    pub async fn verify<KeyManager>(&self, key_manager: &KeyManager) -> Result<bool, ()>
    where
        KeyManager: key_manager::KeyManager,
    {
        // FIXME: fix lifetimes in DeepHashChunk::Chunk and deep_hash to avoid copying the data
        let signature_data = deep_hash(DeepHashChunk::Chunks(vec![
            DeepHashChunk::Chunk(BUNDLR_AS_BUFFER.into()),
            DeepHashChunk::Chunk(ONE_AS_BUFFER.into()),
            DeepHashChunk::Chunk(self.id.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.size.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.fee.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.currency.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.block.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.validator.as_bytes().to_owned().into()),
        ]))
        .await
        .map_err(|err| {
            error!("Failed to build data for signing: {:?}", err);
        })?;

        let decoded_signature =
            BASE64URL_NOPAD
                .decode(self.signature.as_bytes())
                .map_err(|err| {
                    error!("Failed to decode signature: {:?}", err);
                })?;

        Ok(key_manager.verify_bundler_signature(&signature_data, &decoded_signature))
    }

    pub async fn sign<KeyManager>(&self, key_manager: &KeyManager) -> Result<String, ()>
    where
        KeyManager: key_manager::KeyManager,
    {
        let bundler_address = key_manager.bundler_address().to_string();

        let signature_data = deep_hash(DeepHashChunk::Chunks(vec![
            DeepHashChunk::Chunk(VALIDATOR_AS_BUFFER.into()),
            DeepHashChunk::Chunk(ONE_AS_BUFFER.into()),
            DeepHashChunk::Chunk(self.id.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.size.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.fee.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.currency.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.block.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(self.validator.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(bundler_address.as_bytes().to_owned().into()),
        ]))
        .await
        .map_err(|err| {
            error!("Failed to build data for signing: {:?}", err);
        })?;

        Ok(BASE64URL_NOPAD.encode(&key_manager.validator_sign(&signature_data)))
    }
}

pub async fn sign_route<Context, KeyManager>(
    ctx: Data<Context>,
    body: Json<SignRequest>,
) -> actix_web::Result<HttpResponse, ValidatorServerError>
where
    Context: self::Config<KeyManager> + RuntimeContext + Send,
    KeyManager: key_manager::KeyManager,
{
    // FIXME: checking role should be a function of block height
    // we probably need to track role for previous, current and next epochs
    // to allow bundler node and this validator to see new blocks at different
    // times
    if ctx.get_validator_state().role() != ValidatorRole::Cosigner {
        return Ok(HttpResponse::BadRequest().finish());
    }

    let body = body.into_inner();

    // Verify
    let exists = {
        let conn = ctx.get_db_connection();
        let filter = id.eq(body.id.clone());
        actix_rt::task::spawn_blocking(move || {
            match transactions.filter(filter).count().get_result(&conn) {
                Ok(0) => Ok(false),
                Ok(_) => Ok(true),
                Err(err) => Err(err),
            }
        })
    };

    if let Ok(true) = exists
        .await
        .map_err(|_| ValidatorServerError::InternalError)?
    {
        return Ok(HttpResponse::Accepted().finish());
    }

    let current_block = ctx.current_block();
    let key_manager = ctx.key_manager();

    if body.validator != *ctx.validator_address() {
        return Ok(HttpResponse::BadRequest().body("Invalid validator address"));
    }

    // Check that the body.block is not too far in the past nor too far in the future
    match body.block.cmp(&current_block) {
        std::cmp::Ordering::Less => {
            return Ok(HttpResponse::BadRequest().body("Invalid block number"))
        }
        std::cmp::Ordering::Greater if body.block - current_block < 395 => {
            return Ok(HttpResponse::BadRequest().body("Invalid block number"))
        }
        std::cmp::Ordering::Greater if body.block - current_block > 405 => {
            return Ok(HttpResponse::BadRequest().body("Invalid block number"))
        }
        _ => (),
    }

    match body.verify(key_manager).await {
        Ok(true) => (),
        Ok(false) => return Ok(HttpResponse::BadRequest().body("Invalid bundler signature")),
        Err(()) => return Err(ValidatorServerError::InternalError),
    };

    // Sign
    let sig = match body.sign(key_manager).await {
        Ok(sig) => sig,
        Err(()) => return Err(ValidatorServerError::InternalError),
    };

    // Add to db
    let current_epoch = ctx.current_epoch();

    let new_transaction = NewTransaction {
        id: body.id,
        epoch: Epoch(current_epoch),
        block_promised: body.block.into(),
        block_actual: None,
        signature: sig.as_bytes().to_vec(),
        validated: false,
        bundle_id: None,
    };

    let conn = ctx.get_db_connection();
    actix_rt::task::spawn_blocking(move || {
        diesel::insert_into(transactions)
            .values::<NewTransaction>(new_transaction)
            .execute(&conn)
    })
    .await??;

    Ok(HttpResponse::Ok()
        .insert_header(("Content-Type", "application/octet-stream"))
        .body(sig.as_bytes().to_vec()))
}

#[cfg(test)]
mod tests {
    use actix_web::{
        http::header::ContentType,
        test::{call_service, init_service, TestRequest},
        web::{self, Data},
        App,
    };
    use bundlr_sdk::{
        deep_hash::{DeepHashChunk, ONE_AS_BUFFER},
        deep_hash_sync::deep_hash_sync,
    };
    use data_encoding::BASE64URL_NOPAD;
    use openssl::{
        hash::MessageDigest,
        pkey::{PKey, Private},
        rsa::Padding,
        sign,
    };
    use reqwest::StatusCode;

    use crate::{
        consts::{BUNDLR_AS_BUFFER, VALIDATOR_AS_BUFFER},
        context::{test_utils::test_context, AppContext},
        http::reqwest::mock::MockHttpClient,
        key_manager::{test_utils::test_keys, KeyManager},
        server::routes::sign::{sign_route, Config},
        state::ValidatorStateAccess,
    };

    use super::SignRequest;
    fn test_message(
        signing_key: &PKey<Private>,
        block: u128,
        validator: String,
        tx: &str,
    ) -> SignRequest {
        let size = 0usize;
        let fee = 0u128;
        let currency = "FOO";
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
            let mut signer = sign::Signer::new(MessageDigest::sha256(), &signing_key).unwrap();
            signer.set_rsa_padding(Padding::PKCS1_PSS).unwrap();
            signer.update(&signature_data).unwrap();
            let mut buf = vec![0; 512];
            let len = signer.sign(&mut buf).unwrap();
            (buf, len)
        };

        let sig = BASE64URL_NOPAD.encode(&buf[0..len]);

        SignRequest {
            id: tx.to_owned(),
            size,
            fee,
            currency: currency.to_owned(),
            block,
            validator,
            signature: sig,
        }
    }

    #[actix_rt::test]
    async fn verify_unsigned_body() {
        let (key_manager, bundler_private_key) = test_keys();

        let msg = test_message(
            &bundler_private_key,
            500,
            key_manager.validator_address().to_string(),
            "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z1",
        );

        assert!(msg.verify(&key_manager).await.unwrap())
    }

    #[actix_rt::test]
    async fn sign_unsigned_body() {
        // TODO: use pre-generated keys
        // instead of using random keys, use pre-generated ones so that
        // the signature can be verified against expected value

        let (key_manager, bundler_private_key) = test_keys();

        let msg = test_message(
            &bundler_private_key,
            500,
            key_manager.validator_address().to_string(),
            "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z2",
        );

        let sig = msg.sign(&key_manager).await.unwrap();

        let bundler_address = key_manager.bundler_address().to_string();
        let signature_data = deep_hash_sync(DeepHashChunk::Chunks(vec![
            DeepHashChunk::Chunk(VALIDATOR_AS_BUFFER.into()),
            DeepHashChunk::Chunk(ONE_AS_BUFFER.into()),
            DeepHashChunk::Chunk(msg.id.into()),
            DeepHashChunk::Chunk(msg.size.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(msg.fee.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(msg.currency.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(msg.block.to_string().as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(msg.validator.as_bytes().to_owned().into()),
            DeepHashChunk::Chunk(bundler_address.as_bytes().to_owned().into()),
        ]))
        .unwrap();

        let decoded_signature = BASE64URL_NOPAD.decode(sig.as_bytes()).unwrap();
        assert!(key_manager.verify_validator_signature(&signature_data, &decoded_signature));
    }

    #[actix_web::test]
    async fn valid_sign_request_returns_valid_validator_signature() {
        let (key_manager, bundler_private_key) = crate::key_manager::test_utils::test_keys();
        let ctx = test_context(key_manager);

        let app = App::new().app_data(Data::new(ctx.clone())).route(
            "/",
            web::post().to(sign_route::<AppContext<MockHttpClient>, _>),
        );

        let app = init_service(app).await;

        let msg = test_message(
            &bundler_private_key,
            400,
            ctx.key_manager().validator_address().to_string(),
            "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z3",
        );

        let req = TestRequest::post()
            .uri("/")
            .insert_header(ContentType::json())
            .set_json(msg)
            .to_request();

        let res = call_service(&app, req).await;
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "Failed: {:?}",
            res.into_body()
        );
    }

    #[actix_web::test]
    async fn block_number_too_far_ahead_yields_bad_request() {
        let (key_manager, bundler_private_key) = crate::key_manager::test_utils::test_keys();
        let ctx = test_context(key_manager);

        let app = App::new().app_data(Data::new(ctx.clone())).route(
            "/",
            web::post().to(sign_route::<AppContext<MockHttpClient>, _>),
        );

        let app = init_service(app).await;

        let msg = test_message(
            &bundler_private_key,
            406,
            ctx.key_manager().validator_address().to_string(),
            "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z4",
        );

        let req = TestRequest::post()
            .uri("/")
            .insert_header(ContentType::json())
            .set_json(msg)
            .to_request();

        let res = call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST,);
    }

    #[actix_web::test]
    async fn block_number_too_far_behind_yields_bad_request() {
        let (key_manager, bundler_private_key) = crate::key_manager::test_utils::test_keys();
        let ctx = test_context(key_manager);
        ctx.get_validator_state().set_current_block(6);

        let app = App::new().app_data(Data::new(ctx.clone())).route(
            "/",
            web::post().to(sign_route::<AppContext<MockHttpClient>, _>),
        );

        let app = init_service(app).await;

        let msg = test_message(
            &bundler_private_key,
            400,
            ctx.key_manager().validator_address().to_string(),
            "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z5",
        );

        let req = TestRequest::post()
            .uri("/")
            .insert_header(ContentType::json())
            .set_json(msg)
            .to_request();

        let res = call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST,);
    }

    #[actix_web::test]
    async fn wrong_bundler_signature_yields_bad_request() {
        let (key_manager, _) = crate::key_manager::test_utils::test_keys();
        let ctx = test_context(key_manager);

        let app = App::new().app_data(Data::new(ctx.clone())).route(
            "/",
            web::post().to(sign_route::<AppContext<MockHttpClient>, _>),
        );

        let app = init_service(app).await;

        let msg = {
            let (_, wrong_key) = crate::key_manager::test_utils::test_keys();
            test_message(
                &wrong_key,
                400,
                ctx.key_manager().validator_address().to_string(),
                "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z6",
            )
        };

        let req = TestRequest::post()
            .uri("/")
            .insert_header(ContentType::json())
            .set_json(msg)
            .to_request();

        let res = call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST,);
    }

    #[actix_web::test]
    async fn wrong_validator_address_yields_bad_request() {
        let (key_manager, bundler_private_key) = crate::key_manager::test_utils::test_keys();
        let ctx = test_context(key_manager);

        let app = App::new().app_data(Data::new(ctx.clone())).route(
            "/",
            web::post().to(sign_route::<AppContext<MockHttpClient>, _>),
        );

        let app = init_service(app).await;

        let msg = {
            test_message(
                &bundler_private_key,
                400,
                // Use bundler address, main point is to use any other address,
                // but validator's correct one
                ctx.key_manager().bundler_address().to_string(),
                "dtdOmHZMOtGb2C0zLqLBUABrONDZ5rzRh9NengT1-Z7",
            )
        };

        let req = TestRequest::post()
            .uri("/")
            .insert_header(ContentType::json())
            .set_json(msg)
            .to_request();

        let res = call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST,);
    }
}
