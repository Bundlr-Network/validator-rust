pub mod error;
pub mod routes;

use std::net::SocketAddr;

use actix_web::{
    middleware::Logger,
    web::{self, Data},
    App, HttpServer,
};
use diesel::{
    r2d2::{ConnectionManager, PooledConnection},
    PgConnection,
};
use paris::info;
use routes::get_tx::get_tx;
use routes::index::index;

use crate::{
    database::queries::QueryContext, key_manager, server::routes::sign::sign_route,
    state::{ValidatorStateAccess, ValidatorRole},
};

#[cfg(feature = "test-routes")]
use crate::server::routes::test::set_state;

pub trait RuntimeContext {
    fn bind_address(&self) -> &SocketAddr;
    fn get_db_connection(&self) -> PooledConnection<ConnectionManager<PgConnection>>;
}

pub async fn run_server<Context, KeyManager>(ctx: Context) -> std::io::Result<()>
where
    Context: RuntimeContext
        + routes::sign::Config<KeyManager>
        + ValidatorStateAccess
        + QueryContext
        + Clone
        + Send
        + 'static,
    KeyManager: key_manager::KeyManager + Clone + Send + 'static,
{
    info!("Starting up HTTP server...");

    let runtime_context = ctx.clone();

    let state = runtime_context.get_validator_state().clone();
    ctrlc::set_handler(move || {
        if state.role() == ValidatorRole::Idle {
            info!("Received CTRL-C signal. Shutting down as validator is idle...");
            std::process::exit(0);
        } else {
            info!("Received CTRL-C signal. Can't shutdown as validator is still active!");
        }
    }).expect("Couldn't setup CTRL-C handler");

    HttpServer::new(move || {
        {
            // use double braces to enable inner attributes
            #![allow(clippy::let_and_return)]

            let app = App::new()
                .app_data(Data::new(runtime_context.clone()))
                .wrap(Logger::default())
                .route("/", web::get().to(index))
                .route("/tx/{tx_id}", web::get().to(get_tx::<Context>))
                .service(
                    web::scope("/cosigner")
                        .route("/sign", web::post().to(sign_route::<Context, KeyManager>)),
                )
                .service(web::scope("/idle").route("/", web::get().to(index)));

            #[cfg(feature = "test-routes")]
            let app = app
                .service(web::scope("/test").route("/state", web::post().to(set_state::<Context>)));

            app
        }
    })
    .shutdown_timeout(5)
    .bind(ctx.bind_address())?
    .run()
    .await
}
