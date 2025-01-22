use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;
use tracing::debug;

use crate::handlers::{
    message_handler::{
        Message, MessageName, MessageRegistry, Ping, Pipe, QueryHandlerRequests, Request,
    },
    operator_handler::operator_handler_state::OperatorInstanceConfig,
};

#[derive(Debug, Error)]
pub enum IdentifyExchangeRequestError {
    #[error("operator type not implemented: {0}")]
    OperatorTypeNotImplemented(String),
    #[error("received the wrong message type")]
    ReceivedTheWrongMessageType,
    #[error("received no operator instances for the exchange")]
    ReceivedNoOperatorInstancesForTheExchange,
    #[error("received multiple operator instances for the exchange")]
    ReceivedMultipleOperatorInstancesForTheExchange,
    #[error("exchange operator instance id not set")]
    ExchangeOperatorInstanceIdNotSet,
    #[error("received message without a worker id")]
    ReceivedMessageWithoutAWorkerId,
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error(
        "missing part of response: exchange_operator_instance_id={0:?}, exchange_worker_id={1:?}"
    )]
    MissingPartOfResponse(Option<u128>, Option<u128>),
}

pub struct IdentifyExchangeResponse {
    pub exchange_operator_instance_id: u128,
    pub exchange_worker_id: u128,
}

pub struct IdentifyExchangeRequest<'a> {
    pub exchange_operator_instance_id: Option<u128>,
    pub exchange_worker_id: Option<u128>,
    exchange_id: String,
    query_id: u128,
    pipe: &'a mut Pipe,
    msg_reg: Arc<MessageRegistry>,
}

impl<'a> IdentifyExchangeRequest<'a> {
    pub async fn request_outbound_exhcnage(
        op_in_config: &OperatorInstanceConfig,
        pipe: &'a mut Pipe,
        msg_reg: Arc<MessageRegistry>,
    ) -> Result<IdentifyExchangeResponse> {
        let exchange_id = match &op_in_config.operator.operator_type {
            crate::planner::OperatorType::Producer {
                outbound_exchange_id,
                ..
            } => outbound_exchange_id.clone(),
            crate::planner::OperatorType::Exchange { .. } => {
                return Err(
                    IdentifyExchangeRequestError::OperatorTypeNotImplemented(format!(
                        "{:?}",
                        op_in_config.operator.operator_type
                    ))
                    .into(),
                );
            }
        };

        let mut res = IdentifyExchangeRequest {
            exchange_operator_instance_id: None,
            exchange_worker_id: None,
            exchange_id,
            query_id: op_in_config.query_id,
            pipe,
            msg_reg,
        };
        res.identify_exchange().await?;
        if res.exchange_operator_instance_id.is_some() && res.exchange_worker_id.is_some() {
            Ok(IdentifyExchangeResponse {
                exchange_operator_instance_id: res.exchange_operator_instance_id.unwrap(),
                exchange_worker_id: res.exchange_worker_id.unwrap(),
            })
        } else {
            Err(IdentifyExchangeRequestError::MissingPartOfResponse(
                res.exchange_operator_instance_id.clone(),
                res.exchange_worker_id.clone(),
            )
            .into())
        }
    }

    async fn identify_exchange(&mut self) -> Result<()> {
        self.exchange_operator_instance_id =
            Some(self.get_exchange_operator_instance_id_with_retry(2).await?);
        self.exchange_worker_id = Some(self.get_exchange_worker_id_with_retry(2).await?);
        Ok(())
    }

    async fn get_exchange_worker_id_with_retry(&mut self, num_retries: u8) -> Result<u128> {
        for retry_idx in 0..(num_retries + 1) {
            let res = self.get_exchange_worker_id().await;
            match res {
                Ok(val) => {
                    return Ok(val);
                }
                Err(err) => {
                    if retry_idx == num_retries {
                        return Err(err.context("failed to get the exchange operator worker id"));
                    } else {
                        tokio::time::sleep(std::time::Duration::from_secs(std::cmp::min(
                            retry_idx as u64 + 1,
                            5,
                        )))
                        .await;
                        continue;
                    }
                }
            }
        }
        Err(IdentifyExchangeRequestError::NotImplemented(
            "unable to get the operator worker id but failed to provide error".to_string(),
        )
        .into())
    }

    async fn get_exchange_worker_id(&mut self) -> Result<u128> {
        let mut msg = Message::new(Box::new(Ping::Ping));
        if let Some(exchange_operator_instance_id) = self.exchange_operator_instance_id {
            msg = msg.set_route_to_operation_id(exchange_operator_instance_id);
        } else {
            return Err(IdentifyExchangeRequestError::ExchangeOperatorInstanceIdNotSet.into());
        }

        let resp_msg = self
            .pipe
            .send_request(Request {
                msg,
                expect_response_msg_name: MessageName::Ping,
                timeout: chrono::Duration::seconds(10),
            })
            .await?;

        let ping_msg: &Ping = self.msg_reg.try_cast_msg(&resp_msg)?;
        match ping_msg {
            Ping::Pong => {
                if let Some(worker_id) = resp_msg.sent_from_worker_id {
                    Ok(worker_id)
                } else {
                    Err(IdentifyExchangeRequestError::ReceivedMessageWithoutAWorkerId.into())
                }
            }
            Ping::Ping => Err(IdentifyExchangeRequestError::ReceivedTheWrongMessageType.into()),
        }
    }

    async fn get_exchange_operator_instance_id_with_retry(
        &mut self,
        num_retries: u8,
    ) -> Result<u128> {
        for retry_idx in 0..(num_retries + 1) {
            let res = self.get_exchange_operator_instance_id().await;
            match res {
                Ok(val) => {
                    return Ok(val);
                }
                Err(err) => {
                    if retry_idx == num_retries {
                        return Err(err.context("failed to get the exchange operator instance id"));
                    } else {
                        tokio::time::sleep(std::time::Duration::from_secs(std::cmp::min(
                            retry_idx as u64 + 1,
                            5,
                        )))
                        .await;
                        continue;
                    }
                }
            }
        }
        Err(IdentifyExchangeRequestError::NotImplemented(
            "unable to get the operator instance id but failed to provide error".to_string(),
        )
        .into())
    }

    async fn get_exchange_operator_instance_id(&mut self) -> Result<u128> {
        // find the worker with the exchange
        let list_msg = Message::new(Box::new(
            QueryHandlerRequests::ListOperatorInstancesRequest {
                query_id: self.query_id.clone(),
                operator_id: self.exchange_id.clone(),
            },
        ));

        let resp_msg = self
            .pipe
            .send_request(Request {
                msg: list_msg,
                expect_response_msg_name: MessageName::QueryHandlerRequests,
                timeout: chrono::Duration::seconds(10),
            })
            .await?;

        debug!("received list response");

        let resp_msg: &QueryHandlerRequests = self.msg_reg.try_cast_msg(&resp_msg)?;
        match resp_msg {
            QueryHandlerRequests::ListOperatorInstancesResponse { op_instance_ids } => {
                if op_instance_ids.len() == 1 {
                    Ok(op_instance_ids.get(0).unwrap().clone())
                } else if op_instance_ids.len() == 0 {
                    return Err(
                        IdentifyExchangeRequestError::ReceivedNoOperatorInstancesForTheExchange
                            .into(),
                    );
                } else {
                    return Err(
                        IdentifyExchangeRequestError::ReceivedMultipleOperatorInstancesForTheExchange
                            .into(),
                        );
                }
            }
            _ => {
                return Err(IdentifyExchangeRequestError::ReceivedTheWrongMessageType.into());
            }
        }
    }
}
