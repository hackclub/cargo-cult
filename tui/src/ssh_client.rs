use russh::{ChannelMsg, client};
use tokio::sync::mpsc::Receiver;
use async_trait::async_trait;
use std::path::Path;
use tokio::net::ToSocketAddrs;
use std::error::Error;
use std::io::Write;
use std::ops::Deref;
use russh_keys::load_secret_key;
use std::time::Duration;
use std::sync::Arc;
use std::str;
use crate::{SharedTerminalParams, TerminalCode, TerminalParams};

struct ForwardingClient();

#[async_trait]
impl client::Handler for ForwardingClient {
    type Error = russh::Error;

   async fn check_server_key(&mut self, 
                             _key: &russh_keys::key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    } 
}

pub struct SSHForwardingSession<'a, Out: Write> {
    session: client::Handle<ForwardingClient>,

    params: SharedTerminalParams,

    input: &'a mut Receiver<TerminalCode>,
    output: &'a mut Out
}

impl<'a, Out: Write> SSHForwardingSession<'a, Out> {
    pub async fn connect<P: AsRef<Path>, A: ToSocketAddrs>(
        key_path: P,
        user: impl Into<String>,
        addrs: A,
        params: SharedTerminalParams,
        input: &'a mut Receiver<TerminalCode>,
        output: &'a mut Out 
    ) -> Result<SSHForwardingSession<'a, Out>, Box<dyn Error>> {
        let key_pair = load_secret_key(key_path, None)?;

        let config = client::Config {
            inactivity_timeout: Some(Duration::from_secs(60*30)),
            ..<_>::default()
        };

        let config = Arc::new(config);
        let sh = ForwardingClient {};

        let mut session = client::connect(config, addrs, sh).await?;

        let auth_res = session
            .authenticate_publickey(user, Arc::new(key_pair))
            .await?;

        if !auth_res {
            return Err(Box::from("Auth w/ publickey failed"))
        }

        Ok(Self { session, params, input, output})
    }

    pub async fn call(&mut self, command: &str) -> Result<u32, Box<dyn Error>> {
        let mut channel = self.session.channel_open_session().await?;

        let params = self.params.lock().await;
        // todo: handle terminal resize (on ssh server side?)
        let &TerminalParams {row_height, col_width, ref modes, ref term, username: _} = params.deref();

        channel
            .request_pty(
                false,
                term.as_str(),
                col_width,
                row_height,
                0,
                0,
                modes.as_slice(),
            )
            .await?;
        channel.exec(true, command).await?;

        let code;

        loop {
            // Handle one of the possible events:
            tokio::select! {
                // There's terminal input available from the user
                Some(r) = self.input.recv() => {
                    channel.data(r.raw_bytes.as_slice()).await?
                },
                // There's an event available on the session channel
                Some(msg) = channel.wait() => {
                    match msg {
                        // Write data to the terminal
                        ChannelMsg::Data { ref data } => {
                            self.output.write_all(data)?;
                            self.output.flush()?;
                        }
                        // The command has returned an exit code
                        ChannelMsg::ExitStatus { exit_status } => {
                            code = exit_status;
                            channel.eof().await?;
                            break;
                        }
                        _ => {}
                    }
                },
            }
        }

        Ok(code)
    }
}
