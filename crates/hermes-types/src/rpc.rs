use postcard::from_bytes;
use serde::{Serialize, de::DeserializeOwned};
use zenoh::key_expr::KeyExpr;
use zenoh::query::Query;
use std::ops::Deref;
use crate::{RecvMessage, SendMessage};
use std::time::Duration;

pub struct Request<T>{
    pub parameters: T,
    pub key_expr: KeyExpr<'static>,
}

impl<T> Deref for Request<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.parameters
    }
}

impl<T: DeserializeOwned> TryFrom<&Query> for Request<T>{
    type Error = postcard::Error;
    fn try_from(value: &Query) -> Result<Self, Self::Error> {
        let payload = from_bytes::<T>(&value.payload().unwrap().to_bytes())?;
        Ok(Self{
            parameters: payload,
            key_expr: value.key_expr().clone(),
        })
    }
}

#[derive(Debug)]
pub enum QueryError { Transport, NoReply, Denied(String), Decode }

// no-payload GET (e.g. mode/get)
async fn resolve_reply<T: DeserializeOwned>(
    replies: zenoh::handlers::FifoChannelHandler<zenoh::query::Reply>
) -> Result<RecvMessage<T>, QueryError> {
    let reply = replies.recv_async().await.map_err(|_| QueryError::NoReply)?;
    match reply.result() {
        Ok(sample) => {
            let recv: RecvMessage<T> = sample.clone().try_into().map_err(|_| QueryError::Decode)?;
            Ok(recv)
        }
        Err(e) => Err(QueryError::Denied(format!("{:?}", e))),
    }
}
pub async fn query<T: DeserializeOwned>(
    session: &zenoh::Session,
    key_expr: &str,
    timeout: Duration,
) -> Result<RecvMessage<T>, QueryError> {
    let replies = session.get(key_expr).timeout(timeout).await
        .map_err(|_| QueryError::Transport)?;
    resolve_reply(replies).await
}

// payload-carrying RPC (e.g. request_mode)
pub async fn query_with<P: Serialize, T: DeserializeOwned>(
    session: &zenoh::Session,
    key_expr: &str,
    payload: P,
    timeout: Duration,
) -> Result<RecvMessage<T>, QueryError> {
    let replies = session.get(key_expr).payload(SendMessage::new(payload)).timeout(timeout).await
        .map_err(|_| QueryError::Transport)?;
    resolve_reply(replies).await
}

// fan-out GET (e.g. `robot/*/params/list`): collect every reply until the
// stream closes or the timeout expires, skipping per-reply errors.
pub async fn query_all<T: DeserializeOwned>(
    session: &zenoh::Session,
    key_expr: &str,
    timeout: Duration,
) -> Result<Vec<RecvMessage<T>>, QueryError> {
    let replies = session.get(key_expr).timeout(timeout).await
        .map_err(|_| QueryError::Transport)?;
    let mut out = Vec::new();
    while let Ok(reply) = replies.recv_async().await {
        if let Ok(sample) = reply.result()
            && let Ok(recv) = sample.clone().try_into()
        {
            out.push(recv);
        }
    }
    Ok(out)
}