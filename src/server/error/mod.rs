use actix_rt::task::JoinError;
use actix_web::{
    error,
    http::{header, StatusCode},
    HttpResponse, HttpResponseBuilder,
};
use derive_more::{Display, Error};
use openssl::error::ErrorStack;

#[warn(dead_code)]
#[derive(Debug, Display, Error)]
pub enum ValidatorServerError {
    #[display(fmt = "internal error")]
    InternalError,

    #[display(fmt = "bad request")]
    BadClientData,

    #[display(fmt = "timeout")]
    Timeout,
}

impl error::ResponseError for ValidatorServerError {
    fn error_response(&self) -> HttpResponse {
        HttpResponseBuilder::new(self.status_code())
            .insert_header((header::CONTENT_TYPE, "text/html; charset=utf-8"))
            .body(self.to_string())
    }

    fn status_code(&self) -> StatusCode {
        match *self {
            ValidatorServerError::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            ValidatorServerError::BadClientData => StatusCode::BAD_REQUEST,
            ValidatorServerError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        }
    }
}

impl From<ErrorStack> for ValidatorServerError {
    fn from(e: ErrorStack) -> Self {
        log::error!("Error occurred while performing crypto function - {}", e);
        ValidatorServerError::InternalError
    }
}

impl From<JoinError> for ValidatorServerError {
    fn from(e: JoinError) -> Self {
        log::error!("Error occurred while performing blocking task - {}", e);
        ValidatorServerError::InternalError
    }
}

impl From<diesel::result::Error> for ValidatorServerError {
    fn from(e: diesel::result::Error) -> Self {
        log::error!("Error occurred while db op - {}", e);
        ValidatorServerError::InternalError
    }
}
