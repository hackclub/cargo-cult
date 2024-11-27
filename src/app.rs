use std::cmp::{min};
use std::fmt::{Display, Formatter};
use tokio::sync::mpsc::{Receiver, unbounded_channel, UnboundedReceiver, UnboundedSender};
use crossterm::style::{Color, Print, StyledContent, Stylize};
use std::str;
use crossterm::{ExecutableCommand, execute, queue, QueueableCommand};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::style::Color::Reset;
use std::io::{ErrorKind, Write};
use std::marker::PhantomData;
use std::time::Duration;
use crossterm::terminal::{Clear, DisableLineWrap, EnableLineWrap, SetTitle};
use crossterm::terminal::ClearType::{CurrentLine, FromCursorDown};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use crate::{SharedTerminalParams, TerminalCode};
use crate::AsciiCode::{ArrowDown, ArrowUp, Backspace, Char, Enter, EoT};
use crate::database::{FormData, SubmissionsAirtableBase};
use MenuOptions::{_JoinSlack, Gallery, HowToStart, Submit, WhatIsThis};
use crate::app::TerminalHandleMsg::{Data, Flush};
use crate::ssh_client::SSHForwardingSession;

enum TerminalHandleMsg {
    Flush,
    Data(Vec<u8>),
}

struct AsyncWriter<Out: Write+Send+'static> {
    sender: UnboundedSender<TerminalHandleMsg>,
    _worker: JoinHandle<()>, // auto-exited when sender is dropped

    _phantom_out: PhantomData<Out>
}

impl<Out: Write+Send> AsyncWriter<Out> {
    fn new(out: Out) -> Self {
        let (send, recv) = unbounded_channel::<TerminalHandleMsg>();
        Self {
            sender: send,
            _worker: tokio::spawn(Self::worker(recv, out)),

            _phantom_out: PhantomData
        }
    }

    async fn worker(mut recv: UnboundedReceiver<TerminalHandleMsg>, mut out: Out) {
        while let Some(msg) = recv.recv().await {
            match msg {
                Data(c) => {
                    let _ = out.write(c.as_slice()).unwrap();
                }
                Flush => {
                    out.flush().unwrap();
                }
            }
        }
    }
}

impl<Out: Write+Send> Write for AsyncWriter<Out> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sender.send(Data(Vec::from(buf))).unwrap();

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.sender.send(Flush)
            .map_err(|_| std::io::Error::new(ErrorKind::Other, "Send Error"))
    }
}

pub struct App<Out: Write+Send+'static, F> where F: FnOnce() {
    out: AsyncWriter<Out>,
    input: Receiver<TerminalCode>,
    params: SharedTerminalParams,
    
    exit: Option<F> 
}

impl<Out: Write+Send, F> App<Out, F> where F: FnOnce() {
    pub fn new(out: Out, input: Receiver<TerminalCode>, params: SharedTerminalParams, exit: F) -> Self {
        let writer = AsyncWriter::new(out);
        Self {out: writer, input, params, exit: Some(exit)}
    }
}

#[derive(Clone)]
enum MenuOptions {
    WhatIsThis,
    HowToStart,
    Submit,
    _JoinSlack,
    Gallery
}

impl Display for MenuOptions {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            WhatIsThis => "What is this?",
            HowToStart => "How to start",
            Submit => "Submit your project",
            _JoinSlack => "Join us!",
            Gallery => "See the gallery"
        })
    }
}

