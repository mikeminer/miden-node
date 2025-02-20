use std::{io, sync::Arc};

use actix_cors::Cors;
use actix_files::Files;
use actix_web::{
    middleware::{DefaultHeaders, Logger},
    web, App, HttpServer,
};
use async_mutex::Mutex;
use clap::Parser;
use cli::Cli;
use config::FaucetConfig;
use handlers::{get_metadata, get_tokens};
use miden_client::{
    client::{rpc::TonicRpcClient, Client},
    store::sqlite_store::SqliteStore,
};
use miden_node_utils::config::load_config;
use miden_objects::accounts::AccountId;
use tracing::info;

use crate::cli::{ImportArgs, InitArgs};

mod cli;
mod config;
mod errors;
mod handlers;
mod utils;

pub type FaucetClient = Client<TonicRpcClient, SqliteStore>;

#[derive(Clone)]
pub struct FaucetState {
    id: AccountId,
    asset_amount: u64,
    client: Arc<Mutex<FaucetClient>>,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();

    miden_node_utils::logging::setup_logging().map_err(|e| {
        io::Error::new(io::ErrorKind::Other, format!("Failed to load logging: {}", e))
    })?;

    let mut client: FaucetClient;
    let config: FaucetConfig;
    let amount: u64;

    // Create faucet account
    let faucet_account = match &cli.command {
        cli::Commands::Init(InitArgs {
            asset_amount,
            token_symbol,
            decimals,
            max_supply,
            config: faucet_config,
        }) => {
            config = load_config(faucet_config.as_path()).extract().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to load configuration file: {}", e),
                )
            })?;

            client = utils::build_client(config.database_filepath.clone());

            amount = *asset_amount;
            utils::create_fungible_faucet(token_symbol, decimals, max_supply, &mut client).map_err(
                |e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Failed to create faucet account: {}", e),
                    )
                },
            )
        },
        cli::Commands::Import(ImportArgs {
            asset_amount,
            faucet_path,
            config: faucet_config,
        }) => {
            config = load_config(faucet_config.as_path()).extract().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to load configuration file: {}", e),
                )
            })?;

            client = utils::build_client(config.database_filepath.clone());

            amount = *asset_amount;
            utils::import_fungible_faucet(faucet_path, &mut client).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to import faucet account: {}", e),
                )
            })
        },
    }?;

    // Sync client
    client.sync_state().await.map_err(|e| {
        io::Error::new(io::ErrorKind::NotConnected, format!("Failed to sync state: {e:?}"))
    })?;

    info!("✅ Faucet setup successful, account id: {}", faucet_account.id());

    info!("🚀 Starting server on: {}", config.as_url());

    // Instantiate faucet state
    let faucet_state = FaucetState {
        id: faucet_account.id(),
        asset_amount: amount,
        client: Arc::new(Mutex::new(client)),
    };

    HttpServer::new(move || {
        let cors = Cors::default().allow_any_origin().allow_any_method();
        App::new()
            .app_data(web::Data::new(faucet_state.clone()))
            .wrap(cors)
            .wrap(Logger::default())
            .wrap(DefaultHeaders::new().add(("Cache-Control", "no-cache")))
            .service(get_metadata)
            .service(get_tokens)
            .service(
                Files::new("/", "faucet/src/static")
                    .use_etag(false)
                    .use_last_modified(false)
                    .index_file("index.html"),
            )
    })
    .bind((config.endpoint.host, config.endpoint.port))?
    .run()
    .await?;

    Ok(())
}
