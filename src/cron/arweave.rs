use http::uri::PathAndQuery;
use http::Uri;
use paris::error;
use paris::info;
use serde::Deserialize;
use serde::Serialize;
use std::fmt::Debug;

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use crate::http::Client;

#[derive(Deserialize, Serialize, Clone)]
pub struct NetworkInfo {
    pub network: String,
    pub version: usize,
    pub release: usize,
    pub height: usize,
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

#[derive(Debug, Display, Error, Clone)]
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

const TX_QUERY: &str = "query($owners: [String!], $first: Int) { transactions(owners: $owners, first: $first) { pageInfo { hasNextPage } edges { cursor node { id owner { address } signature recipient tags { name value } block { height id timestamp } } } } }";

fn path_and_query(raw_query: &str) -> PathAndQuery {
    format!("/graphql?query={}", urlencoding::encode(raw_query))
        .parse()
        .unwrap()
}

#[derive(Clone)]
pub struct Arweave {
    pub uri: http::uri::Uri,
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
    pub fn new(uri: &http::uri::Uri) -> Arweave {
        Arweave { uri: uri.clone() }
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
        info!("Downloading bundle {} content", &transaction_id);
        let raw_path = format!("./bundles/{}", transaction_id);
        let file_path = Path::new(&raw_path);
        let mut buffer = File::create(&file_path).unwrap();

        let uri =
            http::uri::Uri::from_str(&format!("{}{}", self.get_host(), transaction_id).to_string())
                .unwrap();
        let req: http::Request<String> = http::request::Builder::new()
            .method(http::Method::GET)
            .uri(uri)
            .body("".to_string())
            .unwrap();

        let req: reqwest::Request = reqwest::Request::try_from(req).unwrap();
        let mut res: reqwest::Response =
            ctx.get_client().execute(req).await.expect("request failed");
        if res.status().is_success() {
            while let Some(chunk) = res.chunk().await? {
                match buffer.write(&chunk) {
                    Ok(_) => {}
                    Err(err) => {
                        error!("Error writing on file {:?}: {:?}", file_path.to_str(), err)
                    }
                }
            }
            return Ok(String::from(file_path.to_string_lossy()));
        } else {
            Err(res.error_for_status().err().unwrap())
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
        let raw_variables = format!(
            "{{\"owners\": [\"{}\"], \"first\": {}, \"after\": {}}}",
            owner,
            first.unwrap_or(10),
            match after {
                None => r"null".to_string(),
                Some(a) => a,
            }
        );

        let data = format!(
            "{{\"query\":\"{}\",\"variables\":{}}}",
            TX_QUERY, raw_variables
        );

        let mut req_url_parts = self.get_host().into_parts();
        req_url_parts.path_and_query = Some(path_and_query(TX_QUERY));
        let req_url = Uri::from_parts(req_url_parts).unwrap();

        let req: http::Request<String> = http::request::Builder::new()
            .method(http::Method::POST)
            .uri(&req_url)
            .body(serde_json::to_string(&data).unwrap())
            .unwrap();

        let req: reqwest::Request = reqwest::Request::try_from(req).unwrap();
        let res = ctx.get_client().execute(req).await.unwrap();

        match res.status() {
            reqwest::StatusCode::OK => {
                let res: GraphqlQueryResponse = res.json().await.unwrap();
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

    fn get_host(&self) -> http::uri::Uri {
        self.uri.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, str::FromStr};

    use crate::{
        context::test_utils::test_context_with_http_client,
        cron::arweave::{path_and_query, Arweave, TX_QUERY},
        http::reqwest::mock::MockHttpClient,
        key_manager::test_utils::test_keys,
    };
    use http::{uri, Method, Uri};
    use reqwest::{Request, Response};

    #[test]
    fn urlencode_arweave_query() {
        let arweave_uri = "https://arweave.net".parse::<uri::Uri>().unwrap();

        let mut parts = arweave_uri.clone().into_parts();
        parts.path_and_query = Some(path_and_query(TX_QUERY));

        let url = uri::Uri::from_parts(parts).unwrap();

        assert_eq!(url.query().unwrap(), "query=query%28%24owners%3A%20%5BString%21%5D%2C%20%24first%3A%20Int%29%20%7B%20transactions%28owners%3A%20%24owners%2C%20first%3A%20%24first%29%20%7B%20pageInfo%20%7B%20hasNextPage%20%7D%20edges%20%7B%20cursor%20node%20%7B%20id%20owner%20%7B%20address%20%7D%20signature%20recipient%20tags%20%7B%20name%20value%20%7D%20block%20%7B%20height%20id%20timestamp%20%7D%20%7D%20%7D%20%7D%20%7D")
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
            uri: Uri::from_str(&"http://example.com".to_string()).unwrap(),
        };
        arweave.get_tx_data(&ctx, "tx_id").await.unwrap();

        let raw_path = "./bundles/tx_id";
        let file_path = Path::new(raw_path).is_file();
        assert!(file_path);
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
                let url = "http://example.com/graphql?query=query%28%24owners%3A%20%5BString%21%5D%2C%20%24first%3A%20Int%29%20%7B%20transactions%28owners%3A%20%24owners%2C%20first%3A%20%24first%29%20%7B%20pageInfo%20%7B%20hasNextPage%20%7D%20edges%20%7B%20cursor%20node%20%7B%20id%20owner%20%7B%20address%20%7D%20signature%20recipient%20tags%20%7B%20name%20value%20%7D%20block%20%7B%20height%20id%20timestamp%20%7D%20%7D%20%7D%20%7D%20%7D";
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
            uri: Uri::from_str(&"http://example.com".to_string()).unwrap(),
        };
        arweave
            .get_latest_transactions(&ctx, "owner", None, None)
            .await
            .unwrap();
    }
}
