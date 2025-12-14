use crate::error::KrakenError;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use tokio_tungstenite::{
    connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};
use tracing::{error, info, warn};
use url::Url;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct ConnectionManager {
    url: Url,
    event_sender: mpsc::Sender<Result<String, KrakenError>>,
    command_receiver: mpsc::Receiver<String>,
}

impl ConnectionManager {
    pub fn new(
        url: &str,
        event_sender: mpsc::Sender<Result<String, KrakenError>>,
        command_receiver: mpsc::Receiver<String>,
    ) -> Result<Self, KrakenError> {
        Ok(Self {
            url: Url::parse(url)?,
            event_sender,
            command_receiver,
        })
    }

    // The main loop, this will run forever untill the app closes,
    pub async fn run(mut self) {
    loop {
        info!("Connecting to {}...", self.url);

        match connect_async(self.url.as_str()).await {
            Ok((ws_stream, _)) => {
                info!("Connected to Kraken");

                // we split the socket
                // 'write' sends messages to Kraken.
                // 'read' lsitens for messages from Kraken.

                let (mut write, mut read) = ws_stream.split();

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    if self.event_sender.send(Ok(text.to_string())).await.is_err(){
                                        break; // client dropped, stop engine
                                    }
                                }
                                Some(Ok(Message::Ping(_))) => {
                                    // Auto Reply with pong 
                                }
                                Some(Err(e)) => {
                                    error!("Websocker error: {}", e);
                                    break;
                                }
                                None => {
                                    warn!("Stream Ended Unexpectedly");
                                    break;
                                }
                                _ => {}
                            }
                        }
                        cmd = self.command_receiver.recv() => {
                            match cmd{
                                Some(payload) => {
                                    if let Err(e) = write.send(Message::Text(payload.into())).await {
                                        error!("Failed to send message to Kraken: {}", e);
                                        break;
                                    }
                                }
                                None => {
                                    info!("Client command channel closed, Shutting down...");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Connection failed: {}. Retrying....",e);
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
    }
}