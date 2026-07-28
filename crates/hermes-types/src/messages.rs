use serde::{Deserialize, Serialize};
use zenoh::{key_expr::KeyExpr, bytes::ZBytes, sample::{Sample, SampleKind}, time::Timestamp};
use postcard::{to_allocvec, from_bytes};
use serde::de::DeserializeOwned;
use std::ops::Deref;
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MsgMetadata{
    pub key_expr: KeyExpr<'static>,
    pub kind: SampleKind,
    pub timestamp: Option<Timestamp>,
}

/// The message sending struct. The only real purpose of this is to serialize a message cleanly.
#[derive(Debug, Clone)]
pub struct SendMessage<T>{
    pub payload: T
}

impl<T> Deref for SendMessage<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.payload
    }

}

impl<T> SendMessage<T> {
    pub fn new(payload: T) -> Self {
        Self {
            payload
        }
    }
    pub fn into_inner(self) -> T {
        self.payload
    }
}

/// Dangerous! Possible to panic from here, careful with your structs!
impl<T: Serialize> From<SendMessage<T>> for ZBytes {
    fn from(m: SendMessage<T>) -> ZBytes {
        ZBytes::from(to_allocvec(&m.payload).expect("Could not serialize message. Check if type is serializable."))
    }
}

/// The Received Message unpacker. Obtains both the core info and metadata related to it.
#[derive(Debug, Clone)]
pub struct RecvMessage<T> {
    pub payload: T,
    pub metadata: MsgMetadata
}

impl<T> Deref for RecvMessage<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.payload
    }

}

impl<T> RecvMessage<T> {
    pub fn new(payload: T) -> Self {
        Self {
            payload,
            metadata: MsgMetadata{
                key_expr: KeyExpr::new("a/b").unwrap(),
                kind: SampleKind::Put,
                timestamp: None
            }
        }
    }
    pub fn key_expr(&self) -> &KeyExpr<'static> {
        &self.metadata.key_expr
    }

    pub fn kind(&self) -> SampleKind {
        self.metadata.kind // on the back it's represented by u8 so no pressure in cloning
    }

    pub fn timestamp(&self) -> Option<Timestamp> {
        self.metadata.timestamp
    }

    pub fn system_time(&self) -> Option<SystemTime> {
        Some(self.metadata.timestamp.as_ref()?.get_time().to_system_time())
    }
}

impl<T: DeserializeOwned> TryFrom<Sample> for RecvMessage<T> {
    type Error = postcard::Error;
    fn try_from(value: Sample) -> Result<Self, Self::Error> {
        let payload = from_bytes::<T>(&value.payload().to_bytes())?;
        Ok(Self {
            payload,
            metadata: MsgMetadata{
                key_expr: value.key_expr().clone(),
                kind: value.kind(),
                timestamp: value.timestamp().cloned(),
            }
        })
    }
}
