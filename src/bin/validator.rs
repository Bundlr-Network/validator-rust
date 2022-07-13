use clap::Parser;
use data_encoding::{DecodeError, BASE64URL_NOPAD};
use diesel::{
    r2d2::{self, ConnectionManager},
    PgConnection,
};
use env_logger::Env;
use jsonwebkey::{JsonWebKey, Key, PublicExponent, RsaPublic};
use log::info;
use serde::Deserialize;
use std::{fs, net::SocketAddr, str::FromStr};
use sysinfo::{System, SystemExt};
use url::Url;

use validator::{
    bundler::BundlerConfig,
    hardware::HardwareCheck,
    http::reqwest::ReqwestClient,
    key_manager::{InMemoryKeyManager, InMemoryKeyManagerConfig},
};
use validator::{context::AppContext, state::generate_state};
use validator::{cron::run_crons, server::run_server};

#[derive(Clone, Debug, Parser)]
struct CliOpts {
    /// Do not start cron jobs
    #[clap(long)]
    no_cron: bool,

    /// Do not start app in server mode
    #[clap(long)]
    no_server: bool,

    /// Database connection URL
    #[clap(long, env)]
    database_url: String,

    /// Listen address for the server
    #[clap(short, long, env, default_value = "127.0.0.1:42069")]
    listen: SocketAddr,

    /// URL for the bundler connection
    #[clap(long, env = "BUNDLER_URL")]
    bundler_url: Url,

    /// Path to JWK file holding validator private key
    #[clap(long, env = "VALIDATOR_KEY")]
    validator_key: String,

    #[clap(long, env = "ARWEAVE_URL")]
    arweave_url: Option<Url>,

    #[clap(long)]
    bundler_key: Option<Url>,

    #[clap(
        long,
        env = "CONTRACT_GATEWAY",
        default_value = "http://localhost:3000"
    )]
    contract_gateway_url: Url,
}

// TODO: merge config should return own type as returned arweave_url can never be None
fn merge_configs(config: CliOpts, bundler_config: BundlerConfig) -> CliOpts {
    let arweave_url = match config.arweave_url {
        Some(u) => Some(u),
        None => {
            let url_string = format!("https://{}", bundler_config.gateway);
            let url = url::Url::from_str(&url_string).expect("Invalid Arweave gateway URL");
            Some(url)
        }
    };

    CliOpts {
        arweave_url,
        ..config
    }
}

fn public_only_jwk_from_rsa_n(encoded_n: &str) -> Result<JsonWebKey, DecodeError> {
    Ok(JsonWebKey::new(Key::RSA {
        public: RsaPublic {
            e: PublicExponent,
            n: BASE64URL_NOPAD.decode(encoded_n.as_bytes())?.into(),
        },
        private: None,
    }))
}

struct Keys(JsonWebKey, JsonWebKey);

impl InMemoryKeyManagerConfig for Keys {
    fn bundler_jwk(&self) -> &JsonWebKey {
        &self.0
    }

    fn validator_jwk(&self) -> &JsonWebKey {
        &self.1
    }
}

#[derive(Deserialize)]
struct PublicResponse {
    n: String,
}

#[async_trait::async_trait]
pub trait IntoAsync<T> {
    async fn into_async(&self) -> T;
}

