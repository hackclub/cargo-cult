use std::io::{ErrorKind, Write as _};
use crossterm::style::{Color, Print, StyledContent, Stylize};
use crossterm::{ExecutableCommand, execute};
use crossterm::cursor::{MoveToColumn};
use crossterm::style::Color::{Reset};
use crossterm::terminal::{Clear};
use crossterm::terminal::ClearType::CurrentLine;

use russh::{server::{Auth, Session}, ChannelId, server, Channel};
use std::collections::{HashMap};
use std::fmt::Display;
use std::io::ErrorKind::NotFound;
use tokio::fs::{File, OpenOptions};
use tokio::sync::Mutex;
use async_trait::async_trait;
use russh::server::Msg;
use russh::server::Server as _;
use russh::server::Handle;
use tokio::task::JoinHandle;
use std::str;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Sender, Receiver, UnboundedSender, UnboundedReceiver, unbounded_channel};
use crate::AsciiCode::{Backspace, Char, Enter};
use crate::TerminalHandleMsg::{Data, Flush};

struct SSHClient(Sender<AsciiCode>, JoinHandle<()>);

impl Drop for SSHClient {
    fn drop(&mut self) {
        let SSHClient(_, handle) = self;
        handle.abort();
    }
}

#[derive(Clone)]
struct Server {
    clients: Arc<Mutex<HashMap<usize, SSHClient>>>,
    id: usize,
}

struct TerminalHandle {
    sender: UnboundedSender<TerminalHandleMsg>,
    _worker: JoinHandle<()> // auto-exited when sender is dropped
}

enum TerminalHandleMsg {
    Flush,
    Data(Vec<u8>)
}

impl TerminalHandle {
    fn new(handle: Handle, channel_id: ChannelId) -> Self {
        let (send, recv) = unbounded_channel::<TerminalHandleMsg>();
        let sink = Vec::new();
        Self {
            sender: send,
            _worker: tokio::spawn(Self::worker(sink, recv, handle, channel_id)),
        }
    }
    
    async fn worker(mut sink: Vec<u8>, mut recv: UnboundedReceiver<TerminalHandleMsg>, handle: Handle, channel_id: ChannelId) {
        while let Some(msg) = recv.recv().await {
            let sink = &mut sink;
            match msg {
                Data(c) => {
                    sink.extend_from_slice(c.as_slice());
                }
                Flush => {
                    let data = sink.clone().into(); 
                    handle.data(channel_id, data).await.unwrap();
                    sink.clear(); 
                }
            }

        }
    }
}

impl std::io::Write for TerminalHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let original_length = buf.len();

        let buf = String::from_utf8_lossy(buf);
        let buf = buf.replace('\n', "\r\n");
        
        self.sender.send(Data(Vec::from(buf))).unwrap();

        Ok(original_length)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.sender.send(Flush)
            .map_err(|_| std::io::Error::new(ErrorKind::Other, "Send Error")) 
    }
}

impl server::Server for Server {
    type Handler = Self;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        let s = self.clone();
        self.id += 1;
        s
    }
    fn handle_session_error(&mut self, _error: <Self::Handler as server::Handler>::Error) {
        eprintln!("Session error: {:#?}", _error);
    }
}

enum AsciiCode {
    Char(u8),
    Backspace,
    Enter,
}

