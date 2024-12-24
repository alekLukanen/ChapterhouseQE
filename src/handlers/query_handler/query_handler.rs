use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::query_handler_state::{self, QueryHandlerState};
use crate::handlers::message_handler::{
    Message, MessageName, MessageRegistry, OperatorInstanceAvailable, Pipe, RunQuery, RunQueryResp,
};
use crate::handlers::message_router_handler::{
    MessageConsumer, MessageReceiver, MessageRouterState, Subscriber,
};
use crate::planner::{LogicalPlanner, PhysicalPlanner};

#[derive(Debug)]
pub struct QueryHandler {
    state: QueryHandlerState,
    message_router_state: Arc<Mutex<MessageRouterState>>,
    router_pipe: Pipe<Message>,
    sender: mpsc::Sender<Message>,
    msg_reg: Arc<Box<MessageRegistry>>,
}

impl QueryHandler {
    pub async fn new(
        message_router_state: Arc<Mutex<MessageRouterState>>,
        msg_reg: Arc<Box<MessageRegistry>>,
    ) -> QueryHandler {
        let router_sender = message_router_state.lock().await.sender();
        let (pipe, sender) = Pipe::new_with_existing_sender(router_sender, 1);

        let handler = QueryHandler {
            state: QueryHandlerState::new(),
            message_router_state,
            router_pipe: pipe,
            sender,
            msg_reg,
        };

        handler
    }

    pub fn subscriber(&self) -> Box<dyn Subscriber> {
        Box::new(QueryHandlerSubscriber {
            sender: self.sender.clone(),
        })
    }

    pub async fn async_main(&mut self, ct: CancellationToken) -> Result<()> {
        self.message_router_state
            .lock()
            .await
            .add_internal_subscriber(self.subscriber())?;

        loop {
            tokio::select! {
                Some(msg) = self.router_pipe.recv() => {
                    match msg.msg.msg_name() {
                        MessageName::RunQuery => {
                            self.run_query(&msg).await?;
                        },
                        _ => {
                            info!("unknown message received");
                        },
                    }
                }
                _ = ct.cancelled() => {
                    break;
                }
            }
        }

        info!("closing the query handler...");
        self.router_pipe.close_receiver();

        Ok(())
    }

    async fn run_query(&mut self, msg: &Message) -> Result<()> {
        let run_query: &RunQuery = self.msg_reg.cast_msg(&msg);

        let logical_plan = match LogicalPlanner::new(run_query.query.clone()).build() {
            Ok(plan) => plan,
            Err(err) => {
                info!("error: {}", err);
                let not_created_resp = msg.reply(Box::new(RunQueryResp::NotCreated));
                self.router_pipe.send(not_created_resp).await?;
                return Ok(());
            }
        };
        let physical_plan = match PhysicalPlanner::new(logical_plan).build() {
            Ok(plan) => plan,
            Err(err) => {
                info!("error: {}", err);
                let not_created_resp = msg.reply(Box::new(RunQueryResp::NotCreated));
                self.router_pipe.send(not_created_resp).await?;
                return Ok(());
            }
        };

        let query = query_handler_state::Query::new(run_query.query.clone(), physical_plan);
        let run_query_resp = msg.reply(Box::new(RunQueryResp::Created {
            query_id: query.id.clone(),
        }));
        let query_id = query.id;

        info!("query: {:?}", query);

        self.state.add_query(query);
        self.router_pipe.send(run_query_resp).await?;

        info!("added a new query");

        // get all of the new operator instances and send out notifications for each
        let operator_instances = self
            .state
            .get_available_operator_instance_ids(query_id.clone())?;

        for op_in_id in operator_instances {
            let compute = self
                .state
                .get_operator_instance_compute(query_id.clone(), op_in_id)?;
            self.router_pipe
                .send(Message::new(Box::new(OperatorInstanceAvailable::new(
                    op_in_id, compute,
                ))))
                .await?;
        }

        Ok(())
    }
}

/////////////////////////////////////////////////
// Message subscriber for the query handler
#[derive(Debug)]
pub struct QueryHandlerSubscriber {
    sender: mpsc::Sender<Message>,
}

impl Subscriber for QueryHandlerSubscriber {}

impl MessageConsumer for QueryHandlerSubscriber {
    fn consumes_message(&self, msg: &Message) -> bool {
        match msg.msg.msg_name() {
            MessageName::RunQuery => true,
            _ => false,
        }
    }
}

impl MessageReceiver for QueryHandlerSubscriber {
    fn sender(&self) -> mpsc::Sender<Message> {
        self.sender.clone()
    }
}
