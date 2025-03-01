use std::sync::Arc;

use anyhow::Result;
use bytes::BytesMut;
use thiserror::Error;

use super::messages;
use super::messages::message::{
    GenericMessageParser, Message, MessageParser, SendableMessage, SerializedMessage,
    SerializedMessageError,
};
use super::messages::query::GetQueryDataRespParser;

#[derive(Debug, Clone, Error)]
pub enum MessageRegistryError {
    #[error("incomplete message")]
    IncompleteMessage,
    #[error("message id not in registry: {0}")]
    MessageIdNotInRegistry(u16),
    #[error("unable to cast message to expected type: {0}")]
    UnableToCastMessageToExpectedType(String),
}

#[derive(Debug)]
pub struct RegisteredMessage {
    msg_parser: Box<dyn MessageParser>,
}

#[derive(Debug)]
pub struct MessageRegistry {
    msg_listing: Vec<Arc<RegisteredMessage>>,
}

impl MessageRegistry {
    pub fn new() -> MessageRegistry {
        let mut reg = MessageRegistry {
            msg_listing: Vec::new(),
        };
        reg.register_messages();
        reg
    }

    fn register_messages(&mut self) {
        // common
        self.add(Box::new(
            GenericMessageParser::<messages::common::Ping>::new(),
        ));
        self.add(Box::new(
            GenericMessageParser::<messages::common::Identify>::new(),
        ));
        self.add(Box::new(GenericMessageParser::<
            messages::common::GenericResponse,
        >::new()));

        // query
        self.add(Box::new(
            GenericMessageParser::<messages::query::RunQuery>::new(),
        ));
        self.add(Box::new(GenericMessageParser::<
            messages::query::RunQueryResp,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::OperatorInstanceAvailable,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::OperatorInstanceAssignment,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::QueryHandlerRequests,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::OperatorInstanceStatusChange,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::GetQueryStatus,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::GetQueryStatusResp,
        >::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::query::GetQueryData,
        >::new()));
        self.add(Box::new(GetQueryDataRespParser::new()));

        // operator
        self.add(Box::new(GenericMessageParser::<
            messages::operator::OperatorInstanceStatusChange,
        >::new()));
        self.add(Box::new(
            GenericMessageParser::<messages::operator::Shutdown>::new(),
        ));

        // exchange
        self.add(Box::new(messages::exchange::ExchangeRequestsParser::new()));
        self.add(Box::new(GenericMessageParser::<
            messages::exchange::OperatorStatusChange,
        >::new()));
    }

    pub async fn build_msg(&self, buf: &mut BytesMut) -> Result<Option<Message>> {
        let reg_msg_id = SerializedMessage::parse_registered_msg_id(buf)?;

        let reg_msg = if let Some(reg_msg) = self.find_by_reg_msg_id(reg_msg_id.clone()) {
            reg_msg
        } else {
            return Err(MessageRegistryError::MessageIdNotInRegistry(reg_msg_id.clone()).into());
        };

        match SerializedMessage::parse(buf) {
            Ok(ser_msg) => {
                let msg = tokio::task::spawn_blocking(move || reg_msg.msg_parser.to_msg(ser_msg))
                    .await??;
                Ok(Some(msg))
            }
            Err(SerializedMessageError::Incomplete) => {
                Err(MessageRegistryError::IncompleteMessage.into())
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn build_msg_bytes(&self, msg: Message) -> Result<Vec<u8>> {
        let data = tokio::task::spawn_blocking(move || msg.to_bytes()).await??;
        Ok(data)
    }

    pub fn cast_msg<'a, T: SendableMessage>(&'a self, msg: &'a Message) -> &'a T
    where
        T: 'static + SendableMessage,
    {
        match msg.msg.as_any_ref().downcast_ref::<T>() {
            Some(cast_msg) => return cast_msg,
            None => panic!("unable to cast message by name: {}", msg.msg.msg_name()),
        }
    }

    pub fn try_cast_msg<'a, T: SendableMessage>(&'a self, msg: &'a Message) -> Result<&'a T>
    where
        T: 'static + SendableMessage,
    {
        match msg.msg.as_any_ref().downcast_ref::<T>() {
            Some(cast_msg) => Ok(cast_msg),
            None => Err(MessageRegistryError::UnableToCastMessageToExpectedType(
                msg.msg.msg_name().to_string(),
            )
            .into()),
        }
    }

    pub fn try_cast_msg_owned<T: SendableMessage>(&self, msg: Message) -> Result<T> {
        let msg_name = msg.msg.msg_name();
        match msg.msg.as_any().downcast::<T>() {
            Ok(cast_msg) => Ok(*cast_msg),
            Err(_) => Err(MessageRegistryError::UnableToCastMessageToExpectedType(
                msg_name.to_string(),
            )
            .into()),
        }
    }

    pub fn add(&mut self, msg_parser: Box<dyn MessageParser>) {
        self.msg_listing
            .push(Arc::new(RegisteredMessage { msg_parser }));
    }

    fn find_by_reg_msg_id(&self, reg_msg_id: u16) -> Option<Arc<RegisteredMessage>> {
        if let Some(reg_msg) = self
            .msg_listing
            .iter()
            .find(|&item| item.msg_parser.msg_name().as_u16() == reg_msg_id)
        {
            Some(reg_msg.clone())
        } else {
            None
        }
    }
}
