use async_trait::async_trait;
use russh::{Channel, ChannelId, Pty, server};
use std::str;
use russh::server::{Auth, Msg, Session, Server as _, Handle};
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;
use std::io::{Write};
use std::time::Duration;
use futures::executor::block_on;
use russh::Error::SendError;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::{Sender};
use tokio::task::JoinHandle;
use crate::{SharedTerminalParams, TerminalCode, TerminalParams};
use crate::app::App;
use crate::terminal::channel_data_to_terminal_codes;

pub async fn ssh_server() {
    let mut key = String::new();
    let mut file = File::open("ssh_key").await.unwrap();
    file.read_to_string(&mut key).await.unwrap();
    let key = russh_keys::decode_secret_key(&key, None).unwrap();

    let config = server::Config {
        inactivity_timeout: Some(Duration::from_secs(3600)),
        auth_rejection_time: Duration::from_secs(3),
        auth_rejection_time_initial: Some(Duration::from_secs(0)),
        keys: vec![key],
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut sh = Server::new();

    sh.run_on_address(config, ("0.0.0.0", 22)).await.unwrap();
}

struct TerminalHandle {
    handle: Handle,
    channel_id: ChannelId,
    
    sink: Vec<u8>
}

impl TerminalHandle {
    fn new(handle: Handle, channel_id: ChannelId) -> Self {
        Self {
            handle, channel_id,
            sink: Vec::new()
        }
    }
}

impl Write for TerminalHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let original_length = buf.len();

        self.sink.extend_from_slice(buf);

        Ok(original_length)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let data = self.sink.clone().into();
        block_on(self.handle.data(self.channel_id, data)).unwrap();
        self.sink.clear();
        
        Ok(())
    }
}


struct Server {
    sender: Option<Sender<TerminalCode>>,
    handle: Option<JoinHandle<()>>,
    params: Option<SharedTerminalParams>,
    
    username: Option<String>
}

impl Server {
    fn new() -> Self {
        Self {
            sender: None,
            handle: None,
            params: None,
            username: None
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(ref handle) = self.handle {
            handle.abort()
        }
    }
}

impl server::Server for Server {
    type Handler = Self;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        Self::new()
    }
    
    fn handle_session_error(&mut self, _error: <Self::Handler as server::Handler>::Error) {
        eprintln!("Session error: {:#?}", _error);
    }
}

#[async_trait]
impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_none(&mut self, user: &str) -> Result<Auth, Self::Error> {
        dbg!(user);
        self.username = Some(user.to_string());
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {

        for code in channel_data_to_terminal_codes(data) {
            self.sender.as_ref().ok_or(SendError)?.send(code).await.expect("sending to work")
        }

        Ok(())
    }

    async fn pty_request(&mut self,
                         channel: ChannelId,
                         term: &str,
                         col_width: u32,
                         row_height: u32,
                         _pix_width: u32,
                         _pix_height: u32,
                         modes: &[(Pty, u32)],
                         session: &mut Session) -> Result<(), Self::Error> {
        
        let mut terminal_handle = TerminalHandle::new(session.handle(), channel);
        terminal_handle.flush()?;

        let terminal_params = Arc::from(Mutex::from(TerminalParams {
            term: String::from(term),
            col_width,
            row_height,
            modes: Vec::from(modes),
            username: self.username.take().unwrap()
        }));
       
        let (tx, rx) = mpsc::channel(1);

        let handle = session.handle();

        let mut app = {
            let handle = handle.clone();
            App::new(terminal_handle, rx, terminal_params.clone(), move || {
                tokio::spawn(async move {
                    handle.eof(channel).await.unwrap();
                    handle.close(channel).await.unwrap();
                });
            })
        };
        
        self.sender = Some(tx);

        {
            let terminal_params = terminal_params.clone();
            let handle = handle.clone();
            self.handle = Some(tokio::spawn(async move {
                let _ = tokio::spawn(async move {
                    let username = terminal_params.clone().lock().await.username.clone();
                    if username.starts_with("[") && username.ends_with("]") {
                        app.run_project(username[1..username.len() - 1].to_string()).await.unwrap();
                    } else {
                        app.run().await.unwrap();
                    }
                }).await;
                
                handle.eof(channel).await.unwrap();
                handle.close(channel).await.unwrap();
            }));
        }

        self.params = Some(terminal_params.clone());

        Ok(())
    }
}