#[async_trait]
impl server::Handler for Server {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_close(&mut self, _channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        let mut clients = self.clients.lock().await;
        clients.remove(&self.id).expect("key to exist");
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let mut clients = self.clients.lock().await;
        let mut terminal_handle = TerminalHandle::new(session.handle(), channel.id()); 

        terminal_handle.flush()?; 

        let (tx, rx) = mpsc::channel::<AsciiCode>(1);

        clients.insert(self.id,
                       SSHClient(tx,
                        tokio::spawn(async move {
                            YSWSForm { out: terminal_handle, input: rx }.run().await.unwrap();
                            channel.eof().await.unwrap();
                            channel.close().await.unwrap();
                        })));


        Ok(true)
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let clients = self.clients.lock().await;
        let SSHClient(sender, _) = clients.get(&self.id).expect("client to exist");

        let mut i = 0;
        while i < data.len() {
            match data[i] {
                27 if i + 1 < data.len() && data[i + 1] == 91 => {
                    i += 2;

                    let mut command = Vec::new();

                    while i < data.len() && data[i].is_ascii() {
                        command.push(data[i]);
                        i += 1;
                    }

                    if i < data.len() {
                        command.push(data[i]);
                        i += 1
                    }
                }
                127 => {
                    sender.send(Backspace).await.expect("sending to work");
                    i += 1
                }
                0..=31 => {
                    match data[i] {
                        3 => session.close(channel), // ctrl-c
                        8 => sender.send(Backspace).await.expect("sending to work"), // backspace
                        13 => sender.send(Enter).await.expect("sending to work"), // enter
                        _ => {}
                    }
                    i += 1;
                }
                _ => {
                    sender.send(Char(data[i])).await.expect("sending to work");
                    i += 1;
                }
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() {
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
    let mut sh = Server {
        clients: Arc::new(Mutex::new(HashMap::new())),
        id: 0,
    };

    sh.run_on_address(config, ("0.0.0.0", 2222)).await.unwrap();
}

struct FormData {
    name: String,
    slack_handle: String,
    email: String,

    address_line1: String,
    address_line2: String,
    city: String,
    state: String,
    zip: String,
    country: String,

    package_link: String,
    description: String,
    hours: String,
}

impl FormData {
    fn new() -> Self {
        Self {
            name: "".to_string(),
            slack_handle: "".to_string(),
            email: "".to_string(),
            address_line1: "".to_string(),
            address_line2: "".to_string(),
            city: "".to_string(),
            state: "".to_string(),
            zip: "".to_string(),
            country: "".to_string(),
            package_link: "".to_string(),
            description: "".to_string(),
            hours: "".to_string(),
        }
    }
}

impl Display for FormData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,
               "\
               Name: {}\n\
               Slack Handle: {}\n\
               Email: {}\n\
               Address:\
               \n{}\n{}{}{}\n{}\n{}\n{}\n\
               Package Link: {}\n\
               Description: \
               \n{}\n\
               Hours: {}\n\n",
               self.name,
               self.slack_handle,
               self.email,
               self.address_line1,
               self.address_line2,
               if self.address_line2.is_empty() { "" } else { "\n" },
               self.city,
               self.state,
               self.zip,
               self.country,
               self.package_link,
               self.description,
               self.hours)
    }
}

struct YSWSForm {
    out: TerminalHandle,
    input: Receiver<AsciiCode>,
}

impl YSWSForm {
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    fn newline(&mut self) -> std::io::Result<()> {
        writeln!(self.out)?;
        self.flush()
    }

    fn println(&mut self, message: String) -> std::io::Result<()> {
        write!(self.out, "{}", message)?;
        self.newline()
    }

