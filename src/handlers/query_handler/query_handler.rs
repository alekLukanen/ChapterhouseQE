use std::sync::Arc;

use anyhow::{Context, Result};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use uuid::Uuid;

use super::query_handler_state::{self, QueryHandlerState, QueryHandlerStateError, Status};
use crate::handlers::message_handler::messages;
use crate::handlers::message_handler::messages::message::{Message, MessageName};
use crate::handlers::message_handler::{MessageRegistry, Pipe};
use crate::handlers::message_router_handler::{
    MessageConsumer, MessageReceiver, MessageRouterState, Subscriber,
};
use crate::handlers::operator_handler::operators::requests;
use crate::planner;

#[derive(Debug, Error)]
pub enum QueryHandlerError {
    #[error("incorrect message: {0}")]
    IncorrectMessage(String),
}

#[derive(Debug)]
pub struct QueryHandler {
    worker_id: u128,
    operator_id: u128,
    state: QueryHandlerState,
    message_router_state: Arc<Mutex<MessageRouterState>>,
    router_pipe: Pipe,
    sender: mpsc::Sender<Message>,
    msg_reg: Arc<MessageRegistry>,
}

impl QueryHandler {
    pub async fn new(
        worker_id: u128,
        message_router_state: Arc<Mutex<MessageRouterState>>,
        msg_reg: Arc<MessageRegistry>,
    ) -> QueryHandler {
        let operator_id = Uuid::new_v4().as_u128();

        let router_sender = message_router_state.lock().await.sender();
        let (mut pipe, sender) = Pipe::new_with_existing_sender(router_sender, 10);
        pipe.set_sent_from_operation_id(operator_id);

        let handler = QueryHandler {
            worker_id,
            operator_id,
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
            operator_id: self.operator_id.clone(),
            sender: self.sender.clone(),
            msg_reg: self.msg_reg.clone(),
        })
    }

    pub async fn async_main(&mut self, ct: CancellationToken) -> Result<()> {
        debug!(operator_id = self.operator_id, "started query handler");

        self.message_router_state
            .lock()
            .await
            .add_internal_subscriber(self.subscriber(), self.operator_id);

        let res = self.inner_async_main(ct.clone()).await;

        self.message_router_state
            .lock()
            .await
            .remove_internal_subscriber(&self.operator_id);
        self.router_pipe.close_receiver();

        debug!(operator_id = self.operator_id, "closed the query handler");

        res
    }

    async fn inner_async_main(&mut self, ct: CancellationToken) -> Result<()> {
        loop {
            tokio::select! {
                Some(msg) = self.router_pipe.recv() => {
                    debug!("received message: {}", msg);
                    let res = self.handle_message(msg).await;
                    if let Err(err) = res {
                        if let Some(err_state) = err.downcast_ref::<QueryHandlerStateError>() {
                            error!("state error: {}", err_state);
                        } else {
                            return Err(err);
                        }
                    }
                }
                _ = ct.cancelled() => {
                    break;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&mut self, msg: Message) -> Result<()> {
        match msg.msg.msg_name() {
            MessageName::RunQuery => self
                .handle_run_query(&msg)
                .await
                .context("failed handling the run query message"),
            MessageName::GetQueryStatus => self
                .handle_get_query_status(&msg)
                .await
                .context("failed handling the get query status message"),
            MessageName::OperatorInstanceAvailable => self
                .handle_operator_instance_response(&msg)
                .await
                .context("failed handling the operator instance available response message"),
            MessageName::OperatorInstanceAssignment => self
                .handle_operator_instance_assignment_responses(&msg)
                .await
                .context("failed handling the operator instance assignment response messages"),
            MessageName::QueryHandlerRequests => self
                .handle_query_handler_request_list_operator_instances(&msg)
                .await
                .context("failed handling the query handler request"),
            MessageName::QueryOperatorInstanceStatusChange => self
                .handle_operator_instance_status_change(&msg)
                .await
                .context("failed handling the operator instance status change"),
            _ => {
                info!("unhandled message received: {:?}", msg);
                Ok(())
            }
        }
    }

    async fn handle_get_query_status(&mut self, msg: &Message) -> Result<()> {
        let cast_msg: &messages::query::GetQueryStatus = self.msg_reg.try_cast_msg(msg)?;

        let query_res = self.state.find_query(&cast_msg.query_id);
        match query_res {
            Ok(query) => {
                let resp_msg = msg.reply(Box::new(messages::query::GetQueryStatusResp::Status(
                    query.status.clone(),
                )));
                self.router_pipe.send(resp_msg).await?;
            }
            Err(err) => match err.downcast_ref::<QueryHandlerStateError>() {
                Some(QueryHandlerStateError::QueryNotFound(_)) => {
                    let resp_msg =
                        msg.reply(Box::new(messages::query::GetQueryStatusResp::QueryNotFound));
                    self.router_pipe.send(resp_msg).await?;
                }
                None => {}
                _ => {
                    return Err(err);
                }
            },
        }

        Ok(())
    }

    async fn handle_operator_instance_status_change(&mut self, msg: &Message) -> Result<()> {
        let cast_msg: &messages::query::OperatorInstanceStatusChange =
            self.msg_reg.try_cast_msg(msg)?;

        // send response early since any state errors can't be handled by the
        // operator handler
        let resp_msg = msg.reply(Box::new(messages::common::GenericResponse::Ok));
        self.router_pipe.send(resp_msg).await?;

        // update the operator instance status
        let (query_id, op_in_id) = match cast_msg {
            messages::query::OperatorInstanceStatusChange::Complete {
                query_id,
                operator_instance_id,
            } => {
                self.state.update_operator_instance_status(
                    query_id,
                    operator_instance_id,
                    Status::Complete,
                )?;
                (query_id, operator_instance_id)
            }
            messages::query::OperatorInstanceStatusChange::Error {
                query_id,
                operator_instance_id,
                error,
            } => {
                self.state.update_operator_instance_status(
                    query_id,
                    operator_instance_id,
                    Status::Error(error.clone()),
                )?;
                (query_id, operator_instance_id)
            }
        };

        // notify the exchanges of the producer status change
        if self
            .state
            .operator_instance_is_producer(query_id, op_in_id)?
            && self
                .state
                .all_operator_instances_complete(query_id, op_in_id)?
        {
            let ref mut pipe = self.router_pipe;
            let query = self.state.find_query(query_id)?;
            let op_id = self
                .state
                .find_operator_instance(query, op_in_id)?
                .operator_id
                .clone();

            // notify downstream exchange operators if the producer operator is complete
            let outbound_exchange_id = self.state.get_outbound_exchange_id(query_id, op_in_id)?;
            let outbound_exchange_instances = self
                .state
                .get_operator_instances(query_id, &outbound_exchange_id)?;

            debug!(
                outbound_exchange = outbound_exchange_id,
                "sending operator status change to exchange operator"
            );
            for exchange_instance in outbound_exchange_instances {
                requests::exchange::OperatorStatusChangeRequest::completed_request(
                    exchange_instance.id.clone(),
                    op_id.clone(),
                    pipe,
                    self.msg_reg.clone(),
                )
                .await?;
            }

            // notify the exchanges to shutdown if all inbound and outbound producer
            // operators linked to it are complete
            let exchange_ids = self
                .state
                .get_exchange_ids_without_any_consumers(query_id)?;

            debug!(
                exchanges = format!("{:?}", exchange_ids),
                "exchanges without any running inbound or outbound producers"
            );
            for exchange_id in exchange_ids {
                let outbound_exchange_instances =
                    self.state.get_operator_instances(query_id, &exchange_id)?;
                for exchange_instance in outbound_exchange_instances {
                    if !exchange_instance.status.available()
                        || matches!(exchange_instance.status, Status::SentShutdown(_))
                    {
                        continue;
                    }

                    debug!(
                        exchange_id = exchange_id,
                        exchange_instance_id = exchange_instance.id,
                        "sending shutdown request to exchange operator",
                    );

                    requests::operator::ShutdownRequest::shutdown_immediate_request(
                        exchange_instance.id.clone(),
                        pipe,
                        self.msg_reg.clone(),
                    )
                    .await?;

                    self.state.update_operator_instance_status(
                        query_id,
                        &exchange_instance.id,
                        Status::SentShutdown(chrono::Utc::now()),
                    )?;
                }
            }
        }

        // recompute the query status and exit early if the
        let query_status = self.state.refresh_query_status(query_id)?;
        if query_status.terminal() {
            info!(
                query_id = query_id.clone(),
                status = query_status.to_string(),
                "query finished"
            );
            return Ok(());
        }

        Ok(())
    }

    async fn handle_query_handler_request_list_operator_instances(
        &mut self,
        msg: &Message,
    ) -> Result<()> {
        let list_operator_instances_request: &messages::query::QueryHandlerRequests =
            self.msg_reg.try_cast_msg(msg)?;
        match list_operator_instances_request {
            messages::query::QueryHandlerRequests::ListOperatorInstancesRequest {
                query_id,
                operator_id,
            } => {
                let op_instances = self.state.get_operator_instances(query_id, operator_id)?;
                let resp_msg = msg.reply(Box::new(
                    messages::query::QueryHandlerRequests::ListOperatorInstancesResponse {
                        op_instance_ids: op_instances.iter().map(|item| item.id).collect(),
                    },
                ));
                self.router_pipe.send(resp_msg).await?;

                Ok(())
            }
            _ => Err(QueryHandlerError::IncorrectMessage(format!(
                "{:?}",
                list_operator_instances_request
            ))
            .into()),
        }
    }

    async fn handle_operator_instance_assignment_responses(&mut self, msg: &Message) -> Result<()> {
        let op_in_assign: &messages::query::OperatorInstanceAssignment =
            self.msg_reg.try_cast_msg(msg)?;
        match op_in_assign {
            messages::query::OperatorInstanceAssignment::AssignAcceptedResponse {
                query_id,
                op_instance_id,
                ..
            } => {
                info!(
                    "assign accepted response: worker_id={:?}, query_id={}, op_in_id={}",
                    msg.sent_from_worker_id, query_id, op_instance_id
                );
                if self.state.find_query(query_id)?.status == Status::Queued {
                    self.state.update_query_status(query_id, Status::Running)?;
                }
                self.state.update_operator_instance_status(
                    query_id,
                    op_instance_id,
                    Status::Running,
                )?;
            }
            messages::query::OperatorInstanceAssignment::AssignRejectedResponse {
                query_id,
                op_instance_id,
                error,
                ..
            } => {
                info!(
                    "assign rejected response: query_id={}, op_in_id={}",
                    query_id, op_instance_id
                );
                self.state
                    .update_query_status(query_id, Status::Error(error.clone()))?;
                self.state.update_operator_instance_status(
                    query_id,
                    op_instance_id,
                    Status::Error(error.clone()),
                )?;
            }
            messages::query::OperatorInstanceAssignment::Assign { .. } => {
                return Err(
                    QueryHandlerError::IncorrectMessage(format!("{:?}", op_in_assign)).into(),
                );
            }
        }

        Ok(())
    }

    async fn handle_operator_instance_response(&mut self, msg: &Message) -> Result<()> {
        let op_avail_resp: &messages::query::OperatorInstanceAvailable =
            self.msg_reg.try_cast_msg(msg)?;
        let can_accept_up_to = match op_avail_resp {
            messages::query::OperatorInstanceAvailable::NotificationResponse {
                can_accept_up_to,
            } => can_accept_up_to,
            _ => {
                return Err(
                    QueryHandlerError::IncorrectMessage(format!("{:?}", op_avail_resp)).into(),
                );
            }
        };

        let operator_instances = self
            .state
            .claim_operator_instances_up_to_compute_available(can_accept_up_to);

        let msgs = operator_instances
            .iter()
            .map(|item| {
                msg.reply(Box::new(
                    messages::query::OperatorInstanceAssignment::Assign {
                        query_handler_worker_id: self.worker_id.clone(),
                        op_instance_id: item.1.id,
                        query_id: item.0,
                        pipeline_id: item.1.pipeline_id.clone(),
                        operator: item.2.clone(),
                    },
                ))
            })
            .collect();
        self.router_pipe.send_all(msgs).await?;

        Ok(())
    }

    async fn handle_run_query(&mut self, msg: &Message) -> Result<()> {
        let run_query: &messages::query::RunQuery = self.msg_reg.try_cast_msg(&msg)?;

        let logical_plan = match planner::LogicalPlanner::new(run_query.query.clone()).build() {
            Ok(plan) => plan,
            Err(err) => {
                info!("error: {}", err);
                let not_created_resp =
                    msg.reply(Box::new(messages::query::RunQueryResp::NotCreated));
                self.router_pipe.send(not_created_resp).await?;
                return Ok(());
            }
        };
        let physical_plan = match planner::PhysicalPlanner::new(logical_plan).build() {
            Ok(plan) => plan,
            Err(err) => {
                info!("error: {}", err);
                let not_created_resp =
                    msg.reply(Box::new(messages::query::RunQueryResp::NotCreated));
                self.router_pipe.send(not_created_resp).await?;
                return Ok(());
            }
        };

        let mut query = query_handler_state::Query::new(run_query.query.clone(), physical_plan);
        query.init();

        let run_query_resp = msg.reply(Box::new(messages::query::RunQueryResp::Created {
            query_id: query.id.clone(),
        }));

        self.state.add_query(query);
        self.router_pipe.send(run_query_resp).await?;

        let in_avail_msg = Message::new(Box::new(
            messages::query::OperatorInstanceAvailable::Notification,
        ));
        self.router_pipe.send(in_avail_msg).await?;

        Ok(())
    }
}

/////////////////////////////////////////////////
// Message subscriber for the query handler
#[derive(Debug)]
pub struct QueryHandlerSubscriber {
    operator_id: u128,
    sender: mpsc::Sender<Message>,
    msg_reg: Arc<MessageRegistry>,
}

impl Subscriber for QueryHandlerSubscriber {}

impl MessageConsumer for QueryHandlerSubscriber {
    fn consumes_message(&self, msg: &Message) -> bool {
        // always accept these messages
        match msg.msg.msg_name() {
            MessageName::RunQuery => return true,
            MessageName::GetQueryStatus => return true,
            MessageName::OperatorInstanceAvailable => {
                match self
                    .msg_reg
                    .try_cast_msg::<messages::query::OperatorInstanceAvailable>(msg)
                {
                    Ok(messages::query::OperatorInstanceAvailable::NotificationResponse {
                        ..
                    }) => return true,
                    _ => return false,
                }
            }
            MessageName::OperatorInstanceAssignment => {
                match self
                    .msg_reg
                    .try_cast_msg::<messages::query::OperatorInstanceAssignment>(msg)
                {
                    Ok(messages::query::OperatorInstanceAssignment::AssignAcceptedResponse {
                        ..
                    }) => return true,
                    Ok(messages::query::OperatorInstanceAssignment::AssignRejectedResponse {
                        ..
                    }) => return true,
                    _ => return false,
                }
            }
            MessageName::QueryHandlerRequests => {
                match self
                    .msg_reg
                    .try_cast_msg::<messages::query::QueryHandlerRequests>(msg)
                {
                    Ok(messages::query::QueryHandlerRequests::ListOperatorInstancesRequest {
                        ..
                    }) => return true,
                    Ok(messages::query::QueryHandlerRequests::ListOperatorInstancesResponse {
                        ..
                    }) => return false,
                    Err(_) => return false,
                }
            }
            MessageName::QueryOperatorInstanceStatusChange => return true,
            _ => (),
        }

        // only accept other messages intended for this operator
        if msg.sent_from_connection_id.is_none()
            && (msg.route_to_connection_id.is_some()
                || msg.route_to_operation_id != Some(self.operator_id))
        {
            return false;
        }

        match msg.msg.msg_name() {
            MessageName::CommonGenericResponse => true,
            _ => false,
        }
    }
}

impl MessageReceiver for QueryHandlerSubscriber {
    fn sender(&self) -> mpsc::Sender<Message> {
        self.sender.clone()
    }
}
