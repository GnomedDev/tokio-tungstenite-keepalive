#![warn(clippy::pedantic)]

use std::sync::Arc;
use std::marker::PhantomData;
use once_cell::sync::OnceCell;
use std::{pin::Pin, task::Poll};

use futures::{StreamExt, SinkExt, FutureExt};
use tokio_tungstenite::{WebSocketStream, tungstenite, tungstenite::Message};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc::{UnboundedSender as Sender, UnboundedReceiver as Receiver},
};

#[pin_project::pin_project]
pub struct KeptAliveWebSocket<S> {
    #[pin]
    next_chan: Receiver<Message>,
    send_chan: Sender<Message>,
    err_cell: Arc<OnceCell<tungstenite::Error>>,

    phantom: PhantomData<S>
}

impl<S> KeptAliveWebSocket<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    <WebSocketStream<S> as futures::Stream>::Item: Is<Type=Result<Message, tungstenite::Error>> + Send,
    WebSocketStream<S>: futures::Stream,
{
    /// Wraps a websocket with to handle Ping messages as soon as they are recieved, while buffering
    /// other messages to be recieved when consumed as a [`Stream`].
    pub fn new(mut websocket: WebSocketStream<S>) -> Self {
        let (mut next_chan_send, next_chan_recv) = tokio::sync::mpsc::unbounded_channel();
        let (send_chan_send, mut send_chan_recv) = tokio::sync::mpsc::unbounded_channel();

        let err_cell = Arc::new(OnceCell::new());
        let err_cell_clone = err_cell.clone();

        tokio::spawn(async move {
            if let Err(err) = Self::handle_msgs(&mut websocket, &mut next_chan_send, &mut send_chan_recv).await {
                err_cell_clone.set(err).expect("Error has been set before!");
            }
        });

        Self {
            next_chan: next_chan_recv,
            send_chan: send_chan_send,
            err_cell,

            phantom: PhantomData
        }
    }

    async fn handle_msgs(ws: &mut WebSocketStream<S>, next_chan: &mut Sender<Message>, send_chan: &mut Receiver<Message>) -> Result<(), tungstenite::Error> {
        loop {
            futures::select! {
                ws_msg = ws.next() => {
                    let ws_msg = if let Some(msg) = ws_msg {
                        narrow(msg)?
                    } else {
                        return Ok(());
                    };

                    if next_chan.send(ws_msg).is_err() {
                        return Ok(())
                    }
                },
                to_send = send_chan.recv().fuse() => {
                    if let Some(to_send) = to_send {
                        ws.send(to_send).await?;
                    } else{
                        return Ok(())
                    }
                }
            }
        }
    }
}

impl<S> KeptAliveWebSocket<S> {
    /// Sends a message to the websocket, without waiting until it has been sent.
    ///
    /// # Errors
    /// This errors if the websocket has returned an error previously, and this
    /// [`KeptAliveWebSocket`] has been poisoned.
    pub fn send(&self, msg: Message) -> Result<(), &tungstenite::Error> {
        if let Some(err) = self.err_cell.get() {
            return Err(err)
        }

        self.send_chan.send(msg).expect("Background task has been closed!");
        Ok(())
    }

    /// Returns the current poison error, if the websocket has failed to send a message.
    ///
    /// If this is [`Some`], [`KeptAliveWebSocket::send`] and [`Stream`] methods will error
    /// or be a no-op.
    #[must_use]
    pub fn poison(&self) -> Option<&tungstenite::Error> {
        self.err_cell.get()
    }
}

impl<S> futures::Stream for KeptAliveWebSocket<S> {
    type Item = Message;

    fn poll_next(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Option<Self::Item>> {
        if self.err_cell.get().is_some() {
            Poll::Ready(None)
        } else {
            self.project().next_chan.poll_recv(cx)
        }
    }
}

/// Trait to allow [`WebsocketStream`] to be properly constrained to only return [`Message`]
pub trait Is {
    type Type;
    fn into(self) -> Self::Type;
}

impl<T> Is for T {
    type Type = T;
    fn into(self) -> Self::Type {
        self
    }
}

fn narrow<T: Is<Type=U>, U>(t: T) -> U {
    t.into()
}