    async fn run(&mut self) -> Result<(), std::io::Error> {
        let mut data = FormData::new();

        self.newline()?;
        self.println(Self::text_box("Welcome to the Cargo Cult!".white().bold(), Color::DarkRed, 1, 3, 2))?;

        self.println("  First thing's first... what's your name?".bold().to_string())?;
        data.name = self.prompt("Fiona Hackworth", true).await?;
        self.newline()?;

        self.println(format!("  Hi, {}! What's your Slack handle?", data.name).bold().to_string())?;
        data.slack_handle = self.prompt("@fiona", true).await?;
        self.newline()?;

        self.println("  Now, what's your email?".bold().to_string())?;
        data.email = self.prompt("fiona@hackclub.com", true).await?;
        self.newline()?;

        self.println("  Now, for your address. Please fill in the following:".bold().to_string())?;
        data.address_line1 = self.prompt("Address Line 1", true).await?;
        data.address_line2 = self.prompt("Address Line 2 (optional)", false).await?;
        data.city = self.prompt("City", true).await?;
        data.state = self.prompt("State/Province", true).await?;
        data.zip = self.prompt("ZIP/Postal Code", true).await?;
        data.country = self.prompt("Country", true).await?;
        self.newline()?;

        self.println(format!("  What's the link to your package on {}?", "crates.io".white().on_dark_magenta()).bold().to_string())?;
        data.package_link = self.prompt("https://crates.io/crates/hc-cargo-cult", true).await?;
        self.newline()?;

        self.println("  Write a short description for your project.".bold().to_string())?;
        data.description = self.prompt("A CLI form to collect responses for the Cargo Cult YSWS.", true).await?;
        self.newline()?;

        self.println("  How many hours did you spend on your project?".bold().to_string())?;
        data.hours = self.prompt("3 hours, plus 5 hours learning Rust", true).await?;
        self.newline()?;

        self.println("  ".to_owned() + &" Wahoo! Thanks for submitting. ".white().bold().on_dark_blue().to_string())?;
        self.newline()?;

        println!("{}", data);

        let mut file = match OpenOptions::new().append(true).open("responses.txt").await {
            Ok(file) => file,
            Err(err) if err.kind() == NotFound => File::create_new("responses.txt").await.expect("opening file to work"),
            other => other.unwrap() 
        };
        file.write_all(data.to_string().as_bytes()).await?;

        Ok(())
    }

    fn write_prompt(&mut self, text: String, default_text: &str) -> Result<(), std::io::Error> {
        execute!(
            self.out,
            Clear(CurrentLine),
            MoveToColumn(0),
            Print("> ".reset().bold()),
            Print(if !text.is_empty() { text.clone() } else { default_text.dark_grey().to_string() })
        )?;
        if text.is_empty() { self.out.execute(MoveToColumn(2))?; }
        Ok(())
    }

    async fn prompt(&mut self, default_text: &str, required: bool) -> Result<String, std::io::Error> {
        let mut input = "".to_string();
        let mut first_pass = true;

        while first_pass || (required && input.is_empty()) {
            if !first_pass && input.is_empty() {
                self.write_prompt("This field is required!".white().on_dark_red().slow_blink().to_string(), default_text)?;
            } else {
                self.write_prompt(input.clone(), default_text)?;
            }

            first_pass = false;

            while let Some(code) = self.input.recv().await {
                match code {
                    Backspace => {
                        input.pop();
                    }
                    Enter => {
                        self.write_prompt(input.clone(), default_text)?;
                        break;
                    }
                    Char(c) => {
                        if let Ok(text) = str::from_utf8(&[c]) {
                            input.push_str(text);
                        }
                    }
                }

                self.write_prompt(input.clone(), default_text)?;
            }
        }

        self.println("".reset().to_string())?;
        self.flush()?;

        Ok(input)
    }

    fn text_box(text: StyledContent<&str>, bg: Color, padding_y: usize, padding_x: usize, margin_x: usize) -> String {
        let mut result = String::new();
        let src_len = text.content().len();

        let margin_x = ||
            " ".repeat(margin_x)
                .on(Reset).to_string();

        let top_bottom_lines = || {
            let mut result = String::new();

            for _ in 0..padding_y {
                result.push_str(&margin_x());
                result.push_str(
                    &" ".repeat(padding_x * 2 + src_len)
                        .on(bg).to_string()
                );
                result.push_str(&"\n".on(Reset).to_string())
            }
            result
        };

        let pad_x = ||
            " ".repeat(padding_x)
                .on(bg).to_string();

        result.push_str(&top_bottom_lines());

        result.push_str(&margin_x());
        result.push_str(&pad_x());
        result.push_str(&text.on(bg).to_string());
        result.push_str(&pad_x());
        result.push('\n');

        result.push_str(&top_bottom_lines());

        result
    }
}

