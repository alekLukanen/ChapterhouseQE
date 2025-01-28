use core::fmt;
use tokio::sync::mpsc::{self, Sender};

use crate::handlers::message_handler::messages::message::Message;

pub trait MessageConsumer: fmt::Debug + Send + Sync {
    fn consumes_message(&self, msg: &Message) -> bool;
}

pub trait MessageReceiver: fmt::Debug + Send + Sync {
    fn sender(&self) -> mpsc::Sender<Message>;
}

pub trait Subscriber: MessageConsumer + MessageReceiver {}

#[derive(Debug)]
pub struct InternalSubscriber {
    pub sub: Box<dyn Subscriber>,
    pub sender: Sender<Message>,
}

impl InternalSubscriber {
    pub fn new(sub: Box<dyn Subscriber>, sender: Sender<Message>) -> InternalSubscriber {
        InternalSubscriber { sub, sender }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExternalSubscriber {
    InboundClientConnection {
        connection_id: u128,
        inbound_stream_id: u128,
    },
    OutboundWorker {
        worker_id: u128,
        outbound_stream_id: u128,
    },
}

impl MessageConsumer for ExternalSubscriber {
    fn consumes_message(&self, msg: &Message) -> bool {
        match self {
            Self::InboundClientConnection { connection_id, .. } => match msg.route_to_connection_id
            {
                Some(rid) => *connection_id == rid,
                _ => false,
            },
            Self::OutboundWorker { worker_id, .. } => {
                match (msg.route_to_worker_id, msg.route_to_connection_id) {
                    (Some(w_rid), None) => *worker_id == w_rid,
                    (None, None) => true,
                    _ => false,
                }
            }
        }
    }
}
