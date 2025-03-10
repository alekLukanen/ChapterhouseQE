use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::message::{GenericMessage, MessageName, SendableMessage};

///////////////////////////////////////////
//

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Shutdown {
    Immediate,
}

impl GenericMessage for Shutdown {
    fn msg_name() -> MessageName {
        MessageName::OperatorShutdown
    }
    fn build_msg(data: &Vec<u8>) -> Result<Box<dyn SendableMessage>> {
        let msg: Shutdown = serde_json::from_slice(data)?;
        Ok(Box::new(msg))
    }
}

///////////////////////////////////////////
//

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperatorInstanceStatusChange {
    Complete,
    Error(String),
}

impl GenericMessage for OperatorInstanceStatusChange {
    fn msg_name() -> MessageName {
        MessageName::OperatorOperatorInstanceStatusChange
    }
    fn build_msg(data: &Vec<u8>) -> Result<Box<dyn SendableMessage>> {
        let msg: OperatorInstanceStatusChange = serde_json::from_slice(data)?;
        Ok(Box::new(msg))
    }
}
