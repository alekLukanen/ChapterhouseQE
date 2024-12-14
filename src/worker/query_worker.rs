use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::info;
use uuid::Uuid;

use crate::handlers::message_handler::InboundConnectionPoolHandler;
use crate::handlers::message_router_handler::MessageRouterHandler;

pub struct QueryWorkerConfig {
    address: String,
    connect_to_addresses: Vec<String>,
}

impl QueryWorkerConfig {
    pub fn new(address: String, connect_to_addresses: Vec<String>) -> QueryWorkerConfig {
        QueryWorkerConfig {
            address,
            connect_to_addresses,
        }
    }
}

pub struct QueryWorker {
    worker_id: u128,
    config: QueryWorkerConfig,
    cancelation_token: CancellationToken,
}

impl QueryWorker {
    pub fn new(config: QueryWorkerConfig) -> QueryWorker {
        let ct = CancellationToken::new();
        return QueryWorker {
            worker_id: Uuid::new_v4().as_u128(),
            config,
            cancelation_token: ct,
        };
    }

    pub fn start(&mut self) -> Result<()> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Failed to create Tokio runtime: {}", e))?;

        runtime.block_on(self.async_main())
    }

    async fn async_main(&mut self) -> Result<()> {
        let tt = TaskTracker::new();

        // Messenger and Router ////////////////////////
        let mut inbound_connection_pool_handler = InboundConnectionPoolHandler::new(
            self.config.address.clone(),
            self.config.connect_to_addresses.clone(),
        );

        let router_receiver = inbound_connection_pool_handler.outbound_receiver();
        let router_sender = inbound_connection_pool_handler.inbound_sender();
        let mut message_router = MessageRouterHandler::new(router_sender, router_receiver);

        let messenger_ct = self.cancelation_token.clone();
        tt.spawn(async move {
            if let Err(err) = inbound_connection_pool_handler
                .async_main(messenger_ct)
                .await
            {
                info!("error: {}", err);
            }
        });

        let message_router_ct = self.cancelation_token.clone();
        tt.spawn(async move {
            if let Err(err) = message_router.async_main(message_router_ct).await {
                info!("error: {}", err);
            }
        });

        // TaskTracker /////////////////////
        // wait for the cancelation token to be cancelled and all tasks to be cancelled
        tt.close();
        tt.wait().await;

        Ok(())
    }
}
