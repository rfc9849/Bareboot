#[cfg(unix)]
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use futures::channel::mpsc::{UnboundedReceiver, unbounded};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Show,
}

impl Message {
    pub fn encode(&self) -> &'static str {
        match self {
            Self::Show => "show",
        }
    }

    pub fn decode(line: &str) -> Option<Self> {
        (line.trim() == "show").then_some(Self::Show)
    }
}

pub enum Role {
    Primary(UnboundedReceiver<Message>),
    Secondary,
}

#[cfg(unix)]
pub fn acquire(path: &Path, message: &Message) -> Role {
    use std::os::unix::net::{UnixListener, UnixStream};
    if let Ok(mut stream) = UnixStream::connect(path) {
        let _ = writeln!(stream, "{}", message.encode());
        return Role::Secondary;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(path);
    let (sender, receiver) = unbounded();
    match UnixListener::bind(path) {
        Ok(listener) => {
            let _ = sender.unbounded_send(message.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.take(1024));
                    if reader.read_line(&mut line).is_ok()
                        && let Some(msg) = Message::decode(&line)
                        && sender.unbounded_send(msg).is_err()
                    {
                        break;
                    }
                }
            });
            Role::Primary(receiver)
        }
        Err(_) => Role::Primary(receiver),
    }
}

#[cfg(not(unix))]
pub fn acquire(_: &Path, message: &Message) -> Role {
    let (sender, receiver) = unbounded();
    let _ = sender.unbounded_send(message.clone());
    std::mem::forget(sender);
    Role::Primary(receiver)
}
