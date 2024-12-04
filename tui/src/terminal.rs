use std::io::{Stdout, stdout};
use std::process::exit;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, stdin};
use tokio::sync::{mpsc, Mutex};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use tokio::sync::mpsc::Receiver;
use crate::{AsciiCode, SharedTerminalParams, TerminalCode, TerminalParams};
use crate::app::App;
use crate::AsciiCode::{ArrowDown, ArrowUp, Backspace, Char, Enter, EoT};

pub async fn make_terminal_app() ->  App<Stdout, fn()> {
    let params: SharedTerminalParams = Arc::new(Mutex::new(get_terminal_params().unwrap()));
    let receiver = create_input_receiver().await;
    App::new(stdout(), receiver, params, || {
        disable_raw_mode().expect("TODO: panic message");
        exit(0)
    })
}

fn get_terminal_params() -> anyhow::Result<TerminalParams> {
    let (cols, rows) = size()?;
    let term = std::env::var("TERM")?;


    Ok(TerminalParams {
        col_width: cols as u32,
        row_height: rows as u32,
        term,
        modes: Vec::new(),
        username: whoami::username()
    })
}

async fn create_input_receiver() -> Receiver<TerminalCode> {
    enable_raw_mode().expect("TODO: panic message");
    
    let (tx, rx) = mpsc::channel::<TerminalCode>(1);

    tokio::spawn(async move {
        let mut buf = Vec::<u8>::new();
        loop {
            stdin().read_buf(&mut buf).await.unwrap();
            for code in channel_data_to_terminal_codes(buf.as_slice()) {
                tx.send(code).await.unwrap()
            }
            buf.clear();
        }
    });

    rx
}

pub fn channel_data_to_terminal_codes(data: &[u8]) -> Vec<TerminalCode> {
    let mut result = Vec::new();

    let mut push_msg = |ascii_code: Option<AsciiCode>, raw_bytes: Vec<u8> |
        result.push(TerminalCode {ascii_code, raw_bytes });

    let mut i = 0;
    while i < data.len() {
        match data[i] {
            27 if i + 1 < data.len() && data[i + 1] == 91 => {
                let start_i = i;
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

                match command.as_slice() {
                    [65] => push_msg(Some(ArrowUp), Vec::from(&data[start_i..i])),
                    [66] => push_msg(Some(ArrowDown), Vec::from(&data[start_i..i])),
                    _ => push_msg(None, Vec::from(&data[start_i..i]))
                }
            }
            127 => {
                push_msg(Some(Backspace), Vec::from(&[data[i]]));
                i += 1
            }
            0..=31 => {
                match data[i] {
                    3 => push_msg(Some(EoT), vec![data[i]]), // ctrl-c
                    8 => push_msg(Some(Backspace), vec![data[i]]),
                    13 => push_msg(Some(Enter), vec![data[i]]),
                    _ => {}
                }
                i += 1;
            }
            _ => {
                push_msg(Some(Char(data[i])), vec![data[i]]);
                i += 1;
            }
        }
    }

    result
}