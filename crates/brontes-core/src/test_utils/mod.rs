use std::{
    collections::{hash_map::Entry, HashMap},
    env,
    sync::{Arc, OnceLock},
};

#[cfg(feature = "local-clickhouse")]
use brontes_database::clickhouse::Clickhouse;
#[cfg(not(feature = "local-clickhouse"))]
use brontes_database::clickhouse::ClickhouseHttpClient;
pub use brontes_database::libmdbx::{DBWriter, LibmdbxReadWriter, LibmdbxReader};
use brontes_database::{libmdbx::LibmdbxInit, Tables};
use brontes_metrics::PoirotMetricEvents;
use brontes_types::{db::metadata::Metadata, structured_trace::TxTrace, traits::TracingProvider};
use futures::future::join_all;
#[cfg(feature = "local-reth")]
use reth_db::DatabaseEnv;
use reth_primitives::{Header, B256};
use reth_provider::ProviderError;
#[cfg(feature = "local-reth")]
use reth_tasks::TaskManager;
#[cfg(feature = "local-reth")]
use reth_tracing_ext::init_db;
#[cfg(feature = "local-reth")]
use reth_tracing_ext::TracingClient;
use thiserror::Error;
use tokio::{
    runtime::Handle,
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};
use tracing::Level;
use tracing_subscriber::filter::Directive;

use crate::decoding::parser::TraceParser;
#[cfg(not(feature = "local-reth"))]
use crate::local_provider::LocalProvider;

/// Functionality to load all state needed for any testing requirements
pub struct TraceLoader {
    pub libmdbx: &'static LibmdbxReadWriter,
    pub tracing_provider: TraceParser<'static, Box<dyn TracingProvider>, LibmdbxReadWriter>,
    // store so when we trace we don't get a closed rx error
    _metrics: UnboundedReceiver<PoirotMetricEvents>,
}

impl TraceLoader {
    pub async fn new() -> Self {
        let libmdbx = get_db_handle();
        let (a, b) = unbounded_channel();
        let handle = tokio::runtime::Handle::current();
        let tracing_provider = init_trace_parser(handle, a, libmdbx, 10).await;

        let this = Self {
            libmdbx,
            tracing_provider,
            _metrics: b,
        };
        this.init_on_start().await.unwrap();

        this
    }

    pub fn get_provider(&self) -> Arc<Box<dyn TracingProvider>> {
        self.tracing_provider.get_tracer()
    }

    pub async fn trace_block(
        &self,
        block: u64,
    ) -> Result<(Vec<TxTrace>, Header), TraceLoaderError> {
        self.tracing_provider
            .execute_block(block)
            .await
            .ok_or_else(|| TraceLoaderError::BlockTraceError(block))
    }

    pub async fn get_metadata(
        &self,
        block: u64,
        pricing: bool,
    ) -> Result<Metadata, TraceLoaderError> {
        if pricing {
            if let Ok(res) = self.test_metadata_with_pricing(block) {
                Ok(res)
            } else {
                self.fetch_missing_metadata(block).await?;
                self.test_metadata_with_pricing(block)
                    .map_err(|_| TraceLoaderError::NoMetadataFound(block))
            }
        } else if let Ok(res) = self.test_metadata(block) {
            Ok(res)
        } else {
            self.fetch_missing_metadata(block).await?;
            return self
                .test_metadata(block)
                .map_err(|_| TraceLoaderError::NoMetadataFound(block));
        }
    }

    async fn init_on_start(&self) -> eyre::Result<()> {
        let clickhouse = Box::leak(Box::new(load_clickhouse()));
        if self.libmdbx.init_full_range_tables(clickhouse).await {
            self.libmdbx
                .initialize_tables(
                    clickhouse,
                    self.tracing_provider.get_tracer(),
                    &[
                        Tables::PoolCreationBlocks,
                        Tables::TokenDecimals,
                        Tables::AddressToProtocolInfo,
                    ],
                    false,
                    None,
                )
                .await?;
        }

        Ok(())
    }

    pub async fn fetch_missing_metadata(&self, block: u64) -> eyre::Result<()> {
        tracing::info!(%block, "fetching missing metadata");

        let clickhouse = Box::leak(Box::new(load_clickhouse()));
        self.libmdbx
            .initialize_tables(
                clickhouse,
                self.tracing_provider.get_tracer(),
                &[Tables::BlockInfo, Tables::CexPrice],
                false,
                Some((block - 2, block + 2)),
            )
            .await?;

        Ok(())
    }

    pub fn test_metadata_with_pricing(&self, block_num: u64) -> eyre::Result<Metadata> {
        self.libmdbx.get_metadata(block_num)
    }

    pub fn test_metadata(&self, block_num: u64) -> eyre::Result<Metadata> {
        self.libmdbx.get_metadata_no_dex_price(block_num)
    }

