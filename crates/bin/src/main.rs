use std::{
    env,
    error::Error,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
};

use alloy_providers::provider::Provider;
use brontes::{Brontes, PROMETHEUS_ENDPOINT_IP, PROMETHEUS_ENDPOINT_PORT};
use brontes_classifier::{Classifier, PROTOCOL_ADDRESS_MAPPING};
use brontes_core::decoding::Parser as DParser;
use brontes_database::{
    database::{Database, USDT_ADDRESS, WETH_ADDRESS},
    Pair,
};
use brontes_inspect::{
    atomic_backrun::AtomicBackrunInspector, cex_dex::CexDexInspector, jit::JitInspector,
    sandwich::SandwichInspector, Inspector,
};
use brontes_metrics::{prometheus_exporter::initialize, PoirotMetricsListener};
use clap::Parser;
use metrics_process::Collector;
use reth_tracing_ext::TracingClient;
use tokio::{pin, sync::mpsc::unbounded_channel};
use tracing::{error, info, Level};
use tracing_subscriber::{prelude::__tracing_subscriber_SubscriberExt, EnvFilter, Layer, Registry};
mod cli;

use cli::{print_banner, Commands, Opts};

fn main() {
    print_banner();
    dotenv::dotenv().ok();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env_lossy();

    let subscriber = Registry::default().with(tracing_subscriber::fmt::layer().with_filter(filter));

    tracing::subscriber::set_global_default(subscriber)
        .expect("Could not set global default subscriber");

    match runtime.block_on(run()) {
        Ok(()) => info!("SUCCESS!"),
        Err(e) => {
            error!("Error: {:?}", e);

            let mut source: Option<&dyn Error> = e.source();
            while let Some(err) = source {
                error!("Caused by: {:?}", err);
                source = err.source();
            }
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    // parse cli
    let opt = Opts::parse();
    let Commands::Brontes(command) = opt.sub;

    initalize_prometheus().await;

    // Fetch required environment variables.
    let db_path = get_env_vars()?;

    let (metrics_tx, metrics_rx) = unbounded_channel();

    let metrics_listener = PoirotMetricsListener::new(metrics_rx);

    let db_endpoint = env::var("RETH_ENDPOINT").expect("No db Endpoint in .env");
    let db_port = env::var("RETH_PORT").expect("No DB port.env");
    let url = format!("{db_endpoint}:{db_port}");
    let provider = Provider::new(&url).unwrap();

    let pair = Pair(WETH_ADDRESS, USDT_ADDRESS);
    // init inspectors
    let sandwich = Box::new(SandwichInspector::new(pair)) as Box<dyn Inspector>;
    let cex_dex = Box::new(CexDexInspector::new(pair)) as Box<dyn Inspector>;
    let jit = Box::new(JitInspector::new(pair)) as Box<dyn Inspector>;
    let backrun = Box::new(AtomicBackrunInspector::new(pair)) as Box<dyn Inspector>;

    let inspectors = &[&sandwich, &cex_dex, &jit, &backrun];

    let db = Database::default();

    let (mut manager, tracer) =
        TracingClient::new(Path::new(&db_path), tokio::runtime::Handle::current());

    let parser = DParser::new(
        metrics_tx,
        &db,
        tracer,
        Box::new(|address| !PROTOCOL_ADDRESS_MAPPING.contains_key(&address.0 .0)),
    );
    let classifier = Classifier::new();

    #[cfg(not(feature = "local"))]
    let chain_tip = parser.get_latest_block_number().unwrap();
    #[cfg(feature = "local")]
    let chain_tip = parser.get_latest_block_number().await.unwrap();

    let brontes = Brontes::new(
        command.start_block,
        command.end_block,
        chain_tip,
        command.max_tasks,
        &provider,
        &parser,
        &db,
        &classifier,
        inspectors,
    );

    pin!(brontes);
    pin!(metrics_listener);

    // wait for completion
    tokio::select! {
        _ = &mut brontes => {
        }
        _ = Pin::new(&mut manager) => {
        }
        _ = &mut metrics_listener => {
        }
    }
    manager.graceful_shutdown();

    Ok(())
}

async fn initalize_prometheus() {
    // initializes the prometheus endpoint
    initialize(
        SocketAddr::new(
            IpAddr::V4(Ipv4Addr::from(PROMETHEUS_ENDPOINT_IP)),
            PROMETHEUS_ENDPOINT_PORT,
        ),
        Collector::default(),
    )
    .await
    .unwrap();
    info!("Initialized prometheus endpoint");
}

fn get_env_vars() -> Result<String, Box<dyn Error>> {
    let db_path = env::var("DB_PATH").map_err(|_| Box::new(std::env::VarError::NotPresent))?;
    info!("Found DB Path");

    Ok(db_path)
}