impl<Out: Write+Send, F> App<Out, F> where F: FnOnce() {
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }

    fn newline(&mut self) -> std::io::Result<()> {
        write!(self.out, "\r\n")?;
        self.flush()
    }

    fn println(&mut self, message: impl Display) -> std::io::Result<()> {
        write!(self.out, "{}", message)?;
        self.newline()
    }

    pub async fn menu(&mut self) -> std::io::Result<()> {
        self.out.execute(SetTitle("cargo cult"))?;
        
        self.newline()?;
        self.println(Self::text_box("Welcome to the Cargo Cult!".white().bold(), Color::DarkRed, 1, 3, 2))?;

        let options = &[WhatIsThis, HowToStart, Gallery, Submit];
        loop {
            match options[self.single_select(options).await?] {
                WhatIsThis => {
                    self.println(format!("Running a little experiment- {} by building command-line apps - we'll start with the Rust Book (chapters 1-12),\
                    and if you publish your app to crates.io {} (11/10), I'll send you a physical copy of the book!\r\n\r\n\
                    I'll be online in #rust all day for the rest of the week to help out, and once your offering is ready, ssh cargo-cult.hackclub.com to submit! The only \
                    criteria is that you make something unique and useful to you.",
                                         "have you ever wanted to learn Rust? This week, let's learn together".bold(),
                                         "by this Sunday night".bold()
                    ))?;
                },
                HowToStart => {
                    self.println("I'd start with the Rust Book (https://doc.rust-lang.org/book/), chapters \
                        1-12! It'll introduce you to the language and show you the constructs you need to make a CLI app.\r\n\r\n\
                        If you're looking to go further (highly recommend if you already know Rust!), I'd recommend taking a look \
                        at some crates built for making CLIs- Clap is great for argument parsing, Crossterm is great for manipulating \
                        the terminal, and Ratatui is great for building out fully-featured TUIs".to_string())?;
                },
                _JoinSlack => {
                    self.println("Join the Hack Club Slack (hackclub.com/slack), and head to the #rust channel to talk to other teenagers \
                    making apps in Rust!".to_string())?;
                },
                Gallery => return self.gallery().await,
                Submit => return self.submission_form().await
            }

            self.newline()?;
        }
    }

    pub async fn gallery(&mut self) -> std::io::Result<()> {
        // TODO: error handling?
        let responses = SubmissionsAirtableBase::new().get().await.expect("getting submissions to wrok");

        let result  = self.single_select(
            responses.iter().map(
                |resp| format!("{}\r\n{}\r\n", resp.package_name.clone().unwrap(), resp.description)
            ).collect::<Vec<String>>().as_slice()
        ).await?;
        let result = responses.get(result).expect("result value to exist");

        let cmd_name = result.package_name.clone().unwrap();
        let cmd_name = cmd_name.as_str();
        let project_name = result.name.as_str();

        self.docker_session(cmd_name, project_name).await;

        Ok(())
    }

    async fn docker_session(&mut self, cmd_name: &str, author_name: &str) {

        let mut session = SSHForwardingSession::connect(
            "id_ed25519",
            "cargo-cult",
            "localhost:2222",
            self.params.clone(),
            &mut self.input,
            &mut self.out
        ).await.unwrap();
        
        let username = self.params.lock().await.username.clone();

        let _ = timeout(Duration::from_secs(60 * 30),
            session.call(format!("docker run -it cargo-cult '{}' '{}' '{}'", username, cmd_name, author_name).as_str())
        ).await;
    }

    async fn submission_form(&mut self) -> std::io::Result<()> {
        let mut data = FormData::new();

        self.println("  First thing's first... what's your name?".bold())?;
        data.name = self.prompt("Fiona Hackworth", true).await?;
        self.newline()?;

        self.println(format!("  Hi, {}! What's your Slack handle?", data.name).bold())?;
        data.slack_handle = self.prompt("@fiona", true).await?;
        self.newline()?;

        self.println("  Now, what's your email?".bold())?;
        data.email = self.prompt("fiona@hackclub.com", true).await?;
        self.newline()?;

        self.println("  Now, for your address. Please fill in the following:".bold())?;
        data.address_line1 = self.prompt("Address Line 1", true).await?;
        data.address_line2 = self.prompt("Address Line 2 (optional)", false).await?;
        data.city = self.prompt("City", true).await?;
        data.state = self.prompt("State/Province", true).await?;
        data.zip = self.prompt("ZIP/Postal Code", true).await?;
        data.country = self.prompt("Country", true).await?;
        self.newline()?;

        self.println(format!("  What's the link to your package on {}?", "crates.io".white().on_dark_magenta()).bold())?;
        data.package_link = self.prompt("https://crates.io/crates/hc-cargo-cult", true).await?;
        self.newline()?;

        self.println("  Write a short description for your project.".bold())?;
        data.description = self.prompt("A CLI form to collect responses for the Cargo Cult YSWS.", true).await?;
        self.newline()?;

        self.println("  How many hours did you spend on your project?".bold())?;
        data.hours = self.prompt("3 hours, plus 5 hours learning Rust", true).await?;
        self.newline()?;

        self.println("   Wahoo! Thanks for submitting. ".white().bold().on_dark_blue())?;
        self.newline()?;

        let mut airtable = SubmissionsAirtableBase::new();
        airtable.create(data).await.expect("uploading to airtable to work");

        Ok(())
    }

    async fn prompt(&mut self, default_text: &str, required: bool) -> std::io::Result<String> {
        let mut render = |text: String| -> Result<(), std::io::Error> {
            execute!(
            self.out,
            Clear(CurrentLine),
            MoveToColumn(0),
            Print("> ".reset().bold()),
            Print(if !text.is_empty() { text.clone() } else { default_text.dark_grey().to_string() })
        )?;
            if text.is_empty() { self.out.execute(MoveToColumn(2))?; }
            Ok(())
        };

        let mut input = "".to_string();
        let mut first_pass = true;

        while first_pass || (required && input.is_empty()) {
            if !first_pass && input.is_empty() {
                render("This field is required!".white().on_dark_red().slow_blink().to_string())?;
            } else {
                render(input.clone())?;
            }

            first_pass = false;

            while let Some(terminal_code) = self.input.recv().await {
                if let Some(code) = terminal_code.ascii_code {
                    match code {
                        Backspace => { input.pop(); }
                        Enter => break,
                        Char(c) => {
                            if let Ok(text) = str::from_utf8(&[c]) {
                                input.push_str(text);
                            }
                        }
                        EoT => self.exit.take().unwrap()(),
                        _ => {}
                    }
                }

                render(input.clone())?;
            }
        }

        self.println("".reset())?;

        Ok(input)
    }

    async fn single_select<T: Clone + Display>(&mut self, options: &[T]) -> Result<usize, std::io::Error> {

        let total_lines: usize = {
            let lines = options.iter().map(|option|
                option.to_string().split("\r\n").count()).sum::<usize>();

            lines + 1 
        };

        let box_rows = {
            let terminal_height = self.params.clone().lock().await.clone().row_height;

            min(total_lines, terminal_height as usize)
        };
        
        let mut scroll_pos = 0;

        let mut index = 0;
        
        // this lambda is extremely cursed but it works. i don't know how or why
        let mut render = |index: usize, first_time: bool| -> std::io::Result<()> {
            self.out.execute(DisableLineWrap)?;
            
            let mut buffer = String::new();
            for (i, option) in options.iter().enumerate() {
                let element = format!("{}{}\r\n",
                                     "> ".bold(),
                                     if index == i {
                                         option.to_string().bold()
                                     } else { option.to_string().reset() }, 
                );
                buffer.push_str(element.as_str());
                let element_lines = element.split("\r\n").count();

                if index == i {
                    let lines = buffer.split("\r\n").count();
                   if lines.saturating_sub(scroll_pos) > box_rows {
                       scroll_pos += lines - scroll_pos - box_rows - 1;
                   } else if lines - element_lines < scroll_pos {
                       scroll_pos = lines - element_lines; 
                   }
                }
            }

            if !first_time {
                queue!(
                self.out,
                    Print("".reset()),
                MoveToColumn(0),
                    MoveUp((box_rows - 1) as u16),
                Clear(FromCursorDown),
            )?;
            }

            let buffer: String = buffer.split("\r\n").skip(scroll_pos).take(box_rows).collect::<Vec<&str>>().join("\r\n");

            self.out.queue(Print(buffer))?;
            self.out.queue(MoveToColumn(1))?;
            self.out.queue(EnableLineWrap)?;
            self.out.flush()?;
            Ok(())
        };


        render(index, true)?;

        while let Some(terminal_code) = self.input.recv().await {
            if let Some(code) = terminal_code.ascii_code {
                match code {
                    Enter => {
                        break;
                    }
                    ArrowUp => {
                        index = index.saturating_sub(1)
                    }
                    ArrowDown => {
                        if index < options.len() - 1 { index += 1 }
                    }
                    EoT => {
                        self.exit.take().unwrap()()
                    }
                    _ => {}
                }
            }

            render(index, false)?;
        }

        self.println("".reset())?;

        Ok(index)
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
                result.push_str(&"\r\n".on(Reset).to_string())
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
        result.push_str("\r\n");

        result.push_str(&top_bottom_lines());

        result
    }
}
