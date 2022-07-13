use log::error;
use log::info;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use url::Url;

use crate::context::ArweaveAccess;
use crate::http::Client;
use crate::state::ValidatorStateAccess;

#[derive(Deserialize, Serialize, Clone)]
pub struct NetworkInfo {
    pub network: String,
    pub version: usize,
    pub release: usize,
    pub height: u128,
    pub current: String,
    pub blocks: usize,
    pub peers: usize,
    pub queue_length: usize,
    pub node_state_latency: usize,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct Tag {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct Owner {
    pub address: String,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct Fee {
    winston: String,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct TransactionData {
    size: String,
    r#type: Option<String>,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct BlockInfo {
    pub id: String,
    pub timestamp: i64,
    pub height: u128,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct Transaction {
    pub id: String,
    pub owner: Owner,
    pub signature: String,
    pub recipient: Option<String>,
    pub tags: Vec<Tag>,
    pub block: Option<BlockInfo>,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct GraphqlNodes {
    pub node: Transaction,
    pub cursor: String,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GraphqlEdges {
    pub edges: Vec<GraphqlNodes>,
    pub page_info: PageInfo,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct TransactionsGqlResponse {
    pub transactions: GraphqlEdges,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
pub struct GraphqlQueryResponse {
    pub data: TransactionsGqlResponse,
}

#[derive(Deserialize, Serialize, Default, Clone)]
pub struct TransactionStatus {
    pub block_indep_hash: String,
}

use derive_more::{Display, Error};
use std::convert::From;

use super::CronJobError;

#[derive(Debug, Display, Error, Clone, PartialEq)]
pub enum ArweaveError {
    TxsNotFound,
    MalformedQuery,
    InternalServerError,
    GatewayTimeout,
    UnknownErr,
}

impl From<anyhow::Error> for ArweaveError {
    fn from(_err: anyhow::Error) -> ArweaveError {
        ArweaveError::UnknownErr
    }
}

#[derive(Clone)]
pub enum ArweaveProtocol {
    Http,
    Https,
}

#[derive(Clone)]
pub struct Arweave {
    pub url: Url,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct GqlVariables {
    pub owners: Vec<String>,
    pub first: u128,
    pub after: Option<String>,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct ReqBody {
    pub query: String,
    pub variables: GqlVariables,
}

pub trait ArweaveContext<HttpClient>
where
    HttpClient: crate::http::Client<Request = reqwest::Request, Response = reqwest::Response>,
{
    fn get_client(&self) -> &HttpClient;
}

#[warn(dead_code)]
impl Arweave {
    pub fn new(url: Url) -> Arweave {
        Arweave { url }
    }

    pub async fn get_network_info<Context, HttpClient>(
        &self,
        ctx: &Context,
    ) -> reqwest::Result<NetworkInfo>
    where
        Context: ArweaveContext<HttpClient>,
        HttpClient: Client<Request = reqwest::Request, Response = reqwest::Response>,
    {
        info!("Fetch network info");
        let uri = http::uri::Uri::from_str(&format!("{}info", self.get_host())).unwrap();
        let req: http::Request<String> = http::request::Builder::new()
            .method(http::Method::GET)
            .uri(uri)
            .body("".to_string())
            .unwrap();

        let req: reqwest::Request = reqwest::Request::try_from(req).unwrap();
        let res: reqwest::Response = ctx.get_client().execute(req).await.expect("request failed"); // FIXME: should not panic, handle failure
        if res.status().is_success() {
            return res.json().await;
        } else {
            Err(res.error_for_status().err().unwrap()) // FIXME: do not unwrap
        }
    }

    pub async fn get_tx_data<Context, HttpClient>(
        &self,
        ctx: &Context,
        transaction_id: &str,
    ) -> reqwest::Result<String>
    where
        Context: ArweaveContext<HttpClient>,
        HttpClient: Client<Request = reqwest::Request, Response = reqwest::Response>,
    {
        info!("Downloading bundle {} content ...", &transaction_id);
        let raw_path = format!("./bundles/{}", transaction_id);
        let file_path = Path::new(&raw_path);
        let mut buffer = File::create(&file_path).unwrap(); // FIXME: change to expect

        let uri =
            http::uri::Uri::from_str(&format!("{}{}", self.get_host(), transaction_id)).unwrap();
        let req: http::Request<String> = http::request::Builder::new()
            .method(http::Method::GET)
            .uri(uri)
            .body("".to_string())
            .unwrap();

        let req: reqwest::Request = reqwest::Request::try_from(req).unwrap();
        let mut res: reqwest::Response =
            ctx.get_client().execute(req).await.expect("request failed"); // FIXME: should not panic, handle failure
        if res.status().is_success() {
            while let Some(chunk) = res.chunk().await? {
                match buffer.write(&chunk) {
                    Ok(_) => {}
                    Err(err) => {
                        error!("Error writing on file {:?}: {:?}", file_path.to_str(), err)
                    }
                }
            }
            info!("Downloaded {} content!", &transaction_id);
            return Ok(String::from(file_path.to_string_lossy()));
        } else {
            Err(res.error_for_status().err().unwrap()) // FIXME: do not unwrap
        }
    }

    pub async fn get_latest_transactions<Context, HttpClient>(
        &self,
        ctx: &Context,
        owner: &str,
        first: Option<i64>,
        after: Option<String>,
    ) -> Result<(Vec<Transaction>, bool, Option<String>), ArweaveError>
    where
        Context: ArweaveContext<HttpClient>,
        HttpClient: Client<Request = reqwest::Request, Response = reqwest::Response>,
    {
        let raw_query = "query($owners: [String!], $first: Int) { transactions(owners: $owners, first: $first) { pageInfo { hasNextPage } edges { cursor node { id owner { address } signature recipient tags { name value } block { height id timestamp } } } } }";
        let raw_variables = format!(
            "{{\"owners\": [\"{}\"], \"first\": {}, \"after\": {}}}",
            owner,
            first.unwrap_or(10),
            match after {
                None => r"null".to_string(),
                Some(a) => a,
            }
        );

        let url = format!("{}graphql?query={}", self.get_host(), raw_query);

        // TODO: why to build object by parsing from string and then turn it later back to string
        let data = format!(
            "{{\"query\":\"{}\",\"variables\":{}}}",
            raw_query, raw_variables
        );

        let reqwest_client = reqwest::Client::new();
        let body = serde_json::from_str::<ReqBody>(&data);
        let req = reqwest_client
            .post(&url)
            .json(&body.unwrap()) // FIXME: do not unwrap
            .build()
            .unwrap();
        let res = ctx.get_client().execute(req).await.unwrap(); // FIXME: do not unwrap

        match res.status() {
            reqwest::StatusCode::OK => {
                let res: GraphqlQueryResponse = res.json().await.unwrap(); // FIXME: do not unwrap
                let mut txs: Vec<Transaction> = Vec::<Transaction>::new();
                let mut end_cursor: Option<String> = None;
                for tx in &res.data.transactions.edges {
                    txs.push(tx.node.clone());
                    end_cursor = Some(tx.cursor.clone());
                }
                let has_next_page = res.data.transactions.page_info.has_next_page;

                Ok((txs, has_next_page, end_cursor))
            }
            reqwest::StatusCode::BAD_REQUEST => Err(ArweaveError::MalformedQuery),
            reqwest::StatusCode::NOT_FOUND => Err(ArweaveError::TxsNotFound),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR => Err(ArweaveError::InternalServerError),
            reqwest::StatusCode::GATEWAY_TIMEOUT => Err(ArweaveError::GatewayTimeout),
            _ => Err(ArweaveError::UnknownErr),
        }
    }

    fn get_host(&self) -> Url {
        self.url.clone()
    }
}

pub async fn sync_network_info<Context, HttpClient>(ctx: &Context) -> Result<(), CronJobError>
where
    Context: ArweaveContext<HttpClient> + ArweaveAccess + ValidatorStateAccess,
    HttpClient: crate::http::Client<Request = reqwest::Request, Response = reqwest::Response>,
{
    let network_info = ctx.arweave().get_network_info(ctx).await.map_err(|err| {
        error!("Request for network info failed: {:?}", err);
        CronJobError::ArweaveError(ArweaveError::UnknownErr)
    })?;

    let state = ctx.get_validator_state();

    info!("Update state: current_block={}", network_info.height);
    state.set_current_block(network_info.height);

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, str::FromStr};

    use crate::{
        context::test_utils::test_context_with_http_client, cron::arweave::Arweave,
        http::reqwest::mock::MockHttpClient, key_manager::test_utils::test_keys,
    };
    use http::Method;
    use reqwest::{Request, Response};
    use url::Url;

    #[actix_rt::test]
    async fn get_network_info() {
        let client = MockHttpClient::new(|a: &Request, b: &Request| a.url() == b.url())
            .when(|req: &Request| {
                let url = "http://example.com/info";
                req.method() == Method::GET && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let data = "{\"network\":\"arweave.N.1\",\"version\":5,\"release\":43,\"height\":551511,\"current\":\"XIDpYbc3b5iuiqclSl_Hrx263Sd4zzmrNja1cvFlqNWUGuyymhhGZYI4WMsID1K3\",\"blocks\":97375,\"peers\":64,\"queue_length\":0,\"node_state_latency\":18}";
                let response = http::response::Builder::new()
                    .status(200)
                    .body(data)
                    .unwrap();
                Response::from(response)
            });

        let (key_manager, _bundle_pvk) = test_keys();
        let ctx = test_context_with_http_client(key_manager, client);
        let arweave = Arweave {
            url: Url::from_str("http://example.com").unwrap(),
        };
        let network_info = arweave.get_network_info(&ctx).await.unwrap();

        assert_eq!(network_info.height, 551511);
    }

    #[actix_rt::test]
    async fn get_tx_data_should_return_ok() {
        let client = MockHttpClient::new(|a: &Request, b: &Request| a.url() == b.url())
            .when(|req: &Request| {
                let url = "http://example.com/tx_id";
                req.method() == Method::GET && &req.url().to_string() == url
            })
            .then(|_: &Request| {
                let data = "stream";

                let response = http::response::Builder::new()
                    .status(200)
                    .body(data)
                    .unwrap();
                Response::from(response)
            });

        let (key_manager, _bundle_pvk) = test_keys();
        let ctx = test_context_with_http_client(key_manager, client);
        let arweave = Arweave {
            url: Url::from_str(&"http://example.com".to_string()).unwrap(),
        };
        arweave.get_tx_data(&ctx, "tx_id").await.unwrap();

        let raw_path = "./bundles/tx_id";
        let file_path = Path::new(raw_path).is_file();
        assert!(file_path); // FIXME: remove/replace use of assert
        match fs::remove_file(raw_path) {
            Ok(_) => (),
            Err(_) => println!(
                "File {} not removed properly, please delete it manually",
                raw_path
            ),
        }
    }

    #[actix_rt::test]
    async fn get_latest_transactions_should_return_ok() {
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
            });

        let (key_manager, _bundle_pvk) = test_keys();
        let ctx = test_context_with_http_client(key_manager, client);
        let arweave = Arweave {
            url: Url::from_str(&"http://example.com".to_string()).unwrap(),
        };
        arweave
            .get_latest_transactions(&ctx, "owner", None, None)
            .await
            .unwrap();
    }
}