    pub async fn get_block_traces_with_header(
        &self,
        block: u64,
    ) -> Result<BlockTracesWithHeaderAnd<()>, TraceLoaderError> {
        let (traces, header) = self.trace_block(block).await?;
        Ok(BlockTracesWithHeaderAnd {
            traces,
            header,
            block,
            other: (),
        })
    }

    pub async fn get_block_traces_with_header_range(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<BlockTracesWithHeaderAnd<()>>, TraceLoaderError> {
        join_all((start_block..=end_block).map(|block| async move {
            let (traces, header) = self.trace_block(block).await?;
            Ok(BlockTracesWithHeaderAnd {
                traces,
                header,
                block,
                other: (),
            })
        }))
        .await
        .into_iter()
        .collect()
    }

    pub async fn get_block_traces_with_header_and_metadata(
        &self,
        block: u64,
    ) -> Result<BlockTracesWithHeaderAnd<Metadata>, TraceLoaderError> {
        let (traces, header) = self.trace_block(block).await?;
        let metadata = self.get_metadata(block, false).await?;

        Ok(BlockTracesWithHeaderAnd {
            block,
            traces,
            header,
            other: metadata,
        })
    }

    pub async fn get_block_traces_with_header_and_metadata_range(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<BlockTracesWithHeaderAnd<Metadata>>, TraceLoaderError> {
        join_all((start_block..=end_block).map(|block| async move {
            let (traces, header) = self.trace_block(block).await?;
            let metadata = self.get_metadata(block, false).await?;
            Ok(BlockTracesWithHeaderAnd {
                traces,
                header,
                block,
                other: metadata,
            })
        }))
        .await
        .into_iter()
        .collect()
    }

    pub async fn get_tx_trace_with_header(
        &self,
        tx_hash: B256,
    ) -> Result<TxTracesWithHeaderAnd<()>, TraceLoaderError> {
        let (block, tx_idx) = self
            .tracing_provider
            .get_tracer()
            .block_and_tx_index(tx_hash)
            .await?;
        let (traces, header) = self.trace_block(block).await?;
        let trace = traces[tx_idx].clone();

        Ok(TxTracesWithHeaderAnd {
            block,
            tx_hash,
            trace,
            header,
            other: (),
        })
    }

    pub async fn get_tx_traces_with_header(
        &self,
        tx_hashes: Vec<B256>,
    ) -> Result<Vec<BlockTracesWithHeaderAnd<()>>, TraceLoaderError> {
        let mut flattened: HashMap<u64, BlockTracesWithHeaderAnd<()>> = HashMap::new();
        join_all(tx_hashes.into_iter().map(|tx_hash| async move {
            let (block, tx_idx) = self
                .tracing_provider
                .get_tracer()
                .block_and_tx_index(tx_hash)
                .await?;
            let (traces, header) = self.trace_block(block).await?;
            let trace = traces[tx_idx].clone();

            Ok(TxTracesWithHeaderAnd {
                block,
                tx_hash,
                trace,
                header,
                other: (),
            })
        }))
        .await
        .into_iter()
        .for_each(|res: Result<TxTracesWithHeaderAnd<()>, TraceLoaderError>| {
            if let Ok(res) = res {
                match flattened.entry(res.block) {
                    Entry::Occupied(mut o) => {
                        let e = o.get_mut();
                        e.traces.push(res.trace)
                    }
                    Entry::Vacant(v) => {
                        let entry = BlockTracesWithHeaderAnd {
                            traces: vec![res.trace],
                            block: res.block,
                            other: (),
                            header: res.header,
                        };
                        v.insert(entry);
                    }
                }
            }
        });

        let mut res = flattened
            .into_values()
            .map(|mut traces| {
                traces
                    .traces
                    .sort_by(|t0, t1| t0.tx_index.cmp(&t1.tx_index));
                traces
            })
            .collect::<Vec<_>>();
        res.sort_by(|a, b| a.block.cmp(&b.block));

        Ok(res)
    }

    pub async fn get_tx_trace_with_header_and_metadata(
        &self,
        tx_hash: B256,
    ) -> Result<TxTracesWithHeaderAnd<Metadata>, TraceLoaderError> {
        let (block, tx_idx) = self
            .tracing_provider
            .get_tracer()
            .block_and_tx_index(tx_hash)
            .await?;
        let (traces, header) = self.trace_block(block).await?;
        let metadata = self.get_metadata(block, false).await?;
        let trace = traces[tx_idx].clone();

        Ok(TxTracesWithHeaderAnd {
            block,
            tx_hash,
            trace,
            header,
            other: metadata,
        })
    }

    pub async fn get_tx_traces_with_header_and_metadata(
        &self,
        tx_hashes: Vec<B256>,
    ) -> Result<Vec<TxTracesWithHeaderAnd<Metadata>>, TraceLoaderError> {
        join_all(tx_hashes.into_iter().map(|tx_hash| async move {
            let (block, tx_idx) = self
                .tracing_provider
                .get_tracer()
                .block_and_tx_index(tx_hash)
                .await?;
            let (traces, header) = self.trace_block(block).await?;
            let metadata = self.get_metadata(block, false).await?;
            let trace = traces[tx_idx].clone();

            Ok(TxTracesWithHeaderAnd {
                block,
                tx_hash,
                trace,
                header,
                other: metadata,
            })
        }))
        .await
        .into_iter()
        .collect()
    }
}

#[derive(Debug, Error)]
pub enum TraceLoaderError {
    #[error("no metadata found in libmdbx for block: {0}")]
    NoMetadataFound(u64),
    #[error("failed to trace block: {0}")]
    BlockTraceError(u64),
    #[error(transparent)]
    ProviderError(#[from] ProviderError),
    #[error(transparent)]
    EyreError(#[from] eyre::Report),
}

pub struct TxTracesWithHeaderAnd<T> {
    pub block: u64,
    pub tx_hash: B256,
    pub trace: TxTrace,
    pub header: Header,
    pub other: T,
}

pub struct BlockTracesWithHeaderAnd<T> {
    pub block: u64,
    pub traces: Vec<TxTrace>,
    pub header: Header,
    pub other: T,
}

// done because we can only have 1 instance of libmdbx or we error
static DB_HANDLE: OnceLock<LibmdbxReadWriter> = OnceLock::new();
#[cfg(feature = "local-reth")]
static RETH_DB_HANDLE: OnceLock<Arc<DatabaseEnv>> = OnceLock::new();

pub fn get_db_handle() -> &'static LibmdbxReadWriter {
    DB_HANDLE.get_or_init(|| {
        let _ = dotenv::dotenv();
        init_tracing();
        let brontes_db_endpoint =
            env::var("BRONTES_TEST_DB_PATH").expect("No BRONTES_DB_PATH in .env");
        LibmdbxReadWriter::init_db(&brontes_db_endpoint, None)
            .unwrap_or_else(|_| panic!("failed to open db path {}", brontes_db_endpoint))
    })
}

#[cfg(feature = "local-reth")]
pub fn get_reth_db_handle() -> Arc<DatabaseEnv> {
    RETH_DB_HANDLE
        .get_or_init(|| {
            let db_path = env::var("DB_PATH").expect("No DB_PATH in .env");
            Arc::new(init_db(db_path).unwrap())
        })
        .clone()
}

// if we want more tracing/logging/metrics layers, build and push to this vec
// the stdout one (logging) is the only 1 we need
// peep the Database repo -> bin/sorella-db/src/cli.rs line 34 for example
pub fn init_tracing() {
    // all lower level logging directives include higher level ones (Trace includes
    // all, Debug includes all but Trace, ...)
    let verbosity_level = Level::INFO; // Error >= Warn >= Info >= Debug >= Trace
    let directive: Directive = format!("{verbosity_level}").parse().unwrap();
    let layers = vec![brontes_tracing::stdout(directive)];

    brontes_tracing::init(layers);
}

#[cfg(feature = "local-reth")]
pub async fn init_trace_parser(
    handle: Handle,
    metrics_tx: UnboundedSender<PoirotMetricEvents>,
    libmdbx: &LibmdbxReadWriter,
    max_tasks: u32,
) -> TraceParser<'_, Box<dyn TracingProvider>, LibmdbxReadWriter> {
    let executor = TaskManager::new(handle.clone());
    let client =
        TracingClient::new_with_db(get_reth_db_handle(), max_tasks as u64, executor.executor());
    handle.spawn(executor);
    let tracer = Box::new(client) as Box<dyn TracingProvider>;

    TraceParser::new(libmdbx, Arc::new(tracer), Arc::new(metrics_tx)).await
}

#[cfg(not(feature = "local-reth"))]
pub async fn init_trace_parser(
    handle: Handle,
    metrics_tx: UnboundedSender<PoirotMetricEvents>,
    libmdbx: &LibmdbxReadWriter,
    _max_tasks: u32,
) -> TraceParser<'_, Box<dyn TracingProvider>, LibmdbxReadWriter> {
    let db_endpoint = env::var("RETH_ENDPOINT").expect("No db Endpoint in .env");
    let db_port = env::var("RETH_PORT").expect("No DB port.env");
    let url = format!("{db_endpoint}:{db_port}");
    let tracer = Box::new(LocalProvider::new(url)) as Box<dyn TracingProvider>;

    TraceParser::new(libmdbx, Arc::new(tracer), Arc::new(metrics_tx)).await
}

#[cfg(feature = "local-clickhouse")]
pub fn load_clickhouse() -> Clickhouse {
    Clickhouse::default()
}

#[cfg(not(feature = "local-clickhouse"))]
pub fn load_clickhouse() -> ClickhouseHttpClient {
    let clickhouse_api = env::var("CLICKHOUSE_API").expect("No CLICKHOUSE_API in .env");
    let clickhouse_api_key = env::var("CLICKHOUSE_API_KEY").expect("No CLICKHOUSE_API_KEY in .env");
    ClickhouseHttpClient::new(clickhouse_api, clickhouse_api_key)
}
