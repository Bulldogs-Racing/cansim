//! `canlab serve`: local WebSocket API for the visual editor.
//!
//! Sync threads only (tungstenite server, no async runtime): one thread per
//! connection, one shared `Mutex<Session>`. Requests are handled
//! sequentially per connection; a slow client cannot corrupt the session.
//!
//! Every request gets exactly one JSON reply. `GetEvents { sinceSeq }` is
//! the analyzer's poll primitive — the GUI polls it and renders new rows.

use super::proto::{parse_client_msg, seq_event, state_name, ClientMsg, ServerMsg};
use super::session::Session;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tungstenite::Message;

/// Serve forever on `127.0.0.1:port`. Returns only on accept failure.
pub fn serve(port: u16, preload: Option<&Path>) -> i32 {
    let session = Arc::new(Mutex::new(Session::new()));
    if let Some(path) = preload {
        match session.lock().unwrap().load(path) {
            Ok(()) => println!("Loaded {}", path.display()),
            Err(e) => {
                eprintln!("cannot preload {}: {e}", path.display());
                return 1;
            }
        }
    }
    let addr = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {addr}: {e}");
            return 1;
        }
    };
    println!("CanLab API serving on ws://{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let session = Arc::clone(&session);
                std::thread::spawn(move || handle_conn(stream, session));
            }
            Err(e) => {
                eprintln!("accept failed: {e}");
                return 1;
            }
        }
    }
    0
}

/// Bind an ephemeral port for tests; returns the chosen port.
pub fn serve_ephemeral(session: Arc<Mutex<Session>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let session = Arc::clone(&session);
                    std::thread::spawn(move || handle_conn(stream, session));
                }
                Err(_) => break,
            }
        }
    });
    port
}

fn handle_conn(stream: std::net::TcpStream, session: Arc<Mutex<Session>>) {
    let mut ws = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(_) => return,
    };
    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            Message::Text(text) => {
                let reply = dispatch(&session, &text);
                let out = serde_json::to_string(&reply)
                    .unwrap_or_else(|_| r#"{"type":"Error","message":"encode failed"}"#.into());
                if ws.send(Message::Text(out.into())).is_err() {
                    break;
                }
            }
            Message::Binary(_) => {
                let reply = ServerMsg::Error {
                    message: "binary frames not supported; send JSON text".into(),
                };
                if ws
                    .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                    .is_err()
                {
                    break;
                }
            }
            Message::Ping(data) => {
                if ws.send(Message::Pong(data)).is_err() {
                    break;
                }
            }
            Message::Close(_) => {
                let _ = ws.close(None);
                break;
            }
            _ => {}
        }
    }
}

/// One request in, one reply out. Lock scope covers the whole dispatch so
/// concurrent tabs observe a consistent session.
fn dispatch(session: &Arc<Mutex<Session>>, text: &str) -> ServerMsg {
    let req = match parse_client_msg(text) {
        Ok(r) => r,
        Err(message) => return ServerMsg::Error { message },
    };
    let mut s = session.lock().unwrap();
    match req {
        ClientMsg::Ping => ServerMsg::Pong,
        ClientMsg::Load { path } => match s.load(Path::new(&path)) {
            Ok(()) => status_of(&s),
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Start => match s.start() {
            Ok(sum) => ServerMsg::RunSummary {
                transmitted: sum.transmitted,
                received: sum.received,
                nextSeq: s.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Pause => ok_or_error(s.pause().map(|()| status_of(&s))),
        ClientMsg::Resume => ok_or_error(s.resume().map(|()| status_of(&s))),
        ClientMsg::Stop => ok_or_error(s.stop().map(|()| status_of(&s))),
        ClientMsg::Reset => ok_or_error(s.reset().map(|()| status_of(&s))),
        ClientMsg::Step { deltaNs } => match s.step(deltaNs) {
            Ok(now) => ServerMsg::Stepped {
                nowNs: now,
                nextSeq: s.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Inject {
            sender,
            id,
            extended,
            data,
        } => match s.inject(&sender, id, extended, &data) {
            Ok(receivers) => ServerMsg::Injected {
                receivers,
                nextSeq: s.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::GetEvents { sinceSeq } => {
            let log = s.events();
            let from = sinceSeq.min(log.len());
            ServerMsg::Events {
                events: log
                    .iter()
                    .enumerate()
                    .skip(from)
                    .map(|(i, e)| seq_event(i, e))
                    .collect(),
                nextSeq: log.len(),
            }
        }
        ClientMsg::GetStatus => status_of(&s),
        ClientMsg::GetProject => match s.project() {
            Some(p) => ServerMsg::Project { project: p.clone() },
            None => ServerMsg::Error {
                message: "no project loaded".into(),
            },
        },
    }
}

fn ok_or_error(r: Result<ServerMsg, super::session::SessionError>) -> ServerMsg {
    r.unwrap_or_else(|e| ServerMsg::Error {
        message: e.to_string(),
    })
}

fn status_of(s: &Session) -> ServerMsg {
    let (buses, nodes) = match s.project() {
        Some(p) => (
            p.buses.iter().map(|b| b.id.clone()).collect(),
            p.nodes.iter().map(|n| n.id.clone()).collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    ServerMsg::Status {
        state: state_name(s.state()),
        projectPath: s.project_path().map(|p| p.display().to_string()),
        buses,
        nodes,
        nextSeq: s.events().len(),
    }
}
