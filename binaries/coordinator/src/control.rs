use crate::{
    tcp_utils::{tcp_receive, tcp_send},
    Event,
};
use dora_core::topics::{ControlRequest, ControlRequestReply};
use eyre::{eyre, Context};
use futures::{
    future::{self, Either},
    FutureExt, Stream, StreamExt,
};
use futures_concurrency::future::Race;
use std::{io::ErrorKind, net::SocketAddr};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;

pub(crate) async fn control_events(
    control_listen_addr: SocketAddr,
) -> eyre::Result<impl Stream<Item = Event>> {
    let (tx, rx) = mpsc::channel(10);

    tokio::spawn(listen(control_listen_addr, tx));

    Ok(ReceiverStream::new(rx).map(Event::Control))
}

async fn listen(control_listen_addr: SocketAddr, tx: mpsc::Sender<ControlEvent>) {
    let result = TcpListener::bind(control_listen_addr)
        .await
        .wrap_err("failed to listen for control messages");
    let incoming = match result {
        Ok(incoming) => incoming,
        Err(err) => {
            let _ = tx.blocking_send(err.into());
            return;
        }
    };

    loop {
        let new_connection = incoming.accept().map(Either::Left);
        let coordinator_stop = tx.closed().map(Either::Right);
        let connection = match (new_connection, coordinator_stop).race().await {
            future::Either::Left(connection) => connection,
            future::Either::Right(()) => {
                // coordinator was stopped
                break;
            }
        };
        match connection.wrap_err("failed to connect") {
            Ok((connection, _)) => {
                let tx = tx.clone();
                tokio::spawn(handle_requests(connection, tx));
            }
            Err(err) => {
                if tx.blocking_send(err.into()).is_err() {
                    break;
                }
            }
        }
    }
}

async fn handle_requests(mut connection: TcpStream, tx: mpsc::Sender<ControlEvent>) {
    loop {
        let next_request = tcp_receive(&mut connection).map(Either::Left);
        let coordinator_stopped = tx.closed().map(Either::Right);
        let raw = match (next_request, coordinator_stopped).race().await {
            Either::Right(()) => break,
            Either::Left(request) => match request {
                Ok(message) => message,
                Err(err) => match err.kind() {
                    ErrorKind::UnexpectedEof => {
                        tracing::trace!("Control connection closed");
                        break;
                    }
                    err => {
                        let err = eyre!(err).wrap_err("failed to receive incoming message");
                        tracing::error!("{err}");
                        break;
                    }
                },
            },
        };

        let result =
            match serde_json::from_slice(&raw).wrap_err("failed to deserialize incoming message") {
                Ok(request) => handle_request(request, &tx).await,
                Err(err) => Err(err),
            };

        let reply = result.unwrap_or_else(|err| ControlRequestReply::Error(format!("{err}")));
        let serialized =
            match serde_json::to_vec(&reply).wrap_err("failed to serialize ControlRequestReply") {
                Ok(s) => s,
                Err(err) => {
                    tracing::error!("{err:?}");
                    break;
                }
            };
        match tcp_send(&mut connection, &serialized).await {
            Ok(()) => {}
            Err(err) => match err.kind() {
                ErrorKind::UnexpectedEof => {
                    tracing::debug!("Control connection closed while trying to send reply");
                    break;
                }
                err => {
                    let err = eyre!(err).wrap_err("failed to send reply");
                    tracing::error!("{err}");
                    break;
                }
            },
        }

        if matches!(reply, ControlRequestReply::CoordinatorStopped) {
            break;
        }
    }
}

async fn handle_request(
    request: ControlRequest,
    tx: &mpsc::Sender<ControlEvent>,
) -> eyre::Result<ControlRequestReply> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let event = ControlEvent::IncomingRequest {
        request,
        reply_sender: reply_tx,
    };

    if tx.send(event).await.is_err() {
        return Ok(ControlRequestReply::CoordinatorStopped);
    }

    reply_rx
        .await
        .unwrap_or(Ok(ControlRequestReply::CoordinatorStopped))
}

#[derive(Debug)]
pub enum ControlEvent {
    IncomingRequest {
        request: ControlRequest,
        reply_sender: oneshot::Sender<eyre::Result<ControlRequestReply>>,
    },
    Error(eyre::Report),
}

impl From<eyre::Report> for ControlEvent {
    fn from(err: eyre::Report) -> Self {
        ControlEvent::Error(err)
    }
}
