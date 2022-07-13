use crate::http::{self, method::Method};
use bundlr_contracts_validators::{
    slashing::Proposal as SlashProposal, slashing::Vote, State as ContractState,
};
use derive_more::{Display, Error};
use log::error;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Display, Error, Clone, PartialEq)]
pub enum ContractGatewayError {
    RequestFailed,
}

pub trait ContractGatewayAccess {
    fn contract_gateway(&self) -> &ContractGateway;
}

#[derive(Clone, Debug)]
pub struct ContractGateway {
    pub url: Url,
}

#[derive(Clone, Debug, Serialize)]
struct VoteRequest<'a> {
    tx: &'a str,
    vote: Vote,
}

#[derive(Clone, Debug, Deserialize)]
struct VoteResponse {
    status: String,
}

impl ContractGateway {
    pub async fn get_current_state<Context, HttpClient>(
        &self,
        ctx: &Context,
    ) -> Result<ContractState, ContractGatewayError>
    where
        Context: http::ClientAccess<HttpClient>,
        HttpClient: http::Client<Request = reqwest::Request, Response = reqwest::Response>,
    {
        let url = format!("{}validators/state", self.url);

        let req = http::request::Builder::new()
            .method(Method::GET)
            .uri(url)
            .body("".to_string())
            .map(|req| {
                reqwest::Request::try_from(req)
                    .expect("Failed to convert http::request::Request into reqwest::Request")
            })
            .expect("Failed to parse URL for fetching contract state");

        let res = ctx
            .get_http_client()
            .execute(req)
            .await
            .map_err(|_| ContractGatewayError::RequestFailed)?; // TODO: needs better error

        res.json()
            .await
            .map_err(|_| ContractGatewayError::RequestFailed) // TODO: needs better error
    }

    pub async fn vote_for_proposal<Context, HttpClient>(
        &self,
        ctx: &Context,
        proposal: &SlashProposal,
        vote: Vote,
    ) -> Result<(), ContractGatewayError>
    where
        Context: http::ClientAccess<HttpClient>,
        HttpClient: http::Client<Request = reqwest::Request, Response = reqwest::Response>,
    {
        let url = format!("{}validators/vote", self.url);

        let req = http::request::Builder::new()
            .method(Method::POST)
            .uri(url)
            .body(
                serde_json::to_string(&VoteRequest {
                    tx: &proposal.id,
                    vote,
                })
                .map_err(|err| {
                    error!("Building request failed: {:?}", err);
                    // TODO: needs better error
                    ContractGatewayError::RequestFailed
                })?,
            )
            .map(|req| {
                reqwest::Request::try_from(req)
                    .expect("Failed to convert http::request::Request into reqwest::Request")
            })
            .expect("Failed to parse URL for fetching contract state");

        let res = ctx.get_http_client().execute(req).await.map_err(|err| {
            error!("Request failed: {:?}", err);
            // TODO: needs better error
            ContractGatewayError::RequestFailed
        })?; // TODO: needs better error

        let res: VoteResponse = res.json().await.map_err(|err| {
            error!("Failed to deserialize the response: {:?}", err);
            // TODO: needs better error
            ContractGatewayError::RequestFailed
        })?;

        if res.status != "OK" {
            error!("Request failed: {:?}", res);
            return Err(ContractGatewayError::RequestFailed);
        }

        Ok(())
    }
}