#[async_trait::async_trait]
impl IntoAsync<AppContext> for CliOpts {
    async fn into_async(&self) -> AppContext {
        let fmt_bundler_url: String = self.bundler_url.to_string().replace(&['\"', '\''][..], "");
        dbg!(&fmt_bundler_url);
        let n_response = reqwest::get(format!("{}public", fmt_bundler_url))
            .await
            .expect("Couldn't get public key from bundler")
            .text()
            .await
            .expect("Couldn't parse public key response from bundler");

        let public_response = serde_json::from_str::<PublicResponse>(&n_response).unwrap();
        let bundler_jwk =
            public_only_jwk_from_rsa_n(&public_response.n).expect("Failed to decode bundler key");

        let validator_jwk: JsonWebKey = {
            let file = fs::read_to_string(&self.validator_key).unwrap();
            file.parse().unwrap()
        };

        let key_manager = InMemoryKeyManager::new(&Keys(bundler_jwk, validator_jwk));
        let state = generate_state();

        let connection_mgr = ConnectionManager::<PgConnection>::new(&self.database_url);

        let pool = r2d2::Pool::builder()
            .build(connection_mgr)
            .expect("Failed to create database connection pool.");

        let arweave_url = match &self.arweave_url {
            Some(url) => url,
            None => unreachable!(),
        };

        AppContext::new(
            key_manager,
            pool,
            self.listen,
            state,
            reqwest::Client::new(),
            arweave_url,
            &self.bundler_url,
            &self.contract_gateway_url,
        )
    }
}

fn main() -> () {
    actix_rt::System::new().block_on(async {
        let sys = System::new_all();
        System::print_hardware_info(&sys);
        // let enough_resources = System::has_enough_resources(&sys);
        // if !enough_resources {
        //     println!("Hardware check failed: Not enough resources, check Readme file");
        //     process::exit(1);
        // }

        dotenv::dotenv().ok();

        env_logger::init_from_env(Env::default().default_filter_or("info"));

        let http_client = ReqwestClient::new(reqwest::Client::new());
        let app_config = CliOpts::parse();
        let bundler_config =
            BundlerConfig::fetch_config(http_client, &app_config.bundler_url).await;
        let config = merge_configs(app_config, bundler_config);
        let ctx = config.into_async().await;

        if !config.no_cron {
            info!("Running with cron");
            tokio::task::spawn_local(run_crons(ctx.clone()));
        };

        if !config.no_server {
            info!("Running with server");
            run_server(ctx.clone()).await.unwrap()
        };
    });
}

#[cfg(test)]
mod tests {
    use crate::public_only_jwk_from_rsa_n;

    #[test]
    fn when_building_jwk_from_encoded_public_key_then_serialized_n_matches() {
        let encoded_n = "sq9JbppKLlAKtQwalfX5DagnGMlTirditXk7y4jgoeA7DEM0Z6cVPE5xMQ9kz_T9VppP6BFHtHyZCZODercEVWipzkr36tfQkR5EDGUQyLivdxUzbWgVkzw7D27PJEa4cd1Uy6r18rYLqERgbRvAZph5YJZmpSJk7r3MwnQquuktjvSpfCLFwSxP1w879-ss_JalM9ICzRi38henONio8gll6GV9-omrWwRMZer_15bspCK5txCwpY137nfKwKD5YBAuzxxcj424M7zlSHlsafBwaRwFbf8gHtW03iJER4lR4GxeY0WvnYaB3KDISHQp53a9nlbmiWO5WcHHYsR83OT2eJ0Pl3RWA-_imk_SNwGQTCjmA6tf_UVwL8HzYS2iyuu85b7iYK9ZQoh8nqbNC6qibICE4h9Fe3bN7AgitIe9XzCTOXDfMr4ahjC8kkqJ1z4zNAI6-Leei_Mgd8JtZh2vqFNZhXK0lSadFl_9Oh3AET7tUds2E7s-6zpRPd9oBZu6-kNuHDRJ6TQhZSwJ9ZO5HYsccb_G_1so72aXJymR9ggJgWr4J3bawAYYnqmvmzGklYOlE_5HVnMxf-UxpT7ztdsHbc9QEH6W2bzwxbpjTczEZs3JCCB3c-NewNHsj9PYM3b5tTlTNP9kNAwPZHWpt11t79LuNkNGt9LfOek";

        let jwk = public_only_jwk_from_rsa_n(encoded_n).expect("Failed to decode public key");

        let json_str = serde_json::to_string(&jwk).unwrap();

        let json: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let n = json.get("n").unwrap().as_str().unwrap();

        assert_eq!(encoded_n, n);
    }
}
